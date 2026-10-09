//! Move notation: SAN (standard algebraic notation, "Nf3", "exd5", "O-O", "e8=Q+")
//! and UCI parsing ("e2e4", "e7e8q").

use crate::movegen::{GenType, find_legal_move, generate, has_legal_move};
use crate::moves::{Move, MoveList};
use crate::position::Position;
use crate::types::{PieceKind, Square};

/// Short allocation-free text (max. 8 characters) – a SAN move always fits.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SanText {
    bytes: [u8; 8],
    len: u8,
}

impl SanText {
    fn push(&mut self, c: char) {
        if (self.len as usize) < self.bytes.len() {
            self.bytes[self.len as usize] = c as u8;
            self.len += 1;
        }
    }

    fn push_square(&mut self, sq: Square) {
        let name = sq.name();
        self.push(name[0] as char);
        self.push(name[1] as char);
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
}

impl core::fmt::Debug for SanText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::fmt::Display for SanText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Converts a legal move to SAN (including "+" for check and "#" for checkmate).
pub fn to_san(pos: &Position, mv: Move) -> SanText {
    let mut text = SanText::default();
    let Some(kind) = pos.kind_at(mv.from()) else {
        return text;
    };

    if mv.is_castle() {
        let castle = if mv.to().file() == 6 { "O-O" } else { "O-O-O" };
        for c in castle.chars() {
            text.push(c);
        }
    } else if kind == PieceKind::Pawn {
        if mv.is_capture() {
            text.push((b'a' + mv.from().file()) as char);
            text.push('x');
        }
        text.push_square(mv.to());
        if let Some(promo) = mv.promotion_kind() {
            text.push('=');
            text.push(promo.letter());
        }
    } else {
        text.push(kind.letter());
        // Disambiguation when several pieces of the same kind can move to the same square.
        let mut pseudo = MoveList::new();
        generate(pos, GenType::All, &mut pseudo);
        let info = pos.legality();
        let (mut same_file, mut same_rank, mut ambiguous) = (false, false, false);
        for &other in pseudo.iter() {
            if other != mv
                && other.to() == mv.to()
                && pos.kind_at(other.from()) == Some(kind)
                && pos.is_legal(other, &info)
            {
                ambiguous = true;
                same_file |= other.from().file() == mv.from().file();
                same_rank |= other.from().rank() == mv.from().rank();
            }
        }
        if ambiguous {
            let name = mv.from().name();
            if !same_file {
                text.push(name[0] as char);
            } else if !same_rank {
                text.push(name[1] as char);
            } else {
                text.push(name[0] as char);
                text.push(name[1] as char);
            }
        }
        if mv.is_capture() {
            text.push('x');
        }
        text.push_square(mv.to());
    }

    let next = pos.make_move(mv);
    if next.in_check() {
        text.push(if has_legal_move(&next) { '+' } else { '#' });
    }
    text
}

/// Finds a legal move from UCI text ("e2e4", "e7e8q"). Invalid text → `None`.
pub fn parse_uci(pos: &Position, text: &str) -> Option<Move> {
    let text = text.trim();
    if text.len() != 4 && text.len() != 5 {
        return None;
    }
    let from = Square::parse(text.get(0..2)?)?;
    let to = Square::parse(text.get(2..4)?)?;
    let promotion = match text.get(4..5) {
        Some(letter) => {
            let kind = PieceKind::from_letter(letter.chars().next()?)?;
            if !PieceKind::PROMOTIONS.contains(&kind) {
                return None;
            }
            Some(kind)
        }
        None => None,
    };
    find_legal_move(pos, from, to, promotion)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn san(fen: &str, uci: &str) -> SanText {
        let pos = Position::from_fen(fen).unwrap();
        let mv = parse_uci(&pos, uci).unwrap_or_else(|| panic!("move {uci} is not legal"));
        to_san(&pos, mv)
    }

    #[test]
    fn basic_san() {
        let start = crate::position::START_FEN;
        assert_eq!(san(start, "e2e4").as_str(), "e4");
        assert_eq!(san(start, "g1f3").as_str(), "Nf3");
    }

    #[test]
    fn captures_castling_promotion() {
        assert_eq!(
            san(
                "rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
                "e4d5"
            )
            .as_str(),
            "exd5"
        );
        assert_eq!(
            san("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1g1").as_str(),
            "O-O"
        );
        assert_eq!(
            san("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1c1").as_str(),
            "O-O-O"
        );
        assert_eq!(
            san("8/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7e8q").as_str(),
            "e8=Q"
        );
        assert_eq!(
            san("3r4/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7d8n").as_str(),
            "exd8=N"
        );
    }

    #[test]
    fn disambiguation_and_mate() {
        // Two knights can reach d2 → "Nbd2".
        assert_eq!(
            san("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1", "b1d2").as_str(),
            "Nbd2"
        );
        // Two rooks on the same file → disambiguate by rank.
        assert_eq!(
            san("4k3/R7/8/8/8/8/R7/4K3 w - - 0 1", "a2a5").as_str(),
            "R2a5"
        );
        // Checkmate with the queen.
        assert_eq!(
            san("7k/8/6K1/8/8/8/8/1Q6 w - - 0 1", "b1b8").as_str(),
            "Qb8#"
        );
        assert_eq!(
            san("7k/8/8/8/8/8/8/1Q4K1 w - - 0 1", "b1b8").as_str(),
            "Qb8+"
        );
    }

    #[test]
    fn parse_uci_rejects_illegal() {
        let pos = Position::start();
        assert!(parse_uci(&pos, "e2e5").is_none());
        assert!(parse_uci(&pos, "e2").is_none());
        assert!(parse_uci(&pos, "zz99").is_none());
        assert!(parse_uci(&pos, "e2e4").is_some());
    }
}
