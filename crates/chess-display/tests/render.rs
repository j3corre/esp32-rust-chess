//! Screen tests – drawing goes into memory instead of a display.

use chess_display::{Frame, Lang, Renderer, Screen};
use chess_hub::hub::ColorChoice;
use chess_hub::{DisplayView, Hub};
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::*;

/// In-memory "display" (240×240, or 160×80 for the small display).
struct FrameBuffer {
    width: i32,
    height: i32,
    pixels: Vec<Rgb565>,
}

impl FrameBuffer {
    fn new() -> Self {
        Self::sized(240, 240)
    }

    fn sized(width: i32, height: i32) -> Self {
        FrameBuffer {
            width,
            height,
            pixels: vec![Rgb565::BLACK; (width * height) as usize],
        }
    }

    fn at(&self, x: i32, y: i32) -> Rgb565 {
        self.pixels[(y * self.width + x) as usize]
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if (0..self.width).contains(&p.x) && (0..self.height).contains(&p.y) {
                self.pixels[(p.y * self.width + p.x) as usize] = c;
            }
        }
        Ok(())
    }
}

/// Saves the framebuffer as a PNG to `target/display-snapshots/` (for visual inspection).
/// A simple uncompressed PNG ("stored" blocks), no extra dependencies.
fn save_png(fb: &FrameBuffer, name: &str) {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..fb.height {
        raw.push(0); // filter "none"
        for x in 0..fb.width {
            let c = fb.at(x, y);
            raw.extend_from_slice(&[
                c.r() << 3 | c.r() >> 2,
                c.g() << 2 | c.g() >> 4,
                c.b() << 3 | c.b() >> 2,
            ]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        zlib.push(u8::from(last));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&(b << 16 | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&(fb.width as u32).to_be_bytes());
    header.extend_from_slice(&(fb.height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/display-snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), png).unwrap();
}

fn demo_hub() -> Hub {
    let mut hub = Hub::new(7);
    hub.tick(0); // creates the demo game
    hub
}

fn game_frame(view: &DisplayView) -> Frame<'_> {
    Frame {
        screen: Screen::Game,
        view,
        lang: Lang::En,
    }
}

#[test]
fn game_screen_draws_board_and_redraws_only_changes() {
    let mut hub = demo_hub();
    let mut fb = FrameBuffer::new();
    let mut renderer = Renderer::new();

    let view = DisplayView::capture(&hub, 0, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    let full = renderer.pixels_drawn;
    assert!(full >= 240 * 240, "first draw is complete ({full} px)");

    // Square a1 (bottom left) is dark with a white rook; the square's corner keeps the square color.
    let dark = fb.at(20, 2 + 7 * 25);
    assert_ne!(dark, Rgb565::BLACK);
    // e1 holds the white king: white fill and black outline.
    let (x0, y0) = (20 + 4 * 25, 2 + 7 * 25);
    let square: Vec<Rgb565> = (0..25)
        .flat_map(|y| (0..25).map(move |x| (x, y)))
        .map(|(x, y)| fb.at(x0 + x, y0 + y))
        .collect();
    assert!(square.contains(&Rgb565::WHITE));
    assert!(square.contains(&Rgb565::BLACK));
    save_png(&fb, "display-game.png");

    // Same frame again → nothing is drawn.
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    assert_eq!(renderer.pixels_drawn, 0);

    // A move in the demo game → only a few squares are redrawn (+ text rows).
    let job = hub.next_engine_job(0).unwrap();
    let mv = chess_core::parse_uci(&job.root, "e2e4").unwrap();
    hub.engine_done(10, &job, mv, 5, 12_345);
    let view = DisplayView::capture(&hub, 10, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    let changed = renderer.pixels_drawn;
    assert!(changed > 0 && changed < 20_000, "redrew {changed} px");
    save_png(&fb, "display-game-after-move.png");
}

/// Is anything drawn in the rectangle (x0..x1, y0..y1) (a color other than the bottom-right background)?
fn has_ink(fb: &FrameBuffer, xs: core::ops::Range<i32>, ys: core::ops::Range<i32>) -> bool {
    let background = fb.at(239, 239);
    xs.flat_map(|x| ys.clone().map(move |y| (x, y)))
        .any(|(x, y)| fb.at(x, y) != background)
}

#[test]
fn demo_shows_principal_variation_and_best_move() {
    let mut hub = demo_hub();
    let mut fb = FrameBuffer::new();
    let mut renderer = Renderer::new();
    let job = hub.next_engine_job(0).unwrap();
    let mut pos = job.root.clone();
    let mut pv = Vec::new();
    for uci in [
        "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6",
    ] {
        let mv = chess_core::parse_uci(&pos, uci).unwrap();
        pv.push(mv);
        pos = pos.make_move(mv);
    }
    hub.engine_iteration(&job, 8, 35, &pv);
    hub.engine_progress(8, 14, 120_000, 16_400);
    let view = DisplayView::capture(&hub, 0, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    save_png(&fb, "display-game-pv.png");
    // Variation on the left below the board, best move at the far right of the status row.
    assert!(has_ink(&fb, 4..60, 206..216));
    assert!(has_ink(&fb, 200..236, 222..235));

    // Same variation, only a different speed → the variation row is not redrawn.
    hub.engine_progress(8, 14, 130_000, 16_500);
    let view = DisplayView::capture(&hub, 0, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    assert!(
        renderer.pixels_drawn <= 240 * 13 + 2_000,
        "{} px",
        renderer.pixels_drawn
    );

    // Move played → in demo the variation's continuation stays (gray), the best move on the right disappears.
    hub.engine_done(10, &job, pv[0], 8, 16_400);
    let view = DisplayView::capture(&hub, 10, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    save_png(&fb, "display-game-pv-after-move.png");
    assert!(has_ink(&fb, 4..60, 206..216));
    assert!(!has_ink(&fb, 200..236, 222..235));
}

#[test]
fn human_game_never_shows_continuation_for_the_human() {
    // Human vs. engine: after the engine's move the human is to move – the variation would be a hint,
    // so the display shows the player names and no move on the right.
    let mut hub = demo_hub();
    let (alice, _) = hub.hello(0, None, "Alice").unwrap();
    let game = hub
        .play_engine(0, alice, 5, ColorChoice::Black, None)
        .unwrap();
    let job = hub.next_engine_job(0).unwrap();
    assert_eq!(job.game, game);
    let e4 = chess_core::parse_uci(&job.root, "e2e4").unwrap();
    let e5 = chess_core::parse_uci(&job.root.make_move(e4), "e7e5").unwrap();
    hub.engine_iteration(&job, 6, 30, &[e4, e5]);
    let mut fb = FrameBuffer::new();
    let mut renderer = Renderer::new();
    let view = DisplayView::capture(&hub, 0, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    assert!(
        has_ink(&fb, 200..236, 222..235),
        "engine searching → move on the right"
    );
    hub.engine_done(10, &job, e4, 6, 16_000);
    let view = DisplayView::capture(&hub, 10, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    save_png(&fb, "display-human-to-move.png");
    assert!(
        !has_ink(&fb, 200..236, 222..235),
        "human to move → no move on the right"
    );
    // Names are centered (a variation would start on the left).
    assert!(!has_ink(&fb, 0..20, 206..216));
}

#[test]
fn featured_game_switches_to_human_game() {
    let mut hub = demo_hub();
    let (alice, _) = hub.hello(0, None, "Alice").unwrap();
    hub.play_engine(0, alice, 4, ColorChoice::White, None)
        .unwrap();
    let view = DisplayView::capture(&hub, 0, 0, None);
    assert_eq!(view.featured.as_ref().unwrap().white.as_str(), "Alice");
    let mut fb = FrameBuffer::new();
    Renderer::new().render(&mut fb, &game_frame(&view)).unwrap();
}

#[test]
fn overview_qr_reset_and_message_screens() {
    let hub = demo_hub();
    let view = DisplayView::capture(&hub, 0, 0, None);
    let mut fb = FrameBuffer::new();
    let mut renderer = Renderer::new();

    for lang in [Lang::En, Lang::Cs] {
        renderer
            .render(
                &mut fb,
                &Frame {
                    screen: Screen::Overview,
                    view: &view,
                    lang,
                },
            )
            .unwrap();
        save_png(&fb, "display-overview.png");
        assert!(renderer.pixels_drawn > 0);
        renderer.invalidate();
    }

    let qr = Screen::Qr {
        title: "Scan to play",
        content: "http://192.168.1.50/",
        line1: "http://192.168.1.50/",
        line2: "Home WiFi",
    };
    renderer
        .render(
            &mut fb,
            &Frame {
                screen: qr,
                view: &view,
                lang: Lang::En,
            },
        )
        .unwrap();
    // The QR is on a white background, with dark and light modules in the middle.
    let mut dark = 0;
    for y in 30..190 {
        for x in 30..210 {
            if fb.at(x, y) == Rgb565::BLACK {
                dark += 1;
            }
        }
    }
    assert!(dark > 3000, "dark QR pixels: {dark}");
    save_png(&fb, "display-qr.png");
    // Redrawing the same QR draws nothing.
    renderer
        .render(
            &mut fb,
            &Frame {
                screen: qr,
                view: &view,
                lang: Lang::En,
            },
        )
        .unwrap();
    assert_eq!(renderer.pixels_drawn, 0);

    for seconds in [7, 6, 0] {
        renderer
            .render(
                &mut fb,
                &Frame {
                    screen: Screen::Reset { seconds },
                    view: &view,
                    lang: Lang::Cs,
                },
            )
            .unwrap();
        if seconds == 6 {
            save_png(&fb, "display-reset.png");
        }
        assert!(renderer.pixels_drawn > 0);
    }

    let message = Screen::Message {
        title: "ESP32 Chess",
        subtitle: "Starting...",
    };
    renderer
        .render(
            &mut fb,
            &Frame {
                screen: message,
                view: &view,
                lang: Lang::En,
            },
        )
        .unwrap();
    save_png(&fb, "display-message.png");
    assert!(renderer.pixels_drawn > 0);
}

#[test]
fn info_screen_redraws_only_changed_lines() {
    let hub = Hub::new(1);
    let view = DisplayView::capture(&hub, 0, 0, None);
    let mut display = FrameBuffer::new();
    let mut renderer = Renderer::new();
    let lines = [
        "Players online: 2 (max 5)",
        "Games: 3 running, 12 played",
        "Uptime: 1:02:03",
        "Engine: 9289 n/s, depth 9",
        "Wi-Fi: HomeWiFi 192.168.1.50",
        "AP: ESP-Chess-4FFC, 1 client",
        "Web: 3.1 req/s, avg 24 ms",
        "Web: max 118 ms, busy 2/8",
        "Heap: 60 kB free",
    ];
    fn info<'a>(view: &'a DisplayView, lines: &'a [&'a str]) -> Frame<'a> {
        Frame {
            screen: Screen::Info {
                title: "Stats",
                lines,
            },
            view,
            lang: Lang::En,
        }
    }
    renderer.render(&mut display, &info(&view, &lines)).unwrap();
    assert!(renderer.pixels_drawn > 0);
    save_png(&display, "display-info.png");
    // same lines → nothing is drawn
    renderer.render(&mut display, &info(&view, &lines)).unwrap();
    assert_eq!(renderer.pixels_drawn, 0);
    // one line changed → only that line (240 × 13 px)
    let mut changed = lines;
    changed[0] = "Players online: 3 (max 5)";
    renderer
        .render(&mut display, &info(&view, &changed))
        .unwrap();
    assert_eq!(renderer.pixels_drawn, 240 * 13);
    // a long line with diacritics is truncated without panicking
    let long = ["Players online: ěščřžýáíéúůěščřžýáíéúů 123456"];
    renderer.render(&mut display, &info(&view, &long)).unwrap();
}

// ----------------------------------------------------------------------------
// Small 160×80 display (LilyGO T-Dongle-S3)
// ----------------------------------------------------------------------------

#[test]
fn small_display_screens() {
    use chess_display::{SMALL_HEIGHT, SMALL_WIDTH, SmallRenderer};
    let (w, h) = (SMALL_WIDTH as i32, SMALL_HEIGHT as i32);
    let mut hub = demo_hub();
    let mut fb = FrameBuffer::sized(w, h);
    let mut renderer = SmallRenderer::new();

    // Demo game with an engine variation.
    let job = hub.next_engine_job(0).unwrap();
    let mut pos = job.root.clone();
    let mut pv = Vec::new();
    for uci in [
        "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6",
    ] {
        let mv = chess_core::parse_uci(&pos, uci).unwrap();
        pv.push(mv);
        pos = pos.make_move(mv);
    }
    hub.engine_iteration(&job, 10, 35, &pv);
    hub.engine_progress(10, 16, 400_000, 40_700);
    let view = DisplayView::capture(&hub, 0, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    let full = renderer.pixels_drawn;
    assert!(full >= (w * h) as u32, "first draw is complete ({full} px)");
    save_png(&fb, "small-game.png");
    // Same frame → nothing is drawn.
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    assert_eq!(renderer.pixels_drawn, 0);
    // A move → only a few squares and rows.
    hub.engine_done(10, &job, pv[0], 10, 40_700);
    let view = DisplayView::capture(&hub, 10, 0, None);
    renderer.render(&mut fb, &game_frame(&view)).unwrap();
    assert!(
        renderer.pixels_drawn < 6_000,
        "{} px",
        renderer.pixels_drawn
    );
    save_png(&fb, "small-game-after-move.png");

    // Other screens (nothing may panic or draw outside the display).
    let screens = [
        (
            "small-qr.png",
            Screen::Qr {
                title: "Join Wi-Fi",
                content: "WIFI:T:nopass;S:ESP-Chess-4FFC;;",
                line1: "Wi-Fi: ESP-Chess-4FFC",
                line2: "",
            },
        ),
        (
            "small-qr-url.png",
            Screen::Qr {
                title: "Open in browser",
                content: "http://192.168.4.1/",
                line1: "http://192.168.4.1/",
                line2: "ESP-Chess-4FFC",
            },
        ),
        ("small-reset.png", Screen::Reset { seconds: 6 }),
        (
            "small-message.png",
            Screen::Message {
                title: "ESP32 Chess",
                subtitle: "Starting...",
            },
        ),
        ("small-overview.png", Screen::Overview),
        (
            "small-info.png",
            Screen::Info {
                title: "Stats v0.2.10",
                lines: &[
                    "Players online: 2 (max 5)",
                    "Games: 3 running, 12 played",
                    "Uptime: 1:02:03",
                    "Engine: 40712 n/s, depth 11",
                    "Wi-Fi: not connected",
                    "AP: ESP-Chess-4FFC",
                    "Web: 3.1 req/s, avg 24 ms",
                    "Web: max 118 ms, busy 2/6",
                    "Web: 812 requests, 0 errors",
                    "Heap: 31 kB free, chip 47°C",
                ],
            },
        ),
    ];
    for (name, screen) in screens {
        let frame = Frame {
            screen,
            view: &view,
            lang: Lang::En,
        };
        renderer.render(&mut fb, &frame).unwrap();
        save_png(&fb, name);
    }
}
