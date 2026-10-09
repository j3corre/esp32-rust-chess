//! Piece attacks as bitboards.
//!
//! A bitboard is a `u64` where bit N stands for square N (a1 = bit 0, h8 = bit 63).
//!
//! - Knight, king and pawn: precomputed tables (64 squares × 8 B).
//! - Bishop, rook, queen: **"kindergarten" bitboards** – a single 32-bit multiplication
//!   collapses the occupancy of a line into 6 bits, the attacks are read from a rank attack
//!   table (512 B) and spread back onto the line by another multiplication. Everything works
//!   on 32-bit halves: the ESP32-C6 (RV32IMAC) has no 64-bit operations or byte swap, but
//!   multiplication is cheap. (The usual "magic bitboards" would need ~800 kB of tables.)
//!
//! All tables are computed at compile time (`const fn`) and take no RAM.

use crate::types::{Color, Square};

/// Bitboard – a set of squares.
pub type Bitboard = u64;

/// Mask of the a-file (a1, a2, …, a8).
pub const FILE_A: Bitboard = 0x0101_0101_0101_0101;
/// Mask of the h-file.
pub const FILE_H: Bitboard = FILE_A << 7;
/// Mask of the first rank.
pub const RANK_1: Bitboard = 0xFF;

/// Mask of rank `rank` (0 = first rank).
#[inline]
pub const fn rank_mask(rank: u8) -> Bitboard {
    RANK_1 << (rank * 8)
}

/// Mask of file `file` (0 = a-file).
#[inline]
pub const fn file_mask(file: u8) -> Bitboard {
    FILE_A << file
}

/// Returns the lowest set bit and clears it ("pop").
/// The caller guarantees that the bitboard is not empty.
#[inline]
pub fn pop_lsb(bb: &mut Bitboard) -> Square {
    let index = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    Square::new(index)
}

/// Lowest set bit (the bitboard must not be empty).
#[inline]
pub fn lsb(bb: Bitboard) -> Square {
    Square::new(bb.trailing_zeros() as u8)
}

/// Iterator over the squares of a bitboard (from a1 to h8).
pub struct Squares(pub Bitboard);

impl Iterator for Squares {
    type Item = Square;

    #[inline]
    fn next(&mut self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            Some(pop_lsb(&mut self.0))
        }
    }
}

// ---------------------------------------------------------------------------
// Tables for leaping pieces
// ---------------------------------------------------------------------------

/// Attacks of a piece that moves a single step along the given (file, rank) vectors.
const fn step_attacks(square: usize, steps: &[(i8, i8)]) -> Bitboard {
    let file = (square % 8) as i8;
    let rank = (square / 8) as i8;
    let mut result = 0u64;
    let mut i = 0;
    while i < steps.len() {
        let f = file + steps[i].0;
        let r = rank + steps[i].1;
        if f >= 0 && f < 8 && r >= 0 && r < 8 {
            result |= 1u64 << (r * 8 + f);
        }
        i += 1;
    }
    result
}

const KNIGHT_STEPS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING_STEPS: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

const fn build_table(steps: &[(i8, i8)]) -> [Bitboard; 64] {
    let mut table = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        table[sq] = step_attacks(sq, steps);
        sq += 1;
    }
    table
}

static KNIGHT: [Bitboard; 64] = build_table(&KNIGHT_STEPS);
static KING: [Bitboard; 64] = build_table(&KING_STEPS);
/// Pawn attacks: [color][square] – squares the pawn attacks (diagonally forward).
static PAWN: [[Bitboard; 64]; 2] = [
    build_table(&[(-1, 1), (1, 1)]),
    build_table(&[(-1, -1), (1, -1)]),
];

#[inline]
pub fn knight_attacks(sq: Square) -> Bitboard {
    KNIGHT[sq.index()]
}

#[inline]
pub fn king_attacks(sq: Square) -> Bitboard {
    KING[sq.index()]
}

/// Squares attacked by a pawn of `color` standing on `sq`.
#[inline]
pub fn pawn_attacks(color: Color, sq: Square) -> Bitboard {
    PAWN[color.index()][sq.index()]
}

// ---------------------------------------------------------------------------
// Sliding pieces – kindergarten bitboards
// ---------------------------------------------------------------------------

/// Masks of the lines through a square (excluding the square itself):
/// [0] file, [1] diagonal (a1–h8 direction), [2] anti-diagonal (h1–a8 direction).
static LINE_MASKS: [[Bitboard; 3]; 64] = build_line_masks();

const fn build_line_masks() -> [[Bitboard; 3]; 64] {
    let mut masks = [[0u64; 3]; 64];
    let mut sq = 0;
    while sq < 64 {
        let file = (sq % 8) as i8;
        let rank = (sq / 8) as i8;
        let mut t = 0;
        while t < 8 {
            let t8 = t as i8;
            // File: same `file`, any rank.
            if t8 != rank {
                masks[sq][0] |= 1u64 << (t8 * 8 + file);
            }
            // Diagonal: file - rank = constant.
            let df = file - rank + t8;
            if df >= 0 && df < 8 && t8 != rank {
                masks[sq][1] |= 1u64 << (t8 * 8 + df);
            }
            // Anti-diagonal: file + rank = constant.
            let af = file + rank - t8;
            if af >= 0 && af < 8 && t8 != rank {
                masks[sq][2] |= 1u64 << (t8 * 8 + af);
            }
            t += 1;
        }
        sq += 1;
    }
    masks
}

/// Rank attacks: [occupancy of the 6 inner squares of the rank][file of the piece] → attack byte.
/// 64 × 8 bytes = 512 B.
static RANK_ATTACKS: [[u8; 8]; 64] = build_rank_attacks();

const fn build_rank_attacks() -> [[u8; 8]; 64] {
    let mut table = [[0u8; 8]; 64];
    let mut occ6 = 0;
    while occ6 < 64 {
        // Inner squares b..g map to bits 1..6; the edge squares never change blocking.
        let occupancy = (occ6 << 1) as u8;
        let mut file = 0;
        while file < 8 {
            let mut attacks = 0u8;
            // To the right.
            let mut f = file + 1;
            while f < 8 {
                attacks |= 1 << f;
                if occupancy & (1 << f) != 0 {
                    break;
                }
                f += 1;
            }
            // To the left.
            let mut f = file as i32 - 1;
            while f >= 0 {
                attacks |= 1 << f;
                if occupancy & (1 << f) != 0 {
                    break;
                }
                f -= 1;
            }
            table[occ6][file] = attacks;
            file += 1;
        }
        occ6 += 1;
    }
    table
}

/// Attacks along the diagonal / anti-diagonal given by `mask` (excluding the square itself).
/// The line has at most one square per file, so both halves can be merged (OR) and the
/// multiplication by 0x02020202 sums all bytes (without carries) into the top one – files b–g.
#[inline(always)]
fn diagonal_attacks(occupied: Bitboard, file: u8, mask: Bitboard) -> Bitboard {
    let (mask_lo, mask_hi) = (mask as u32, (mask >> 32) as u32);
    let line = (occupied as u32 & mask_lo) | ((occupied >> 32) as u32 & mask_hi);
    let occ6 = line.wrapping_mul(0x0202_0202) >> 26;
    let fill = RANK_ATTACKS[occ6 as usize][file as usize] as u32 * 0x0101_0101;
    ((fill & mask_hi) as u64) << 32 | (fill & mask_lo) as u64
}

/// File attacks: the file bits (one per byte) are collapsed by multiplying with 0x01020408
/// into 4 bits per half (ranks 1–4 and 5–8), the attacks are read from the rank attack table
/// (the file rotated onto a rank) and spread back onto the file by multiplying with 0x00204081.
#[inline(always)]
fn file_attacks(occupied: Bitboard, sq: Square) -> Bitboard {
    let (file, rank) = (sq.file() as u32, sq.rank());
    let lo = (occupied as u32 >> file) & 0x0101_0101;
    let hi = ((occupied >> 32) as u32 >> file) & 0x0101_0101;
    let occ8 = (lo.wrapping_mul(0x0102_0408) >> 24) | ((hi.wrapping_mul(0x0102_0408) >> 24) << 4);
    let attacks = RANK_ATTACKS[((occ8 >> 1) & 63) as usize][rank as usize] as u32;
    let spread = |nibble: u32| (nibble.wrapping_mul(0x0020_4081) & 0x0101_0101) << file;
    (spread(attacks >> 4) as u64) << 32 | spread(attacks & 15) as u64
}

/// Rank attacks using the small table.
#[inline(always)]
fn rank_attacks(occupied: Bitboard, sq: Square) -> Bitboard {
    let rank = sq.rank();
    // only the relevant 32-bit half (a 64-bit shift is expensive on RV32)
    let half = if rank < 4 {
        occupied as u32
    } else {
        (occupied >> 32) as u32
    };
    let shift = (rank & 3) * 8;
    let occ6 = (half >> (shift + 1)) & 63;
    let attacks = (RANK_ATTACKS[occ6 as usize][sq.file() as usize] as u32) << shift;
    if rank < 4 {
        attacks as u64
    } else {
        (attacks as u64) << 32
    }
}

/// Bishop attacks from `sq` with occupancy `occupied`.
#[inline]
pub fn bishop_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    let masks = &LINE_MASKS[sq.index()];
    diagonal_attacks(occupied, sq.file(), masks[1])
        | diagonal_attacks(occupied, sq.file(), masks[2])
}

/// Rook attacks from `sq` with occupancy `occupied`.
#[inline]
pub fn rook_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    file_attacks(occupied, sq) | rank_attacks(occupied, sq)
}

/// Queen attacks = bishop + rook.
#[inline]
pub fn queen_attacks(sq: Square, occupied: Bitboard) -> Bitboard {
    bishop_attacks(sq, occupied) | rook_attacks(sq, occupied)
}

/// The full line (rank/file/diagonal) through squares `a` and `b`,
/// or 0 if they do not share a line. Used for pins.
pub fn line_through(a: Square, b: Square) -> Bitboard {
    if a == b {
        return 0;
    }
    let masks = &LINE_MASKS[a.index()];
    if a.rank() == b.rank() {
        return rank_mask(a.rank());
    }
    for mask in masks {
        if mask & b.bb() != 0 {
            return mask | a.bb();
        }
    }
    0
}

/// Squares strictly between `a` and `b` if they share a line, otherwise 0.
/// Along a line the squares between two squares are exactly those with indices in between
/// (the index grows monotonically along the line).
#[inline]
pub fn between(a: Square, b: Square) -> Bitboard {
    let line = line_through(a, b);
    let (lo, hi) = if a.index() < b.index() {
        (a.index(), b.index())
    } else {
        (b.index(), a.index())
    };
    // bits lo+1 .. hi-1
    let span = (1u64 << hi).wrapping_sub(2u64 << lo);
    line & span
}

/// Squares a rook on `sq` attacks on an empty board (rank + file).
#[inline]
pub fn rook_rays(sq: Square) -> Bitboard {
    LINE_MASKS[sq.index()][0] | (rank_mask(sq.rank()) ^ sq.bb())
}

/// Squares a bishop on `sq` attacks on an empty board (both diagonals).
#[inline]
pub fn bishop_rays(sq: Square) -> Bitboard {
    let masks = &LINE_MASKS[sq.index()];
    masks[1] | masks[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(name: &str) -> Square {
        Square::parse(name).unwrap()
    }

    /// Slow but obviously correct slider attacks, used as a reference.
    fn slow_slider(square: Square, occupied: Bitboard, dirs: &[(i8, i8)]) -> Bitboard {
        let mut result = 0;
        for &(df, dr) in dirs {
            let mut f = square.file() as i8;
            let mut r = square.rank() as i8;
            loop {
                f += df;
                r += dr;
                if !(0..8).contains(&f) || !(0..8).contains(&r) {
                    break;
                }
                let bit = 1u64 << (r * 8 + f);
                result |= bit;
                if occupied & bit != 0 {
                    break;
                }
            }
        }
        result
    }

    #[test]
    fn sliders_match_slow_reference() {
        // Pseudo-random occupancies (xorshift) for every square.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            // sparse, medium and dense occupancies
            let occupied = match i % 3 {
                0 => seed & seed.rotate_left(17) & seed.rotate_left(31),
                1 => seed,
                _ => seed | seed.rotate_left(23),
            };
            for index in 0..64 {
                let s = Square::new(index);
                let rook = slow_slider(s, occupied, &[(1, 0), (-1, 0), (0, 1), (0, -1)]);
                let bishop = slow_slider(s, occupied, &[(1, 1), (-1, 1), (1, -1), (-1, -1)]);
                assert_eq!(rook_attacks(s, occupied), rook, "rook {s}");
                assert_eq!(bishop_attacks(s, occupied), bishop, "bishop {s}");
            }
        }
    }

    #[test]
    fn sliders_exhaustive() {
        // Every occupancy of the piece's lines (Carry-Rippler over the subsets of the mask).
        for index in 0..64 {
            let s = Square::new(index);
            let masks = &LINE_MASKS[index as usize];
            let rook_mask = masks[0] | (rank_mask(s.rank()) & !s.bb());
            let bishop_mask = masks[1] | masks[2];
            let mut subset = 0u64;
            loop {
                let rook = slow_slider(s, subset, &[(1, 0), (-1, 0), (0, 1), (0, -1)]);
                assert_eq!(rook_attacks(s, subset), rook, "rook {s}");
                subset = subset.wrapping_sub(rook_mask) & rook_mask;
                if subset == 0 {
                    break;
                }
            }
            loop {
                let bishop = slow_slider(s, subset, &[(1, 1), (-1, 1), (1, -1), (-1, -1)]);
                assert_eq!(bishop_attacks(s, subset), bishop, "bishop {s}");
                subset = subset.wrapping_sub(bishop_mask) & bishop_mask;
                if subset == 0 {
                    break;
                }
            }
        }
    }

    #[test]
    fn jump_tables() {
        assert_eq!(knight_attacks(sq("a1")), sq("b3").bb() | sq("c2").bb());
        assert_eq!(king_attacks(sq("a1")).count_ones(), 3);
        assert_eq!(
            pawn_attacks(Color::White, sq("e4")),
            sq("d5").bb() | sq("f5").bb()
        );
        assert_eq!(pawn_attacks(Color::Black, sq("a7")), sq("b6").bb());
    }

    #[test]
    fn between_all_pairs() {
        // Original method: queen attacks from both ends with occupancy {a, b} intersect between them.
        for a in 0..64 {
            for b in 0..64 {
                let (a, b) = (Square::new(a), Square::new(b));
                let line = line_through(a, b);
                let expected = if line == 0 {
                    0
                } else {
                    let occupied = a.bb() | b.bb();
                    queen_attacks(a, occupied) & line & queen_attacks(b, occupied)
                };
                assert_eq!(between(a, b), expected, "{a} {b}");
            }
            let s = Square::new(a);
            assert_eq!(rook_rays(s), rook_attacks(s, 0));
            assert_eq!(bishop_rays(s), bishop_attacks(s, 0));
        }
    }

    #[test]
    fn between_and_lines() {
        assert_eq!(between(sq("a1"), sq("d4")), sq("b2").bb() | sq("c3").bb());
        assert_eq!(between(sq("a1"), sq("a3")), sq("a2").bb());
        assert_eq!(between(sq("a1"), sq("b3")), 0);
        assert_eq!(line_through(sq("e1"), sq("e8")), file_mask(4));
        assert_eq!(line_through(sq("a1"), sq("h1")), rank_mask(0));
        assert_eq!(line_through(sq("a1"), sq("b3")), 0);
    }
}
