//! Engine thread: takes jobs from the hub, searches, returns results.
//! On the board the main thread (thread-mode executor) does the same.

use std::time::Duration;

use chess_engine::{Engine, IterationInfo, Progress, SearchHooks, SearchTables, TtEntry};
use chess_hub::EngineJob;
use chess_web::Platform;

use crate::platform::SimPlatform;

/// Number of transposition table entries (same as on the board: 64 kB / 12 B).
const TT_ENTRIES: usize = 5_461;

struct Hooks<'a> {
    platform: &'a SimPlatform,
    job: &'a EngineJob,
}

impl SearchHooks for Hooks<'_> {
    fn now_ms(&self) -> u64 {
        self.platform.now_ms()
    }

    fn should_stop(&self) -> bool {
        self.platform.with_hub(|hub| hub.job_obsolete(self.job))
    }

    fn on_progress(&mut self, p: &Progress) {
        self.platform
            .with_hub(|hub| hub.engine_progress(p.depth, p.seldepth, p.nodes, p.nps()));
    }

    fn on_iteration(&mut self, info: &IterationInfo) {
        self.platform
            .with_hub(|hub| hub.engine_iteration(self.job, info.depth, info.score, info.pv()));
    }
}

pub fn spawn(platform: &'static SimPlatform, fast: bool) {
    std::thread::Builder::new()
        .name("engine".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || run(platform, fast))
        .expect("engine thread");
}

fn run(platform: &'static SimPlatform, fast: bool) {
    let mut tables = Box::new(SearchTables::new());
    let mut tt = vec![TtEntry::EMPTY; TT_ENTRIES];
    let mut engine = Engine::new(&mut tables, &mut tt);
    let mut last_game = None;

    loop {
        let now = platform.now_ms();
        let Some(mut job) = platform.with_hub(|hub| hub.next_engine_job(now)) else {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        };
        if fast && let Some(ms) = job.limits.move_time_ms.as_mut() {
            // Fast mode for tests: 20× shorter thinking (min. 30 ms).
            *ms = (*ms / 20).max(30);
        }
        if last_game != Some(job.game) {
            engine.new_game();
            last_game = Some(job.game);
        }
        let mut hooks = Hooks {
            platform,
            job: &job,
        };
        let result = engine.search(&job.root, &job.history, &job.limits, &mut hooks);
        let now = platform.now_ms();
        platform.with_hub(|hub| {
            hub.engine_done(now, &job, result.best_move, result.depth, result.nps())
        });
    }
}
