//! Board position: piece placement, side to move, castling rights, en passant.
//!
//! A position is small (~150 B) and copyable. A move is made with
//! [`Position::make_move`], which returns a **new** position ("copy-make") –
//! no complicated unmake logic is needed.

use crate::attacks::{
    self, Bitboard, bishop_attacks, king_attacks, knight_attacks, pawn_attacks, queen_attacks,
    rook_attacks,
};
use crate::moves::{Move, flag};
use crate::types::{Color, Piece, PieceKind, Square};
use crate::zobrist::{KEYS, piece_key};

/// Castling rights as bits.
pub mod castling {
    pub const WHITE_KING: u8 = 1;
    pub const WHITE_QUEEN: u8 = 2;
    pub const BLACK_KING: u8 = 4;
    pub const BLACK_QUEEN: u8 = 8;
    pub const ALL: u8 = 15;
}

/// Mask ANDed into the castling rights after a move from/to the given square.
/// E.g. a move from/to h1 (rook) clears white's kingside castling right.
const CASTLING_MASK: [u8; 64] = {
    let mut mask = [castling::ALL; 64];
    mask[0] = castling::ALL & !castling::WHITE_QUEEN; // a1
    mask[4] = castling::ALL & !(castling::WHITE_KING | castling::WHITE_QUEEN); // e1
    mask[7] = castling::ALL & !castling::WHITE_KING; // h1
    mask[56] = castling::ALL & !castling::BLACK_QUEEN; // a8
    mask[60] = castling::ALL & !(castling::BLACK_KING | castling::BLACK_QUEEN); // e8
    mask[63] = castling::ALL & !castling::BLACK_KING; // h8
    mask
};

/// Starting position as FEN.
pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Mailbox value of an empty square.
const EMPTY: u8 = 255;

/// FEN parsing error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenError {
    Board,
    Side,
    Castling,
    EnPassant,
    Counters,
    /// A king is missing, or the side not to move is in check.
    Illegal,
}

/// Change of the piece placement caused by a move – what disappeared and what appeared.
/// Used by NNUE (incremental updates) and by the display (redrawing squares).
#[derive(Clone, Copy, Debug, Default)]
pub struct PieceChanges {
    pub removed: [(u8, u8); 2],
    pub removed_len: u8,
    pub added: [(u8, u8); 2],
    pub added_len: u8,
}

impl PieceChanges {
    fn remove(&mut self, piece: Piece, sq: Square) {
        self.removed[self.removed_len as usize] = (piece.index() as u8, sq.raw());
        self.removed_len += 1;
    }

    fn add(&mut self, piece: Piece, sq: Square) {
        self.added[self.added_len as usize] = (piece.index() as u8, sq.raw());
        self.added_len += 1;
    }

    /// Removed pieces as (piece, square).
    pub fn removed(&self) -> impl Iterator<Item = (Piece, Square)> + '_ {
        self.removed[..self.removed_len as usize]
            .iter()
            .map(|&(p, s)| (piece_from_index(p), Square::new(s)))
    }

    /// Added pieces as (piece, square).
    pub fn added(&self) -> impl Iterator<Item = (Piece, Square)> + '_ {
        self.added[..self.added_len as usize]
            .iter()
            .map(|&(p, s)| (piece_from_index(p), Square::new(s)))
    }
}

#[inline]
fn piece_from_index(index: u8) -> Piece {
    Piece::from_index(index)
}

/// Data for fast legality checks of moves in one position.
#[derive(Clone, Copy, Debug)]
pub struct Legality {
    /// Own pinned pieces.
    pub pinned: Bitboard,
    /// Is the side to move in check?
    pub in_check: bool,
    /// Square of the own king.
    pub king: Square,
    /// In check: target squares on which a non-king piece resolves the check (the checking
    /// piece + squares between it and the king; empty on double check). Otherwise all squares.
    pub evasion: Bitboard,
}

/// Chess position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    /// Bitboard for each piece kind (both colors together).
    kinds: [Bitboard; 6],
    /// Bitboard of all pieces of a color.
    colors: [Bitboard; 2],
    /// Mailbox: the piece on each square (`EMPTY` = empty) – fast "what stands on square X".
    board: [u8; 64],
    side: Color,
    castling: u8,
    /// En passant target square (only if the capture is actually possible), otherwise `None`.
    en_passant: Option<Square>,
    /// Plies since the last capture or pawn move (fifty-move rule).
    halfmove_clock: u16,
    /// Move number (starts at 1, incremented after black's move).
    fullmove: u16,
    hash: u64,
}

impl Default for Position {
    fn default() -> Self {
        Position::start()
    }
}

impl Position {
    /// The starting position.
    pub fn start() -> Position {
        Position::from_fen(START_FEN).expect("the starting FEN is valid")
    }

    fn empty() -> Position {
        Position {
            kinds: [0; 6],
            colors: [0; 2],
            board: [EMPTY; 64],
            side: Color::White,
            castling: 0,
            en_passant: None,
            halfmove_clock: 0,
            fullmove: 1,
            hash: 0,
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    #[inline]
    pub fn side_to_move(&self) -> Color {
        self.side
    }

    #[inline]
    pub fn hash(&self) -> u64 {
        self.hash
    }

    #[inline]
    pub fn castling_rights(&self) -> u8 {
        self.castling
    }

    #[inline]
    pub fn en_passant(&self) -> Option<Square> {
        self.en_passant
    }

    #[inline]
    pub fn halfmove_clock(&self) -> u16 {
        self.halfmove_clock
    }

    #[inline]
    pub fn fullmove_number(&self) -> u16 {
        self.fullmove
    }

    /// Piece on a square (or `None`).
    #[inline]
    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        let code = self.board[sq.index()];
        if code == EMPTY {
            None
        } else {
            Some(piece_from_index(code))
        }
    }

    /// Piece kind on a square (faster than `piece_at`, without color).
    #[inline]
    pub fn kind_at(&self, sq: Square) -> Option<PieceKind> {
        let code = self.board[sq.index()];
        if code == EMPTY {
            None
        } else {
            Some(PieceKind::from_index((code % 6) as usize))
        }
    }

    /// Pieces of the given kind and color.
    #[inline]
    pub fn pieces(&self, color: Color, kind: PieceKind) -> Bitboard {
        self.kinds[kind.index()] & self.colors[color.index()]
    }

    /// Pieces of the given kind (both colors).
    #[inline]
    pub fn kind_bb(&self, kind: PieceKind) -> Bitboard {
        self.kinds[kind.index()]
    }

    /// All pieces of one color.
    #[inline]
    pub fn color_bb(&self, color: Color) -> Bitboard {
        self.colors[color.index()]
    }

    /// All occupied squares.
    #[inline]
    pub fn occupied(&self) -> Bitboard {
        self.colors[0] | self.colors[1]
    }

    /// King square of the given color.
    #[inline]
    pub fn king_square(&self, color: Color) -> Square {
        attacks::lsb(self.pieces(color, PieceKind::King))
    }

    /// All pieces (of both colors) attacking `sq` with occupancy `occupied`.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn attackers_to(&self, sq: Square, occupied: Bitboard) -> Bitboard {
        let bishops_queens =
            self.kinds[PieceKind::Bishop.index()] | self.kinds[PieceKind::Queen.index()];
        let rooks_queens =
            self.kinds[PieceKind::Rook.index()] | self.kinds[PieceKind::Queen.index()];
        (pawn_attacks(Color::White, sq) & self.pieces(Color::Black, PieceKind::Pawn))
            | (pawn_attacks(Color::Black, sq) & self.pieces(Color::White, PieceKind::Pawn))
            | (knight_attacks(sq) & self.kinds[PieceKind::Knight.index()])
            | (king_attacks(sq) & self.kinds[PieceKind::King.index()])
            | (bishop_attacks(sq, occupied) & bishops_queens)
            | (rook_attacks(sq, occupied) & rooks_queens)
    }

    /// Does side `by` attack `sq`?
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn is_attacked(&self, sq: Square, by: Color) -> bool {
        let them = self.colors[by.index()];
        let occupied = self.occupied();
        if pawn_attacks(by.opponent(), sq) & self.kinds[PieceKind::Pawn.index()] & them != 0 {
            return true;
        }
        if knight_attacks(sq) & self.kinds[PieceKind::Knight.index()] & them != 0 {
            return true;
        }
        if king_attacks(sq) & self.kinds[PieceKind::King.index()] & them != 0 {
            return true;
        }
        let queens = self.kinds[PieceKind::Queen.index()];
        let diagonal = (self.kinds[PieceKind::Bishop.index()] | queens) & them;
        if diagonal != 0 && bishop_attacks(sq, occupied) & diagonal != 0 {
            return true;
        }
        let straight = (self.kinds[PieceKind::Rook.index()] | queens) & them;
        straight != 0 && rook_attacks(sq, occupied) & straight != 0
    }

    /// Enemy pieces giving check to the side to move.
    pub fn checkers(&self) -> Bitboard {
        let king = self.king_square(self.side);
        self.attackers_to(king, self.occupied()) & self.colors[self.side.opponent().index()]
    }

    /// Is the side to move in check?
    #[inline]
    pub fn in_check(&self) -> bool {
        self.is_attacked(self.king_square(self.side), self.side.opponent())
    }

    /// Pieces of `color` pinned to their king
    /// (moving them away would expose a check from an enemy slider).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn pinned(&self, color: Color) -> Bitboard {
        let king = self.king_square(color);
        let them = self.colors[color.opponent().index()];
        let queens = self.kinds[PieceKind::Queen.index()];
        let snipers = ((attacks::rook_rays(king) & (self.kinds[PieceKind::Rook.index()] | queens))
            | (attacks::bishop_rays(king) & (self.kinds[PieceKind::Bishop.index()] | queens)))
            & them;
        let occupied = self.occupied();
        let mut pinned = 0;
        for sniper in attacks::Squares(snipers) {
            let blockers = attacks::between(king, sniper) & occupied;
            // Pin = exactly one piece, ours, stands between the king and the attacker.
            if blockers.count_ones() == 1 {
                pinned |= blockers & self.colors[color.index()];
            }
        }
        pinned
    }

    /// Total number of pieces on the board.
    pub fn piece_count(&self) -> u32 {
        self.occupied().count_ones()
    }

    /// Does `color` have pieces other than pawns and the king? (For the engine's null move.)
    pub fn has_non_pawn_material(&self, color: Color) -> bool {
        let pawns_kings = self.kinds[PieceKind::Pawn.index()] | self.kinds[PieceKind::King.index()];
        self.colors[color.index()] & !pawns_kings != 0
    }

    // ------------------------------------------------------------------
    // Modification
    // ------------------------------------------------------------------

    fn put(&mut self, piece: Piece, sq: Square) {
        let bit = sq.bb();
        self.kinds[piece.kind().index()] |= bit;
        self.colors[piece.color().index()] |= bit;
        self.board[sq.index()] = piece.index() as u8;
        self.hash ^= piece_key(piece, sq);
    }

    fn remove(&mut self, piece: Piece, sq: Square) {
        let bit = sq.bb();
        self.kinds[piece.kind().index()] &= !bit;
        self.colors[piece.color().index()] &= !bit;
        self.board[sq.index()] = EMPTY;
        self.hash ^= piece_key(piece, sq);
    }

    fn set_en_passant(&mut self, sq: Option<Square>) {
        if let Some(old) = self.en_passant {
            self.hash ^= KEYS.en_passant[old.file() as usize];
        }
        self.en_passant = sq;
        if let Some(new) = sq {
            self.hash ^= KEYS.en_passant[new.file() as usize];
        }
    }

    fn set_castling(&mut self, rights: u8) {
        self.hash ^= KEYS.castling[self.castling as usize];
        self.castling = rights;
        self.hash ^= KEYS.castling[rights as usize];
    }

    /// Makes a (pseudo-)legal move and returns the new position.
    /// Legality (king not left in check) is checked by the generator / `is_legal`.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn make_move(&self, mv: Move) -> Position {
        let mut next = self.clone();
        next.apply_move(mv);
        next
    }

    /// Like `make_move`, but also returns the list of piece changes (for NNUE / the display).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn make_move_with_changes(&self, mv: Move) -> (Position, PieceChanges) {
        let mut next = self.clone();
        let mut changes = PieceChanges::default();
        next.apply_move_recording(mv, Some(&mut changes));
        (next, changes)
    }

    /// Computes which pieces a move removes and adds (without making the move).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn piece_changes(&self, mv: Move) -> PieceChanges {
        let mut changes = PieceChanges::default();
        let from = mv.from();
        let to = mv.to();
        let Some(moving) = self.piece_at(from) else {
            return changes;
        };
        let us = moving.color();
        changes.remove(moving, from);
        if mv.is_en_passant() {
            let captured_sq = Square::from_coords(to.file(), from.rank());
            changes.remove(Piece::new(us.opponent(), PieceKind::Pawn), captured_sq);
        } else if let Some(captured) = self.piece_at(to) {
            changes.remove(captured, to);
        }
        let placed = match mv.promotion_kind() {
            Some(kind) => Piece::new(us, kind),
            None => moving,
        };
        changes.add(placed, to);
        if mv.is_castle() {
            let (rook_from, rook_to) = castle_rook_squares(mv);
            let rook = Piece::new(us, PieceKind::Rook);
            // The rook just moves – recorded as a removal + an addition.
            // (Two removal slots are enough: castling never captures.)
            changes.remove(rook, rook_from);
            changes.add(rook, rook_to);
        }
        changes
    }

    #[inline]
    fn apply_move(&mut self, mv: Move) {
        self.apply_move_recording(mv, None);
    }

    /// Makes the move; writes the piece changes into `changes` in the same form as [`Position::piece_changes`].
    #[inline(always)]
    fn apply_move_recording(&mut self, mv: Move, mut changes: Option<&mut PieceChanges>) {
        let us = self.side;
        let them = us.opponent();
        let from = mv.from();
        let to = mv.to();
        let moving = self.piece_at(from).expect("a move must start on a piece");

        self.set_en_passant(None);
        self.halfmove_clock += 1;

        if let Some(c) = changes.as_deref_mut() {
            c.remove(moving, from);
        }

        // Capture.
        if mv.is_en_passant() {
            let captured_sq = Square::from_coords(to.file(), from.rank());
            let captured = Piece::new(them, PieceKind::Pawn);
            self.remove(captured, captured_sq);
            self.halfmove_clock = 0;
            if let Some(c) = changes.as_deref_mut() {
                c.remove(captured, captured_sq);
            }
        } else if let Some(captured) = self.piece_at(to) {
            self.remove(captured, to);
            self.halfmove_clock = 0;
            if let Some(c) = changes.as_deref_mut() {
                c.remove(captured, to);
            }
        }

        // Move the piece (promoting it if needed).
        self.remove(moving, from);
        let placed = match mv.promotion_kind() {
            Some(kind) => Piece::new(us, kind),
            None => moving,
        };
        self.put(placed, to);
        if let Some(c) = changes.as_deref_mut() {
            c.add(placed, to);
        }

        if moving.kind() == PieceKind::Pawn {
            self.halfmove_clock = 0;
            if mv.flags() == flag::DOUBLE_PUSH {
                // Set en passant only if the opponent can actually play it
                // (otherwise identical positions would differ in hash and repetition would fail).
                let ep = Square::from_coords(from.file(), (from.rank() + to.rank()) / 2);
                if pawn_attacks(us, ep) & self.pieces(them, PieceKind::Pawn) != 0 {
                    self.set_en_passant(Some(ep));
                }
            }
        }

        if mv.is_castle() {
            let (rook_from, rook_to) = castle_rook_squares(mv);
            let rook = Piece::new(us, PieceKind::Rook);
            self.remove(rook, rook_from);
            self.put(rook, rook_to);
            if let Some(c) = changes {
                c.remove(rook, rook_from);
                c.add(rook, rook_to);
            }
        }

        let rights = self.castling & CASTLING_MASK[from.index()] & CASTLING_MASK[to.index()];
        if rights != self.castling {
            self.set_castling(rights);
        }

        if us == Color::Black {
            self.fullmove += 1;
        }
        self.side = them;
        self.hash ^= KEYS.side;
    }

    /// "Null move" – just passes the turn to the opponent (used by the engine for null-move pruning).
    pub fn make_null_move(&self) -> Position {
        let mut next = self.clone();
        next.set_en_passant(None);
        next.halfmove_clock += 1;
        next.side = next.side.opponent();
        next.hash ^= KEYS.side;
        next
    }

    /// Precomputes data for fast move legality checks (once per search node).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn legality(&self) -> Legality {
        self.legality_with_check(self.in_check())
    }

    /// Like [`Position::legality`] when the caller already knows whether the side to move
    /// is in check (checkers are not looked up when not in check).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn legality_with_check(&self, in_check: bool) -> Legality {
        let king = self.king_square(self.side);
        let checkers = if in_check { self.checkers() } else { 0 };
        let evasion = match checkers.count_ones() {
            0 => !0,
            1 => checkers | attacks::between(king, Square::new(checkers.trailing_zeros() as u8)),
            _ => 0, // double check – only a king move resolves it
        };
        Legality {
            pinned: self.pinned(self.side),
            in_check: checkers != 0,
            king,
            evasion,
        }
    }

    /// Is a pseudo-legal move legal (does it not leave the own king in check)?
    /// `info` = result of [`Position::legality`] for this position.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    pub fn is_legal(&self, mv: Move, info: &Legality) -> bool {
        let us = self.side;
        let from = mv.from();
        let king = info.king;
        let pinned = info.pinned;

        if from == king {
            if mv.is_castle() {
                // Castling: the generator has already checked the squares the king passes.
                return true;
            }
            // The king must not step onto an attacked square (remove it from the occupancy
            // so it does not block an attack along the line it retreats on).
            let occupied = self.occupied() ^ from.bb();
            let attackers =
                self.attackers_to(mv.to(), occupied) & self.colors[us.opponent().index()];
            return attackers & !mv.to().bb() == 0;
        }

        if mv.is_en_passant() {
            // Rare case (discovered attack along the rank, capturing the checking pawn) – verify by making the move.
            let next = self.make_move(mv);
            return !next.is_attacked(next.king_square(us), us.opponent());
        }

        // In check the move must capture the checker or block between it and the king.
        // (A pinned piece cannot resolve a check: the pin and check lines only meet at the king.)
        if mv.to().bb() & info.evasion == 0 {
            return false;
        }

        // A pinned piece may only move along the pin line.
        pinned & from.bb() == 0 || attacks::line_through(king, from) & mv.to().bb() != 0
    }

    // ------------------------------------------------------------------
    // FEN
    // ------------------------------------------------------------------

    /// Parses a position from FEN. The move counters are optional.
    pub fn from_fen(fen: &str) -> Result<Position, FenError> {
        let mut pos = Position::empty();
        let mut parts = fen.split_ascii_whitespace();

        let board = parts.next().ok_or(FenError::Board)?;
        let mut rank: i32 = 7;
        let mut file: i32 = 0;
        for c in board.chars() {
            match c {
                '/' => {
                    if file != 8 {
                        return Err(FenError::Board);
                    }
                    rank -= 1;
                    file = 0;
                }
                '1'..='8' => file += c as i32 - '0' as i32,
                _ => {
                    let piece = Piece::from_fen_char(c).ok_or(FenError::Board)?;
                    if !(0..8).contains(&file) || !(0..8).contains(&rank) {
                        return Err(FenError::Board);
                    }
                    pos.put(piece, Square::from_coords(file as u8, rank as u8));
                    file += 1;
                }
            }
            if file > 8 || rank < 0 {
                return Err(FenError::Board);
            }
        }
        if rank != 0 || file != 8 {
            return Err(FenError::Board);
        }

        match parts.next() {
            Some("w") | None => {}
            Some("b") => {
                pos.side = Color::Black;
                pos.hash ^= KEYS.side;
            }
            Some(_) => return Err(FenError::Side),
        }

        let mut rights = 0;
        if let Some(text) = parts.next() {
            for c in text.chars() {
                rights |= match c {
                    'K' => castling::WHITE_KING,
                    'Q' => castling::WHITE_QUEEN,
                    'k' => castling::BLACK_KING,
                    'q' => castling::BLACK_QUEEN,
                    '-' => 0,
                    _ => return Err(FenError::Castling),
                };
            }
        }
        // Keep castling rights only where the king and rook really stand on their initial squares.
        rights &= pos.valid_castling_mask();
        pos.set_castling(rights);

        if let Some(text) = parts.next()
            && text != "-"
        {
            let sq = Square::parse(text).ok_or(FenError::EnPassant)?;
            if sq.rank() != 2 && sq.rank() != 5 {
                return Err(FenError::EnPassant);
            }
            // Same as when making a move: only if the side to move can capture.
            let us = pos.side;
            if pawn_attacks(us.opponent(), sq) & pos.pieces(us, PieceKind::Pawn) != 0 {
                pos.set_en_passant(Some(sq));
            }
        }

        if let Some(text) = parts.next() {
            pos.halfmove_clock = text.parse().map_err(|_| FenError::Counters)?;
        }
        if let Some(text) = parts.next() {
            pos.fullmove = text.parse::<u16>().map_err(|_| FenError::Counters)?.max(1);
        }

        // Basic legality check: each side has exactly one king
        // and the side not to move must not be in check.
        for color in Color::ALL {
            if pos.pieces(color, PieceKind::King).count_ones() != 1 {
                return Err(FenError::Illegal);
            }
        }
        let them = pos.side.opponent();
        if pos.is_attacked(pos.king_square(them), pos.side) {
            return Err(FenError::Illegal);
        }
        // Pawns must not stand on the first or eighth rank.
        if pos.kinds[PieceKind::Pawn.index()] & (attacks::rank_mask(0) | attacks::rank_mask(7)) != 0
        {
            return Err(FenError::Illegal);
        }
        Ok(pos)
    }

    /// Castling rights that make sense given the placement of the kings and rooks.
    fn valid_castling_mask(&self) -> u8 {
        let mut mask = 0;
        let has = |color, kind, name: &str| {
            self.pieces(color, kind) & Square::parse(name).map_or(0, |s| s.bb()) != 0
        };
        if has(Color::White, PieceKind::King, "e1") {
            if has(Color::White, PieceKind::Rook, "h1") {
                mask |= castling::WHITE_KING;
            }
            if has(Color::White, PieceKind::Rook, "a1") {
                mask |= castling::WHITE_QUEEN;
            }
        }
        if has(Color::Black, PieceKind::King, "e8") {
            if has(Color::Black, PieceKind::Rook, "h8") {
                mask |= castling::BLACK_KING;
            }
            if has(Color::Black, PieceKind::Rook, "a8") {
                mask |= castling::BLACK_QUEEN;
            }
        }
        mask
    }

    /// Writes the position as FEN into any `fmt::Write` (e.g. a `String`).
    pub fn write_fen<W: core::fmt::Write>(&self, out: &mut W) -> core::fmt::Result {
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                match self.piece_at(Square::from_coords(file, rank)) {
                    Some(piece) => {
                        if empty > 0 {
                            write!(out, "{empty}")?;
                            empty = 0;
                        }
                        out.write_char(piece.fen_char())?;
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                write!(out, "{empty}")?;
            }
            if rank > 0 {
                out.write_char('/')?;
            }
        }
        out.write_str(if self.side == Color::White {
            " w "
        } else {
            " b "
        })?;
        if self.castling == 0 {
            out.write_char('-')?;
        } else {
            for (bit, c) in [
                (castling::WHITE_KING, 'K'),
                (castling::WHITE_QUEEN, 'Q'),
                (castling::BLACK_KING, 'k'),
                (castling::BLACK_QUEEN, 'q'),
            ] {
                if self.castling & bit != 0 {
                    out.write_char(c)?;
                }
            }
        }
        match self.en_passant {
            Some(sq) => write!(out, " {sq}")?,
            None => out.write_str(" -")?,
        }
        write!(out, " {} {}", self.halfmove_clock, self.fullmove)
    }

    /// Writes only the piece placement (first FEN field) – for game previews.
    pub fn write_board_fen<W: core::fmt::Write>(&self, out: &mut W) -> core::fmt::Result {
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                match self.piece_at(Square::from_coords(file, rank)) {
                    Some(piece) => {
                        if empty > 0 {
                            write!(out, "{empty}")?;
                            empty = 0;
                        }
                        out.write_char(piece.fen_char())?;
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                write!(out, "{empty}")?;
            }
            if rank > 0 {
                out.write_char('/')?;
            }
        }
        Ok(())
    }
}

/// Where the rook moves from and to when castling.
fn castle_rook_squares(mv: Move) -> (Square, Square) {
    let rank = mv.from().rank();
    if mv.flags() == flag::KING_CASTLE {
        (Square::from_coords(7, rank), Square::from_coords(5, rank))
    } else {
        (Square::from_coords(0, rank), Square::from_coords(3, rank))
    }
}

/// Attacks of a piece of the given kind from a square (for the move generator and SEE).
#[inline]
pub fn piece_attacks(kind: PieceKind, color: Color, sq: Square, occupied: Bitboard) -> Bitboard {
    match kind {
        PieceKind::Pawn => pawn_attacks(color, sq),
        PieceKind::Knight => knight_attacks(sq),
        PieceKind::Bishop => bishop_attacks(sq, occupied),
        PieceKind::Rook => rook_attacks(sq, occupied),
        PieceKind::Queen => queen_attacks(sq, occupied),
        PieceKind::King => king_attacks(sq),
    }
}
