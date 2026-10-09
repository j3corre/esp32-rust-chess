//! Search limits, difficulty levels and the interface to the outside world (hooks).

use chess_core::Move;

/// Limits of a single search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchLimits {
    /// Fixed time per move in milliseconds (`None` = no time limit).
    pub move_time_ms: Option<u64>,
    /// Minimum depth: `move_time_ms` applies only after it is completed (0 = no minimum).
    pub min_depth: u8,
    /// Hard time limit in ms – always applies, even before `min_depth` is reached (`None` = none).
    pub hard_time_ms: Option<u64>,
    /// Maximum depth (`None` = unlimited).
    pub max_depth: Option<u8>,
    /// Maximum number of nodes (`None` = unlimited).
    pub max_nodes: Option<u64>,
    /// Random noise added to the evaluation (in centipawns) – weakens the engine
    /// at lower levels. 0 = no noise.
    pub eval_noise: i32,
    /// Noise seed (different for each game so the engine does not always play the same).
    pub noise_seed: u64,
}

impl SearchLimits {
    /// Fixed time per move.
    pub const fn move_time(ms: u64) -> SearchLimits {
        SearchLimits {
            move_time_ms: Some(ms),
            min_depth: 0,
            hard_time_ms: None,
            max_depth: None,
            max_nodes: None,
            eval_noise: 0,
            noise_seed: 0,
        }
    }

    /// Fixed depth (for tests – the result is deterministic).
    pub const fn depth(depth: u8) -> SearchLimits {
        SearchLimits {
            move_time_ms: None,
            min_depth: 0,
            hard_time_ms: None,
            max_depth: Some(depth),
            max_nodes: None,
            eval_noise: 0,
            noise_seed: 0,
        }
    }
}

/// Number of difficulty levels for the player.
pub const LEVEL_COUNT: u8 = 8;

/// Limits for difficulty level 1–8 (1 = beginner, 8 = full strength, 10 s per move).
/// Lower levels have a limited depth and random noise in the evaluation (they make mistakes).
pub fn level_limits(level: u8, seed: u64) -> SearchLimits {
    let level = level.clamp(1, LEVEL_COUNT);
    // (time in ms, max. depth, noise)
    let (time, depth, noise): (u64, Option<u8>, i32) = match level {
        1 => (300, Some(1), 250),
        2 => (400, Some(2), 150),
        3 => (500, Some(3), 80),
        4 => (700, Some(4), 30),
        5 => (1_000, None, 0),
        6 => (2_500, None, 0),
        7 => (5_000, None, 0),
        _ => (10_000, None, 0),
    };
    SearchLimits {
        move_time_ms: Some(time),
        min_depth: 0,
        hard_time_ms: None,
        max_depth: depth,
        max_nodes: None,
        eval_noise: noise,
        noise_seed: seed,
    }
}

/// Search progress (reported often – every 1024 nodes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    /// Depth currently being searched.
    pub depth: u8,
    /// Deepest ply reached (including quiescence search).
    pub seldepth: u8,
    /// Nodes searched since the start of the search.
    pub nodes: u64,
    /// Elapsed time in ms.
    pub elapsed_ms: u64,
}

impl Progress {
    /// Rychlost v uzlech za sekundu.
    pub fn nps(&self) -> u64 {
        self.nodes * 1000 / self.elapsed_ms.max(1)
    }
}

/// Maximum length of the returned principal variation.
pub const PV_LEN: usize = 12;

/// Result of a completed iteration (depth).
#[derive(Clone, Copy, Debug, Default)]
pub struct IterationInfo {
    pub depth: u8,
    pub seldepth: u8,
    /// Score from the side to move's point of view (centipawns, or mate, see [`mate_in`]).
    pub score: i32,
    pub nodes: u64,
    pub elapsed_ms: u64,
    pub pv: [Move; PV_LEN],
    pub pv_len: u8,
}

impl IterationInfo {
    pub fn pv(&self) -> &[Move] {
        &self.pv[..self.pv_len as usize]
    }
}

/// Mate in how many moves (positive = we are winning, negative = we are losing), otherwise `None`.
pub fn mate_in(score: i32) -> Option<i32> {
    use crate::search::MATE;
    if score.abs() > crate::nnue::MAX_NON_MATE {
        let plies = MATE - score.abs();
        let moves = (plies + 1) / 2;
        Some(if score > 0 { moves } else { -moves })
    } else {
        None
    }
}

/// Interface through which the search talks to the outside world (time, interruption, statistics).
/// On the board it is implemented by the firmware, on the PC by the simulator and the tests.
pub trait SearchHooks {
    /// Current time in milliseconds (from an arbitrary origin).
    fn now_ms(&self) -> u64;

    /// After how many nodes to check the time and interruption and to report progress.
    /// 1024 is fine on a PC; on a slow microcontroller (~3k nodes/s) a smaller value is better,
    /// otherwise the check would happen only once every ~0.3 s.
    fn check_interval(&self) -> u64 {
        1024
    }

    /// Should the search stop immediately? (E.g. the player resigned or a more important task came.)
    fn should_stop(&self) -> bool {
        false
    }

    /// Progress statistics (for the display: depth, kN/s).
    fn on_progress(&mut self, _progress: &Progress) {}

    /// A completed iteration.
    fn on_iteration(&mut self, _info: &IterationInfo) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_get_stronger() {
        let mut previous = 0;
        for level in 1..=LEVEL_COUNT {
            let limits = level_limits(level, 1);
            let time = limits.move_time_ms.unwrap();
            assert!(time >= previous);
            previous = time;
        }
        assert_eq!(level_limits(99, 0).move_time_ms, Some(10_000));
        assert_eq!(level_limits(0, 0).max_depth, Some(1));
    }

    #[test]
    fn mate_distance() {
        use crate::search::MATE;
        assert_eq!(mate_in(MATE - 1), Some(1));
        assert_eq!(mate_in(MATE - 3), Some(2));
        assert_eq!(mate_in(-(MATE - 2)), Some(-1));
        assert_eq!(mate_in(150), None);
    }
}
