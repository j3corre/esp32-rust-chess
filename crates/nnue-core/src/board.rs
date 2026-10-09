//! Minimal FEN parser (piece placement and side to move only) for the tests.

use crate::PieceSq;

pub struct Board {
    pub pieces: Vec<PieceSq>,
    pub stm: usize,
}

impl Board {
    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let mut parts = fen.split_whitespace();
        let placement = parts.next().ok_or("empty FEN")?;
        let stm = match parts.next().unwrap_or("w") {
            "w" => 0,
            "b" => 1,
            s => return Err(format!("invalid side to move: {s}")),
        };
        let mut pieces = Vec::with_capacity(32);
        for (row, rank_str) in placement.split('/').enumerate() {
            if row >= 8 {
                return Err("too many ranks".into());
            }
            let rank = 7 - row;
            let mut file = 0usize;
            for ch in rank_str.chars() {
                if let Some(d) = ch.to_digit(10) {
                    file += d as usize;
                    continue;
                }
                let kind = "pnbrqk"
                    .find(ch.to_ascii_lowercase())
                    .ok_or(format!("unknown piece {ch}"))?;
                let color = if ch.is_ascii_uppercase() { 0 } else { 1 };
                if file >= 8 {
                    return Err("rank is too long".into());
                }
                pieces.push(PieceSq {
                    color,
                    kind: kind as u8,
                    sq: (rank * 8 + file) as u8,
                });
                file += 1;
            }
        }
        Ok(Board { pieces, stm })
    }

    pub fn piece_count(&self) -> u32 {
        self.pieces.len() as u32
    }
}
