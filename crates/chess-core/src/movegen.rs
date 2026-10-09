//! Move generator.
//!
//! Generates **pseudo-legal** moves (they follow the piece movement rules but may leave
//! the own king in check). Legality is checked by `Position::is_legal`, so the engine
//! only verifies moves it actually searches (saves time when pruning).
//! The convenience function [`legal_moves`] serves the web UI and the game rules.

use crate::attacks::{Bitboard, Squares, king_attacks, pawn_attacks};
use crate::moves::{Move, MoveList, MoveSink, flag};
use crate::position::{Position, castling, piece_attacks};
use crate::types::{Color, PieceKind, Square};

/// Which moves to generate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GenType {
    /// All moves.
    All,
    /// Captures and promotions only (for quiescence search).
    Tactical,
}

/// Generates the pseudo-legal moves of the side to move.
pub fn generate<S: MoveSink>(pos: &Position, kind: GenType, sink: &mut S) {
    let us = pos.side_to_move();
    let them = us.opponent();
    let own = pos.color_bb(us);
    let enemy = pos.color_bb(them);
    let occupied = own | enemy;

    // Target squares for pieces: only enemy pieces when generating tactical moves.
    let targets = match kind {
        GenType::All => !own,
        GenType::Tactical => enemy,
    };

    generate_pawn_moves(pos, kind, sink);

    for piece in [
        PieceKind::Knight,
        PieceKind::Bishop,
        PieceKind::Rook,
        PieceKind::Queen,
    ] {
        for from in Squares(pos.pieces(us, piece)) {
            let moves = piece_attacks(piece, us, from, occupied) & targets;
            push_targets(from, moves, enemy, sink);
        }
    }

    let king = pos.king_square(us);
    push_targets(king, king_attacks(king) & targets, enemy, sink);

    if kind == GenType::All {
        generate_castling(pos, sink);
    }
}

/// Adds moves from `from` to every square in `targets` (captures are detected via `enemy`).
#[inline]
fn push_targets<S: MoveSink>(from: Square, targets: Bitboard, enemy: Bitboard, sink: &mut S) {
    for to in Squares(targets) {
        let flags = if enemy & to.bb() != 0 {
            flag::CAPTURE
        } else {
            flag::QUIET
        };
        sink.push(Move::new(from, to, flags));
    }
}

fn generate_pawn_moves<S: MoveSink>(pos: &Position, kind: GenType, sink: &mut S) {
    let us = pos.side_to_move();
    let pawns = pos.pieces(us, PieceKind::Pawn);
    let enemy = pos.color_bb(us.opponent());
    let empty = !pos.occupied();

    // One rank forward (+8 for white, −8 for black).
    let (forward, start_rank, promo_rank): (i8, u8, u8) = match us {
        Color::White => (8, 1, 7),
        Color::Black => (-8, 6, 0),
    };

    for from in Squares(pawns) {
        let one = from.offset(forward);
        let is_promotion = one.rank() == promo_rank;

        // Pushes (promotions are generated in tactical mode too).
        if empty & one.bb() != 0 {
            if is_promotion {
                push_promotions(from, one, false, kind, sink);
            } else if kind == GenType::All {
                sink.push(Move::new(from, one, flag::QUIET));
                if from.rank() == start_rank {
                    let two = one.offset(forward);
                    if empty & two.bb() != 0 {
                        sink.push(Move::new(from, two, flag::DOUBLE_PUSH));
                    }
                }
            }
        }

        // Diagonal captures.
        for to in Squares(pawn_attacks(us, from) & enemy) {
            if is_promotion {
                push_promotions(from, to, true, kind, sink);
            } else {
                sink.push(Move::new(from, to, flag::CAPTURE));
            }
        }

        // En passant.
        if let Some(ep) = pos.en_passant()
            && pawn_attacks(us, from) & ep.bb() != 0
        {
            sink.push(Move::new(from, ep, flag::EN_PASSANT));
        }
    }
}

fn push_promotions<S: MoveSink>(
    from: Square,
    to: Square,
    capture: bool,
    kind: GenType,
    sink: &mut S,
) {
    // Quiescence search only needs queen promotions (underpromotions are pointless there).
    let pieces: &[PieceKind] = match kind {
        GenType::All => &PieceKind::PROMOTIONS,
        GenType::Tactical => &[PieceKind::Queen],
    };
    for &piece in pieces {
        sink.push(Move::promotion(from, to, piece, capture));
    }
}

fn generate_castling<S: MoveSink>(pos: &Position, sink: &mut S) {
    let us = pos.side_to_move();
    let them = us.opponent();
    let rights = pos.castling_rights();
    let (king_right, queen_right, rank) = match us {
        Color::White => (castling::WHITE_KING, castling::WHITE_QUEEN, 0),
        Color::Black => (castling::BLACK_KING, castling::BLACK_QUEEN, 7),
    };
    if rights & (king_right | queen_right) == 0 {
        return;
    }
    let king = Square::from_coords(4, rank);
    if pos.is_attacked(king, them) {
        return; // castling out of check is not allowed
    }
    let occupied = pos.occupied();
    let sq = |file| Square::from_coords(file, rank);

    // Kingside castling: f and g must be empty and not attacked.
    if rights & king_right != 0
        && occupied & (sq(5).bb() | sq(6).bb()) == 0
        && !pos.is_attacked(sq(5), them)
        && !pos.is_attacked(sq(6), them)
    {
        sink.push(Move::new(king, sq(6), flag::KING_CASTLE));
    }
    // Queenside castling: b, c, d empty; c and d not attacked.
    if rights & queen_right != 0
        && occupied & (sq(1).bb() | sq(2).bb() | sq(3).bb()) == 0
        && !pos.is_attacked(sq(3), them)
        && !pos.is_attacked(sq(2), them)
    {
        sink.push(Move::new(king, sq(2), flag::QUEEN_CASTLE));
    }
}

/// All **legal** moves in the position (for the rules, the web UI and tests).
pub fn legal_moves(pos: &Position) -> MoveList {
    let mut pseudo = MoveList::new();
    generate(pos, GenType::All, &mut pseudo);
    let info = pos.legality();
    let mut legal = MoveList::new();
    for &mv in pseudo.iter() {
        if pos.is_legal(mv, &info) {
            legal.push(mv);
        }
    }
    legal
}

/// Does the side to move have at least one legal move? (Faster than `legal_moves`.)
pub fn has_legal_move(pos: &Position) -> bool {
    let mut pseudo = MoveList::new();
    generate(pos, GenType::All, &mut pseudo);
    let info = pos.legality();
    pseudo.iter().any(|&mv| pos.is_legal(mv, &info))
}

/// Finds a legal move by its from/to squares (+ promotion piece, if any).
/// The web UI sends moves in UCI form, e.g. "e2e4" or "e7e8q".
pub fn find_legal_move(
    pos: &Position,
    from: Square,
    to: Square,
    promotion: Option<PieceKind>,
) -> Option<Move> {
    legal_moves(pos)
        .iter()
        .copied()
        .find(|mv| mv.from() == from && mv.to() == to && mv.promotion_kind() == promotion)
}

/// Perft – number of leaf nodes of the move tree to the given depth.
/// The standard correctness test of a move generator (known values for test positions).
pub fn perft(pos: &Position, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let mut pseudo = MoveList::new();
    generate(pos, GenType::All, &mut pseudo);
    let info = pos.legality();
    let mut nodes = 0;
    for &mv in pseudo.iter() {
        if !pos.is_legal(mv, &info) {
            continue;
        }
        nodes += if depth == 1 {
            1
        } else {
            perft(&pos.make_move(mv), depth - 1)
        };
    }
    nodes
}
