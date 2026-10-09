//! SEE – static exchange evaluation.
//!
//! Estimates the material gain/loss of a sequence of captures on one square when both sides
//! always capture with their cheapest piece and may stop at any time. Negative SEE = the capture
//! does not pay off (the engine skips it in quiescence search).
//! Port of the `see()`/`see_rec()` functions from tinychess.

use chess_core::attacks::{Bitboard, bishop_attacks, lsb, rook_attacks};
use chess_core::{Color, Move, PieceKind, Position, Square};

/// Piece values for SEE: P N B R Q K.
pub const PIECE_VALUES: [i32; 6] = [100, 320, 330, 500, 900, 20_000];

/// SEE of a move from the side to move's point of view.
#[cfg_attr(
    all(feature = "iram", target_os = "none"),
    unsafe(link_section = ".rwtext")
)]
pub fn see(pos: &Position, mv: Move) -> i32 {
    let to = mv.to();
    let captured_kind = pos.kind_at(to);

    // Quiet move: only a promotion has a value.
    if !mv.is_capture() && captured_kind.is_none() {
        return match mv.promotion_kind() {
            Some(kind) => PIECE_VALUES[kind.index()] - PIECE_VALUES[0],
            None => 0,
        };
    }

    // En passant: the captured pawn is not on the target square.
    let captured_value = captured_kind.map_or(PIECE_VALUES[0], |k| PIECE_VALUES[k.index()]);
    let moving_kind = pos.kind_at(mv.from()).unwrap_or(PieceKind::Pawn);
    let moving_value = match mv.promotion_kind() {
        Some(kind) => PIECE_VALUES[kind.index()],
        None => PIECE_VALUES[moving_kind.index()],
    };

    // The moving piece leaves its square (it may uncover further attackers "behind" it).
    let mut occupied = pos.occupied() ^ mv.from().bb();
    if mv.is_en_passant() {
        let ep_captured = Square::from_coords(to.file(), mv.from().rank());
        occupied ^= ep_captured.bb();
    }

    let us = pos.side_to_move();
    captured_value - exchange(pos, occupied, to, us.opponent(), moving_value)
}

/// Capture sequence on square `to`: side `stm` may capture a piece worth `captured`.
/// Returns the best gain for `stm` (≥ 0, because it may decline to capture).
///
/// Computes the same as the recursive [`see_rec`] (kept as a reference for tests), but finds the
/// attackers only once: after each capture it only adds the sliders uncovered behind the captured
/// piece. The result is bit-for-bit identical, so the search is unchanged.
#[cfg_attr(
    all(feature = "iram", target_os = "none"),
    unsafe(link_section = ".rwtext")
)]
fn exchange(
    pos: &Position,
    mut occupied: Bitboard,
    to: Square,
    mut stm: Color,
    captured: i32,
) -> i32 {
    const MAX_DEPTH: usize = 32;
    let diagonal = pos.kind_bb(PieceKind::Bishop) | pos.kind_bb(PieceKind::Queen);
    let straight = pos.kind_bb(PieceKind::Rook) | pos.kind_bb(PieceKind::Queen);
    let mut attackers = pos.attackers_to(to, occupied) & occupied;
    // values[d] = value of the piece the side to move captures at depth d
    let mut values = [0i32; MAX_DEPTH];
    let mut captured = captured;
    let mut depth = 0;
    let terminal = loop {
        if depth >= MAX_DEPTH {
            break 0;
        }
        let mine = attackers & pos.color_bb(stm);
        if mine == 0 {
            break 0;
        }
        // Cheapest attacker.
        let mut kind = PieceKind::King;
        let mut from = Square::new(0);
        for candidate in PieceKind::ALL {
            let bb = mine & pos.kind_bb(candidate);
            if bb != 0 {
                kind = candidate;
                from = lsb(bb);
                break;
            }
        }
        if kind == PieceKind::King {
            // The king may capture only if nobody defends the square any more.
            let after = occupied ^ from.bb();
            let defended = pos.attackers_to(to, after) & pos.color_bb(stm.opponent()) & after != 0;
            break if defended { 0 } else { captured };
        }
        values[depth] = captured;
        occupied ^= from.bb();
        // Behind a pawn, bishop or queen a diagonal attack may be uncovered, behind a rook or queen a
        // straight one; a knight never stands on a line with square `to`.
        if matches!(kind, PieceKind::Pawn | PieceKind::Bishop | PieceKind::Queen) {
            attackers |= bishop_attacks(to, occupied) & diagonal;
        }
        if matches!(kind, PieceKind::Rook | PieceKind::Queen) {
            attackers |= rook_attacks(to, occupied) & straight;
        }
        attackers &= occupied;
        captured = PIECE_VALUES[kind.index()];
        stm = stm.opponent();
        depth += 1;
    };
    // Backwards: at each depth the side captures only if it pays off.
    let mut gain = terminal;
    while depth > 0 {
        depth -= 1;
        gain = (values[depth] - gain).max(0);
    }
    gain
}

/// Recursion: side `stm` may capture a piece worth `captured` on square `to`.
/// Returns the best gain for `stm` (≥ 0, because it may decline). Original form – test reference.
#[cfg(test)]
fn see_rec(
    pos: &Position,
    occupied: Bitboard,
    to: Square,
    stm: Color,
    captured: i32,
    depth: u32,
) -> i32 {
    if depth > 31 {
        return 0;
    }
    let attackers = pos.attackers_to(to, occupied) & occupied;
    let mine = attackers & pos.color_bb(stm);
    if mine == 0 {
        return 0;
    }

    // Cheapest attacker.
    let mut kind = PieceKind::King;
    let mut from = Square::new(0);
    for candidate in PieceKind::ALL {
        let bb = mine & pos.kind_bb(candidate);
        if bb != 0 {
            kind = candidate;
            from = lsb(bb);
            break;
        }
    }

    if kind == PieceKind::King {
        // The king may capture only if nobody defends the square any more.
        let after = occupied ^ from.bb();
        if pos.attackers_to(to, after) & pos.color_bb(stm.opponent()) & after != 0 {
            return 0;
        }
        return captured;
    }

    let continuation = see_rec(
        pos,
        occupied ^ from.bb(),
        to,
        stm.opponent(),
        PIECE_VALUES[kind.index()],
        depth + 1,
    );
    (captured - continuation).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::parse_uci;

    fn see_of(fen: &str, uci: &str) -> i32 {
        let pos = Position::from_fen(fen).unwrap();
        see(&pos, parse_uci(&pos, uci).unwrap())
    }

    #[test]
    fn simple_exchanges() {
        // Undefended pawn: net gain 100.
        assert_eq!(see_of("4k3/8/8/3p4/4P3/8/8/4K3 w - - 0 1", "e4d5"), 100);
        // Queen takes a defended pawn: 100 − 900.
        assert_eq!(see_of("4k3/8/2p5/3p4/8/8/8/3QK3 w - - 0 1", "d1d5"), -800);
        // Rook takes an undefended rook: gain 500.
        assert_eq!(see_of("4k3/8/8/3r4/8/8/3R4/3RK3 w - - 0 1", "d2d5"), 500);
        // Knight takes a pawn defended by a pawn, rook behind the knight (x-ray): N×P, P×N, R×P = 100 − 320 + 100.
        assert_eq!(see_of("4k3/8/2p5/3p4/8/4N3/8/3RK3 w - - 0 1", "e3d5"), -120);
    }

    #[test]
    fn exchange_matches_recursive_reference() {
        // All captures in many positions (random games from several starts): the iterative SEE
        // must give exactly the same result as the original recursion.
        let starts = [
            chess_core::START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "r2q1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R2QKB1R w KQ - 0 9",
            "2r2rk1/1bqnbppp/p2ppn2/1p6/3NPP2/1BN1B3/PPPQ2PP/2KR3R w - - 0 1",
        ];
        let mut checked = 0;
        for (n, fen) in starts.iter().enumerate() {
            for game in 0..40u64 {
                let mut pos = Position::from_fen(fen).unwrap();
                let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ (game * 7919 + n as u64);
                for _ in 0..80 {
                    let moves = chess_core::legal_moves(&pos);
                    if moves.is_empty() {
                        break;
                    }
                    for &mv in moves.as_slice() {
                        if !mv.is_capture() {
                            continue;
                        }
                        let to = mv.to();
                        let moving_kind = pos.kind_at(mv.from()).unwrap();
                        let moving_value = match mv.promotion_kind() {
                            Some(kind) => PIECE_VALUES[kind.index()],
                            None => PIECE_VALUES[moving_kind.index()],
                        };
                        let mut occupied = pos.occupied() ^ mv.from().bb();
                        if mv.is_en_passant() {
                            occupied ^= Square::from_coords(to.file(), mv.from().rank()).bb();
                        }
                        let stm = pos.side_to_move().opponent();
                        assert_eq!(
                            exchange(&pos, occupied, to, stm, moving_value),
                            see_rec(&pos, occupied, to, stm, moving_value, 0),
                            "{mv} in position {:?}",
                            pos.hash()
                        );
                        checked += 1;
                    }
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    pos = pos.make_move(moves.as_slice()[seed as usize % moves.len()]);
                }
            }
        }
        assert!(checked > 10_000, "too few captures: {checked}");
    }

    #[test]
    fn quiet_moves_and_promotions() {
        assert_eq!(see_of(chess_core::START_FEN, "e2e4"), 0);
        assert_eq!(see_of("8/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7e8q"), 800);
    }
}
