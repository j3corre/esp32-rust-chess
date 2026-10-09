//! ESP32 Chess – a chess computer for the Waveshare ESP32-C6-LCD-1.3 board (with a display),
//! the LilyGO T-Dongle-S3 (small display) and plain ESP32-C3 / ESP32-S3 boards (no display,
//! played via the web) – see `chip.rs`.
//!
//! At startup it:
//! 1. initialises the display, the flash (saved Wi-Fi) and the game server (hub),
//! 2. starts the engine thread (demo game engine vs. engine, 10 s per move),
//! 3. turns on Wi-Fi: its own network `ESP-Chess-XXXX` + the home network if configured,
//! 4. spawns the tasks: web, DHCP, captive DNS, display, button, hub clock.
//!
//! Division of work (single RISC-V core):
//! - main thread (priority 2): the Embassy executor with all async tasks,
//! - engine thread (priority 1): searches whenever the main thread is idle.

// UCI mode starts neither the network nor the display – the rest of the firmware is unused there.
#![cfg_attr(
    feature = "uci",
    allow(
        dead_code,
        unreachable_code,
        unused_imports,
        unused_variables,
        unused_mut
    )
)]
#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is unsafe with esp_hal types that hold buffers during data transfers"
)]

#[cfg(feature = "display")]
#[cfg_attr(feature = "tdongle", path = "board_tdongle.rs")]
mod board;
mod button;
mod chip;
mod engine_thread;
mod net;
// The T-Dongle-S3 has no PSRAM – no driver (its code takes ~7 kB of internal RAM, which the
// main-thread stack would miss on the board with a display).
#[cfg(all(feature = "esp32s3", not(feature = "tdongle")))]
mod psram;
mod shared;
mod stats;
mod storage;
mod temp;
#[cfg(feature = "uci")]
mod uci;
#[cfg(feature = "display")]
mod ui;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use log::info;
#[cfg(feature = "display")]
use static_cell::StaticCell;

use chess_hub::Hub;

// On panic, prints the error and a backtrace to the serial console.
use esp_backtrace as _;

// The Wi-Fi driver and the engine thread need dynamic allocation (heap).
extern crate alloc;

// Application descriptor required by the ESP-IDF bootloader (version, name, …).
esp_bootloader_esp_idf::esp_app_desc!();

/// SPI work buffer for the display (larger = faster drawing).
#[cfg(feature = "display")]
static SPI_BUFFER: StaticCell<[u8; 1024]> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // ESP32-S3 with PSRAM: large transposition table (detected from eFuse; without PSRAM a small
    // one in SRAM).
    #[cfg(all(feature = "esp32s3", not(feature = "tdongle")))]
    let psram = psram::init(peripherals.PSRAM);
    #[cfg(feature = "tdongle")]
    let psram = None;

    // Heap: Wi-Fi driver (~55 kB) + engine thread stack (48 kB) + reserve.
    // ~105 kB is used at run time, the rest (~27 kB) is reserve.
    // The first block uses the memory the bootloader releases after startup ("reclaimed").
    // (Static block of 68 kB: part of the SRAM holds the hot search code – feature
    // chess-engine/iram – and the rest is the main-thread stack, checked by tools/elf-size.)
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: chip::STATIC_HEAP);

    // Thread scheduler (the Wi-Fi driver and the engine need it).
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    // The main thread (network, web, display) takes precedence over the engine.
    esp_rtos::CurrentThreadHandle::get().set_priority(2);

    // Display first, so it is visible right away that the board is alive.
    #[cfg(feature = "display")]
    let screen = board::init_display(
        board::display_pins!(peripherals),
        SPI_BUFFER.init([0; 1024]),
    );
    #[cfg(feature = "display")]
    info!("Display initialised");
    #[cfg(not(feature = "display"))]
    info!("{} without a display – play via the web", chip::NAME);

    // Saved Wi-Fi decides whether the first client to connect may configure the network.
    storage::init(peripherals.FLASH);
    let credentials = storage::load();
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let mut hub = Hub::new(seed);
    hub.set_provisioning(credentials.is_none());
    fill_system_info(&mut hub.system);
    shared::HUB.lock(|cell| *cell.borrow_mut() = Some(hub));
    info!(
        "Saved Wi-Fi: {}",
        credentials
            .as_ref()
            .map_or("none (the first client to connect can set it)", |c| c
                .ssid
                .as_str())
    );

    #[cfg(not(feature = "esp32s3"))]
    engine_thread::start();
    #[cfg(feature = "esp32s3")]
    engine_thread::start_on_second_core(peripherals.CPU_CTRL, peripherals.FROM_CPU_INTR1, psram);
    #[cfg(feature = "uci")]
    {
        // UCI mode: only the engine and reading commands from USB (the engine thread replies).
        let _ = (peripherals.WIFI, credentials);
        #[cfg(feature = "display")]
        let _ = screen;
        let usb = esp_hal::usb::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE);
        let (rx, tx) = usb.split();
        uci::set_output(tx);
        spawner.spawn(uci::reader_task(rx).unwrap());
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    #[cfg(not(feature = "uci"))]
    net::start(spawner, peripherals.WIFI, seed, credentials);

    let button = Input::new(
        chip::boot_pin!(peripherals),
        InputConfig::default().with_pull(Pull::Up),
    );
    spawner.spawn(button::button_task(button).unwrap());
    #[cfg(feature = "display")]
    spawner.spawn(ui::display_task(screen).unwrap());

    // Chip temperature (built-in sensor; SENS registers on the S3).
    #[cfg(not(feature = "esp32s3"))]
    let mut chip_temp = temp::ChipTemp::new(peripherals.TSENS);
    #[cfg(feature = "esp32s3")]
    let mut chip_temp = temp::ChipTemp::new(peripherals.SENS);

    // Main task: hub clock (chess clocks, cleanup, demo game), chip temperature every 5 s and
    // a free-memory report every 30 s (for tuning buffer sizes).
    let (mut next_report, mut next_temp) = (0, 0);
    loop {
        let now = shared::now_ms();
        shared::with_hub(|hub| hub.tick(now));
        if now >= next_temp {
            let temp_dc = chip_temp.as_mut().and_then(temp::ChipTemp::read_dc);
            stats::CHIP_TEMP_DC.store(
                temp_dc.map_or(stats::NO_TEMP, i32::from),
                portable_atomic::Ordering::Relaxed,
            );
            shared::with_hub(|hub| hub.system.temp_dc = temp_dc);
            next_temp = now + 5_000;
        }
        if now >= next_report {
            let free = esp_alloc::HEAP.free();
            stats::HEAP_FREE.store(free as u32, portable_atomic::Ordering::Relaxed);
            let temp_dc = stats::CHIP_TEMP_DC.load(portable_atomic::Ordering::Relaxed);
            if temp_dc == stats::NO_TEMP {
                info!(
                    "Free heap: {} B (used {} B), chip temperature unknown",
                    free,
                    esp_alloc::HEAP.used()
                );
            } else {
                info!(
                    "Free heap: {} B (used {} B), chip temperature {}.{} °C",
                    free,
                    esp_alloc::HEAP.used(),
                    temp_dc / 10,
                    (temp_dc % 10).abs()
                );
            }
            match net::tx_power_quarter_dbm() {
                Some(p) => info!("Wi-Fi: max TX power {}.{:02} dBm", p / 4, p % 4 * 25),
                None => info!("Wi-Fi: TX power unknown (Wi-Fi not running)"),
            }
            let web = stats::close_window(30_000);
            info!(
                "Web: {}.{} req/s, {} connections, handling avg {} ms / max {} ms, \
                 max {}/{} tasks busy, {} errors",
                web.req_per_s_x10 / 10,
                web.req_per_s_x10 % 10,
                web.connections,
                web.avg_ms,
                web.max_ms,
                web.busy_max,
                net::WEB_TASKS,
                web.errors
            );
            next_report = now + 30_000;
        }
        Timer::after(Duration::from_millis(250)).await;
    }
}

/// Firmware version and description of the embedded NNUE net (shown in the web settings and on the
/// display stats page).
fn fill_system_info(sys: &mut chess_hub::SystemInfo) {
    use core::fmt::Write as _;
    let (file, hidden, bytes, ft) = chess_engine::nnue::net_info();
    let buckets = chess_engine::nnue::net_out_buckets();
    let _ = write!(
        sys.firmware,
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("FW_GIT")
    );
    let _ = write!(sys.chip, "{}, {} MHz", chip::NAME, chip::CPU_MHZ);
    // Measured only with the default net – with another one (NNUE_NET) the web shows no strength.
    sys.elo = if file == "h128-v4-i8.bin" {
        chip::ELO
    } else {
        0
    };
    let _ = sys
        .model
        .push_str(file.strip_suffix(".bin").unwrap_or(file));
    let _ = write!(sys.model_arch, "768→{hidden}×2→1, {ft}");
    if buckets > 1 {
        let _ = write!(sys.model_arch, ", {buckets} buckets");
    }
    sys.model_bytes = bytes as u32;
    info!(
        "Firmware {} on {} – net {} ({}, {} B)",
        sys.firmware, sys.chip, sys.model, sys.model_arch, bytes
    );
}
