//! PSRAM on the ESP32-S3 (…R2 / …R8 chips and modules with in-package PSRAM) – a large
//! transposition table.
//!
//! Its presence is read from eFuse (PSRAM_CAP, like `espefuse summary`), so the same firmware
//! also runs on an S3 without PSRAM: nothing is initialised there and the engine uses a small
//! TT in internal RAM. The PSRAM is not added to the heap – the Wi-Fi driver could take DMA
//! buffers from it; the whole range goes to the TT only (`engine_thread`).

use esp_hal::efuse::{self, PSRAM_CAP, PSRAM_CAP_3};
use esp_hal::psram::{FlashFreq, Psram, PsramConfig, PsramMode, SpiRamFreq};
use log::{info, warn};
use static_cell::StaticCell;

/// The driver must live for the whole run time (it owns the PSRAM peripheral).
static PSRAM: StaticCell<Psram> = StaticCell::new();

/// Initialises the PSRAM and returns the whole mapped range, or `None` if the chip has no PSRAM.
pub fn init(peripheral: esp_hal::peripherals::PSRAM<'static>) -> Option<&'static mut [u8]> {
    let cap: u8 = efuse::read_field_le(PSRAM_CAP);
    let cap3 = efuse::read_bit(PSRAM_CAP_3);
    if cap == 0 && !cap3 {
        info!("PSRAM: not present (eFuse PSRAM_CAP = 0) – small TT in internal RAM");
        return None;
    }
    // PSRAM_CAP 2 = 2 MB (…R2) is always Quad SPI; esp-hal detects the others (8 MB …R8 is usually
    // Octal).
    let mode = if cap == 2 && !cap3 {
        PsramMode::QuadSpi
    } else {
        PsramMode::Auto
    };
    let config = PsramConfig {
        mode,
        ram_frequency: SpiRamFreq::Freq80m,
        flash_frequency: FlashFreq::FlashFreq80m,
        ..PsramConfig::default()
    };
    let psram = PSRAM.init(Psram::new(peripheral, config));
    let (start, size) = psram.raw_parts();
    if size == 0 {
        warn!("PSRAM: reported by eFuse (PSRAM_CAP = {cap}), but initialisation failed");
        return None;
    }
    info!(
        "PSRAM: {} kB at {:p} ({:?}, 80 MHz)",
        size / 1024,
        start,
        mode
    );
    // SAFETY: the range mapped by the driver is ours alone (the PSRAM is not in the heap) and
    // the driver lives in a static, so the mapping stays valid for the whole run time.
    Some(unsafe { core::slice::from_raw_parts_mut(start, size) })
}
