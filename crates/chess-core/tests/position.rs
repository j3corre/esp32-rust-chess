//! Position tests: FEN round trip, incremental hash, piece changes.

use chess_core::{Color, PieceKind, Position, Square, legal_moves, parse_uci};

fn fen_of(pos: &Position) -> String {
    let mut text = String::new();
    pos.write_fen(&mut text).unwrap();
    text
}

#[test]
fn fen_roundtrip() {
    for fen in [
        chess_core::START_FEN,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
    ] {
        let pos = Position::from_fen(fen).unwrap();
        assert_eq!(fen_of(&pos), fen);
    }
}

#[test]
fn invalid_fens_are_rejected() {
    assert!(Position::from_fen("").is_err());
    assert!(Position::from_fen("8/8/8/8/8/8/8/8 w - - 0 1").is_err()); // no kings
    assert!(Position::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w KQkq - 0 1").is_err());
    // The side not to move is in check.
    assert!(Position::from_fen("4k3/8/8/8/8/8/4Q3/4K3 w - - 0 1").is_err());
    assert!(Position::from_fen("P3k3/8/8/8/8/8/8/4K3 w - - 0 1").is_err());
}

#[test]
fn en_passant_only_when_capturable() {
    // After 1.e4 black cannot capture en passant → the e3 square is not stored.
    let pos = Position::start();
    let pos = pos.make_move(parse_uci(&pos, "e2e4").unwrap());
    assert_eq!(pos.en_passant(), None);
    assert!(fen_of(&pos).contains(" b KQkq - "));
}

#[test]
fn incremental_hash_matches_fresh() {
    // The incrementally updated hash must match the hash of the same position loaded from FEN.
    let mut pos =
        Position::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1")
            .unwrap();
    for _ in 0..40 {
        let moves = legal_moves(&pos);
        if moves.is_empty() {
            break;
        }
        // Deterministic "random" move choice.
        let mv = moves.as_slice()[(pos.hash() % moves.len() as u64) as usize];
        pos = pos.make_move(mv);
        let fresh = Position::from_fen(&fen_of(&pos)).unwrap();
        assert_eq!(
            pos.hash(),
            fresh.hash(),
            "hash after {mv} in {}",
            fen_of(&pos)
        );
    }
}

#[test]
fn piece_changes_for_castling_and_en_passant() {
    let pos = Position::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
    let castle = parse_uci(&pos, "e1g1").unwrap();
    let changes = pos.piece_changes(castle);
    assert_eq!(changes.removed().count(), 2);
    assert_eq!(changes.added().count(), 2);

    let pos = Position::from_fen("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1").unwrap();
    let ep = parse_uci(&pos, "e5d6").unwrap();
    assert!(ep.is_en_passant());
    let changes = pos.piece_changes(ep);
    let removed: Vec<_> = changes.removed().collect();
    assert!(removed.iter().any(|&(p, s)| p.kind() == PieceKind::Pawn
        && p.color() == Color::Black
        && s == Square::parse("d5").unwrap()));
    let next = pos.make_move(ep);
    assert_eq!(next.piece_at(Square::parse("d5").unwrap()), None);
}

#[test]
fn make_with_changes_matches_piece_changes() {
    // Changes recorded while making the move must exactly match a separate piece_changes call
    // and the resulting position a separate make_move (castling, en passant, promotions).
    let starts = [
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2P3k1/8/3pP3/8/8/5p2/K7 w - d6 0 1",
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    ];
    for fen in starts {
        let mut pos = Position::from_fen(fen).unwrap();
        for step in 0..120u64 {
            let moves = legal_moves(&pos);
            if moves.is_empty() {
                break;
            }
            for &mv in moves.as_slice() {
                let (next, changes) = pos.make_move_with_changes(mv);
                let expected = pos.piece_changes(mv);
                assert_eq!(
                    changes.removed().collect::<Vec<_>>(),
                    expected.removed().collect::<Vec<_>>()
                );
                assert_eq!(
                    changes.added().collect::<Vec<_>>(),
                    expected.added().collect::<Vec<_>>()
                );
                assert_eq!(next.hash(), pos.make_move(mv).hash());
                assert_eq!(fen_of(&next), fen_of(&pos.make_move(mv)));
            }
            let mv = moves.as_slice()[((pos.hash() ^ step) % moves.len() as u64) as usize];
            pos = pos.make_move(mv);
        }
    }
}
