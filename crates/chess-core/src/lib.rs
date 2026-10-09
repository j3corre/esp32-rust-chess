//! # chess-core
//!
//! Hardware-independent chess rules. The library is `no_std`
//! (runs on a microcontroller as well as on a PC) and does not use the heap.
//!
//! | Module     | Contents |
//! |------------|----------|
//! | `types`    | color, piece, square |
//! | `attacks`  | bitboards and piece attacks |
//! | `moves`    | move (16 bits), move list |
//! | `position` | position, making moves, FEN |
//! | `movegen`  | move generator, perft |
//! | `notation` | SAN and UCI notation |
//! | `rules`    | checkmate, stalemate, draws |
//! | `zobrist`  | position hash |

#![no_std]

pub mod attacks;
pub mod movegen;
pub mod moves;
pub mod notation;
pub mod position;
pub mod rules;
pub mod types;
pub mod zobrist;

pub use attacks::Bitboard;
pub use movegen::{GenType, generate, legal_moves, perft};
pub use moves::{Move, MoveList, MoveSink};
pub use notation::{SanText, parse_uci, to_san};
pub use position::{FenError, Legality, PieceChanges, Position, START_FEN};
pub use rules::{GameStatus, game_status};
pub use types::{Color, Piece, PieceKind, Square};

#[cfg(test)]
extern crate std;
