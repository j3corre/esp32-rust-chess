//! Hand-written evaluation of endgames with a lone king ("mop-up").
//!
//! The NNUE barely knows these positions (they are rare in training data, and clearly won
//! positions are filtered out of it), and its evaluation does not distinguish whether the weaker
//! king is on the edge or in the center. The search then has nothing to follow and does not find
//! the mate – typically with bishop and knight, which needs up to ~33 moves of technique.
//!
//! When one side has only its king left and the other has mating material without pawns, we use
//! a formula: winning value + material + pushing the weaker king to the edge (with bishop and
//! knight to a corner of the bishop's color) + bringing the own king closer. Insufficient
//! material (K+N, K+B, K+2N, same-colored bishops) is a draw.

use chess_core::{Color, PieceKind, Position, Square};

/// Base value of a won endgame (above normal NNUE values, below mate scores).
const WIN: i32 = 2_000;

/// Score from the side to move's point of view if this is a lone-king endgame; otherwise `None`.
pub fn evaluate(pos: &Position) -> Option<i32> {
    for strong in [Color::White, Color::Black] {
        let weak = strong.opponent();
        if pos.color_bb(weak).count_ones() != 1 {
            continue;
        }
        // Stronger side has pawns: leave it to the NNUE (promotion, pawn endgames).
        if pos.pieces(strong, PieceKind::Pawn) != 0 {
            return None;
        }
        let count = |kind| pos.pieces(strong, kind).count_ones() as i32;
        let (queens, rooks, bishops, knights) = (
            count(PieceKind::Queen),
            count(PieceKind::Rook),
            count(PieceKind::Bishop),
            count(PieceKind::Knight),
        );
        let bishop_bb = pos.pieces(strong, PieceKind::Bishop);
        let light_bishop = bishop_bb & LIGHT_SQUARES != 0;
        let dark_bishop = bishop_bb & !LIGHT_SQUARES != 0;
        let can_mate =
            queens + rooks > 0 || (light_bishop && dark_bishop) || (bishops > 0 && knights > 0);
        if !can_mate {
            // K+N, K+B, K+2N, K+same-colored bishops against a lone king: mate cannot be forced.
            return Some(0);
        }

        let weak_king = pos.king_square(weak);
        let strong_king = pos.king_square(strong);
        let material = 900 * queens + 500 * rooks + 330 * bishops + 320 * knights;

        // Push to the edge; with bishop and knight only, to a corner of the bishop's color.
        let push = if queens + rooks == 0 && knights > 0 && !(light_bishop && dark_bishop) {
            let corners: [(u8, u8); 2] = if light_bishop {
                [(0, 7), (7, 0)] // a8, h1 – light squares
            } else {
                [(0, 0), (7, 7)] // a1, h8 – dark squares
            };
            let d = corners
                .iter()
                .map(|&(f, r)| manhattan_to(weak_king, f, r))
                .min()
                .unwrap_or(14);
            // edge in general + strongly the correct corner
            10 * center_distance(weak_king) + 40 * (14 - d)
        } else {
            20 * center_distance(weak_king)
        };
        // Bring the kings together (the own king helps build the mating net).
        let close = 10 * (14 - manhattan(strong_king, weak_king));

        let score = WIN + material + push + close;
        return Some(if pos.side_to_move() == strong {
            score
        } else {
            -score
        });
    }
    None
}

/// Bitboard of the light squares (a1 is dark, h1 light).
const LIGHT_SQUARES: u64 = 0x55AA_55AA_55AA_55AA;

/// Distance from the center: 0 (d4, e4, d5, e5) to 6 (corner).
fn center_distance(sq: Square) -> i32 {
    let axis = |x: u8| if x < 4 { 3 - x as i32 } else { x as i32 - 4 };
    axis(sq.file()) + axis(sq.rank())
}

fn manhattan(a: Square, b: Square) -> i32 {
    manhattan_to(a, b.file(), b.rank())
}

fn manhattan_to(a: Square, file: u8, rank: u8) -> i32 {
    (a.file() as i32 - file as i32).abs() + (a.rank() as i32 - rank as i32).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(fen: &str) -> Option<i32> {
        evaluate(&Position::from_fen(fen).unwrap())
    }

    #[test]
    fn light_squares_mask() {
        // a1 dark, b1 light, h1 light, a8 light, h8 dark
        let light = |sq: u8| LIGHT_SQUARES >> sq & 1 == 1;
        assert!(!light(0) && light(1) && light(7) && light(56) && !light(63));
    }

    #[test]
    fn detects_mop_up_endgames() {
        // K+R vs K, White to move → positive; Black to move → negative
        assert!(eval("8/8/8/4k3/8/8/8/R3K3 w - - 0 1").unwrap() > WIN);
        assert!(eval("8/8/8/4k3/8/8/8/R3K3 b - - 0 1").unwrap() < -WIN);
        // normal position → None
        assert_eq!(
            eval("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
            None
        );
        // stronger side has pawns → NNUE
        assert_eq!(eval("8/8/8/4k3/8/8/4P3/R3K3 w - - 0 1"), None);
        // insufficient material → draw
        assert_eq!(eval("8/8/8/4k3/8/8/8/1N2K3 w - - 0 1"), Some(0));
        assert_eq!(eval("8/8/8/4k3/8/8/8/1NN1K3 w - - 0 1"), Some(0));
        // bishops c1 + e3 = both dark-squared → draw; c1 + f1 = different colors → mate can be forced
        assert_eq!(eval("8/8/8/4k3/8/4B3/8/2B1K3 w - - 0 1"), Some(0));
        assert!(eval("8/8/8/4k3/8/8/8/2B1KB2 w - - 0 1").unwrap() > WIN);
    }

    #[test]
    fn prefers_edge_and_right_corner() {
        // K+R: the weaker king on the edge is better than in the center
        let center = eval("8/8/8/4k3/8/8/8/R3K3 w - - 0 1").unwrap();
        let edge = eval("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap();
        assert!(edge > center);
        // K+B+N with a dark-squared bishop (c1): corner a1/h8 is better than a8/h1
        let right = eval("8/8/8/8/8/2K5/8/k1B1N3 w - - 0 1").unwrap(); // black king a1
        let wrong = eval("k7/8/2K5/8/8/8/8/2B1N3 w - - 0 1").unwrap(); // black king a8
        assert!(right > wrong, "{right} vs {wrong}");
    }
}
