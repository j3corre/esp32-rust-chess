//! BOOT button (GPIO9 on the ESP32-C6/C3, GPIO0 on the ESP32-S3).
//!
//! - short press (< 1 s): switches the display screen (no effect without a display),
//! - hold for 10 s: factory reset – erases the saved Wi-Fi and restarts the board.
//!   After the restart, the first client to connect may configure Wi-Fi.

use portable_atomic::Ordering;

use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use log::info;

use crate::shared::{BUTTON_HELD_MS, BUTTON_PRESSES, RESET_HOLD_MS, RESETTING};
use crate::storage;

const POLL: Duration = Duration::from_millis(50);
const SHORT_PRESS_MAX_MS: u32 = 1_000;

#[embassy_executor::task]
pub async fn button_task(button: Input<'static>) -> ! {
    let mut held_ms: u32 = 0;
    loop {
        Timer::after(POLL).await;
        if button.is_low() {
            held_ms = held_ms.saturating_add(POLL.as_millis() as u32);
            BUTTON_HELD_MS.store(held_ms, Ordering::Relaxed);
            if held_ms >= RESET_HOLD_MS {
                factory_reset().await;
            }
        } else {
            if held_ms > 0 && held_ms < SHORT_PRESS_MAX_MS {
                BUTTON_PRESSES.fetch_add(1, Ordering::Relaxed);
            }
            held_ms = 0;
            BUTTON_HELD_MS.store(0, Ordering::Relaxed);
        }
    }
}

async fn factory_reset() -> ! {
    info!("Factory reset: erasing Wi-Fi settings and restarting");
    RESETTING.store(true, Ordering::Relaxed);
    storage::erase();
    // Wait a moment so the display can show the message.
    Timer::after(Duration::from_millis(1_500)).await;
    esp_hal::system::software_reset()
}
