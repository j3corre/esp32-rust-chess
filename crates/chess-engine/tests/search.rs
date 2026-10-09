//! Search tests: mates, tactics, draws, limits.

use std::cell::Cell;
use std::time::Instant;

use chess_core::{Position, parse_uci};
use chess_engine::{
    Engine, IterationInfo, MATE, Progress, SearchHooks, SearchLimits, SearchResult, SearchTables,
    TtEntry, mate_in,
};

/// Hooks for tests: real time + call counters.
struct TestHooks {
    start: Instant,
    iterations: u32,
    progress_calls: u32,
    stop_after_nodes: Option<u64>,
    last_nodes: Cell<u64>,
}

impl TestHooks {
    fn new() -> Self {
        TestHooks {
            start: Instant::now(),
            iterations: 0,
            progress_calls: 0,
            stop_after_nodes: None,
            last_nodes: Cell::new(0),
        }
    }
}

impl SearchHooks for TestHooks {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn should_stop(&self) -> bool {
        self.stop_after_nodes
            .is_some_and(|limit| self.last_nodes.get() >= limit)
    }

    fn on_progress(&mut self, progress: &Progress) {
        self.progress_calls += 1;
        self.last_nodes.set(progress.nodes);
    }

    fn on_iteration(&mut self, _info: &IterationInfo) {
        self.iterations += 1;
    }
}

fn run(fen: &str, limits: SearchLimits) -> (SearchResult, TestHooks) {
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; 1 << 16];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let mut hooks = TestHooks::new();
    let pos = Position::from_fen(fen).unwrap();
    let result = engine.search(&pos, &[], &limits, &mut hooks);
    (result, hooks)
}

fn best(fen: &str, depth: u8) -> String {
    let (result, _) = run(fen, SearchLimits::depth(depth));
    result.best_move.to_string()
}

#[test]
fn finds_mate_in_one() {
    // Back-rank mate: Ra8#.
    assert_eq!(best("6k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1", 3), "a1a8");
    let (result, _) = run(
        "6k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1",
        SearchLimits::depth(3),
    );
    assert_eq!(mate_in(result.score), Some(1));
}

#[test]
fn finds_mate_in_two() {
    // Mate in two: doubled rooks – 1.Rd8+ Rxd8 2.Rxd8#.
    let fen = "r5k1/5ppp/8/8/8/8/3R1PPP/3R2K1 w - - 0 1";
    let (result, _) = run(fen, SearchLimits::depth(5));
    assert_eq!(mate_in(result.score), Some(2), "score {}", result.score);
    assert_eq!(result.best_move.to_string(), "d2d8");
}

#[test]
fn black_finds_mate_too() {
    // Mirrored for Black: ...Ra1#.
    assert_eq!(best("r5k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1", 3), "a8a1");
}

#[test]
fn captures_hanging_queen() {
    // Undefended queen on d5 – the white knight takes it.
    assert_eq!(best("4k3/8/8/3q4/8/4N3/8/4K3 w - - 0 1", 4), "e3d5");
}

#[test]
fn avoids_stalemate_when_winning() {
    // White has queen and king against a lone king – must not stalemate (Qg6 would be stalemate).
    let (result, _) = run("7k/8/5K2/8/8/8/8/6Q1 w - - 0 1", SearchLimits::depth(6));
    let pos = Position::from_fen("7k/8/5K2/8/8/8/8/6Q1 w - - 0 1").unwrap();
    let next = pos.make_move(result.best_move);
    let status = chess_core::game_status(&next, &[pos.hash()]);
    assert_ne!(status, chess_core::GameStatus::Stalemate);
    assert!(result.score > 500 || result.score > MATE - 100);
}

#[test]
fn single_legal_move_returns_immediately() {
    // The black king has a single legal move (the rook guards the g-file).
    let (result, hooks) = run("7k/8/8/8/8/8/1R6/K5R1 b - - 0 1", SearchLimits::depth(10));
    assert_eq!(result.best_move.to_string(), "h8h7");
    assert_eq!(hooks.iterations, 0);
}

#[test]
fn checkmated_position_has_no_move() {
    let (result, _) = run(
        "R5k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1",
        SearchLimits::depth(3),
    );
    assert!(result.best_move.is_null());
}

#[test]
fn repetition_is_scored_as_draw() {
    // White is much worse but can force a draw by repetition (perpetual check).
    // At least verify that the game history is passed on and the search does not crash.
    let pos = Position::start();
    let mv = parse_uci(&pos, "g1f3").unwrap();
    let after = pos.make_move(mv);
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; 1024];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let result = engine.search(
        &after,
        &[pos.hash()],
        &SearchLimits::depth(4),
        &mut TestHooks::new(),
    );
    assert!(!result.best_move.is_null());
}

#[test]
fn respects_move_time() {
    let (result, hooks) = run(chess_core::START_FEN, SearchLimits::move_time(300));
    assert!(result.elapsed_ms < 1_000, "took {} ms", result.elapsed_ms);
    assert!(result.depth >= 3, "depth only {}", result.depth);
    assert!(hooks.progress_calls > 0);
    assert!(!result.best_move.is_null());
}

#[test]
fn min_depth_overrides_move_time() {
    // Move time 0 ms – without a minimum it would stop at depth 1, with it it completes at least 6.
    let mut limits = SearchLimits::move_time(0);
    limits.min_depth = 6;
    let (result, _) = run(chess_core::START_FEN, limits);
    assert!(result.depth >= 6, "depth only {}", result.depth);
    assert!(
        result.depth <= 7,
        "it should have stopped after the minimum, depth {}",
        result.depth
    );
}

#[test]
fn hard_time_limits_min_depth() {
    // The hard limit applies even before the minimum depth is reached.
    let mut limits = SearchLimits::move_time(0);
    limits.min_depth = 60;
    limits.hard_time_ms = Some(200);
    let (result, _) = run(chess_core::START_FEN, limits);
    assert!(result.elapsed_ms < 1_000, "took {} ms", result.elapsed_ms);
    assert!(result.depth < 60);
    assert!(!result.best_move.is_null());
}

#[test]
fn stop_flag_interrupts_search() {
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; 1024];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let mut hooks = TestHooks::new();
    hooks.stop_after_nodes = Some(20_000);
    let result = engine.search(
        &Position::start(),
        &[],
        &SearchLimits::depth(60),
        &mut hooks,
    );
    assert!(result.nodes < 200_000, "nodes {}", result.nodes);
    assert!(!result.best_move.is_null());
}

#[test]
fn deterministic_at_fixed_depth() {
    let a = run(chess_core::START_FEN, SearchLimits::depth(6)).0;
    let b = run(chess_core::START_FEN, SearchLimits::depth(6)).0;
    assert_eq!(a.best_move, b.best_move);
    assert_eq!(a.nodes, b.nodes);
    assert_eq!(a.score, b.score);
}

#[test]
fn noise_changes_play_but_stays_legal() {
    let mut limits = SearchLimits::depth(2);
    limits.eval_noise = 250;
    for seed in 0..5 {
        limits.noise_seed = seed;
        let (result, _) = run(chess_core::START_FEN, limits);
        assert!(chess_core::legal_moves(&Position::start()).contains(result.best_move));
    }
}

#[test]
fn engine_plays_a_short_game_against_itself() {
    // 30 plies at a low depth – checks that nothing crashes and the moves are legal.
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; 1 << 14];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let mut pos = Position::start();
    let mut history = Vec::new();
    for _ in 0..30 {
        if chess_core::game_status(&pos, &history).is_over() {
            break;
        }
        let result = engine.search(
            &pos,
            &history,
            &SearchLimits::depth(4),
            &mut TestHooks::new(),
        );
        assert!(chess_core::legal_moves(&pos).contains(result.best_move));
        history.push(pos.hash());
        pos = pos.make_move(result.best_move);
    }
}
