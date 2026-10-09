//! Zobrist hashing – every position gets a 64-bit fingerprint.
//!
//! The fingerprint is the XOR of random numbers for every piece on its square, the castling
//! rights, the en passant file and the side to move. A move updates it with just a few XORs
//! (no full recomputation). Used by the engine's transposition table and for repetition
//! detection.

use crate::types::{Piece, Square};

/// Random number tables (computed at compile time).
pub struct Keys {
    pub pieces: [[u64; 64]; 12],
    pub castling: [u64; 16],
    pub en_passant: [u64; 8],
    pub side: u64,
}

/// Simple pseudo-random number generator (SplitMix64), usable in `const fn`.
const fn splitmix(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (state, z ^ (z >> 31))
}

const fn build_keys() -> Keys {
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut keys = Keys {
        pieces: [[0; 64]; 12],
        castling: [0; 16],
        en_passant: [0; 8],
        side: 0,
    };
    let mut p = 0;
    while p < 12 {
        let mut s = 0;
        while s < 64 {
            let (next, value) = splitmix(state);
            state = next;
            keys.pieces[p][s] = value;
            s += 1;
        }
        p += 1;
    }
    let mut i = 0;
    while i < 16 {
        let (next, value) = splitmix(state);
        state = next;
        keys.castling[i] = value;
        i += 1;
    }
    let mut i = 0;
    while i < 8 {
        let (next, value) = splitmix(state);
        state = next;
        keys.en_passant[i] = value;
        i += 1;
    }
    let (_, value) = splitmix(state);
    keys.side = value;
    keys
}

pub static KEYS: Keys = build_keys();

#[inline]
pub fn piece_key(piece: Piece, sq: Square) -> u64 {
    KEYS.pieces[piece.index()][sq.index()]
}
