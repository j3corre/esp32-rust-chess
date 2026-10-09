//! Transposition table (TT) – memory of positions that were already searched.
//!
//! The same position appears many times in the search tree (via different move orders).
//! For each position the TT remembers the best move, the score and the depth, so it does
//! not have to be searched again. The memory is provided by the caller (a static array on
//! the board), so the firmware chooses the size according to the free RAM.

use chess_core::Move;

use crate::nnue::MAX_NON_MATE;

/// Kind of stored score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// Exact value.
    Exact,
    /// The real value is ≥ the stored one (a beta cutoff happened).
    Lower,
    /// The real value is ≤ the stored one (no move beat alpha).
    Upper,
}

/// One table entry (12 bytes).
///
/// Besides the search result it also keeps the static evaluation of the position (NNUE), so it
/// need not be computed again on the next visit – it is deterministic, so the search is unchanged.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TtEntry {
    /// Upper 32 bits of the hash – verifies that the entry belongs to the position.
    key: u32,
    mv: u16,
    score: i16,
    depth: i8,
    /// Bits 0–1 = score kind (0 = empty entry), bits 2–7 = search generation.
    meta: u8,
    /// Static evaluation, [`NO_EVAL`] = not stored.
    eval: i16,
}

/// `eval` value of an entry without a static evaluation.
const NO_EVAL: i16 = i16::MIN;

impl TtEntry {
    pub const EMPTY: TtEntry = TtEntry {
        key: 0,
        mv: 0,
        score: 0,
        depth: 0,
        meta: 0,
        eval: NO_EVAL,
    };

    fn bound(&self) -> Option<Bound> {
        match self.meta & 3 {
            1 => Some(Bound::Exact),
            2 => Some(Bound::Lower),
            3 => Some(Bound::Upper),
            _ => None,
        }
    }

    fn generation(&self) -> u8 {
        self.meta >> 2
    }
}

/// Result of a TT probe.
#[derive(Clone, Copy, Debug)]
pub struct TtHit {
    pub mv: Move,
    pub score: i32,
    pub depth: i32,
    pub bound: Bound,
    /// Static evaluation of the position, if it was stored.
    pub eval: Option<i32>,
}

/// Transposition table over a memory block provided by the caller.
pub struct TranspositionTable<'a> {
    entries: &'a mut [TtEntry],
    generation: u8,
}

impl<'a> TranspositionTable<'a> {
    /// `entries` = memory for the table (any length ≥ 1).
    pub fn new(entries: &'a mut [TtEntry]) -> Self {
        assert!(!entries.is_empty(), "the TT needs at least one entry");
        TranspositionTable {
            entries,
            generation: 0,
        }
    }

    /// Clears the whole table (new game).
    pub fn clear(&mut self) {
        self.entries.fill(TtEntry::EMPTY);
        self.generation = 0;
    }

    /// Start of a new search – older entries will be replaced preferentially.
    pub fn new_search(&mut self) {
        self.generation = (self.generation + 1) & 63;
    }

    /// Entry index: maps the lower 32 bits of the hash uniformly onto the table length
    /// (multiplication instead of division – fast and works for any size).
    #[inline]
    fn index(&self, hash: u64) -> usize {
        (((hash as u32) as u64 * self.entries.len() as u64) >> 32) as usize
    }

    pub fn probe(&self, hash: u64) -> Option<TtHit> {
        let entry = &self.entries[self.index(hash)];
        let bound = entry.bound()?;
        if entry.key != (hash >> 32) as u32 {
            return None;
        }
        Some(TtHit {
            mv: Move::from_raw(entry.mv),
            score: entry.score as i32,
            depth: entry.depth as i32,
            bound,
            eval: (entry.eval != NO_EVAL).then_some(entry.eval as i32),
        })
    }

    /// Stores a result. Replaces: the same position, an empty entry, an older generation
    /// or a shallower search. If the new record has no move (static evaluation only), the
    /// old move (and evaluation) of the same position is kept.
    pub fn store(
        &mut self,
        hash: u64,
        bound: Bound,
        depth: i32,
        score: i32,
        mv: Move,
        eval: Option<i32>,
    ) {
        let index = self.index(hash);
        let generation = self.generation;
        let entry = &mut self.entries[index];
        let key = (hash >> 32) as u32;
        let same = entry.key == key && entry.bound().is_some();
        let replace = same
            || entry.bound().is_none()
            || entry.generation() != generation
            || depth >= entry.depth as i32;
        if !replace {
            return;
        }
        let keep_move = if mv.is_null() && same {
            entry.mv
        } else {
            mv.raw()
        };
        let eval = match eval {
            // The evaluation is always within ±MAX_NON_MATE, so it fits in i16 and is never NO_EVAL.
            Some(e) => e as i16,
            None if same => entry.eval,
            None => NO_EVAL,
        };
        *entry = TtEntry {
            key,
            mv: keep_move,
            score: score.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            depth: depth.clamp(-1, 127) as i8,
            meta: (generation << 2)
                | match bound {
                    Bound::Exact => 1,
                    Bound::Lower => 2,
                    Bound::Upper => 3,
                },
            eval,
        };
    }

    /// Fill rate in permille (for statistics) – estimated from the first 1000 entries.
    pub fn hashfull(&self) -> u32 {
        let sample = self.entries.len().min(1000);
        let used = self.entries[..sample]
            .iter()
            .filter(|e| e.bound().is_some() && e.generation() == self.generation)
            .count();
        (used * 1000 / sample.max(1)) as u32
    }
}

/// Mate scores are stored relative to the node (not the root) so they stay valid at another depth.
pub fn score_to_tt(score: i32, ply: i32) -> i32 {
    if score > MAX_NON_MATE {
        score + ply
    } else if score < -MAX_NON_MATE {
        score - ply
    } else {
        score
    }
}

/// Inverse of [`score_to_tt`].
pub fn score_from_tt(score: i32, ply: i32) -> i32 {
    if score > MAX_NON_MATE {
        score - ply
    } else if score < -MAX_NON_MATE {
        score + ply
    } else {
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_probe() {
        let mut memory = [TtEntry::EMPTY; 100];
        let mut tt = TranspositionTable::new(&mut memory);
        let hash = 0xDEAD_BEEF_1234_5678;
        assert!(tt.probe(hash).is_none());
        let mv = Move::from_raw(0x1234);
        tt.store(hash, Bound::Lower, 5, -42, mv, Some(17));
        let hit = tt.probe(hash).unwrap();
        assert_eq!(hit.mv, mv);
        assert_eq!(hit.score, -42);
        assert_eq!(hit.depth, 5);
        assert_eq!(hit.bound, Bound::Lower);
        assert_eq!(hit.eval, Some(17));
        // A different position with the same index but a different key → not found.
        assert!(tt.probe(hash ^ 0xFFFF_0000_0000_0000).is_none());
        // Storing without a move keeps the old move.
        tt.store(hash, Bound::Exact, 6, 10, Move::NULL, None);
        assert_eq!(tt.probe(hash).unwrap().mv, mv);
        assert_eq!(tt.probe(hash).unwrap().eval, Some(17));
        // A different position at the same index does not inherit the old evaluation.
        tt.store(hash ^ 0xFFFF_0000_0000_0000, Bound::Exact, 9, 1, mv, None);
        assert_eq!(tt.probe(hash ^ 0xFFFF_0000_0000_0000).unwrap().eval, None);
    }

    #[test]
    fn mate_scores_are_ply_relative() {
        let mate_in_3_from_root = 30_000 - 5;
        let stored = score_to_tt(mate_in_3_from_root, 2);
        assert_eq!(score_from_tt(stored, 2), mate_in_3_from_root);
        assert_eq!(score_to_tt(100, 7), 100);
    }
}
