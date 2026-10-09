//! Best-move search – a port of `search.cpp` from tinychess (the Dog engine).
//!
//! Algorithm: **iterative deepening** (depth 1, 2, 3, …) with **aspiration
//! windows**; each iteration runs **PVS** (principal variation search = alpha-beta
//! with a null window for "unlikely" moves) and ends in a **quiescence search**
//! (captures are played out so that only "quiet" positions get evaluated).
//!
//! Pruning and reductions (taken from the original, including the constants):
//! - transposition table (TT), IIR (no TT move in a PV node → one ply shallower),
//! - mate distance pruning,
//! - reverse futility (static eval − 121·depth > beta → cutoff),
//! - razoring (static eval + 350 + 150·depth < alpha → quiescence search only),
//! - null move (R = 3/4) with a verification search,
//! - futility (quiet moves at depth ≤ 2 if eval + 180 + 150·depth < alpha),
//! - LMR (late quiet moves searched shallower, ln·ln table),
//! - extensions: check, single legal move, recapture on the same square,
//! - move ordering: TT move, MVV-LVA + capture history, quiet move histories
//!   (piece→square, from→to, continuation after the previous move).
//!
//! ### Differences from the original (because of the ESP32's small RAM)
//! - positions are copied (copy-make), NNUE accumulators are updated incrementally,
//! - all tree levels share a single **move stack** (instead of a 256-move list per level),
//! - the principal variation is read from the TT (no 64×64 move table needed),
//! - mate distance is computed from the ply from the root (`ply`).

use chess_core::{
    GenType, Legality, Move, MoveSink, PieceKind, Position, Square, generate, legal_moves,
    rules::is_insufficient_material,
};

use crate::limits::{IterationInfo, PV_LEN, Progress, SearchHooks, SearchLimits};
use crate::nnue::{AccumulatorStack, MAX_NON_MATE};
use crate::see::see;
use crate::tt::{Bound, TranspositionTable, TtEntry, TtHit, score_from_tt, score_to_tt};

include!(concat!(env!("OUT_DIR"), "/lmr_table.rs"));

/// Mate score (mate in 0 plies). Mate in N plies = MATE − N.
pub const MATE: i32 = 30_000;
/// Infinity for the alpha-beta window.
const INF: i32 = 32_767;
/// Maximum tree depth (plies from the root, including quiescence search).
pub const MAX_PLY: usize = 96;
/// Size of the shared move stack (on average ~30 moves per tree level).
const MOVE_STACK: usize = 3072;
/// How many positions of the game history are kept for repetition detection.
const GAME_HISTORY: usize = 256;
/// Bound of history values (the gravity formula keeps them in range).
const HISTORY_MAX: i32 = 1023;

/// Large search tables. On the board they live in static memory (not on the stack),
/// which is why the constructor is a `const fn`.
pub struct SearchTables {
    moves: [Move; MOVE_STACK],
    scores: [i32; MOVE_STACK],
    /// Quiet move history: [side][piece kind][target square].
    history: [[[i16; 64]; 6]; 2],
    /// Capture history: [side][attacker][target square][victim].
    capture_history: [[[[i16; 6]; 64]; 6]; 2],
    /// Butterfly history: [side][from][to].
    butterfly: [[[i16; 64]; 64]; 2],
    /// Continuation: [target of the previous move][target of this move].
    continuation: [[i16; 64]; 64],
    /// Position hashes: first the game history, then the path in the search tree.
    hashes: [u64; GAME_HISTORY + MAX_PLY + 2],
    /// NNUE accumulator stack (a child is computed from its parent, going back needs no `undo`).
    acc: AccumulatorStack,
}

impl SearchTables {
    pub const fn new() -> SearchTables {
        SearchTables {
            moves: [Move::NULL; MOVE_STACK],
            scores: [0; MOVE_STACK],
            history: [[[0; 64]; 6]; 2],
            capture_history: [[[[0; 6]; 64]; 6]; 2],
            butterfly: [[[0; 64]; 64]; 2],
            continuation: [[0; 64]; 64],
            hashes: [0; GAME_HISTORY + MAX_PLY + 2],
            acc: AccumulatorStack::new(),
        }
    }

    /// Clears the histories (new game).
    fn clear_history(&mut self) {
        self.history = [[[0; 64]; 6]; 2];
        self.capture_history = [[[[0; 6]; 64]; 6]; 2];
        self.butterfly = [[[0; 64]; 64]; 2];
        self.continuation = [[0; 64]; 64];
    }
}

impl Default for SearchTables {
    fn default() -> Self {
        Self::new()
    }
}

/// Search result.
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchResult {
    /// Best move (`Move::NULL` if the position has no legal move).
    pub best_move: Move,
    /// Score from the side to move's point of view.
    pub score: i32,
    /// Last completed depth.
    pub depth: u8,
    pub seldepth: u8,
    pub nodes: u64,
    pub elapsed_ms: u64,
    pub pv: [Move; PV_LEN],
    pub pv_len: u8,
}

impl SearchResult {
    pub fn pv(&self) -> &[Move] {
        &self.pv[..self.pv_len as usize]
    }

    pub fn nps(&self) -> u64 {
        self.nodes * 1000 / self.elapsed_ms.max(1)
    }
}

/// Engine = tables + transposition table. One engine computes only one move at a time.
pub struct Engine<'a> {
    tables: &'a mut SearchTables,
    tt: TranspositionTable<'a>,
}

impl<'a> Engine<'a> {
    pub fn new(tables: &'a mut SearchTables, tt_memory: &'a mut [TtEntry]) -> Engine<'a> {
        Engine {
            tables,
            tt: TranspositionTable::new(tt_memory),
        }
    }

    /// Forgets the learned histories and the TT (call at the start of a new game).
    pub fn new_game(&mut self) {
        self.tables.clear_history();
        self.tt.clear();
    }

    /// Finds the best move in position `root`.
    ///
    /// `history` = hashes of the positions that preceded `root` in the game (oldest first),
    /// used for draw by repetition.
    pub fn search<H: SearchHooks>(
        &mut self,
        root: &Position,
        history: &[u64],
        limits: &SearchLimits,
        hooks: &mut H,
    ) -> SearchResult {
        // Game history → start of the hash array (the last GAME_HISTORY positions are enough).
        let keep = history.len().min(GAME_HISTORY);
        let base = keep;
        self.tables.hashes[..keep].copy_from_slice(&history[history.len() - keep..]);

        let start_ms = hooks.now_ms();
        let check_interval = hooks.check_interval().max(1);
        let mut search = Search {
            tables: self.tables,
            tt: &mut self.tt,
            hooks,
            limits: *limits,
            base,
            top: 0,
            nodes: 0,
            next_check: check_interval,
            seldepth: 0,
            depth: 0,
            stopped: false,
            start_ms,
            deadline_ms: limits.move_time_ms.map(|ms| start_ms + ms),
            hard_deadline_ms: limits.hard_time_ms.map(|ms| start_ms + ms),
        };
        search.tables.acc.refresh(root);
        search.iterate(root)
    }
}

/// State of a single search (lives only while one move is being computed).
struct Search<'s, 'a> {
    tables: &'s mut SearchTables,
    tt: &'s mut TranspositionTable<'a>,
    /// Through a trait object: hooks are called only once every `check_interval` nodes, and the
    /// search thus exists in a single copy (entirely in RAM on the ESP32, see the `iram` feature).
    hooks: &'s mut dyn SearchHooks,
    limits: SearchLimits,
    /// How many game history hashes are at the start of `tables.hashes`.
    base: usize,
    /// Top of the shared move stack.
    top: usize,
    nodes: u64,
    next_check: u64,
    seldepth: usize,
    depth: u8,
    stopped: bool,
    start_ms: u64,
    deadline_ms: Option<u64>,
    /// Hard time limit (applies even before `limits.min_depth` is completed).
    hard_deadline_ms: Option<u64>,
}

/// Writes generated moves into the free part of the shared stack.
struct StackSink<'m> {
    moves: &'m mut [Move],
    len: usize,
}

impl MoveSink for StackSink<'_> {
    #[inline]
    fn push(&mut self, mv: Move) {
        // When the stack is full, further moves are dropped (does not happen in practice).
        if self.len < self.moves.len() {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }
}

impl Search<'_, '_> {
    // ------------------------------------------------------------------
    // Iterative deepening (search_it in the original)
    // ------------------------------------------------------------------

    fn iterate(&mut self, root: &Position) -> SearchResult {
        let root_moves = legal_moves(root);
        let mut result = SearchResult::default();
        if root_moves.is_empty() {
            return result;
        }
        result.best_move = root_moves.as_slice()[0];

        // Only one legal move – nothing to compute.
        if root_moves.len() == 1 {
            result.score = self.evaluate(root);
            result.pv[0] = result.best_move;
            result.pv_len = 1;
            result.elapsed_ms = self.elapsed();
            return result;
        }

        let max_depth = self
            .limits
            .max_depth
            .unwrap_or(MAX_PLY as u8 - 4)
            .min(MAX_PLY as u8 - 4);
        let (mut alpha, mut beta) = (-INF, INF);
        let (mut add_alpha, mut add_beta) = (75, 75);
        let (mut alpha_repeat, mut beta_repeat) = (0, 0);
        let mut depth: u8 = 1;
        let mut best_hint = Move::NULL;

        while depth <= max_depth {
            self.tt.new_search();
            self.depth = depth;
            self.seldepth = 0;
            let mut root_best = best_hint;
            let score = self.search(
                root,
                depth as i32,
                alpha,
                beta,
                0,
                0,
                Move::NULL,
                &mut root_best,
            );

            if self.stopped {
                break;
            }

            if score <= alpha {
                // Score below the window → widen downwards and repeat the same depth.
                if alpha_repeat >= 3 {
                    alpha = -MATE;
                } else {
                    beta = (alpha + beta) / 2;
                    alpha = (score - add_alpha).max(-MATE);
                    add_alpha += add_alpha / 15 + 1;
                    alpha_repeat += 1;
                }
            } else if score >= beta {
                // Score above the window → widen upwards.
                if beta_repeat >= 3 {
                    beta = MATE;
                } else {
                    alpha = (alpha + beta) / 2;
                    beta = (score + add_beta).min(MATE);
                    add_beta += add_beta / 15 + 1;
                    beta_repeat += 1;
                }
            } else {
                // Score inside the window → the iteration is valid.
                alpha_repeat = 0;
                beta_repeat = 0;
                add_alpha = 75;
                add_beta = 75;
                alpha = (score - add_alpha).max(-MATE);
                beta = (score + add_beta).min(MATE);

                if !root_best.is_null() && root_moves.contains(root_best) {
                    result.best_move = root_best;
                    best_hint = root_best;
                }
                result.score = score;
                result.depth = depth;
                result.seldepth = self.seldepth.min(255) as u8;
                result.nodes = self.nodes;
                result.elapsed_ms = self.elapsed();
                self.fill_pv(root, result.best_move, &mut result);

                let info = IterationInfo {
                    depth,
                    seldepth: result.seldepth,
                    score,
                    nodes: self.nodes,
                    elapsed_ms: result.elapsed_ms,
                    pv: result.pv,
                    pv_len: result.pv_len,
                };
                self.hooks.on_iteration(&info);

                // A mate that has been found cannot be improved by searching deeper.
                if score.abs() > MAX_NON_MATE && (MATE - score.abs()) as u8 <= depth {
                    break;
                }
                depth += 1;
            }

            if let Some(max_nodes) = self.limits.max_nodes
                && self.nodes >= max_nodes
            {
                break;
            }
        }

        result.nodes = self.nodes;
        result.elapsed_ms = self.elapsed();
        result
    }

    /// Principal variation from the TT (from the best move onwards, legal moves only).
    fn fill_pv(&self, root: &Position, best: Move, result: &mut SearchResult) {
        result.pv[0] = best;
        let mut len = 1;
        let mut pos = root.make_move(best);
        while len < PV_LEN {
            let Some(hit) = self.tt.probe(pos.hash()) else {
                break;
            };
            if hit.mv.is_null() || !legal_moves(&pos).contains(hit.mv) {
                break;
            }
            result.pv[len] = hit.mv;
            len += 1;
            pos = pos.make_move(hit.mv);
        }
        result.pv_len = len as u8;
    }

    // ------------------------------------------------------------------
    // Helper functions
    // ------------------------------------------------------------------

    fn elapsed(&self) -> u64 {
        self.hooks.now_ms().saturating_sub(self.start_ms)
    }

    /// Every `check_interval()` nodes: check time / interruption and report progress.
    #[inline]
    fn count_node(&mut self) {
        self.nodes += 1;
        if self.nodes >= self.next_check {
            self.next_check = self.nodes + self.hooks.check_interval().max(1);
            let now = self.hooks.now_ms();
            // The regular move time applies only after the minimum depth is completed (the depth
            // being searched is `self.depth`, so the completed one is `self.depth - 1`).
            let soft = self.depth > self.limits.min_depth
                && self.deadline_ms.is_some_and(|deadline| now >= deadline);
            let hard = self
                .hard_deadline_ms
                .is_some_and(|deadline| now >= deadline);
            if self.hooks.should_stop() || soft || hard {
                self.stopped = true;
            }
            if self.limits.max_nodes.is_some_and(|max| self.nodes >= max) && self.depth > 1 {
                self.stopped = true;
            }
            let progress = Progress {
                depth: self.depth,
                seldepth: self.seldepth.min(255) as u8,
                nodes: self.nodes,
                elapsed_ms: now.saturating_sub(self.start_ms),
            };
            self.hooks.on_progress(&progress);
        }
    }

    /// Evaluates the position with the NNUE network (+ noise at lower levels); endgames with a
    /// lone king are evaluated by hand in [`crate::endgame`] (the NNUE cannot drive them to mate).
    #[inline]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn evaluate(&mut self, pos: &Position) -> i32 {
        let raw = self.raw_eval(pos);
        self.with_noise(pos, raw)
    }

    /// Evaluation without noise – this is what is stored in the transposition table (the noise
    /// depends on the game).
    #[inline]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn raw_eval(&mut self, pos: &Position) -> i32 {
        self.tables.acc.materialize();
        if let Some(score) = crate::endgame::evaluate(pos) {
            return score;
        }
        self.tables.acc.evaluate(pos.side_to_move())
    }

    /// Static evaluation from the TT (if the entry has one), otherwise computed.
    /// Returns (evaluation with noise, evaluation without noise to store in the TT).
    #[inline]
    fn static_eval(&mut self, pos: &Position, tt_hit: Option<TtHit>) -> (i32, i32) {
        let raw = match tt_hit.and_then(|hit| hit.eval) {
            Some(raw) => raw,
            _ => self.raw_eval(pos),
        };
        (self.with_noise(pos, raw), raw)
    }

    /// Adds deterministic "random" noise based on the position and the game seed (lower levels).
    #[inline]
    fn with_noise(&self, pos: &Position, raw: i32) -> i32 {
        let mut score = raw;
        if self.limits.eval_noise > 0 {
            let mixed =
                (pos.hash() ^ self.limits.noise_seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
            let span = 2 * self.limits.eval_noise as u64 + 1;
            score += (mixed % span) as i32 - self.limits.eval_noise;
        }
        score.clamp(-MAX_NON_MATE, MAX_NON_MATE)
    }

    /// Has the position repeated (since the last irreversible move)? Inside the search a single
    /// repetition is enough to count the position as a draw.
    fn is_repetition(&self, pos: &Position, ply: usize) -> bool {
        let current = self.base + ply;
        let window = (pos.halfmove_clock() as usize).min(current);
        let hash = pos.hash();
        let mut distance = 2;
        while distance <= window {
            if self.tables.hashes[current - distance] == hash {
                return true;
            }
            distance += 2;
        }
        false
    }

    fn is_draw(&self, pos: &Position, ply: usize, fifty_limit: u16) -> bool {
        pos.halfmove_clock() >= fifty_limit
            || self.is_repetition(pos, ply)
            || is_insufficient_material(pos)
    }

    /// Generates moves onto the top of the stack; returns the index range.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn generate_moves(&mut self, pos: &Position, kind: GenType) -> (usize, usize) {
        let start = self.top;
        let mut sink = StackSink {
            moves: &mut self.tables.moves[start..],
            len: 0,
        };
        generate(pos, kind, &mut sink);
        (start, start + sink.len)
    }

    /// Picks the move with the highest score in `index..end` and moves it to `index` (selection sort).
    #[inline]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn pick_best(&mut self, index: usize, end: usize) -> (Move, i32) {
        let moves = &mut self.tables.moves[index..end];
        // black_box: scores as a separate pointer. Otherwise LLVM (loop strength reduction) computes
        // score addresses as "moves + array offset", and the Xtensa backend (ESP32-S3, LLVM esp-20)
        // cannot select such an offset ("Cannot select: Constant<…>"). It does not affect the result.
        let scores = core::hint::black_box(&mut self.tables.scores[index..end]);
        let mut best = 0;
        for i in 1..scores.len() {
            if scores[i] > scores[best] {
                best = i;
            }
        }
        moves.swap(0, best);
        scores.swap(0, best);
        (moves[0], scores[0])
    }

    fn has_any_legal_move(pos: &Position) -> bool {
        chess_core::movegen::has_legal_move(pos)
    }

    // ------------------------------------------------------------------
    // Move ordering
    // ------------------------------------------------------------------

    /// Move score for ordering (port of `sort_movelist_compare::move_evaluater`).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn score_move(&self, pos: &Position, mv: Move, first: &[Move], prev: Move) -> i32 {
        if let Some(i) = first.iter().position(|&f| f == mv && !f.is_null()) {
            return i32::MAX - i as i32;
        }
        let Some(from_kind) = pos.kind_at(mv.from()) else {
            return 0;
        };
        let side = pos.side_to_move().index();
        let mut score = 0;

        if let Some(promo) = mv.promotion_kind() {
            score += (promo.index() as i32) << 19;
        }

        if mv.is_capture() {
            let victim = if mv.is_en_passant() {
                PieceKind::Pawn
            } else {
                pos.kind_at(mv.to()).unwrap_or(PieceKind::Pawn)
            };
            score += (victim.index() as i32) << 19;
            if from_kind != PieceKind::King {
                score += (PieceKind::Queen.index() as i32 - from_kind.index() as i32) << 8;
            }
            score += self.tables.capture_history[side][from_kind.index()][mv.to().index()]
                [victim.index()] as i32
                * 16;
        } else {
            score += self.tables.history[side][from_kind.index()][mv.to().index()] as i32;
            score += self.tables.butterfly[side][mv.from().index()][mv.to().index()] as i32;
            if !prev.is_null() {
                score += self.tables.continuation[prev.to().index()][mv.to().index()] as i32;
            }
        }
        score
    }

    /// Gravity history update – keeps the value within ±HISTORY_MAX.
    #[inline]
    fn update_entry(entry: &mut i16, bonus: i32) {
        let bonus = bonus.clamp(-HISTORY_MAX, HISTORY_MAX);
        let value = *entry as i32;
        *entry = (value + bonus - value * bonus.abs() / HISTORY_MAX) as i16;
    }

    /// After a beta cutoff: reward the move that caused it, penalize the moves tried before it.
    fn update_histories(
        &mut self,
        pos: &Position,
        start: usize,
        cutoff: Move,
        depth: i32,
        prev: Move,
    ) {
        let bonus = depth * 30 - 25;
        let side = pos.side_to_move().index();
        let cutoff_is_capture = cutoff.is_capture();
        for i in start..self.top {
            let mv = self.tables.moves[i];
            if mv.is_capture() != cutoff_is_capture {
                continue;
            }
            let Some(kind) = pos.kind_at(mv.from()) else {
                continue;
            };
            let signed = if mv == cutoff { bonus } else { -bonus };
            if cutoff_is_capture {
                let victim = if mv.is_en_passant() {
                    PieceKind::Pawn
                } else {
                    pos.kind_at(mv.to()).unwrap_or(PieceKind::Pawn)
                };
                Self::update_entry(
                    &mut self.tables.capture_history[side][kind.index()][mv.to().index()]
                        [victim.index()],
                    signed,
                );
            } else {
                Self::update_entry(
                    &mut self.tables.history[side][kind.index()][mv.to().index()],
                    signed,
                );
                Self::update_entry(
                    &mut self.tables.butterfly[side][mv.from().index()][mv.to().index()],
                    signed,
                );
                if !prev.is_null() {
                    Self::update_entry(
                        &mut self.tables.continuation[prev.to().index()][mv.to().index()],
                        signed,
                    );
                }
            }
            if mv == cutoff {
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // Main search (alpha-beta / PVS)
    // ------------------------------------------------------------------

    /// Searches the position to depth `depth`. `hint` = a move that worked in a sibling
    /// branch (tried early if it is a capture); on return it holds the best move found.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn search(
        &mut self,
        pos: &Position,
        mut depth: i32,
        mut alpha: i32,
        beta: i32,
        null_depth: u8,
        ply: usize,
        prev: Move,
        hint: &mut Move,
    ) -> i32 {
        if self.stopped {
            return 0;
        }
        if depth <= 0 {
            return self.quiescence(pos, alpha, beta, ply);
        }
        self.count_node();
        if ply >= MAX_PLY - 1 {
            return self.evaluate(pos);
        }

        let is_root = ply == 0;
        let ply_i = ply as i32;

        // Draws (repetition, 50-move rule, insufficient material).
        if !is_root && self.is_draw(pos, ply, 101) {
            if pos.in_check() && !Self::has_any_legal_move(pos) {
                return -MATE + ply_i;
            }
            return 0;
        }

        let start_alpha = alpha;
        let is_pv = beta - alpha > 1;
        let hash = pos.hash();

        // --- Transposition table ---
        let tt_hit = self.tt.probe(hash);
        let mut tt_move = Move::NULL;
        if let Some(hit) = tt_hit {
            // Quick check that the TT move belongs to this position (hash collisions).
            if !hit.mv.is_null()
                && pos
                    .piece_at(hit.mv.from())
                    .is_some_and(|p| p.color() == pos.side_to_move())
            {
                tt_move = hit.mv;
            }
            if hit.depth >= depth && !is_pv {
                let score = score_from_tt(hit.score, ply_i);
                let usable = match hit.bound {
                    Bound::Exact => true,
                    Bound::Lower => score >= beta,
                    Bound::Upper => score <= alpha,
                };
                if usable && (!tt_move.is_null() || !is_root) {
                    if !tt_move.is_null() {
                        *hint = tt_move;
                    }
                    return score;
                }
            }
        } else if is_pv && depth >= 4 {
            // IIR: without knowing the best move, search one ply shallower.
            depth -= 1;
        }

        // --- Mate distance pruning ---
        if (is_pv && tt_hit.is_some()) || (!is_pv && !tt_move.is_null()) {
            let mdp_alpha = alpha.max(-MATE + ply_i);
            let mdp_beta = beta.min(MATE - ply_i - 1);
            if mdp_alpha >= mdp_beta {
                return mdp_alpha;
            }
        }

        let in_check = pos.in_check();
        let mut static_eval = 0;
        let mut tt_eval = None;
        let mut futility_ok = false;

        if !is_root && !in_check && depth <= 7 && beta <= MAX_NON_MATE {
            let (eval, raw) = self.static_eval(pos, tt_hit);
            static_eval = eval;
            tt_eval = Some(raw);
            futility_ok = depth <= 2;

            // Reverse futility pruning.
            if static_eval - depth * 121 > beta {
                return (beta + static_eval) / 2;
            }

            // Razoring.
            if !is_pv && tt_move.is_null() && depth <= 3 && static_eval + 350 + depth * 150 < alpha
            {
                return self.quiescence(pos, alpha, beta, ply);
            }
        }

        // --- Null move ---
        if depth >= 2
            && !in_check
            && !is_root
            && null_depth < 2
            && pos.has_non_pawn_material(pos.side_to_move())
        {
            let reduction = if depth > 6 { 4 } else { 3 };
            let child = pos.make_null_move();
            self.tables.hashes[self.base + ply] = hash;
            let mut ignore = Move::NULL;
            let null_score = -self.search(
                &child,
                (depth - reduction).max(0),
                -beta,
                -beta + 1,
                null_depth + 1,
                ply + 1,
                Move::NULL,
                &mut ignore,
            );
            if self.stopped {
                return 0;
            }
            if null_score >= beta {
                // Verification (zugzwang protection): the same position without the null move.
                let mut ignore = Move::NULL;
                let verification = self.search(
                    pos,
                    (depth - reduction).max(0),
                    beta - 1,
                    beta,
                    null_depth,
                    ply,
                    prev,
                    &mut ignore,
                );
                if verification >= beta {
                    return if null_score.abs() >= MAX_NON_MATE {
                        beta
                    } else {
                        null_score
                    };
                }
            }
        }

        // --- Move generation and scoring ---
        let (start, end) = self.generate_moves(pos, GenType::All);
        let saved_top = self.top;
        self.top = end;
        let mut first = [Move::NULL; 2];
        first[0] = tt_move;
        if hint.is_capture() {
            first[1] = *hint;
        }
        for i in start..end {
            // black_box(i): see `pick_best` – without it LLVM turns the indices into array offsets
            // that the Xtensa backend cannot handle; the loop body is expensive compared to it.
            let i = core::hint::black_box(i);
            let mv = self.tables.moves[i];
            self.tables.scores[i] = self.score_move(pos, mv, &first, prev);
        }

        let n_moves = end - start;
        let new_depth_basic = depth - 1 + i32::from(in_check || n_moves == 1);
        let recapture_square: Option<Square> = if prev.is_capture() {
            Some(prev.to())
        } else {
            None
        };
        let legality: Legality = pos.legality_with_check(in_check);
        let lmr_start = if !in_check && depth >= 2 { 4 } else { 999 };

        let mut best_score = -INF;
        let mut n_played: i32 = 0;
        let mut cutoff_move = Move::NULL;
        let mut child_hint = Move::NULL;

        for index in start..end {
            let (mv, _) = self.pick_best(index, end);
            if !pos.is_legal(mv, &legality) {
                continue;
            }

            let recapture_ext = i32::from(recapture_square == Some(mv.to()) && mv.is_capture());

            // Futility pruning of quiet moves.
            if futility_ok
                && !is_pv
                && n_played > 0
                && n_moves > 1
                && mv.is_quiet()
                && mv != tt_move
                && static_eval + 180 + depth * 150 < alpha
            {
                continue;
            }

            let (child, changes) = pos.make_move_with_changes(mv);
            self.tables.acc.apply(&changes);
            self.tables.hashes[self.base + ply] = hash;

            let mut score;
            if n_played == 0 {
                score = -self.search(
                    &child,
                    new_depth_basic + recapture_ext,
                    -beta,
                    -alpha,
                    null_depth,
                    ply + 1,
                    mv,
                    &mut child_hint,
                );
            } else {
                let mut new_depth = new_depth_basic + recapture_ext;
                let mut is_lmr = false;
                if n_played >= lmr_start && mv.is_quiet() {
                    is_lmr = true;
                    if !is_pv {
                        let reduction = LMR_REDUCTIONS[(depth as usize).min(63)]
                            [(n_played as usize).min(63)]
                            as i32;
                        new_depth = (new_depth_basic - reduction).max(0);
                    } else if n_played >= lmr_start + 2 {
                        new_depth = new_depth_basic * 2 / 3;
                    } else {
                        new_depth = new_depth_basic - 1;
                    }
                }

                score = -self.search(
                    &child,
                    new_depth,
                    -alpha - 1,
                    -alpha,
                    null_depth,
                    ply + 1,
                    mv,
                    &mut child_hint,
                );
                if is_lmr && score > alpha {
                    score = -self.search(
                        &child,
                        depth - 1,
                        -alpha - 1,
                        -alpha,
                        null_depth,
                        ply + 1,
                        mv,
                        &mut child_hint,
                    );
                }
                if score > alpha && score < beta {
                    score = -self.search(
                        &child,
                        depth - 1,
                        -beta,
                        -alpha,
                        null_depth,
                        ply + 1,
                        mv,
                        &mut child_hint,
                    );
                }
            }

            self.tables.acc.undo(&changes);
            n_played += 1;

            if self.stopped {
                self.top = saved_top;
                return 0;
            }

            if score > best_score {
                best_score = score;
                *hint = mv;
                if score > alpha {
                    if score >= beta {
                        cutoff_move = mv;
                        break;
                    }
                    alpha = score;
                }
            }
        }

        if !cutoff_move.is_null() {
            self.update_histories(pos, start, cutoff_move, depth, prev);
        }
        self.top = saved_top;

        if n_played == 0 {
            best_score = if in_check { -MATE + ply_i } else { 0 };
        }

        if !self.stopped {
            let bound = if best_score <= start_alpha {
                Bound::Upper
            } else if best_score >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            let store_move = if best_score > start_alpha {
                *hint
            } else {
                Move::NULL
            };
            self.tt.store(
                hash,
                bound,
                depth,
                score_to_tt(best_score, ply_i),
                store_move,
                tt_eval,
            );
        }
        best_score
    }

    // ------------------------------------------------------------------
    // Quiescence search
    // ------------------------------------------------------------------

    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        unsafe(link_section = ".rwtext")
    )]
    fn quiescence(&mut self, pos: &Position, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        if self.stopped {
            return 0;
        }
        self.count_node();
        self.seldepth = self.seldepth.max(ply);
        if ply >= MAX_PLY - 1 {
            return self.evaluate(pos);
        }
        let ply_i = ply as i32;

        if self.is_draw(pos, ply, 100) {
            if pos.in_check() && !Self::has_any_legal_move(pos) {
                return -MATE + ply_i;
            }
            return 0;
        }

        let start_alpha = alpha;
        let hash = pos.hash();
        let in_check = pos.in_check();
        let mut best_score = -INF;
        let mut tt_eval = None;

        if !in_check {
            // "Stand pat": the side to move does not have to capture – the static evaluation is
            // enough (from the TT if the position is there).
            let tt_hit = self.tt.probe(hash);
            let (eval, raw) = self.static_eval(pos, tt_hit);
            best_score = eval;
            tt_eval = Some(raw);
            if best_score > alpha && best_score >= beta {
                return best_score;
            }
            alpha = alpha.max(best_score);
        }

        // In check all moves (evasions), otherwise only captures and promotions.
        let kind = if in_check {
            GenType::All
        } else {
            GenType::Tactical
        };
        let (start, end) = self.generate_moves(pos, kind);
        let saved_top = self.top;
        self.top = end;
        for i in start..end {
            let i = core::hint::black_box(i); // see the move scoring loop in `search`
            self.tables.scores[i] = see(pos, self.tables.moves[i]);
        }
        let legality = pos.legality_with_check(in_check);

        let mut n_played = 0;
        let mut best_move = Move::NULL;
        for index in start..end {
            let (mv, see_score) = self.pick_best(index, end);
            // A capture with negative SEE cannot help (check evasions are not pruned).
            if see_score < 0 && !in_check {
                continue;
            }
            if !pos.is_legal(mv, &legality) {
                continue;
            }
            n_played += 1;

            let (child, changes) = pos.make_move_with_changes(mv);
            self.tables.acc.apply(&changes);
            self.tables.hashes[self.base + ply] = hash;
            let score = -self.quiescence(&child, -beta, -alpha, ply + 1);
            self.tables.acc.undo(&changes);

            if self.stopped {
                self.top = saved_top;
                return 0;
            }

            if score > best_score {
                best_score = score;
                best_move = mv;
                if score > alpha {
                    if score >= beta {
                        break;
                    }
                    alpha = score;
                }
            }
            if n_played >= 3 && best_score >= MAX_NON_MATE {
                break;
            }
        }
        self.top = saved_top;

        if n_played == 0 {
            if in_check {
                best_score = -MATE + ply_i;
            } else if best_score == -INF {
                best_score = self.evaluate(pos);
            }
        }

        if !self.stopped {
            let bound = if best_score <= start_alpha {
                Bound::Upper
            } else if best_score >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            let store_move = if best_score > start_alpha {
                best_move
            } else {
                Move::NULL
            };
            self.tt.store(
                hash,
                bound,
                0,
                score_to_tt(best_score, ply_i),
                store_move,
                tt_eval,
            );
        }
        best_score
    }
}
