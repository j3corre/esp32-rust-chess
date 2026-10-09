//! End-of-game rules: checkmate, stalemate, draws (repetition, fifty moves, insufficient material).

use crate::attacks::{Bitboard, Squares};
use crate::movegen::has_legal_move;
use crate::position::Position;
use crate::types::{Color, PieceKind};

/// Game state according to the rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameStatus {
    /// The game goes on.
    Ongoing,
    /// Checkmate – `winner` won.
    Checkmate { winner: Color },
    /// Stalemate – draw.
    Stalemate,
    /// Threefold repetition – draw.
    Repetition,
    /// Fifty moves without a capture or pawn move – draw.
    FiftyMoves,
    /// Neither side can checkmate – draw.
    InsufficientMaterial,
}

impl GameStatus {
    pub fn is_over(self) -> bool {
        self != GameStatus::Ongoing
    }
}

/// Light squares of the board (a1 is dark).
const LIGHT_SQUARES: Bitboard = 0x55AA_55AA_55AA_55AA;

/// Insufficient material under FIDE rules (no sequence of moves can lead to checkmate):
/// K–K, K+N–K, K+bishop(s)–K(+bishops) with all bishops on squares of one color.
pub fn is_insufficient_material(pos: &Position) -> bool {
    let heavy =
        pos.kind_bb(PieceKind::Pawn) | pos.kind_bb(PieceKind::Rook) | pos.kind_bb(PieceKind::Queen);
    if heavy != 0 {
        return false;
    }
    let knights = pos.kind_bb(PieceKind::Knight);
    let bishops = pos.kind_bb(PieceKind::Bishop);
    let minors = (knights | bishops).count_ones();
    if minors <= 1 {
        return true;
    }
    // Only bishops (any number), all on squares of the same color.
    knights == 0 && (bishops & LIGHT_SQUARES == 0 || bishops & !LIGHT_SQUARES == 0)
}

/// How many times position `pos` occurred in the history.
/// `history` = hashes of the previous positions of the game (excluding the current one),
/// oldest first. Only positions since the last irreversible move (capture / pawn move) count.
pub fn repetition_count(pos: &Position, history: &[u64]) -> usize {
    let window = (pos.halfmove_clock() as usize).min(history.len());
    history[history.len() - window..]
        .iter()
        .rev()
        .skip(1) // the position one ply back has the other side to move
        .step_by(2)
        .filter(|&&hash| hash == pos.hash())
        .count()
}

/// Determines the game state. For `history` see [`repetition_count`].
pub fn game_status(pos: &Position, history: &[u64]) -> GameStatus {
    if !has_legal_move(pos) {
        return if pos.in_check() {
            GameStatus::Checkmate {
                winner: pos.side_to_move().opponent(),
            }
        } else {
            GameStatus::Stalemate
        };
    }
    if pos.halfmove_clock() >= 100 {
        return GameStatus::FiftyMoves;
    }
    if is_insufficient_material(pos) {
        return GameStatus::InsufficientMaterial;
    }
    if repetition_count(pos, history) >= 2 {
        return GameStatus::Repetition;
    }
    GameStatus::Ongoing
}

/// Material of a side in pawns (P=1, N=B=3, R=5, Q=9) – for showing the advantage in the web UI.
pub fn material(pos: &Position, color: Color) -> i32 {
    const VALUES: [i32; 5] = [1, 3, 3, 5, 9];
    let mut total = 0;
    for (index, value) in VALUES.iter().enumerate() {
        let kind = PieceKind::from_index(index);
        total += value * Squares(pos.pieces(color, kind)).count() as i32;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::parse_uci;

    fn status(fen: &str) -> GameStatus {
        game_status(&Position::from_fen(fen).unwrap(), &[])
    }

    #[test]
    fn mate_and_stalemate() {
        // Scholar's mate.
        assert_eq!(
            status("r1bqkb1r/pppp1Qpp/2n2n2/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 4"),
            GameStatus::Checkmate {
                winner: Color::White
            }
        );
        assert_eq!(
            status("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"),
            GameStatus::Stalemate
        );
        assert_eq!(status(crate::position::START_FEN), GameStatus::Ongoing);
    }

    #[test]
    fn insufficient_material() {
        assert_eq!(
            status("8/8/4k3/8/8/3K4/8/8 w - - 0 1"),
            GameStatus::InsufficientMaterial
        );
        assert_eq!(
            status("8/8/4k3/8/8/3KN3/8/8 w - - 0 1"),
            GameStatus::InsufficientMaterial
        );
        // Bishops on squares of the same color (c1 and f4 are dark squares).
        assert_eq!(
            status("8/8/4k3/8/5b2/3K4/8/2B5 w - - 0 1"),
            GameStatus::InsufficientMaterial
        );
        // Bishops on different colors – checkmate is theoretically possible.
        assert_eq!(
            status("8/8/4k3/8/8/3K4/8/2B2b2 w - - 0 1"),
            GameStatus::Ongoing
        );
        assert_eq!(
            status("8/8/4k3/8/8/3KNN2/8/8 w - - 0 1"),
            GameStatus::Ongoing
        );
    }

    #[test]
    fn threefold_repetition() {
        let mut pos = Position::start();
        let mut history = std::vec::Vec::new();
        for uci in [
            "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
        ] {
            assert_eq!(game_status(&pos, &history), GameStatus::Ongoing);
            let mv = parse_uci(&pos, uci).unwrap();
            history.push(pos.hash());
            pos = pos.make_move(mv);
        }
        // The starting position occurred for the third time.
        assert_eq!(game_status(&pos, &history), GameStatus::Repetition);
    }

    #[test]
    fn fifty_moves() {
        assert_eq!(
            status("8/8/4k3/8/8/3K4/8/R7 w - - 100 80"),
            GameStatus::FiftyMoves
        );
    }
}
