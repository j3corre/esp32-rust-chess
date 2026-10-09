//! Move (`Move`) and move list (`MoveList`).
//!
//! A move is stored in 16 bits: 6 bits from-square, 6 bits to-square, 4 bits move kind.
//! This keeps it small (saves RAM in the transposition table and in move lists).

use crate::types::{PieceKind, Square};

/// Move kind (upper 4 bits).
pub mod flag {
    pub const QUIET: u16 = 0;
    pub const DOUBLE_PUSH: u16 = 1;
    pub const KING_CASTLE: u16 = 2;
    pub const QUEEN_CASTLE: u16 = 3;
    pub const CAPTURE: u16 = 4;
    pub const EN_PASSANT: u16 = 5;
    /// Promotion: 8 + (knight 0, bishop 1, rook 2, queen 3); +4 = capturing promotion.
    pub const PROMOTION: u16 = 8;
    pub const PROMOTION_CAPTURE: u16 = 12;
}

/// A single move. `Move::NULL` (all zeros) means "no move".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Move(u16);

impl Move {
    /// Empty move (a1→a1), used as "nothing".
    pub const NULL: Move = Move(0);

    #[inline]
    pub const fn new(from: Square, to: Square, flags: u16) -> Move {
        Move(from.raw() as u16 | (to.raw() as u16) << 6 | flags << 12)
    }

    /// Pawn promotion to `kind` (with or without a capture).
    #[inline]
    pub const fn promotion(from: Square, to: Square, kind: PieceKind, capture: bool) -> Move {
        let base = if capture {
            flag::PROMOTION_CAPTURE
        } else {
            flag::PROMOTION
        };
        Move::new(from, to, base + kind as u16 - 1)
    }

    /// Raw 16-bit value (for storing in the TT).
    #[inline]
    pub const fn raw(self) -> u16 {
        self.0
    }

    #[inline]
    pub const fn from_raw(raw: u16) -> Move {
        Move(raw)
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn from(self) -> Square {
        Square::new((self.0 & 63) as u8)
    }

    #[inline]
    pub const fn to(self) -> Square {
        Square::new(((self.0 >> 6) & 63) as u8)
    }

    #[inline]
    pub const fn flags(self) -> u16 {
        self.0 >> 12
    }

    /// Does the move capture an enemy piece (including en passant and capturing promotions)?
    #[inline]
    pub const fn is_capture(self) -> bool {
        self.flags() & flag::CAPTURE != 0
    }

    #[inline]
    pub const fn is_promotion(self) -> bool {
        self.flags() & flag::PROMOTION != 0
    }

    #[inline]
    pub const fn is_en_passant(self) -> bool {
        self.flags() == flag::EN_PASSANT
    }

    #[inline]
    pub const fn is_castle(self) -> bool {
        self.flags() == flag::KING_CASTLE || self.flags() == flag::QUEEN_CASTLE
    }

    /// Quiet move = neither a capture nor a promotion (for the engine's move history).
    #[inline]
    pub const fn is_quiet(self) -> bool {
        self.flags() & (flag::CAPTURE | flag::PROMOTION) == 0
    }

    /// The piece the pawn promotes to (promotions only).
    #[inline]
    pub fn promotion_kind(self) -> Option<PieceKind> {
        if self.is_promotion() {
            Some(PieceKind::from_index(((self.flags() & 3) + 1) as usize))
        } else {
            None
        }
    }

    /// UCI notation, e.g. "e2e4", "e7e8q". Returns a byte array and a length.
    pub fn uci(self) -> UciText {
        let mut text = UciText {
            bytes: [0; 5],
            len: 4,
        };
        let from = self.from().name();
        let to = self.to().name();
        text.bytes[..2].copy_from_slice(&from);
        text.bytes[2..4].copy_from_slice(&to);
        if let Some(kind) = self.promotion_kind() {
            text.bytes[4] = kind.letter().to_ascii_lowercase() as u8;
            text.len = 5;
        }
        text
    }
}

/// Short allocation-free UCI move text.
#[derive(Clone, Copy)]
pub struct UciText {
    bytes: [u8; 5],
    len: usize,
}

impl UciText {
    pub fn as_str(&self) -> &str {
        // Contains only ASCII characters we wrote ourselves.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("????")
    }
}

impl core::fmt::Display for Move {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.uci().as_str())
    }
}

/// Something the move generator can store moves into. Implemented by `MoveList`
/// and by the engine's shared move stack (saves RAM).
pub trait MoveSink {
    fn push(&mut self, mv: Move);
}

/// The maximum number of legal moves in any position is 218.
pub const MAX_MOVES: usize = 256;

/// Fixed-capacity move list (no heap).
#[derive(Clone)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    pub const fn new() -> MoveList {
        MoveList {
            moves: [Move::NULL; MAX_MOVES],
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn contains(&self, mv: Move) -> bool {
        self.as_slice().contains(&mv)
    }

    pub fn iter(&self) -> core::slice::Iter<'_, Move> {
        self.as_slice().iter()
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

impl MoveSink for MoveList {
    #[inline]
    fn push(&mut self, mv: Move) {
        // A legal position cannot have more than 256 moves; this is an overflow guard.
        if self.len < MAX_MOVES {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }
}

impl<'a> IntoIterator for &'a MoveList {
    type Item = &'a Move;
    type IntoIter = core::slice::Iter<'a, Move>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_roundtrip() {
        let e2 = Square::parse("e2").unwrap();
        let e4 = Square::parse("e4").unwrap();
        let mv = Move::new(e2, e4, flag::DOUBLE_PUSH);
        assert_eq!(mv.from(), e2);
        assert_eq!(mv.to(), e4);
        assert!(!mv.is_capture());
        assert_eq!(mv.uci().as_str(), "e2e4");

        let e7 = Square::parse("e7").unwrap();
        let d8 = Square::parse("d8").unwrap();
        let promo = Move::promotion(e7, d8, PieceKind::Queen, true);
        assert!(promo.is_capture());
        assert!(promo.is_promotion());
        assert_eq!(promo.promotion_kind(), Some(PieceKind::Queen));
        assert_eq!(promo.uci().as_str(), "e7d8q");
        let knight = Move::promotion(e7, d8, PieceKind::Knight, false);
        assert_eq!(knight.promotion_kind(), Some(PieceKind::Knight));
        assert!(!knight.is_capture());
    }
}
