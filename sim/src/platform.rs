//! `Platform` for the PC: hub in a mutex, time since start, fake WiFi.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chess_hub::Hub;
use chess_web::Platform;

use crate::Options;

pub struct SimPlatform {
    start: Instant,
    hub: Mutex<Hub>,
    /// Address for the captive portal redirect.
    portal: String,
}

impl SimPlatform {
    pub fn new(options: &Options) -> SimPlatform {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let mut hub = Hub::new(seed);
        hub.set_demo_enabled(options.demo);
        // version and model as on the board (shown by the web app in settings)
        let (file, hidden, bytes, ft) = chess_engine::nnue::net_info();
        set_str(
            &mut hub.system.firmware,
            &format!("{} (sim)", env!("CARGO_PKG_VERSION")),
        );
        // the simulator runs on a PC – no board strength (elo 0 = the web app shows "–")
        set_str(&mut hub.system.chip, "PC (simulator)");
        set_str(
            &mut hub.system.model,
            file.strip_suffix(".bin").unwrap_or(file),
        );
        let buckets = chess_engine::nnue::net_out_buckets();
        let arch = if buckets > 1 {
            format!("768→{hidden}×2→1, {ft}, {buckets} buckets")
        } else {
            format!("768→{hidden}×2→1, {ft}")
        };
        set_str(&mut hub.system.model_arch, &arch);
        hub.system.model_bytes = bytes as u32;
        hub.set_provisioning(options.provision);
        set_str(&mut hub.net.mode, "ap");
        set_str(&mut hub.net.ap_ssid, "ESP-Chess-SIM");
        SimPlatform {
            start: Instant::now(),
            hub: Mutex::new(hub),
            portal: format!("localhost:{}", options.port),
        }
    }
}

fn set_str<const N: usize>(target: &mut heapless::String<N>, value: &str) {
    target.clear();
    let _ = target.push_str(value);
}

impl Platform for SimPlatform {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn with_hub<R>(&self, f: impl FnOnce(&mut Hub) -> R) -> R {
        let mut hub = self
            .hub
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut hub)
    }

    fn wifi_save(&self, ssid: &str, _password: &str) {
        println!("[sim] WiFi saved: {ssid} (simulated – the connection is faked)");
        self.with_hub(|hub| {
            set_str(&mut hub.net.mode, "sta");
            set_str(&mut hub.net.sta_ssid, ssid);
            set_str(&mut hub.net.sta_ip, "127.0.0.1");
            hub.net.sta_error.clear();
            hub.net.connecting = false;
        });
    }

    fn wifi_forget(&self) {
        println!("[sim] WiFi forgotten");
        self.with_hub(|hub| {
            set_str(&mut hub.net.mode, "ap");
            hub.net.sta_ssid.clear();
            hub.net.sta_ip.clear();
        });
    }

    fn wifi_scan(&self, out: &mut dyn std::fmt::Write) -> std::fmt::Result {
        // Fake networks for development and tests.
        out.write_str(
            "{\"networks\":[{\"ssid\":\"Home-WiFi\",\"rssi\":-48,\"secure\":true},\
             {\"ssid\":\"Cafe\",\"rssi\":-71,\"secure\":false},\
             {\"ssid\":\"Neighbor 5G\",\"rssi\":-80,\"secure\":true}]}",
        )
    }

    fn portal_host(&self) -> &str {
        &self.portal
    }
}

/// Thread that calls `hub.tick` every 250 ms (clocks, cleanup, demo).
pub fn spawn_ticker(platform: &'static SimPlatform) {
    std::thread::spawn(move || {
        loop {
            let now = platform.now_ms();
            platform.with_hub(|hub| hub.tick(now));
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}
