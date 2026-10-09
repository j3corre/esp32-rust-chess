//! Engine thread.
//!
//! The engine searches synchronously (recursive alpha-beta) for up to 10 s at a time. Running
//! in the Embassy executor it would block the web and the display. That is why it has **its
//! own esp-rtos scheduler thread** with a lower priority (1) than the main thread with the
//! Embassy tasks (2): as soon as the network, web or display need the CPU, the scheduler
//! preempts the engine; when they are idle, the engine searches.
//!
//! The ESP32-S3 has two cores: there the engine runs alone on the second core (APP CPU) and
//! the network, web and Wi-Fi on the first – clients do not slow it down.

use core::ffi::c_void;

use chess_engine::{Engine, IterationInfo, Progress, SearchHooks, SearchTables, TtEntry};
use chess_hub::EngineJob;
use log::info;
use static_cell::ConstStaticCell;

use crate::shared::{now_ms, with_hub};

/// Transposition table (size depends on the chip, see `chip.rs`).
const TT_ENTRIES: usize = crate::chip::TT_ENTRIES;
/// Engine thread stack (search recursion up to 96 plies deep).
const ENGINE_STACK: usize = 48 * 1024;
/// Engine thread priority (the main thread with Embassy has 2).
const ENGINE_PRIORITY: u32 = 1;

/// Large tables live in static memory (.bss), not on the stack.
static TABLES: ConstStaticCell<SearchTables> = ConstStaticCell::new(SearchTables::new());
static TT: ConstStaticCell<[TtEntry; TT_ENTRIES]> =
    ConstStaticCell::new([TtEntry::EMPTY; TT_ENTRIES]);

/// Engine stack on the second ESP32-S3 core (static RAM instead of the heap).
#[cfg(feature = "esp32s3")]
static CORE1_STACK: ConstStaticCell<esp_hal::system::Stack<ENGINE_STACK>> =
    ConstStaticCell::new(esp_hal::system::Stack::new());

/// PSRAM left free for other uses (e.g. tables of a second search thread).
#[cfg(feature = "esp32s3")]
const PSRAM_RESERVE: usize = 256 * 1024;

/// ESP32-S3: the engine as the main thread of the second core (call once after `esp_rtos::start`).
/// `psram` = PSRAM range for a large TT (`None` = chip without PSRAM → small TT in SRAM).
#[cfg(feature = "esp32s3")]
pub fn start_on_second_core(
    cpu_control: esp_hal::peripherals::CPU_CTRL<'static>,
    int1: esp_hal::peripherals::FROM_CPU_INTR1<'static>,
    psram: Option<&'static mut [u8]>,
) {
    let big_tt = psram.map(tt_in_psram);
    if big_tt.is_some() {
        // The small TT in internal RAM is unused – add it to the heap (Wi-Fi, networking).
        let small = TT.take();
        let size = core::mem::size_of_val(small);
        // SAFETY: the static block is ours alone (a ConstStaticCell can be taken only once)
        // and nobody else will use it; it is in internal RAM.
        unsafe {
            esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
                small.as_mut_ptr().cast::<u8>(),
                size,
                esp_alloc::MemoryCapability::Internal.into(),
            ));
        }
        info!("Heap: +{} kB from the unused small TT", size / 1024);
    }
    esp_rtos::start_second_core(cpu_control, int1, CORE1_STACK.take(), move || {
        engine_run(big_tt)
    });
}

/// TT over the whole PSRAM except the reserve (entries are initialised to empty).
#[cfg(feature = "esp32s3")]
fn tt_in_psram(mem: &'static mut [u8]) -> &'static mut [TtEntry] {
    let offset = mem.as_ptr().align_offset(core::mem::align_of::<TtEntry>());
    let usable = mem.len().saturating_sub(offset + PSRAM_RESERVE);
    let count = usable / core::mem::size_of::<TtEntry>();
    // SAFETY: `mem` is our exclusive 'static range; the pointer is aligned for TtEntry
    // and `count` entries fit into the range. Every entry is written before use.
    unsafe {
        let ptr = mem.as_mut_ptr().add(offset).cast::<TtEntry>();
        for i in 0..count {
            ptr.add(i).write(TtEntry::EMPTY);
        }
        core::slice::from_raw_parts_mut(ptr, count)
    }
}

/// Starts the engine thread (call once after `esp_rtos::start`).
#[cfg_attr(feature = "esp32s3", allow(dead_code))]
pub fn start() {
    // SAFETY: the parameter is unused (null), the thread function never returns.
    unsafe {
        esp_radio_rtos_driver::task_create(
            "engine",
            engine_main,
            core::ptr::null_mut(),
            ENGINE_PRIORITY,
            None,
            ENGINE_STACK,
        );
    }
}

/// Communication between the search and the hub.
struct Hooks<'a> {
    job: &'a EngineJob,
}

impl SearchHooks for Hooks<'_> {
    fn now_ms(&self) -> u64 {
        now_ms()
    }

    fn check_interval(&self) -> u64 {
        // ~3k nodes/s on the ESP32-C6 → time and abort check roughly every 40 ms.
        128
    }

    fn should_stop(&self) -> bool {
        with_hub(|hub| hub.job_obsolete(self.job))
    }

    fn on_progress(&mut self, p: &Progress) {
        with_hub(|hub| hub.engine_progress(p.depth, p.seldepth, p.nodes, p.nps()));
    }

    fn on_iteration(&mut self, info: &IterationInfo) {
        with_hub(|hub| hub.engine_iteration(self.job, info.depth, info.score, info.pv()));
    }
}

extern "C" fn engine_main(_param: *mut c_void) {
    engine_run(None)
}

/// Engine loop; `big_tt` = TT in PSRAM, otherwise the small static `TT` in internal RAM.
fn engine_run(big_tt: Option<&'static mut [TtEntry]>) -> ! {
    let in_psram = big_tt.is_some();
    let tt: &'static mut [TtEntry] = match big_tt {
        Some(tt) => tt,
        None => TT.take(),
    };
    let tt_kb = (core::mem::size_of_val(tt) / 1024) as u32;
    info!(
        "TT: {} entries ({} kB, {})",
        tt.len(),
        tt_kb,
        if in_psram { "PSRAM" } else { "internal RAM" }
    );
    with_hub(|hub| {
        hub.system.hash_kb = tt_kb;
        hub.system.hash_psram = in_psram;
        // A large TT in PSRAM is stronger (measured) – only with the default net, whose strength is
        // known.
        #[cfg(feature = "esp32s3")]
        if in_psram && hub.system.elo != 0 {
            hub.system.elo = crate::chip::ELO_PSRAM;
        }
    });
    let mut engine = Engine::new(TABLES.take(), tt);
    let mut last_game = None;
    info!("Engine thread running");
    match chess_engine::nnue::simd_init() {
        Some(true) => info!("SIMD (PIE): self-test OK – NNUE computes 8 neurons at a time"),
        Some(false) => log::warn!("SIMD (PIE): self-test FAILED – NNUE falls back to scalar code"),
        None => {}
    }
    #[cfg(feature = "uci")]
    crate::uci::run(&mut engine);

    loop {
        let now = now_ms();
        let Some(job) = with_hub(|hub| hub.next_engine_job(now)) else {
            // Nothing to search – sleep for 20 ms (the CPU goes to the rest of the system).
            esp_radio_rtos_driver::usleep(20_000);
            continue;
        };
        if last_game != Some(job.game) {
            engine.new_game();
            last_game = Some(job.game);
        }
        let mut hooks = Hooks { job: &job };
        let result = engine.search(&job.root, &job.history, &job.limits, &mut hooks);
        info!(
            "Engine: game {} move {} depth {} score {} {} nodes/s",
            job.game,
            result.best_move,
            result.depth,
            result.score,
            result.nps()
        );
        let now = now_ms();
        with_hub(|hub| hub.engine_done(now, &job, result.best_move, result.depth, result.nps()));
    }
}
