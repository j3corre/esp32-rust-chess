//! nnue-core tests: bit-exactness with the original Dog/tinychess computation, incremental updates.

use nnue_core::board::Board;
use nnue_core::{Accumulator, Arch, Network, PieceSq};

const NET128: &[u8] = include_bytes!("data/net128.bin");

fn aligned(bytes: &[u8]) -> Vec<u32> {
    let mut w = vec![0u32; bytes.len().div_ceil(4)];
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), w.as_mut_ptr() as *mut u8, bytes.len())
    };
    w
}

fn as_bytes(w: &[u32], len: usize) -> &[u8] {
    unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, len) }
}

#[test]
fn legacy_detected() {
    let a = Arch::detect(NET128).unwrap();
    assert!(a.legacy);
    assert_eq!(a.hidden, 128);
    assert_eq!(a.total_len, 197_440);
}

/// The expected values were produced by the engine's original NNUE code (before nnue-core existed).
#[test]
fn bit_exact_with_original_engine() {
    let w = aligned(NET128);
    let net = Network::<128>::new(as_bytes(&w, NET128.len())).unwrap();
    let fens = include_str!("data/fens.txt").lines();
    let expected = include_str!("data/net128_expected.txt").lines();
    let mut acc = Accumulator::<128>::new();
    let mut n = 0;
    for (fen, exp) in fens.zip(expected) {
        let b = Board::from_fen(fen).unwrap();
        acc.refresh(&net, b.pieces.iter().copied());
        assert_eq!(
            acc.evaluate(&net, b.stm, b.piece_count()),
            exp.parse::<i32>().unwrap(),
            "{fen}"
        );
        n += 1;
    }
    assert_eq!(n, 2000);
}

#[test]
fn incremental_matches_refresh() {
    let w = aligned(NET128);
    let net = Network::<128>::new(as_bytes(&w, NET128.len())).unwrap();
    let b = Board::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1")
        .unwrap();
    let mut acc = Accumulator::<128>::new();
    acc.refresh(&net, b.pieces.iter().copied());
    // knight e5→f7 capturing a pawn: sub_add + remove
    let knight = PieceSq {
        color: 0,
        kind: 1,
        sq: 36,
    };
    let moved = PieceSq {
        color: 0,
        kind: 1,
        sq: 53,
    };
    let pawn = PieceSq {
        color: 1,
        kind: 0,
        sq: 53,
    };
    acc.remove(&net, pawn);
    acc.sub_add(&net, knight, moved);
    let mut pieces: Vec<PieceSq> = b
        .pieces
        .iter()
        .copied()
        .filter(|&p| p != knight && p != pawn)
        .collect();
    pieces.push(moved);
    let mut fresh = Accumulator::<128>::new();
    fresh.refresh(&net, pieces.iter().copied());
    for p in 0..2 {
        assert!(acc.lanes(p).eq(fresh.lanes(p)));
    }
    // and back
    acc.sub_add(&net, moved, knight);
    acc.add(&net, pawn);
    let mut orig = Accumulator::<128>::new();
    orig.refresh(&net, b.pieces.iter().copied());
    for p in 0..2 {
        assert!(acc.lanes(p).eq(orig.lanes(p)));
    }
}

#[test]
fn colour_symmetric() {
    let w = aligned(NET128);
    let net = Network::<128>::new(as_bytes(&w, NET128.len())).unwrap();
    let eval = |fen: &str| {
        let b = Board::from_fen(fen).unwrap();
        let mut acc = Accumulator::<128>::new();
        acc.refresh(&net, b.pieces.iter().copied());
        acc.evaluate(&net, b.stm, b.piece_count())
    };
    assert_eq!(
        eval("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"),
        eval("rnbqkbnr/pppp1ppp/8/4p3/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")
    );
}
