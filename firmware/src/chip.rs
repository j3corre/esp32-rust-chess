//! Parameters of the target chip (feature `esp32c6` / `esp32c3` / `esp32s3`).
//!
//! Memory sizes are chosen so that enough main-thread stack is left for Wi-Fi initialisation
//! (checked by `tools/elf-size` on every build) plus a heap reserve at run time.

#[cfg(not(any(feature = "esp32c6", feature = "esp32c3", feature = "esp32s3")))]
compile_error!("select a chip: feature esp32c6, esp32c3 or esp32s3");
#[cfg(any(
    all(feature = "esp32c6", feature = "esp32c3"),
    all(feature = "esp32c6", feature = "esp32s3"),
    all(feature = "esp32c3", feature = "esp32s3"),
))]
compile_error!(
    "select only one chip (for chips other than esp32c6 build with --no-default-features)"
);

/// ESP32-C6: 512 kB SRAM, RISC-V 160 MHz, single core (Waveshare ESP32-C6-LCD-1.3).
#[cfg(feature = "esp32c6")]
mod params {
    pub const NAME: &str = "ESP32-C6";
    pub const CPU_MHZ: u32 = 160;
    /// Strength shown on the web ("2550+ Elo"): measured 2588 ± 33 against Stockfish at
    /// 60+0.6 s (UCI, no Wi-Fi), rounded down for the load from the web and the display.
    pub const ELO: u16 = 2550;
    /// Transposition table: 3,413 entries × 12 B = 40 kB.
    /// (Previously 5,461 = 64 kB → 4,096 = 48 kB for the NNUE accumulator stack in
    /// `SearchTables` → 40 kB for the hot search code in RAM: the main-thread stack needs
    /// ~18 kB, otherwise it overflows during Wi-Fi initialisation. The smaller TT costs
    /// little strength – measured.)
    pub const TT_ENTRIES: usize = 3_413;
    /// Static part of the heap (the second block is the 64 kB released by the bootloader).
    pub const STATIC_HEAP: usize = 68 * 1024;
    /// Web tasks for the own AP and for the home Wi-Fi.
    pub const WEB_TASKS_AP: usize = 4;
    pub const WEB_TASKS_STA: usize = 4;
}

/// ESP32-C3: 400 kB SRAM (16 kB of it cache), RISC-V 160 MHz without atomic instructions.
#[cfg(feature = "esp32c3")]
mod params {
    pub const NAME: &str = "ESP32-C3";
    pub const CPU_MHZ: u32 = 160;
    /// Strength shown on the web ("2400+ Elo"): measured 2459 ± 33, rounded down for the web load.
    pub const ELO: u16 = 2400;
    /// 683 entries × 12 B = 8 kB (112 kB less RAM than the C6; the heap for Wi-Fi and the
    /// engine stack needs ~100 kB + reserve).
    pub const TT_ENTRIES: usize = 683;
    pub const STATIC_HEAP: usize = 56 * 1024;
    pub const WEB_TASKS_AP: usize = 2;
    pub const WEB_TASKS_STA: usize = 2;
}

/// ESP32-S3: 512 kB SRAM (~400 kB for application data), Xtensa LX7 240 MHz, two cores.
#[cfg(feature = "esp32s3")]
mod params {
    #[cfg(not(feature = "tdongle"))]
    pub const NAME: &str = "ESP32-S3";
    #[cfg(feature = "tdongle")]
    pub const NAME: &str = "LilyGO T-Dongle-S3";
    pub const CPU_MHZ: u32 = 240;
    /// Strength shown on the web ("2600+ Elo"): measured 2650 ± 34 with a 16 kB TT (S3 without
    /// PSRAM), rounded down for the web load.
    pub const ELO: u16 = 2600;
    /// With PSRAM (TT ~1.75 MB): measured 2741 ± 50 (200 games against SF 2650) → "2700+ Elo".
    pub const ELO_PSRAM: u16 = 2700;
    /// 1,365 entries × 12 B = 16 kB – only on an S3 without PSRAM; with PSRAM the TT has ~1.75 MB
    /// (psram.rs).
    pub const TT_ENTRIES: usize = 1_365;
    /// The engine stack (48 kB) is static (second core), the heap does not contain it.
    /// 8 kB (previously 16): the PSRAM driver takes ~7 kB of internal RAM (its code must run
    /// from RAM) and the main-thread stack must stay ≥ 16 kB. With PSRAM the heap also gets the
    /// unused small TT (16 kB, `engine_thread::start_on_second_core`); without PSRAM it is
    /// 8 kB smaller.
    pub const STATIC_HEAP: usize = 8 * 1024;
    /// Each web task takes ~9.6 kB of static RAM – two fewer tasks than on the C6.
    pub const WEB_TASKS_AP: usize = 3;
    pub const WEB_TASKS_STA: usize = 3;
}

pub use params::*;

/// BOOT button: GPIO9 on the ESP32-C6/C3, GPIO0 on the ESP32-S3.
#[cfg(not(feature = "esp32s3"))]
macro_rules! boot_pin {
    ($p:expr) => {
        $p.GPIO9
    };
}
#[cfg(feature = "esp32s3")]
macro_rules! boot_pin {
    ($p:expr) => {
        $p.GPIO0
    };
}
pub(crate) use boot_pin;
