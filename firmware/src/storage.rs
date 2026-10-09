//! Stores the Wi-Fi credentials in flash (partition `chesscfg`, see partitions.csv).
//!
//! The record format (with a checksum) lives in `chess_hub::stored` and is tested on a PC.
//! The flash may have only one "owner", so the storage sits in a static mutex and is used by
//! both the Wi-Fi manager (save/forget) and the button (factory reset).

use core::cell::RefCell;

use chess_hub::stored::{RECORD_SIZE, WifiCredentials};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;
use log::{info, warn};

/// Start of the `chesscfg` partition (must match partitions.csv).
const CONFIG_OFFSET: u32 = 0x3F_0000;

static STORE: Mutex<CriticalSectionRawMutex, RefCell<Option<FlashStorage<'static>>>> =
    Mutex::new(RefCell::new(None));

/// Initialisation (once at startup).
pub fn init(flash: FLASH<'static>) {
    STORE.lock(|cell| *cell.borrow_mut() = Some(FlashStorage::new(flash)));
}

fn with_flash<R>(f: impl FnOnce(&mut FlashStorage<'static>) -> R) -> R {
    STORE.lock(|cell| {
        f(cell
            .borrow_mut()
            .as_mut()
            .expect("storage::init was not called"))
    })
}

/// Loads the saved credentials (or `None` when missing / corrupted).
pub fn load() -> Option<WifiCredentials> {
    let mut record = [0u8; RECORD_SIZE];
    if let Err(error) = with_flash(|flash| flash.read(CONFIG_OFFSET, &mut record)) {
        warn!("Reading the configuration from flash failed: {error:?}");
        return None;
    }
    WifiCredentials::decode(&record)
}

/// Saves the credentials (rewrites a sector – takes tens of ms).
pub fn save(credentials: &WifiCredentials) {
    match with_flash(|flash| flash.write(CONFIG_OFFSET, &credentials.encode())) {
        Ok(()) => info!("Wi-Fi \"{}\" saved to flash", credentials.ssid),
        Err(error) => warn!("Writing the configuration failed: {error:?}"),
    }
}

/// Erases the credentials (factory reset): all 0xFF = "erased flash" → the record will not load.
pub fn erase() {
    if let Err(error) = with_flash(|flash| flash.write(CONFIG_OFFSET, &[0xFF; RECORD_SIZE])) {
        warn!("Erasing the configuration failed: {error:?}");
    }
}
