//! Engine benchmark on the PC: `cargo run --release -p chess-engine --example bench`
//!
//! Prints the evaluation of a few positions and the search speed (nodes per second).
//! On the ESP32-C6 the speed is roughly 100–300× lower.

use std::time::Instant;

use chess_core::Position;
use chess_engine::{
    Engine, IterationInfo, SearchHooks, SearchLimits, SearchTables, TtEntry, evaluate_position,
};

struct Hooks(Instant);

impl SearchHooks for Hooks {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }

    fn on_iteration(&mut self, info: &IterationInfo) {
        let pv: Vec<String> = info.pv().iter().map(|m| m.to_string()).collect();
        println!(
            "  depth {:>2}  score {:>6}  nodes {:>9}  {:>6} ms  pv {}",
            info.depth,
            info.score,
            info.nodes,
            info.elapsed_ms,
            pv.join(" ")
        );
    }
}

fn main() {
    let positions = [
        chess_core::START_FEN,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5Q2/PPPP1PPP/RNB1K1NR w KQkq - 4 4",
    ];
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; 1 << 16];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let mut total_nodes = 0;
    let mut total_ms = 0;
    for fen in positions {
        let pos = Position::from_fen(fen).unwrap();
        println!("{fen}\n  static NNUE eval: {}", evaluate_position(&pos));
        engine.new_game();
        let result = engine.search(
            &pos,
            &[],
            &SearchLimits::move_time(2_000),
            &mut Hooks(Instant::now()),
        );
        println!(
            "  → best move {}  ({} nodes/s)\n",
            result.best_move,
            result.nps()
        );
        total_nodes += result.nodes;
        total_ms += result.elapsed_ms;
    }
    println!("Total: {} nodes/s", total_nodes * 1000 / total_ms.max(1));
}
