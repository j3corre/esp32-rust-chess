//! UCI over USB (feature `uci`): the board as a chess engine for a program on a PC.
//!
//! In this mode the firmware starts neither Wi-Fi, the web nor the display – only the engine.
//! A chess GUI on the PC (Arena, cutechess, …) talks to the board through
//! `scripts/unix/esp-uci.py`, which forwards lines between stdin/stdout and the board's
//! USB-Serial-JTAG serial port.
//!
//! - The [`reader_task`] task reads commands from USB. `stop`/`quit` set [`STOP`] immediately
//!   (aborting the search), other lines are queued for the engine thread.
//! - The engine thread ([`run`]) processes the commands in order and replies through the
//!   write half of the USB driver ([`send`] waits until the PC has taken the data –
//!   `esp_println` would drop it when its buffer is full and e.g. a `bestmove` would be
//!   lost). Logging is disabled.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use chess_core::{Position, parse_uci};
use chess_engine::{Engine, IterationInfo, SearchHooks, SearchLimits, mate_in};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Timer};
use esp_hal::Blocking;
use esp_hal::usb::usb_serial_jtag::{UsbSerialJtagRx, UsbSerialJtagTx};
use portable_atomic::{AtomicBool, Ordering};

use crate::shared::now_ms;

/// Abort the running search (`stop`, `quit`).
static STOP: AtomicBool = AtomicBool::new(false);
/// Commands for the engine thread.
static QUEUE: Mutex<CriticalSectionRawMutex, RefCell<VecDeque<String>>> =
    Mutex::new(RefCell::new(VecDeque::new()));
/// Write half of the USB driver (used only by the engine thread).
static TX: Mutex<CriticalSectionRawMutex, RefCell<Option<UsbSerialJtagTx<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

/// Sets the output (called by `main` before the engine thread starts).
pub fn set_output(tx: UsbSerialJtagTx<'static, Blocking>) {
    TX.lock(|cell| *cell.borrow_mut() = Some(tx));
}

/// Sends a line to the PC; waits until USB has sent it (nothing is dropped).
fn send(args: core::fmt::Arguments) {
    use core::fmt::Write as _;
    let mut line = String::new();
    let _ = line.write_fmt(args);
    line.push('\n');
    // The driver is only briefly taken out of the mutex – writing (waiting for USB) runs outside
    // the critical section.
    let Some(mut tx) = TX.lock(|cell| cell.borrow_mut().take()) else {
        return;
    };
    for byte in line.bytes() {
        while tx.write_byte_nb(byte).is_err() {}
    }
    let _ = tx.flush_tx();
    TX.lock(|cell| *cell.borrow_mut() = Some(tx));
}

macro_rules! println {
    ($($arg:tt)*) => { send(format_args!($($arg)*)) };
}

/// Longest accepted line (`position startpos moves …` of a long game is ~2 kB).
const MAX_LINE: usize = 8 * 1024;

/// Reads commands from USB-Serial-JTAG (byte by byte, waits 1 ms when there is no data).
#[embassy_executor::task]
pub async fn reader_task(mut usb: UsbSerialJtagRx<'static, Blocking>) -> ! {
    let mut line = String::new();
    loop {
        match usb.read_byte() {
            Ok(b'\n' | b'\r') => {
                if !line.trim().is_empty() {
                    dispatch(core::mem::take(&mut line));
                }
                line.clear();
            }
            Ok(byte) => {
                if line.len() < MAX_LINE {
                    line.push(byte as char);
                }
            }
            Err(_) => Timer::after(Duration::from_millis(1)).await,
        }
    }
}

fn dispatch(line: String) {
    let cmd = line.split_whitespace().next().unwrap_or("");
    if cmd == "stop" || cmd == "quit" {
        STOP.store(true, Ordering::Relaxed);
        return;
    }
    QUEUE.lock(|q| q.borrow_mut().push_back(line));
}

fn next_command() -> Option<String> {
    QUEUE.lock(|q| q.borrow_mut().pop_front())
}

struct UciHooks {
    start_ms: u64,
}

impl SearchHooks for UciHooks {
    fn now_ms(&self) -> u64 {
        now_ms() - self.start_ms
    }

    fn check_interval(&self) -> u64 {
        128
    }

    fn should_stop(&self) -> bool {
        STOP.load(Ordering::Relaxed)
    }

    fn on_iteration(&mut self, info: &IterationInfo) {
        let nps = info.nodes * 1000 / info.elapsed_ms.max(1);
        let mut pv = String::new();
        for mv in info.pv() {
            use core::fmt::Write as _;
            let _ = write!(pv, " {mv}");
        }
        match mate_in(info.score) {
            Some(moves) => println!(
                "info depth {} seldepth {} score mate {moves} nodes {} nps {nps} time {} pv{pv}",
                info.depth, info.seldepth, info.nodes, info.elapsed_ms
            ),
            None => println!(
                "info depth {} seldepth {} score cp {} nodes {} nps {nps} time {} pv{pv}",
                info.depth, info.seldepth, info.score, info.nodes, info.elapsed_ms
            ),
        }
    }
}

/// Engine thread loop in UCI mode (never returns).
pub fn run(engine: &mut Engine) -> ! {
    // From here on only the UCI protocol, no logs.
    // (On the ESP32-C3 without atomic instructions `log` lacks this function – nothing is
    // logged there after startup anyway, and the bridge drops all other lines.)
    #[cfg(target_has_atomic = "ptr")]
    log::set_max_level(log::LevelFilter::Off);
    let mut position = Position::start();
    let mut history: Vec<u64> = Vec::new();
    println!("info string ESP32 Chess UCI {} ready", crate::chip::NAME);
    loop {
        let Some(line) = next_command() else {
            esp_radio_rtos_driver::usleep(1_000);
            continue;
        };
        let mut words = line.split_whitespace();
        match words.next() {
            Some("uci") => {
                println!(
                    "id name ESP32 Chess {} {} ({})",
                    crate::chip::NAME,
                    env!("CARGO_PKG_VERSION"),
                    env!("FW_GIT")
                );
                println!("id author Jan Blaha");
                println!("uciok");
            }
            Some("isready") => println!("readyok"),
            Some("ucinewgame") => engine.new_game(),
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
                STOP.store(false, Ordering::Relaxed);
                let mut hooks = UciHooks { start_ms: now_ms() };
                let result = engine.search(&position, &history, &limits, &mut hooks);
                println!("bestmove {}", result.best_move);
            }
            _ => {}
        }
    }
}

/// `position startpos moves e2e4 …` / `position fen <fen> moves …`
fn parse_position(words: &[&str]) -> Option<(Position, Vec<u64>)> {
    let moves_at = words
        .iter()
        .position(|&w| w == "moves")
        .unwrap_or(words.len());
    let mut pos = match *words.first()? {
        "startpos" => Position::start(),
        "fen" => {
            let mut fen = String::new();
            for (i, w) in words[1..moves_at].iter().enumerate() {
                if i > 0 {
                    fen.push(' ');
                }
                fen.push_str(w);
            }
            Position::from_fen(&fen).ok()?
        }
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

/// `go movetime …` / `go depth …` / `go nodes …` / `go wtime … btime … winc … binc …` / `go
/// infinite`
/// Time management as in `sim` (UCI on a PC): 1/30 of the remaining time + half the increment.
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
        Some(time) => SearchLimits::move_time((time / 30 + inc.unwrap_or(0) / 2).max(10)),
        None => SearchLimits::depth(90),
    }
}
