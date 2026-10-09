//! # chess-engine
//!
//! A chess engine – a Rust rewrite of [tinychess](https://github.com/Timmy6942025/tinychess)
//! (a fork of the *Dog* engine by Folkert van Heusden, MIT license).
//! The library is `no_std` and does not use a heap: all memory (search tables,
//! transposition table) is provided by the caller.
//!
//! | Module   | Contents |
//! |----------|----------|
//! | `nnue`   | neural network for position evaluation (weights in flash) |
//! | `search` | iterative deepening, PVS, quiescence search |
//! | `tt`     | transposition table |
//! | `see`    | static exchange evaluation |
//! | `limits` | limits, difficulty levels, the `SearchHooks` interface |
//!
//! ## Example
//!
//! ```
//! use chess_engine::{Engine, SearchLimits, SearchTables, SearchHooks, TtEntry};
//! use chess_core::Position;
//!
//! struct Clock;
//! impl SearchHooks for Clock {
//!     fn now_ms(&self) -> u64 { 0 }
//! }
//!
//! let mut tables = Box::new(SearchTables::new());
//! let mut tt = vec![TtEntry::EMPTY; 4096];
//! let mut engine = Engine::new(&mut tables, &mut tt);
//! let result = engine.search(&Position::start(), &[], &SearchLimits::depth(3), &mut Clock);
//! assert!(!result.best_move.is_null());
//! ```

#![no_std]

pub mod endgame;
pub mod limits;
pub mod nnue;
pub mod search;
pub mod see;
pub mod tt;

pub use limits::{
    IterationInfo, LEVEL_COUNT, PV_LEN, Progress, SearchHooks, SearchLimits, level_limits, mate_in,
};
pub use nnue::{Accumulator, AccumulatorStack, MAX_NON_MATE, evaluate_position};
pub use search::{Engine, MATE, SearchResult, SearchTables};
pub use tt::TtEntry;
