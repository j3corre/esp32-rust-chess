//! Basic types: color, piece kind, piece and board square.

/// Color of a player (and of pieces).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    /// Both colors – handy for `for color in Color::ALL` loops.
    pub const ALL: [Color; 2] = [Color::White, Color::Black];

    /// The opposite color (opponent).
    #[inline]
    pub const fn opponent(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    /// Array index (0 = white, 1 = black).
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Sign for evaluations: +1 for white, −1 for black.
    #[inline]
    pub const fn sign(self) -> i32 {
        match self {
            Color::White => 1,
            Color::Black => -1,
        }
    }
}

/// Piece kind (without color). The order matches the indices used by the NNUE network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PieceKind {
    Pawn = 0,
    Knight = 1,
    Bishop = 2,
    Rook = 3,
    Queen = 4,
    King = 5,
}

impl PieceKind {
    /// All piece kinds in order pawn → king.
    pub const ALL: [PieceKind; 6] = [
        PieceKind::Pawn,
        PieceKind::Knight,
        PieceKind::Bishop,
        PieceKind::Rook,
        PieceKind::Queen,
        PieceKind::King,
    ];

    /// Pieces a pawn can promote to (most valuable first).
    pub const PROMOTIONS: [PieceKind; 4] = [
        PieceKind::Queen,
        PieceKind::Rook,
        PieceKind::Bishop,
        PieceKind::Knight,
    ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Conversion from a number 0..=5 (panics on an invalid value).
    #[inline]
    pub const fn from_index(index: usize) -> PieceKind {
        PieceKind::ALL[index]
    }

    /// Upper-case letter for SAN/FEN: N, B, R, Q, K; pawn = 'P'.
    pub const fn letter(self) -> char {
        match self {
            PieceKind::Pawn => 'P',
            PieceKind::Knight => 'N',
            PieceKind::Bishop => 'B',
            PieceKind::Rook => 'R',
            PieceKind::Queen => 'Q',
            PieceKind::King => 'K',
        }
    }

    /// Converts a letter (upper or lower case) to a piece kind.
    pub fn from_letter(letter: char) -> Option<PieceKind> {
        match letter.to_ascii_uppercase() {
            'P' => Some(PieceKind::Pawn),
            'N' => Some(PieceKind::Knight),
            'B' => Some(PieceKind::Bishop),
            'R' => Some(PieceKind::Rook),
            'Q' => Some(PieceKind::Queen),
            'K' => Some(PieceKind::King),
            _ => None,
        }
    }
}

/// Piece = color + kind, stored in one byte (`color * 6 + kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Piece(u8);

impl Piece {
    #[inline]
    pub const fn new(color: Color, kind: PieceKind) -> Piece {
        Piece(color as u8 * 6 + kind as u8)
    }

    #[inline]
    pub const fn color(self) -> Color {
        if self.0 < 6 {
            Color::White
        } else {
            Color::Black
        }
    }

    #[inline]
    pub const fn kind(self) -> PieceKind {
        PieceKind::from_index((self.0 % 6) as usize)
    }

    /// Index 0..12 (for Zobrist tables etc.).
    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Inverse of `index()`: number 0..12 → piece.
    #[inline]
    pub const fn from_index(index: u8) -> Piece {
        debug_assert!(index < 12);
        Piece(index)
    }

    /// FEN character: upper case for white, lower case for black.
    pub const fn fen_char(self) -> char {
        let upper = self.kind().letter();
        match self.color() {
            Color::White => upper,
            Color::Black => upper.to_ascii_lowercase(),
        }
    }

    /// Converts a FEN character to a piece.
    pub fn from_fen_char(c: char) -> Option<Piece> {
        let kind = PieceKind::from_letter(c)?;
        let color = if c.is_ascii_uppercase() {
            Color::White
        } else {
            Color::Black
        };
        Some(Piece::new(color, kind))
    }
}

/// Board square 0..64: a1 = 0, b1 = 1, …, h1 = 7, a2 = 8, …, h8 = 63.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Square(u8);

impl Square {
    #[inline]
    pub const fn new(index: u8) -> Square {
        debug_assert!(index < 64);
        Square(index)
    }

    /// Square from a file (0 = a) and a rank (0 = first rank).
    #[inline]
    pub const fn from_coords(file: u8, rank: u8) -> Square {
        Square(rank * 8 + file)
    }

    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// File 0..8 (a..h).
    #[inline]
    pub const fn file(self) -> u8 {
        self.0 & 7
    }

    /// Rank 0..8 (1..8).
    #[inline]
    pub const fn rank(self) -> u8 {
        self.0 >> 3
    }

    /// Vertical mirror of the board (a1 ↔ a8).
    /// Used by NNUE – positions are always evaluated from the side to move's point of view.
    #[inline]
    pub const fn flip(self) -> Square {
        Square(self.0 ^ 56)
    }

    /// Shift by `delta` squares (the caller guarantees the result is on the board).
    #[inline]
    pub const fn offset(self, delta: i8) -> Square {
        Square((self.0 as i8 + delta) as u8)
    }

    /// Bitboard with a single bit set.
    #[inline]
    pub const fn bb(self) -> u64 {
        1u64 << self.0
    }

    /// Text name, e.g. "e4".
    pub fn name(self) -> [u8; 2] {
        [b'a' + self.file(), b'1' + self.rank()]
    }

    /// Parses text such as "e4" (exactly two characters).
    pub fn parse(text: &str) -> Option<Square> {
        let bytes = text.as_bytes();
        if bytes.len() != 2 {
            return None;
        }
        let file = bytes[0].wrapping_sub(b'a');
        let rank = bytes[1].wrapping_sub(b'1');
        if file < 8 && rank < 8 {
            Some(Square::from_coords(file, rank))
        } else {
            None
        }
    }
}

impl core::fmt::Display for Square {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = self.name();
        write!(f, "{}{}", name[0] as char, name[1] as char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_roundtrip() {
        for color in Color::ALL {
            for kind in PieceKind::ALL {
                let piece = Piece::new(color, kind);
                assert_eq!(piece.color(), color);
                assert_eq!(piece.kind(), kind);
                assert_eq!(Piece::from_fen_char(piece.fen_char()), Some(piece));
            }
        }
    }

    #[test]
    fn square_names() {
        assert_eq!(Square::parse("a1"), Some(Square::new(0)));
        assert_eq!(Square::parse("h8"), Some(Square::new(63)));
        assert_eq!(Square::parse("e4").unwrap().to_string_for_test(), "e4");
        assert_eq!(Square::parse("i1"), None);
        assert_eq!(Square::parse("a9"), None);
        assert_eq!(Square::new(0).flip(), Square::new(56));
    }

    impl Square {
        fn to_string_for_test(self) -> &'static str {
            // Simple allocation-free check: compare the bytes.
            match &self.name() {
                b"e4" => "e4",
                _ => "?",
            }
        }
    }
}
