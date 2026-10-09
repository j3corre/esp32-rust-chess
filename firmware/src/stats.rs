//! Web server and system statistics: a console report every 30 s and a page on the display.
//!
//! Each web task serves one connection at a time; we measure how many connections and
//! requests arrived, how long serving took and the peak number of busy tasks
//! (which tells how many tasks are needed).

use core::cell::Cell;
use portable_atomic::{AtomicI32, AtomicU32, Ordering::Relaxed};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use crate::shared::now_ms;

static BUSY: AtomicU32 = AtomicU32::new(0);
static BUSY_MAX: AtomicU32 = AtomicU32::new(0);
static CONNECTIONS: AtomicU32 = AtomicU32::new(0);
static REQUESTS: AtomicU32 = AtomicU32::new(0);
static ERRORS: AtomicU32 = AtomicU32::new(0);
static TIME_SUM_MS: AtomicU32 = AtomicU32::new(0);
static TIME_MAX_MS: AtomicU32 = AtomicU32::new(0);
static REQUESTS_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Free heap (updated by the main loop).
pub static HEAP_FREE: AtomicU32 = AtomicU32::new(0);
/// Chip temperature in tenths of °C (set by the main loop, read by the display), `NO_TEMP` =
/// unknown.
pub static CHIP_TEMP_DC: AtomicI32 = AtomicI32::new(NO_TEMP);
pub const NO_TEMP: i32 = i32::MIN;

/// Summary of the last window (for the display).
#[derive(Clone, Copy, Default)]
#[cfg_attr(not(feature = "display"), allow(dead_code))]
pub struct WebSnapshot {
    /// Requests per second × 10.
    pub req_per_s_x10: u32,
    pub connections: u32,
    pub avg_ms: u32,
    pub max_ms: u32,
    /// Peak number of simultaneously busy web tasks.
    pub busy_max: u32,
    /// Connections ended by an error (typically a timeout with a client on a weak signal).
    pub errors: u32,
    pub requests_total: u32,
}

static SNAPSHOT: Mutex<CriticalSectionRawMutex, Cell<WebSnapshot>> =
    Mutex::new(Cell::new(WebSnapshot {
        req_per_s_x10: 0,
        connections: 0,
        avg_ms: 0,
        max_ms: 0,
        busy_max: 0,
        errors: 0,
        requests_total: 0,
    }));

/// Start of serving a connection; returns the start time.
pub fn connection_start() -> u64 {
    let busy = BUSY.fetch_add(1, Relaxed) + 1;
    BUSY_MAX.fetch_max(busy, Relaxed);
    now_ms()
}

/// End of serving a connection.
pub fn connection_end(start_ms: u64, requests: u32, error: bool) {
    BUSY.fetch_sub(1, Relaxed);
    let duration = now_ms().saturating_sub(start_ms).min(u32::MAX as u64) as u32;
    CONNECTIONS.fetch_add(1, Relaxed);
    REQUESTS.fetch_add(requests, Relaxed);
    REQUESTS_TOTAL.fetch_add(requests, Relaxed);
    if error {
        ERRORS.fetch_add(1, Relaxed);
    }
    TIME_SUM_MS.fetch_add(duration, Relaxed);
    TIME_MAX_MS.fetch_max(duration, Relaxed);
}

/// Closes the measurement window (length `window_ms`), stores the summary for the display and
/// returns it.
pub fn close_window(window_ms: u64) -> WebSnapshot {
    let connections = CONNECTIONS.swap(0, Relaxed);
    let requests = REQUESTS.swap(0, Relaxed);
    let time_sum = TIME_SUM_MS.swap(0, Relaxed);
    let snapshot = WebSnapshot {
        req_per_s_x10: (requests as u64 * 10_000 / window_ms.max(1)) as u32,
        connections,
        avg_ms: time_sum.checked_div(connections).unwrap_or(0),
        max_ms: TIME_MAX_MS.swap(0, Relaxed),
        // peak over the window, but at least the current number of busy tasks
        busy_max: BUSY_MAX.swap(BUSY.load(Relaxed), Relaxed),
        errors: ERRORS.swap(0, Relaxed),
        requests_total: REQUESTS_TOTAL.load(Relaxed),
    };
    SNAPSHOT.lock(|s| s.set(snapshot));
    snapshot
}

/// Latest summary (for the display).
#[cfg_attr(not(feature = "display"), allow(dead_code))]
pub fn snapshot() -> WebSnapshot {
    SNAPSHOT.lock(|s| s.get())
}
