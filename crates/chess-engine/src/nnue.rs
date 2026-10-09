//! NNUE – the neural network that evaluates positions.
//!
//! The computation is done by the [`nnue_core`] crate; this module only connects it to the
//! engine's types.
//!
//! ## Network structure
//!
//! ```text
//! 768 inputs  = 2 (own/opponent piece) × 6 kinds × 64 squares
//!   → HIDDEN hidden neurons (i16/i8 weights), separately for White's and Black's view ("accumulators")
//!   → SCReLU → [hidden layers] → output (selected by output bucket)
//! ```
//!
//! The default network is `nets/h128-v4-i8.bin` (768→128, i8 FT weights, 8 output buckets) –
//! thanks to its speed it is the strongest option on the ESP32-C6. Another network can be
//! selected at build time with the `NNUE_NET=<path>` environment variable.
//!
//! ## Optimizations for the ESP32-C6 (RISC-V without SIMD)
//!
//! i16 values are processed **two at a time in one `u32`** (SWAR). The weights stay in **flash**
//! and are read through the cache; alignment is ensured by the [`Aligned`] wrapper.

use chess_core::{Color, Piece, PieceChanges, Position, Square};

/// Highest non-mate evaluation (values above it are reserved for mates).
pub const MAX_NON_MATE: i32 = nnue_core::MAX_NON_MATE;

/// Wrapper that forces alignment of data embedded with `include_bytes!`.
#[repr(C, align(64))]
struct Aligned<T: ?Sized>(T);

include!(concat!(env!("OUT_DIR"), "/net_config.rs"));

/// Enables ESP32-S3 SIMD (PIE) for the current core and verifies it against the scalar code.
/// Called by the engine thread at startup. `None` = this build has no SIMD (another chip).
pub fn simd_init() -> Option<bool> {
    #[cfg(all(feature = "nnue-simd", target_arch = "xtensa"))]
    {
        nnue_core::pie::enable();
        Some(nnue_core::pie::self_test())
    }
    #[cfg(not(all(feature = "nnue-simd", target_arch = "xtensa")))]
    None
}

/// Number of output buckets of the embedded network (1 = no buckets).
pub fn net_out_buckets() -> usize {
    ARCH.out_buckets
}

/// Description of the embedded network for printouts: file name, H, size, FT weight type.
pub fn net_info() -> (&'static str, usize, usize, &'static str) {
    let ft = match ARCH.ft_type {
        nnue_core::FtType::I16 => "i16",
        nnue_core::FtType::I8 => "i8",
    };
    (NET_FILE, HIDDEN, NET_LEN, ft)
}

static NET: nnue_core::Network<'static, HIDDEN> = nnue_core::Network::from_arch(&NET_DATA.0, ARCH);

#[inline(always)]
fn piece_sq(piece: Piece, sq: Square) -> nnue_core::PieceSq {
    nnue_core::PieceSq {
        color: piece.color().index() as u8,
        kind: piece.kind().index() as u8,
        sq: sq.index() as u8,
    }
}

/// Unpacks a (piece index, square) pair from `PieceChanges`.
#[inline(always)]
fn unpack((piece, sq): (u8, u8)) -> (Piece, Square) {
    (Piece::from_index(piece), Square::new(sq))
}

/// Accumulators for both perspectives (White and Black) + piece count (for output buckets).
#[derive(Clone)]
pub struct Accumulator {
    inner: nnue_core::Accumulator<HIDDEN>,
    pieces: u32,
}

impl Default for Accumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Accumulator {
    pub const fn new() -> Accumulator {
        Accumulator {
            inner: nnue_core::Accumulator::new(),
            pieces: 0,
        }
    }

    /// Computes the accumulators from scratch for the whole position.
    pub fn refresh(&mut self, pos: &Position) {
        let pieces = (0..64u8).filter_map(|index| {
            let sq = Square::new(index);
            pos.piece_at(sq).map(|piece| piece_sq(piece, sq))
        });
        self.pieces = pieces.clone().count() as u32;
        self.inner.refresh(&NET, pieces);
    }

    /// Adds a piece (both perspectives).
    #[inline]
    pub fn add(&mut self, piece: Piece, sq: Square) {
        self.inner.add(&NET, piece_sq(piece, sq));
        self.pieces += 1;
    }

    /// Removes a piece (both perspectives).
    #[inline]
    pub fn remove(&mut self, piece: Piece, sq: Square) {
        self.inner.remove(&NET, piece_sq(piece, sq));
        self.pieces -= 1;
    }

    /// Applies the changes made by a move (removals + additions).
    ///
    /// The most common case – a plain move (1 removal + 1 addition) – is done in a single pass
    /// over the accumulator (fewer RAM reads/writes).
    pub fn apply(&mut self, changes: &PieceChanges) {
        if changes.removed_len == 1 && changes.added_len == 1 {
            let (rp, rs) = unpack(changes.removed[0]);
            let (ap, asq) = unpack(changes.added[0]);
            self.inner
                .sub_add(&NET, piece_sq(rp, rs), piece_sq(ap, asq));
            return;
        }
        for (piece, sq) in changes.removed() {
            self.remove(piece, sq);
        }
        for (piece, sq) in changes.added() {
            self.add(piece, sq);
        }
    }

    /// Reverts the changes (the inverse of `apply`) – used when returning from recursion.
    pub fn undo(&mut self, changes: &PieceChanges) {
        if changes.removed_len == 1 && changes.added_len == 1 {
            let (rp, rs) = unpack(changes.removed[0]);
            let (ap, asq) = unpack(changes.added[0]);
            self.inner
                .sub_add(&NET, piece_sq(ap, asq), piece_sq(rp, rs));
            return;
        }
        for (piece, sq) in changes.added() {
            self.remove(piece, sq);
        }
        for (piece, sq) in changes.removed() {
            self.add(piece, sq);
        }
    }

    /// Evaluation from the side to move's point of view (in centipawns).
    pub fn evaluate(&self, side_to_move: Color) -> i32 {
        self.inner.evaluate(&NET, side_to_move.index(), self.pieces)
    }

    /// i16 value at index `i` of one perspective (for tests).
    #[cfg(test)]
    fn lane(&self, color: Color, i: usize) -> i16 {
        self.inner.lanes(color.index()).nth(i).unwrap()
    }
}

/// Depth of the accumulator stack (plies). Deeper than this, in-place updates with `undo` are used.
/// The `small-acc-stack` feature (ESP32-C3 with less RAM) shortens the stack to 16 levels
/// (8 kB instead of 16 kB).
pub const ACC_STACK: usize = if cfg!(feature = "small-acc-stack") {
    16
} else {
    32
};

/// Accumulator stack for the search: a move computes the child's accumulator from the parent
/// in one pass, and going back is just an index decrement (no second `undo` pass and no repeated
/// reads of the weights from flash).
///
/// The update is **lazy**: a move only records the piece changes, and the accumulators are
/// brought up to date only when evaluating ([`AccumulatorStack::evaluate`]). Nodes that finish
/// without an evaluation (transposition table cutoff, draw, evaluation taken from the TT…) never
/// read the weights from flash at all.
///
/// Memory: `ACC_STACK` × 2 × HIDDEN × 2 B (H=128: 16 kB). Lives in `SearchTables` (static RAM).
pub struct AccumulatorStack {
    levels: [nnue_core::Accumulator<HIDDEN>; ACC_STACK],
    /// `changes[i]` = piece changes of the move leading from level `i - 1` to level `i`.
    changes: [PieceChanges; ACC_STACK],
    top: usize,
    /// Highest level whose accumulator is computed (≤ `top`).
    computed: usize,
    /// Number of moves applied in place above the top of the stack (deeper than `ACC_STACK`).
    overflow: usize,
    pieces: u32,
    /// Output weights as i8 in RAM (only for networks with small output weights, otherwise
    /// empty) – evaluate then does not read them through the flash cache, where they would
    /// compete with the FT weights and the code.
    out_weights: OutWeights,
    out_ready: bool,
}

/// Copy of the output weights aligned to 16 bytes (the ESP32-S3 SIMD reads 16 bytes at a time).
#[repr(C, align(16))]
struct OutWeights([i8; OUT_WEIGHTS]);

/// Length of the output weight copy in RAM (H=128, 8 buckets: 2 kB).
const OUT_WEIGHTS: usize = nnue_core::Network::<HIDDEN>::small_out_len(&ARCH);

const NO_CHANGES: PieceChanges = PieceChanges {
    removed: [(0, 0); 2],
    removed_len: 0,
    added: [(0, 0); 2],
    added_len: 0,
};

impl Default for AccumulatorStack {
    fn default() -> Self {
        Self::new()
    }
}

impl AccumulatorStack {
    pub const fn new() -> AccumulatorStack {
        AccumulatorStack {
            levels: [const { nnue_core::Accumulator::new() }; ACC_STACK],
            changes: [NO_CHANGES; ACC_STACK],
            top: 0,
            computed: 0,
            overflow: 0,
            pieces: 0,
            out_weights: OutWeights([0; OUT_WEIGHTS]),
            out_ready: false,
        }
    }

    /// Computes the root from scratch and empties the stack.
    pub fn refresh(&mut self, pos: &Position) {
        let pieces = (0..64u8).filter_map(|index| {
            let sq = Square::new(index);
            pos.piece_at(sq).map(|piece| piece_sq(piece, sq))
        });
        self.pieces = pieces.clone().count() as u32;
        self.top = 0;
        self.computed = 0;
        self.overflow = 0;
        self.levels[0].refresh(&NET, pieces);
        if OUT_WEIGHTS > 0 && !self.out_ready {
            NET.small_out_weights(&mut self.out_weights.0);
            self.out_ready = true;
        }
    }

    /// Move: records the changes for a new level (or updates in place if the stack is full).
    #[inline]
    pub fn apply(&mut self, changes: &PieceChanges) {
        self.pieces = self.pieces + changes.added_len as u32 - changes.removed_len as u32;
        if self.overflow > 0 || self.top + 1 >= ACC_STACK {
            self.materialize();
            self.overflow += 1;
            apply_in_place(&mut self.levels[self.top], changes);
            return;
        }
        self.top += 1;
        self.changes[self.top] = *changes;
    }

    /// Takes a move back: an index decrement (or an in-place `undo` above the stack).
    #[inline]
    pub fn undo(&mut self, changes: &PieceChanges) {
        self.pieces = self.pieces + changes.removed_len as u32 - changes.added_len as u32;
        if self.overflow > 0 {
            self.overflow -= 1;
            undo_in_place(&mut self.levels[self.top], changes);
        } else {
            self.top -= 1;
            self.computed = self.computed.min(self.top);
        }
    }

    /// Computes the accumulators from the last computed level up to the top of the stack.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn materialize(&mut self) {
        while self.computed < self.top {
            let level = self.computed + 1;
            let (lo, hi) = self.levels.split_at_mut(level);
            let (parent, child) = (&lo[level - 1], &mut hi[0]);
            let changes = &self.changes[level];
            if changes.removed_len == 1 && changes.added_len == 1 {
                let (rp, rs) = unpack(changes.removed[0]);
                let (ap, asq) = unpack(changes.added[0]);
                child.sub_add_from(parent, &NET, piece_sq(rp, rs), piece_sq(ap, asq));
            } else {
                child.copy_from(parent);
                apply_in_place(child, changes);
            }
            self.computed = level;
        }
    }

    /// Evaluation of the current position from the side to move's point of view (brings the
    /// accumulators up to date first).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn evaluate(&mut self, side_to_move: Color) -> i32 {
        self.materialize();
        let acc = &self.levels[self.top];
        if OUT_WEIGHTS > 0 {
            acc.evaluate_small(&NET, side_to_move.index(), self.pieces, &self.out_weights.0)
        } else {
            acc.evaluate(&NET, side_to_move.index(), self.pieces)
        }
    }

    /// i16 value at index `i` of one perspective at the current level (for tests).
    #[cfg(test)]
    fn lane(&mut self, color: Color, i: usize) -> i16 {
        self.materialize();
        self.levels[self.top].lanes(color.index()).nth(i).unwrap()
    }
}

#[cfg_attr(
    all(feature = "iram", target_os = "none"),
    unsafe(link_section = ".rwtext")
)]
fn apply_in_place(acc: &mut nnue_core::Accumulator<HIDDEN>, changes: &PieceChanges) {
    if changes.removed_len == 1 && changes.added_len == 1 {
        let (rp, rs) = unpack(changes.removed[0]);
        let (ap, asq) = unpack(changes.added[0]);
        acc.sub_add(&NET, piece_sq(rp, rs), piece_sq(ap, asq));
        return;
    }
    for (piece, sq) in changes.removed() {
        acc.remove(&NET, piece_sq(piece, sq));
    }
    for (piece, sq) in changes.added() {
        acc.add(&NET, piece_sq(piece, sq));
    }
}

#[cfg_attr(
    all(feature = "iram", target_os = "none"),
    unsafe(link_section = ".rwtext")
)]
fn undo_in_place(acc: &mut nnue_core::Accumulator<HIDDEN>, changes: &PieceChanges) {
    if changes.removed_len == 1 && changes.added_len == 1 {
        let (rp, rs) = unpack(changes.removed[0]);
        let (ap, asq) = unpack(changes.added[0]);
        acc.sub_add(&NET, piece_sq(ap, asq), piece_sq(rp, rs));
        return;
    }
    for (piece, sq) in changes.added() {
        acc.remove(&NET, piece_sq(piece, sq));
    }
    for (piece, sq) in changes.removed() {
        acc.add(&NET, piece_sq(piece, sq));
    }
}

/// Evaluates a position without incremental accumulators (slow; for tests and one-off use).
pub fn evaluate_position(pos: &Position) -> i32 {
    let mut acc = Accumulator::new();
    acc.refresh(pos);
    acc.evaluate(pos.side_to_move())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::{legal_moves, parse_uci};

    #[test]
    fn incremental_matches_refresh() {
        // After a series of moves the incremental accumulator must exactly match a full refresh.
        let mut pos = Position::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        )
        .unwrap();
        let mut acc = Accumulator::new();
        acc.refresh(&pos);
        for step in 0..60 {
            let moves = legal_moves(&pos);
            if moves.is_empty() {
                break;
            }
            let mv = moves.as_slice()[(pos.hash() as usize + step) % moves.len()];
            let (next, changes) = pos.make_move_with_changes(mv);
            acc.apply(&changes);
            let mut fresh = Accumulator::new();
            fresh.refresh(&next);
            for i in 0..HIDDEN {
                assert_eq!(acc.lane(Color::White, i), fresh.lane(Color::White, i));
                assert_eq!(acc.lane(Color::Black, i), fresh.lane(Color::Black, i));
            }
            // Back and forward again – undo must restore the original state.
            acc.undo(&changes);
            let mut before = Accumulator::new();
            before.refresh(&pos);
            for i in 0..HIDDEN {
                assert_eq!(
                    acc.lane(Color::White, i),
                    before.lane(Color::White, i),
                    "undo after move {mv}"
                );
                assert_eq!(
                    acc.lane(Color::Black, i),
                    before.lane(Color::Black, i),
                    "undo after move {mv}"
                );
            }
            assert_eq!(acc.pieces, before.pieces);
            acc.apply(&changes);
            pos = next;
        }
    }

    #[test]
    fn stack_matches_refresh() {
        extern crate std;
        use std::{boxed::Box, vec};
        // A move sequence deeper than ACC_STACK (in-place overflow) and back – every level must
        // match a full refresh.
        let start = Position::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        )
        .unwrap();
        let mut stack = Box::new(AccumulatorStack::new());
        stack.refresh(&start);
        let mut path = vec![(start.clone(), None)];
        for step in 0..(ACC_STACK + 10) {
            let pos = path.last().unwrap().0.clone();
            let moves = legal_moves(&pos);
            if moves.is_empty() {
                break;
            }
            let mv = moves.as_slice()[(pos.hash() as usize + step) % moves.len()];
            let (next, changes) = pos.make_move_with_changes(mv);
            stack.apply(&changes);
            let mut fresh = Accumulator::new();
            fresh.refresh(&next);
            for i in 0..HIDDEN {
                assert_eq!(stack.lane(Color::White, i), fresh.lane(Color::White, i));
                assert_eq!(stack.lane(Color::Black, i), fresh.lane(Color::Black, i));
            }
            assert_eq!(
                stack.evaluate(next.side_to_move()),
                evaluate_position(&next)
            );
            path.push((next, Some(changes)));
        }
        while path.len() > 1 {
            let (_, changes) = path.pop().unwrap();
            stack.undo(&changes.unwrap());
            let pos = &path.last().unwrap().0;
            assert_eq!(stack.evaluate(pos.side_to_move()), evaluate_position(pos));
        }
    }

    #[test]
    fn evaluation_is_sane() {
        // Starting position: a small advantage for the side to move.
        let start = evaluate_position(&Position::start());
        assert!((-100..=150).contains(&start), "starting position: {start}");

        // White is a queen up → large positive evaluation with White to move.
        let queen_up =
            Position::from_fen("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").unwrap();
        assert!(evaluate_position(&queen_up) > 500);

        // Same position with Black to move → negative (from Black's point of view).
        let queen_up_black =
            Position::from_fen("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1").unwrap();
        assert!(evaluate_position(&queen_up_black) < -500);
    }

    #[test]
    fn evaluation_is_color_symmetric() {
        // A mirrored position with swapped colors must have the same evaluation
        // (the network always evaluates from the side to move's point of view).
        let pos = Position::start();
        let pos = pos.make_move(parse_uci(&pos, "e2e4").unwrap());
        let mirrored =
            Position::from_fen("rnbqkbnr/pppp1ppp/8/4p3/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")
                .unwrap();
        assert_eq!(evaluate_position(&pos), evaluate_position(&mirrored));
    }
}
