//! Display: screen selection and periodic redrawing.
//!
//! A short press of BOOT cycles through the pages (the list depends on the current state):
//! automatic (human games take turns, otherwise the demo) → each running game →
//! games overview → QR codes (join the board's Wi-Fi, address on the AP, address in the home
//! network) → statistics (players, games, uptime, engine, network, web server, memory) → and
//! around again. After 2 minutes without a press the display returns to automatic mode.
//! Holding BOOT: factory reset countdown.

use core::fmt::Write as _;
use portable_atomic::Ordering;

use chess_display::{Frame, INFO_LINE_LEN, INFO_LINES, Key, Lang, Screen};
use chess_hub::{DisplayView, NetInfo};
use embassy_time::{Duration, Timer};
use heapless::String;
use log::warn;

use crate::board::{self, Renderer};
use crate::shared::{
    AP_HOST, BUTTON_HELD_MS, BUTTON_PRESSES, RESET_HOLD_MS, RESETTING, now_ms, with_hub,
};
use crate::{net, stats};

/// Display language (the web has its own switch).
const LANG: Lang = Lang::En;
/// How often the display is redrawn.
const REFRESH: Duration = Duration::from_millis(200);
/// After how long without a BOOT press the display returns to automatic mode.
const BACK_TO_AUTO_MS: u64 = 120_000;
/// When several human games are running, they take turns every 15 s.
const ROTATE_MS: u64 = 15_000;
/// After how long holding BOOT the reset countdown is shown.
const RESET_WARNING_MS: u32 = 2_000;

/// Display page (switched with the BOOT button).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    /// Main game (human games take turns, otherwise the demo).
    Auto,
    /// The i-th running game (including the demo).
    Game(usize),
    /// Overview of all running games.
    Overview,
    /// QR: join the board's Wi-Fi.
    QrJoinAp,
    /// QR: app address on the board's Wi-Fi.
    QrOpenAp,
    /// QR: app address in the home network.
    QrOpenSta,
    /// Statistics.
    Stats,
}

/// List of pages for the current state (number of games, home network connection).
fn pages(active_games: usize, sta_connected: bool) -> heapless::Vec<Page, 24> {
    let mut list = heapless::Vec::new();
    let _ = list.push(Page::Auto);
    if active_games > 1 {
        for i in 0..active_games.min(16) {
            let _ = list.push(Page::Game(i));
        }
        let _ = list.push(Page::Overview);
    }
    let _ = list.push(Page::QrJoinAp);
    let _ = list.push(Page::QrOpenAp);
    if sta_connected {
        let _ = list.push(Page::QrOpenSta);
    }
    let _ = list.push(Page::Stats);
    list
}

/// Texts of the QR screen.
struct QrTexts {
    title: &'static str,
    content: String<96>,
    line1: String<64>,
    line2: String<64>,
}

fn qr_texts(net: &NetInfo, page: Page) -> QrTexts {
    let mut texts = QrTexts {
        title: LANG.text(Key::Scan),
        content: String::new(),
        line1: String::new(),
        line2: String::new(),
    };
    match page {
        Page::QrJoinAp => {
            texts.title = LANG.text(Key::JoinWifi);
            let _ = write!(texts.content, "WIFI:T:nopass;S:{};;", net.ap_ssid);
            let _ = write!(texts.line1, "Wi-Fi: {}", net.ap_ssid);
        }
        Page::QrOpenSta => {
            let _ = write!(texts.content, "http://{}/", net.sta_ip);
            let _ = write!(texts.line1, "http://{}/", net.sta_ip);
            let _ = texts.line2.push_str(&net.sta_ssid);
        }
        _ => {
            texts.title = LANG.text(Key::OpenBrowser);
            let _ = write!(texts.content, "http://{AP_HOST}/");
            let _ = write!(texts.line1, "http://{AP_HOST}/");
            let _ = texts.line2.push_str(&net.ap_ssid);
        }
    }
    texts
}

/// Lines of the statistics page.
fn stats_lines(
    view: &DisplayView,
    net: &NetInfo,
    uptime_ms: u64,
) -> [String<INFO_LINE_LEN>; INFO_LINES] {
    let mut l: [String<INFO_LINE_LEN>; INFO_LINES] = Default::default();
    let web = stats::snapshot();
    let _ = write!(
        l[0],
        "Players online: {} (max {})",
        view.players_online, view.peak_online
    );
    let _ = write!(
        l[1],
        "Games: {} running, {} played",
        view.active_games, view.games_started
    );
    let s = uptime_ms / 1000;
    let (d, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    let _ = if d > 0 {
        write!(l[2], "Uptime: {d}d {h}:{m:02}:{sec:02}")
    } else {
        write!(l[2], "Uptime: {h}:{m:02}:{sec:02}")
    };
    let _ = write!(
        l[3],
        "Engine: {} n/s, depth {}",
        view.engine.last_nps, view.engine.last_depth
    );
    let _ = if net.sta_ip.is_empty() {
        write!(l[4], "Wi-Fi: not connected")
    } else {
        write!(l[4], "Wi-Fi: {} {}", net.sta_ssid, net.sta_ip)
    };
    let _ = write!(l[5], "AP: {}", net.ap_ssid);
    let _ = write!(
        l[6],
        "Web: {}.{} req/s, avg {} ms",
        web.req_per_s_x10 / 10,
        web.req_per_s_x10 % 10,
        web.avg_ms
    );
    let _ = write!(
        l[7],
        "Web: max {} ms, busy {}/{}",
        web.max_ms,
        web.busy_max,
        net::WEB_TASKS
    );
    let _ = write!(
        l[8],
        "Web: {} requests, {} errors",
        web.requests_total, web.errors
    );
    let _ = write!(
        l[9],
        "Heap: {} kB free",
        stats::HEAP_FREE.load(Ordering::Relaxed) / 1024
    );
    let temp_dc = stats::CHIP_TEMP_DC.load(Ordering::Relaxed);
    if temp_dc != stats::NO_TEMP {
        let _ = write!(l[9], ", chip {}\u{b0}C", (temp_dc + 5).div_euclid(10));
    }
    l
}

#[embassy_executor::task]
pub async fn display_task(mut screen: board::Screen<'static>) -> ! {
    let mut renderer = Renderer::new();
    let mut page_index = 0usize;
    let mut last_press = 0u64;
    let started = now_ms();
    // Diagnostics: frames per 10 s and the longest gap between them.
    // With correct scheduling ~50 frames and a gap of ~200–300 ms; if the engine
    // starves the main thread, the gap jumps to seconds.
    let (mut frames, mut max_gap, mut last, mut report_at) =
        (0u32, 0u64, started, started + 10_000);

    loop {
        let now = now_ms();
        frames += 1;
        max_gap = max_gap.max(now - last);
        last = now;
        if now >= report_at {
            log::info!("Display: {frames} frames in 10 s, longest gap {max_gap} ms");
            (frames, max_gap, report_at) = (0, 0, now + 10_000);
        }

        let (active_games, net) =
            with_hub(|hub| (DisplayView::active_game_count(hub), hub.net.clone()));
        let list = pages(active_games, !net.sta_ip.is_empty());
        let presses = BUTTON_PRESSES.swap(0, Ordering::Relaxed) as usize;
        if presses > 0 {
            page_index = (page_index + presses) % list.len();
            last_press = now;
        } else if page_index != 0 && now - last_press >= BACK_TO_AUTO_MS {
            page_index = 0;
        }
        page_index = page_index.min(list.len() - 1);
        let page = list[page_index];

        let rotation = (now / ROTATE_MS) as usize;
        let pinned = match page {
            Page::Game(i) => Some(i),
            _ => None,
        };
        let view = with_hub(|hub| DisplayView::capture(hub, now, rotation, pinned));

        let held = BUTTON_HELD_MS.load(Ordering::Relaxed);
        let qr = qr_texts(&net, page);
        let info = stats_lines(&view, &net, now - started);
        let info_refs: [&str; INFO_LINES] = core::array::from_fn(|i| info[i].as_str());
        let screen_kind = if RESETTING.load(Ordering::Relaxed) {
            Screen::Reset { seconds: 0 }
        } else if held >= RESET_WARNING_MS {
            let left = RESET_HOLD_MS.saturating_sub(held).div_ceil(1000).max(1);
            Screen::Reset {
                seconds: left as u8,
            }
        } else if now - started < 1_500 {
            Screen::Message {
                title: "ESP32 Chess",
                subtitle: LANG.text(Key::Starting),
            }
        } else {
            match page {
                Page::Auto | Page::Game(_) => Screen::Game,
                Page::Overview => Screen::Overview,
                Page::QrJoinAp | Page::QrOpenAp | Page::QrOpenSta => Screen::Qr {
                    title: qr.title,
                    content: &qr.content,
                    line1: &qr.line1,
                    line2: &qr.line2,
                },
                Page::Stats => Screen::Info {
                    title: concat!("Stats v", env!("CARGO_PKG_VERSION")),
                    lines: &info_refs,
                },
            }
        };

        let frame = Frame {
            screen: screen_kind,
            view: &view,
            lang: LANG,
        };
        if let Err(error) = renderer.render(&mut screen.display, &frame) {
            warn!("Drawing error: {error:?}");
            renderer.invalidate();
        }
        Timer::after(REFRESH).await;
    }
}
