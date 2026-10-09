//! UCI mode – the engine as a program for chess GUIs (Arena, cutechess-cli, …).
//! Allows comparing the strength of the Rust port with the original tinychess on a PC.

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use chess_core::{Position, parse_uci};
use chess_engine::{
    Engine, IterationInfo, SearchHooks, SearchLimits, SearchTables, TtEntry, mate_in,
};

struct UciHooks {
    start: Instant,
    stop: Arc<AtomicBool>,
}

impl SearchHooks for UciHooks {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn on_iteration(&mut self, info: &IterationInfo) {
        let score = match mate_in(info.score) {
            Some(moves) => format!("mate {moves}"),
            None => format!("cp {}", info.score),
        };
        let pv: Vec<String> = info.pv().iter().map(|m| m.to_string()).collect();
        let nps = info.nodes * 1000 / info.elapsed_ms.max(1);
        println!(
            "info depth {} seldepth {} score {score} nodes {} nps {nps} time {} pv {}",
            info.depth,
            info.seldepth,
            info.nodes,
            info.elapsed_ms,
            pv.join(" ")
        );
    }
}

/// Default transposition table size in UCI mode (MB).
const DEFAULT_HASH_MB: usize = 16;

pub fn run() {
    // Input is read by a separate thread so a running search can be interrupted with `stop`.
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<String>();
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let cmd = line.split_whitespace().next().unwrap_or("");
                if cmd == "stop" || cmd == "quit" {
                    stop.store(true, Ordering::Relaxed);
                }
                if tx.send(line).is_err() {
                    break;
                }
            }
            stop.store(true, Ordering::Relaxed);
            let _ = tx.send("quit".into());
        });
    }

    let mut tables = Box::new(SearchTables::new());
    let mut hash_mb = DEFAULT_HASH_MB;
    // Exact number of TT entries (0 = derived from Hash) – for tests with the board's size.
    let mut tt_entries = 0usize;
    let mut position = Position::start();
    let mut history: Vec<u64> = Vec::new();

    'outer: loop {
        let entries = if tt_entries > 0 {
            tt_entries
        } else {
            (hash_mb * 1024 * 1024 / std::mem::size_of::<TtEntry>()).max(1024)
        };
        let mut tt = vec![TtEntry::EMPTY; entries];
        let mut engine = Engine::new(&mut tables, &mut tt);
        while let Ok(line) = rx.recv() {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("uci") => {
                    println!("id name ESP32 Chess (Rust port of tinychess/Dog)");
                    println!("id author Jan Blaha");
                    println!("option name Hash type spin default {DEFAULT_HASH_MB} min 1 max 4096");
                    println!("option name Threads type spin default 1 min 1 max 1");
                    println!("option name TTEntries type spin default 0 min 0 max 100000000");
                    println!("uciok");
                }
                Some("isready") => println!("readyok"),
                Some("ucinewgame") => engine.new_game(),
                Some("setoption") => {
                    let rest: Vec<&str> = words.collect();
                    if let Some(mb) =
                        parse_setoption(&rest, "hash").and_then(|v| v.parse::<usize>().ok())
                    {
                        hash_mb = mb.clamp(1, 4096);
                        continue 'outer; // new table → new engine
                    }
                    if let Some(n) =
                        parse_setoption(&rest, "ttentries").and_then(|v| v.parse::<usize>().ok())
                    {
                        tt_entries = n;
                        continue 'outer;
                    }
                }
                Some("position") => {
                    let rest: Vec<&str> = words.collect();
                    if let Some((pos, hist)) = parse_position(&rest) {
                        position = pos;
                        history = hist;
                    }
                }
                Some("go") => {
                    let args: Vec<&str> = words.collect();
                    let limits = parse_go(&args, &position);
                    stop.store(false, Ordering::Relaxed);
                    let mut hooks = UciHooks {
                        start: Instant::now(),
                        stop: stop.clone(),
                    };
                    let result = engine.search(&position, &history, &limits, &mut hooks);
                    println!("bestmove {}", result.best_move);
                }
                Some("quit") => break 'outer,
                _ => {}
            }
            let _ = std::io::stdout().flush();
        }
        break;
    }
}

/// `setoption name <name> value <value>` → the value if the name matches (case-insensitive).
fn parse_setoption(words: &[&str], name: &str) -> Option<String> {
    let n = words.iter().position(|&w| w == "name")?;
    let v = words.iter().position(|&w| w == "value")?;
    let opt = words.get(n + 1..v)?.join(" ");
    opt.eq_ignore_ascii_case(name)
        .then(|| words[v + 1..].join(" "))
}

/// `position startpos moves e2e4 …` / `position fen <fen> moves …`
fn parse_position(words: &[&str]) -> Option<(Position, Vec<u64>)> {
    let moves_at = words
        .iter()
        .position(|&w| w == "moves")
        .unwrap_or(words.len());
    let mut pos = match *words.first()? {
        "startpos" => Position::start(),
        "fen" => Position::from_fen(&words[1..moves_at].join(" ")).ok()?,
        _ => return None,
    };
    let mut history = Vec::new();
    for text in words.iter().skip(moves_at + 1) {
        let mv = parse_uci(&pos, text)?;
        history.push(pos.hash());
        pos = pos.make_move(mv);
    }
    Some((pos, history))
}

/// `go nodes 5000` / `go movetime 1000` / `go depth 8` / `go wtime … btime … winc … binc …` / `go infinite`
fn parse_go(args: &[&str], pos: &Position) -> SearchLimits {
    let value = |key: &str| {
        args.iter()
            .position(|&a| a == key)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    if let Some(nodes) = value("nodes") {
        let mut limits = SearchLimits::depth(90);
        limits.max_nodes = Some(nodes);
        return limits;
    }
    if let Some(depth) = value("depth") {
        return SearchLimits::depth(depth.min(90) as u8);
    }
    if let Some(ms) = value("movetime") {
        return SearchLimits::move_time(ms);
    }
    let (time, inc) = match pos.side_to_move() {
        chess_core::Color::White => (value("wtime"), value("winc")),
        chess_core::Color::Black => (value("btime"), value("binc")),
    };
    match time {
        // Simple time management: 1/30 of the remaining time + half the increment.
        Some(time) => SearchLimits::move_time((time / 30 + inc.unwrap_or(0) / 2).max(10)),
        None => SearchLimits::depth(90),
    }
}
