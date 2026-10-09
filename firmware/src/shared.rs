//! State shared between the tasks and the engine thread.
//!
//! - `HUB` – the game server; protected by a critical section (short operations, µs),
//!   so both the main thread (Embassy tasks) and the engine thread can access it safely.
//! - `WIFI_COMMANDS` – requests from the web to the Wi-Fi manager (save, forget, scan).
//! - `SCAN_RESULTS` – the latest scan of nearby networks.
//! - atomics for the button and the display screens.

use core::cell::RefCell;
use core::fmt::{self, Write};
use portable_atomic::{AtomicBool, AtomicU8, AtomicU32};

use chess_hub::Hub;
use chess_hub::stored::WifiCredentials;
use chess_web::Platform;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use heapless::{String, Vec};

/// The game server (created at startup in `main`).
pub static HUB: Mutex<CriticalSectionRawMutex, RefCell<Option<Hub>>> =
    Mutex::new(RefCell::new(None));

/// Runs `f` with the hub locked.
pub fn with_hub<R>(f: impl FnOnce(&mut Hub) -> R) -> R {
    HUB.lock(|cell| {
        let mut hub = cell.borrow_mut();
        f(hub.as_mut().expect("hub is initialised in main"))
    })
}

/// Current time in ms since startup.
pub fn now_ms() -> u64 {
    embassy_time::Instant::now().as_millis()
}

/// Commands for the Wi-Fi manager.
pub enum WifiCommand {
    Save(WifiCredentials),
    Forget,
    Scan,
}

pub static WIFI_COMMANDS: Channel<CriticalSectionRawMutex, WifiCommand, 2> = Channel::new();

/// One network found by a scan.
#[derive(Clone)]
pub struct ScanItem {
    pub ssid: String<32>,
    pub rssi: i8,
    pub secure: bool,
}

pub static SCAN_RESULTS: Mutex<CriticalSectionRawMutex, RefCell<Vec<ScanItem, 16>>> =
    Mutex::new(RefCell::new(Vec::new()));

/// How long to hold BOOT for a factory reset (erases the Wi-Fi settings).
pub const RESET_HOLD_MS: u32 = 10_000;

/// How long the BOOT button has been held (ms), 0 = not held.
pub static BUTTON_HELD_MS: AtomicU32 = AtomicU32::new(0);
/// Number of short presses (the display switches screens by it).
pub static BUTTON_PRESSES: AtomicU8 = AtomicU8::new(0);
/// A factory reset is in progress (the display shows a message).
pub static RESETTING: AtomicBool = AtomicBool::new(false);

/// Address of the board in its own AP.
pub const AP_IP: [u8; 4] = [192, 168, 4, 1];
pub const AP_HOST: &str = "192.168.4.1";

/// `Platform` implementation for the web layer on the board.
pub struct BoardPlatform;

pub static PLATFORM: BoardPlatform = BoardPlatform;

impl Platform for BoardPlatform {
    fn now_ms(&self) -> u64 {
        now_ms()
    }

    fn with_hub<R>(&self, f: impl FnOnce(&mut Hub) -> R) -> R {
        with_hub(f)
    }

    fn wifi_save(&self, ssid: &str, password: &str) {
        let mut credentials = WifiCredentials::default();
        let _ = credentials.ssid.push_str(ssid);
        let _ = credentials.password.push_str(password);
        let _ = WIFI_COMMANDS.try_send(WifiCommand::Save(credentials));
    }

    fn wifi_forget(&self) {
        let _ = WIFI_COMMANDS.try_send(WifiCommand::Forget);
    }

    fn wifi_scan(&self, out: &mut dyn Write) -> fmt::Result {
        // Return the latest known result and request a new scan.
        let _ = WIFI_COMMANDS.try_send(WifiCommand::Scan);
        SCAN_RESULTS.lock(|cell| {
            let results = cell.borrow();
            out.write_str("{\"networks\":[")?;
            for (i, item) in results.iter().enumerate() {
                if i > 0 {
                    out.write_char(',')?;
                }
                out.write_str("{\"ssid\":")?;
                write_json_string(out, &item.ssid)?;
                write!(out, ",\"rssi\":{},\"secure\":{}}}", item.rssi, item.secure)?;
            }
            out.write_str("]}")
        })
    }

    fn portal_host(&self) -> &str {
        AP_HOST
    }
}

/// JSON string for `dyn Write` (chess_hub::json is generic over `W: Write`).
fn write_json_string(out: &mut dyn Write, text: &str) -> fmt::Result {
    struct Adapter<'a>(&'a mut dyn Write);
    impl Write for Adapter<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.0.write_str(s)
        }
    }
    chess_hub::json::string(&mut Adapter(out), text)
}
