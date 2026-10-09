//! Screen drawing. `Renderer` remembers what is already on the display
//! and on the next call redraws only the changes.

use core::fmt::Write;

use chess_core::{Color, Move, PieceKind, Position, Square, legal_moves, to_san};
use chess_hub::{DisplayView, EngineStats, GamePreview};
use embedded_graphics::mono_font::iso_8859_2::{FONT_6X10, FONT_7X13, FONT_10X20};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};
use heapless::String;

use crate::colors;
use crate::pieces;
use crate::qr::Qr;
use crate::text::{Key, Lang};
use crate::{HEIGHT, WIDTH};

/// Board position on the display.
const BOARD_X: i32 = 20;
const BOARD_Y: i32 = 2;
const SQUARE: u32 = 25;
const BOARD_SIZE: u32 = SQUARE * 8;
/// Evaluation bar left of the board.
const EVAL_X: i32 = 5;
const EVAL_W: u32 = 10;
/// Text rows below the board.
const NAMES_Y: i32 = 206;
const STATS_Y: i32 = 222;

/// What to show.
#[derive(Clone, Copy, Debug)]
pub enum Screen<'a> {
    /// Message (startup, connecting…).
    Message { title: &'a str, subtitle: &'a str },
    /// Main game + stats.
    Game,
    /// Overview: players, games, mini boards.
    Overview,
    /// QR code: title above the code, two lines below it.
    Qr {
        title: &'a str,
        content: &'a str,
        line1: &'a str,
        line2: &'a str,
    },
    /// Factory reset countdown (seconds left; 0 = reset in progress).
    Reset { seconds: u8 },
    /// Text page (statistics): a title and up to [`INFO_LINES`] lines.
    Info {
        title: &'a str,
        lines: &'a [&'a str],
    },
}

/// Maximum number of lines on the [`Screen::Info`] screen.
pub const INFO_LINES: usize = 10;
/// Maximum line length on the [`Screen::Info`] screen (7 px font on 240 px).
pub const INFO_LINE_LEN: usize = 33;
const INFO_TITLE_Y: i32 = 6;
const INFO_FIRST_Y: i32 = 36;
const INFO_LINE_STEP: i32 = 20;

/// One frame to render.
pub struct Frame<'a> {
    pub screen: Screen<'a>,
    pub view: &'a DisplayView,
    pub lang: Lang,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    None,
    Message,
    Game,
    Overview,
    Qr,
    Reset,
    Info,
}

/// The row below the board shows the engine's principal variation (max. 38 chars of the 6×10 font).
const PV_LINE_CHARS: usize = 38;
/// Best move on the right of the status row (longest SAN is "exd8=Q+").
const BEST_MOVE_CHARS: usize = 8;

/// What the variation text is built from (if unchanged, SAN is not recomputed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PvKey {
    game: u16,
    ply: u16,
    /// The variation continues from the current position, or only after its first move (demo between moves).
    pub(crate) skip_first: bool,
    moves: [Move; chess_hub::PV_LEN],
    len: u8,
}

/// Unknown square content (forces a redraw).
const UNKNOWN: u16 = u16::MAX;

/// Display state – what is currently drawn.
pub struct Renderer {
    kind: Kind,
    squares: [u16; 64],
    eval_split: i32,
    /// Last known evaluation for the shown game (game id, centipawns from White's view).
    eval_for: Option<(u16, i32)>,
    turn: Option<Color>,
    names: String<64>,
    /// Color of the drawn names/variation row.
    names_color: Rgb565,
    /// What the variation in `pv_text` was computed from (SAN is recomputed only on change).
    pv_key: Option<PvKey>,
    pv_text: String<64>,
    /// Best move in SAN (empty when the variation does not belong to the current position).
    best_text: String<16>,
    stats: String<64>,
    header: String<64>,
    minis: [Option<(u16, u32)>; 4],
    text_key: String<128>,
    reset_seconds: u8,
    /// Info screen lines that are currently drawn.
    info: [String<INFO_LINE_LEN>; INFO_LINES],
    /// How many pixels were drawn last time (for tests and debugging).
    pub pixels_drawn: u32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub const fn new() -> Renderer {
        Renderer {
            kind: Kind::None,
            squares: [UNKNOWN; 64],
            eval_split: -1,
            eval_for: None,
            turn: None,
            names: String::new(),
            names_color: colors::BACKGROUND,
            pv_key: None,
            pv_text: String::new(),
            best_text: String::new(),
            stats: String::new(),
            header: String::new(),
            minis: [None; 4],
            text_key: String::new(),
            reset_seconds: u8::MAX,
            info: [const { String::new() }; INFO_LINES],
            pixels_drawn: 0,
        }
    }

    /// Forgets what is on the display (the next draw is complete).
    pub fn invalidate(&mut self) {
        self.kind = Kind::None;
    }

    /// Renders a frame (only the changes since the last state).
    pub fn render<D>(&mut self, target: &mut D, frame: &Frame) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        self.pixels_drawn = 0;
        let kind = match frame.screen {
            Screen::Message { .. } => Kind::Message,
            Screen::Game => Kind::Game,
            Screen::Overview => Kind::Overview,
            Screen::Qr { .. } => Kind::Qr,
            Screen::Reset { .. } => Kind::Reset,
            Screen::Info { .. } => Kind::Info,
        };
        if kind != self.kind {
            // New screen → clear and forget everything drawn.
            let background = if kind == Kind::Qr {
                colors::QR_LIGHT
            } else {
                colors::BACKGROUND
            };
            self.fill(
                target,
                Rectangle::new(Point::zero(), Size::new(WIDTH, HEIGHT)),
                background,
            )?;
            self.squares = [UNKNOWN; 64];
            self.eval_split = -1;
            self.turn = None;
            self.names.clear();
            self.stats.clear();
            self.header.clear();
            self.minis = [None; 4];
            self.text_key.clear();
            self.reset_seconds = u8::MAX;
            for line in self.info.iter_mut() {
                line.clear();
            }
            self.kind = kind;
        }
        match frame.screen {
            Screen::Message { title, subtitle } => self.draw_message(target, title, subtitle),
            Screen::Game => self.draw_game(target, frame),
            Screen::Overview => self.draw_overview(target, frame),
            Screen::Qr {
                title,
                content,
                line1,
                line2,
            } => self.draw_qr(target, title, content, line1, line2),
            Screen::Reset { seconds } => self.draw_reset(target, seconds, frame.lang),
            Screen::Info { title, lines } => self.draw_info(target, title, lines),
        }
    }

    // ------------------------------------------------------------------
    // "Info" screen (statistics) – only changed lines are redrawn
    // ------------------------------------------------------------------

    fn draw_info<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        lines: &[&str],
    ) -> Result<(), D::Error> {
        if self.header.as_str() != title {
            self.header.clear();
            let _ = self.header.push_str(title);
            self.text_line(
                target,
                title,
                INFO_TITLE_Y,
                &FONT_10X20,
                colors::TEXT,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
        }
        for i in 0..INFO_LINES {
            let text = lines.get(i).copied().unwrap_or("");
            // truncate to the line capacity on whole characters (UTF-8)
            let end = text
                .char_indices()
                .map(|(i, c)| i + c.len_utf8())
                .take_while(|&end| end <= INFO_LINE_LEN)
                .last()
                .unwrap_or(0);
            let text = &text[..end];
            if self.info[i].as_str() == text {
                continue;
            }
            self.info[i].clear();
            let _ = self.info[i].push_str(text);
            self.text_line(
                target,
                text,
                INFO_FIRST_Y + i as i32 * INFO_LINE_STEP,
                &FONT_7X13,
                colors::TEXT,
                colors::BACKGROUND,
                Alignment::Left,
            )?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Drawing helpers
    // ------------------------------------------------------------------

    fn fill<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        area: Rectangle,
        color: Rgb565,
    ) -> Result<(), D::Error> {
        self.pixels_drawn += area.size.width * area.size.height;
        target.fill_solid(&area, color)
    }

    /// Text on a row: clears the whole row first, then draws.
    #[allow(clippy::too_many_arguments)]
    fn text_line<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        text: &str,
        y: i32,
        font: &MonoFont<'_>,
        color: Rgb565,
        background: Rgb565,
        alignment: Alignment,
    ) -> Result<(), D::Error> {
        let height = font.character_size.height;
        self.fill(
            target,
            Rectangle::new(Point::new(0, y), Size::new(WIDTH, height)),
            background,
        )?;
        let x = match alignment {
            Alignment::Left => 4,
            Alignment::Center => WIDTH as i32 / 2,
            Alignment::Right => WIDTH as i32 - 4,
        };
        draw_text(target, text, Point::new(x, y), font, color, alignment)
    }

    // ------------------------------------------------------------------
    // "Message" screen
    // ------------------------------------------------------------------

    fn draw_message<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        subtitle: &str,
    ) -> Result<(), D::Error> {
        let mut key: String<128> = String::new();
        let _ = write!(key, "{title}\u{1}{subtitle}");
        if key == self.text_key {
            return Ok(());
        }
        self.text_key = key;
        // Knight as a logo (25 px bitmap scaled 2×).
        let knight = chess_core::Piece::new(Color::White, PieceKind::Knight);
        let origin = Point::new(WIDTH as i32 / 2 - 25, 40);
        let pixels = (0..50u32).flat_map(|y| {
            (0..50u32)
                .map(move |x| pieces::pixel(knight, 25, x / 2, y / 2).unwrap_or(colors::BACKGROUND))
        });
        self.pixels_drawn += 2500;
        target.fill_contiguous(&Rectangle::new(origin, Size::new(50, 50)), pixels)?;
        let (title, subtitle) = (clip::<40>(title, 22), clip::<64>(subtitle, 38));
        self.text_line(
            target,
            &title,
            110,
            &FONT_10X20,
            colors::TEXT,
            colors::BACKGROUND,
            Alignment::Center,
        )?;
        self.text_line(
            target,
            &subtitle,
            140,
            &FONT_6X10,
            colors::MUTED,
            colors::BACKGROUND,
            Alignment::Center,
        )
    }

    // ------------------------------------------------------------------
    // "Game" screen
    // ------------------------------------------------------------------

    fn draw_game<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        frame: &Frame,
    ) -> Result<(), D::Error> {
        let view = frame.view;
        let start = Position::start();
        let (position, last_move, featured) = match &view.featured {
            Some(game) => (&game.position, game.last_move, Some(game)),
            None => (&start, None, None),
        };

        // 1) Squares – only the changed ones are drawn.
        let check_square = position
            .in_check()
            .then(|| position.king_square(position.side_to_move()));
        for index in 0..64u8 {
            let sq = Square::new(index);
            let piece_code = position.piece_at(sq).map_or(12u16, |p| p.index() as u16);
            let last = last_move.is_some_and(|m| m.from() == sq || m.to() == sq);
            let check = check_square == Some(sq);
            let code = piece_code | (u16::from(last) << 4) | (u16::from(check) << 5);
            if self.squares[index as usize] != code {
                self.draw_square(target, position, sq, last, check)?;
                self.squares[index as usize] = code;
            }
        }

        // 2) Evaluation bar (White's advantage = more white from the bottom).
        let eval = self.eval_for_featured(view, featured);
        let split = eval_split(eval);
        if split != self.eval_split {
            let top = Rectangle::new(
                Point::new(EVAL_X, BOARD_Y),
                Size::new(EVAL_W, (BOARD_SIZE as i32 - split) as u32),
            );
            let bottom = Rectangle::new(
                Point::new(EVAL_X, BOARD_Y + BOARD_SIZE as i32 - split),
                Size::new(EVAL_W, split as u32),
            );
            self.fill(target, top, colors::rgb(0x404040))?;
            self.fill(target, bottom, colors::rgb(0xF0F0F0))?;
            self.eval_split = split;
        }

        // 3) Side to move (dot top right = Black, bottom right = White).
        let turn = position.side_to_move();
        if self.turn != Some(turn) && featured.is_some_and(|g| g.active) {
            // Clear both spots including the outline (the outline exceeds the circle by a pixel).
            self.fill(
                target,
                Rectangle::new(Point::new(222, 2), Size::new(16, 16)),
                colors::BACKGROUND,
            )?;
            self.fill(
                target,
                Rectangle::new(Point::new(222, 186), Size::new(16, 16)),
                colors::BACKGROUND,
            )?;
            let (y, fill) = match turn {
                Color::White => (188, colors::rgb(0xF0F0F0)),
                Color::Black => (4, colors::rgb(0x202020)),
            };
            let style = embedded_graphics::primitives::PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(colors::ACCENT)
                .stroke_width(2)
                .build();
            Circle::new(Point::new(224, y), 12)
                .into_styled(style)
                .draw(target)?;
            self.pixels_drawn += 288;
            self.turn = Some(turn);
        }

        // 4) Engine principal variation (SAN is recomputed only when the variation changes).
        let key = featured
            .filter(|g| g.active)
            .and_then(|g| pv_key(&view.engine, g));
        if key != self.pv_key {
            (self.pv_text, self.best_text) = match (&key, featured) {
                (Some(key), Some(game)) => pv_texts(&game.position, key),
                _ => (String::new(), String::new()),
            };
            self.pv_key = key;
        }

        // 5) Row below the board: the variation, otherwise player names (and result).
        let mut names: String<64> = String::new();
        let (color, alignment) = if !self.pv_text.is_empty() {
            let _ = names.push_str(&self.pv_text);
            // Engine is searching this very position → highlight; otherwise (demo between moves) gray.
            let live = view.engine.thinking
                && featured.is_some_and(|g| view.engine.game == Some(g.id))
                && key.is_some_and(|k| !k.skip_first);
            let color = if live { colors::TEXT } else { colors::MUTED };
            (color, Alignment::Left)
        } else {
            match featured {
                Some(game) => {
                    if game.demo {
                        let _ = write!(names, "{}: ", frame.lang.text(Key::Demo));
                    }
                    let _ = write!(
                        names,
                        "{} - {}",
                        clip::<20>(&game.white, 14),
                        clip::<20>(&game.black, 14)
                    );
                    if !game.active {
                        let _ = write!(names, "  {}", game.result);
                    }
                }
                None => {
                    let _ = names.push_str(frame.lang.text(Key::NoGames));
                }
            }
            (colors::TEXT, Alignment::Center)
        };
        if names != self.names || color != self.names_color {
            self.text_line(
                target,
                &names,
                NAMES_Y,
                &FONT_6X10,
                color,
                colors::BACKGROUND,
                alignment,
            )?;
            self.names = names;
            self.names_color = color;
        }

        // 6) Status: depth (plies), kN/s, evaluation – and the current best move on the right.
        let engine = &view.engine;
        let (depth, nps) = if engine.thinking {
            (engine.depth, engine.nps)
        } else {
            (engine.last_depth, engine.last_nps)
        };
        let mut stats: String<64> = String::new();
        let _ = write!(
            stats,
            "{}{:<2} {:>3}.{} kN/s ",
            frame.lang.text(Key::Depth),
            depth,
            nps / 1000,
            (nps / 100) % 10
        );
        let sign = if eval < 0 { '-' } else { '+' };
        let _ = write!(
            stats,
            "{sign}{}.{:02}",
            eval.unsigned_abs() / 100,
            eval.unsigned_abs() % 100
        );
        let left_len = stats.len();
        let _ = write!(stats, "\u{1}{}", self.best_text);
        if stats != self.stats {
            let color = if engine.thinking {
                colors::ACCENT
            } else {
                colors::MUTED
            };
            let (left, right) = (&stats[..left_len], &stats[left_len + 1..]);
            self.fill(
                target,
                Rectangle::new(
                    Point::new(0, STATS_Y),
                    Size::new(WIDTH, FONT_7X13.character_size.height),
                ),
                colors::BACKGROUND,
            )?;
            draw_text(
                target,
                left,
                Point::new(4, STATS_Y),
                &FONT_7X13,
                color,
                Alignment::Left,
            )?;
            draw_text(
                target,
                right,
                Point::new(WIDTH as i32 - 4, STATS_Y),
                &FONT_7X13,
                color,
                Alignment::Right,
            )?;
            self.stats = stats;
        }
        Ok(())
    }

    /// Evaluation for the shown game: live from the engine if it is searching this game,
    /// otherwise the last known one for the same game (0 for another game).
    fn eval_for_featured(&mut self, view: &DisplayView, featured: Option<&GamePreview>) -> i32 {
        let Some(game) = featured else {
            return 0;
        };
        if view.engine.game == Some(game.id) && view.engine.depth > 0 {
            self.eval_for = Some((game.id, view.engine.score_white));
        }
        match self.eval_for {
            Some((id, score)) if id == game.id => score,
            _ => 0,
        }
    }

    /// One 25×25 square (background + piece) in a single transfer to the display.
    fn draw_square<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        position: &Position,
        sq: Square,
        last: bool,
        check: bool,
    ) -> Result<(), D::Error> {
        let light = (sq.file() + sq.rank()) % 2 == 1;
        let background = match (light, last, check) {
            (_, _, true) => colors::CHECK,
            (true, true, _) => colors::LIGHT_LAST,
            (false, true, _) => colors::DARK_LAST,
            (true, false, _) => colors::LIGHT,
            (false, false, _) => colors::DARK,
        };
        let x = BOARD_X + sq.file() as i32 * SQUARE as i32;
        let y = BOARD_Y + (7 - sq.rank() as i32) * SQUARE as i32;
        let piece = position.piece_at(sq);
        let pixels = (0..SQUARE).flat_map(move |py| {
            (0..SQUARE).map(move |px| {
                piece
                    .and_then(|p| pieces::pixel(p, SQUARE, px, py))
                    .unwrap_or(background)
            })
        });
        self.pixels_drawn += SQUARE * SQUARE;
        target.fill_contiguous(
            &Rectangle::new(Point::new(x, y), Size::new(SQUARE, SQUARE)),
            pixels,
        )
    }

    // ------------------------------------------------------------------
    // "Overview" screen
    // ------------------------------------------------------------------

    fn draw_overview<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        frame: &Frame,
    ) -> Result<(), D::Error> {
        let view = frame.view;
        let mut header: String<64> = String::new();
        let _ = write!(
            header,
            "{}: {}   {}: {}",
            frame.lang.text(Key::Players),
            view.players_online,
            frame.lang.text(Key::Games),
            view.active_games
        );
        if header != self.header {
            self.text_line(
                target,
                &header,
                2,
                &FONT_7X13,
                colors::TEXT,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
            self.header = header;
        }

        const SLOTS: [(i32, i32); 4] = [(16, 20), (128, 20), (16, 130), (128, 130)];
        for (slot, &(x, y)) in SLOTS.iter().enumerate() {
            let preview = view.previews.get(slot);
            let key = preview.map(|p| (p.id, p.version));
            if self.minis[slot] == key {
                continue;
            }
            self.minis[slot] = key;
            // Clear the slot (including the label).
            self.fill(
                target,
                Rectangle::new(Point::new(x, y), Size::new(96, 108)),
                colors::BACKGROUND,
            )?;
            let Some(preview) = preview else { continue };
            self.draw_mini(
                target,
                &preview.position,
                preview.last_move,
                Point::new(x, y),
            )?;
            let mut label: String<48> = String::new();
            let _ = write!(
                label,
                "{}-{}",
                clip::<20>(&preview.white, 7),
                clip::<20>(&preview.black, 7)
            );
            draw_text(
                target,
                &label,
                Point::new(x + 48, y + 98),
                &FONT_6X10,
                colors::MUTED,
                Alignment::Center,
            )?;
        }
        if view.previews.is_empty() && self.text_key.is_empty() {
            let _ = self.text_key.push_str("empty");
            self.text_line(
                target,
                frame.lang.text(Key::NoGames),
                110,
                &FONT_7X13,
                colors::MUTED,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
        } else if !view.previews.is_empty() && !self.text_key.is_empty() {
            self.text_key.clear();
        }
        Ok(())
    }

    /// Mini board 96×96 (12 px squares) in a single transfer.
    fn draw_mini<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        position: &Position,
        last: Option<chess_core::Move>,
        origin: Point,
    ) -> Result<(), D::Error> {
        let pixels = (0..96u32).flat_map(move |py| {
            (0..96u32).map(move |px| {
                let file = (px / 12) as u8;
                let rank = 7 - (py / 12) as u8;
                let sq = Square::from_coords(file, rank);
                let light = (file + rank) % 2 == 1;
                let highlighted = last.is_some_and(|m| m.from() == sq || m.to() == sq);
                let background = match (light, highlighted) {
                    (true, true) => colors::LIGHT_LAST,
                    (false, true) => colors::DARK_LAST,
                    (true, false) => colors::LIGHT,
                    (false, false) => colors::DARK,
                };
                position
                    .piece_at(sq)
                    .and_then(|p| pieces::pixel(p, 12, px % 12, py % 12))
                    .unwrap_or(background)
            })
        });
        self.pixels_drawn += 96 * 96;
        target.fill_contiguous(&Rectangle::new(origin, Size::new(96, 96)), pixels)
    }

    // ------------------------------------------------------------------
    // "QR code" screen
    // ------------------------------------------------------------------

    fn draw_qr<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        content: &str,
        line1: &str,
        line2: &str,
    ) -> Result<(), D::Error> {
        let mut key: String<128> = String::new();
        let _ = write!(key, "{title}\u{1}{content}\u{1}{line1}\u{1}{line2}");
        if key == self.text_key {
            return Ok(());
        }
        self.text_key = key;
        let bg = colors::QR_LIGHT;
        self.text_line(
            target,
            &clip::<48>(title, 22),
            2,
            &FONT_10X20,
            colors::QR_DARK,
            bg,
            Alignment::Center,
        )?;

        // Code area 176×176 including the quiet zone (2-module border).
        self.fill(
            target,
            Rectangle::new(Point::new(0, 24), Size::new(WIDTH, 178)),
            bg,
        )?;
        if let Some(qr) = Qr::encode(content) {
            let modules = qr.size() + 4;
            let scale = (176 / modules).max(1);
            let side = (modules * scale) as u32;
            let origin = Point::new(
                (WIDTH as i32 - side as i32) / 2,
                24 + (176 - side as i32) / 2,
            );
            let pixels = (0..side).flat_map(|py| {
                let qr = &qr;
                (0..side).map(move |px| {
                    let mx = px as i32 / scale - 2;
                    let my = py as i32 / scale - 2;
                    if qr.module(mx, my) {
                        colors::QR_DARK
                    } else {
                        colors::QR_LIGHT
                    }
                })
            });
            self.pixels_drawn += side * side;
            target.fill_contiguous(&Rectangle::new(origin, Size::new(side, side)), pixels)?;
        }
        self.text_line(
            target,
            &clip::<64>(line1, 34),
            204,
            &FONT_7X13,
            colors::QR_DARK,
            bg,
            Alignment::Center,
        )?;
        self.text_line(
            target,
            &clip::<64>(line2, 34),
            222,
            &FONT_7X13,
            colors::QR_DARK,
            bg,
            Alignment::Center,
        )
    }

    // ------------------------------------------------------------------
    // "Reset" screen
    // ------------------------------------------------------------------

    fn draw_reset<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        seconds: u8,
        lang: Lang,
    ) -> Result<(), D::Error> {
        if seconds == self.reset_seconds {
            return Ok(());
        }
        let first = self.reset_seconds == u8::MAX;
        self.reset_seconds = seconds;
        if seconds == 0 {
            return self.text_line(
                target,
                lang.text(Key::Resetting),
                110,
                &FONT_7X13,
                colors::WARNING,
                colors::BACKGROUND,
                Alignment::Center,
            );
        }
        if first {
            self.text_line(
                target,
                lang.text(Key::ResetIn),
                60,
                &FONT_10X20,
                colors::DANGER,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
            self.text_line(
                target,
                lang.text(Key::ReleaseToCancel),
                190,
                &FONT_7X13,
                colors::MUTED,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
        }
        let mut number: String<8> = String::new();
        let _ = write!(number, "{seconds} s");
        self.text_line(
            target,
            &number,
            100,
            &FONT_10X20,
            colors::TEXT,
            colors::BACKGROUND,
            Alignment::Center,
        )?;
        // Progress bar (10 s → full).
        let done = (10u32.saturating_sub(seconds as u32)).min(10) * 20;
        self.fill(
            target,
            Rectangle::new(Point::new(20, 140), Size::new(200, 12)),
            colors::rgb(0x303030),
        )?;
        self.fill(
            target,
            Rectangle::new(Point::new(20, 140), Size::new(done, 12)),
            colors::DANGER,
        )
    }
}

/// How many pixels of the evaluation bar are white (0..=200). ±8 pawns = full bar.
fn eval_split(score_white: i32) -> i32 {
    let half = BOARD_SIZE as i32 / 2;
    if score_white.abs() > chess_engine_mate_threshold() {
        return if score_white > 0 {
            BOARD_SIZE as i32 - 2
        } else {
            2
        };
    }
    (half + score_white.clamp(-800, 800) * half / 800).clamp(2, BOARD_SIZE as i32 - 2)
}

/// Values above this threshold are mates (same constant as in the engine).
pub(crate) const fn chess_engine_mate_threshold() -> i32 {
    29_500
}

/// Variation to display: only engine moves (not hints – the hub does not write those to `pv`) for
/// this game, either from the current position or, in demo, the continuation after the move just played.
/// In a game with a human the continuation is not shown – it would be a hint for the human.
pub(crate) fn pv_key(engine: &EngineStats, game: &GamePreview) -> Option<PvKey> {
    if engine.pv_game != Some(game.id) || engine.pv_len == 0 {
        return None;
    }
    let skip_first = if engine.pv_ply == game.ply {
        false
    } else if game.demo
        && engine.pv_ply + 1 == game.ply
        && engine.pv_len > 1
        && game.last_move == Some(engine.pv[0])
    {
        true
    } else {
        return None;
    };
    Some(PvKey {
        game: game.id,
        ply: game.ply,
        skip_first,
        moves: engine.pv,
        len: engine.pv_len,
    })
}

/// Variation text ("23...Rxa4 24.Qb3 Ra8", as much as fits on the row) and the best move in SAN
/// (only when the variation starts at the current position). Checks legality – stops on a mismatch.
pub(crate) fn pv_texts(root: &Position, key: &PvKey) -> (String<64>, String<16>) {
    let (mut line, mut best) = (String::<64>::new(), String::<16>::new());
    let mut pos = root.clone();
    let first = usize::from(key.skip_first);
    for (i, &mv) in key.moves[first..key.len as usize].iter().enumerate() {
        if !legal_moves(&pos).contains(mv) {
            break;
        }
        let san = to_san(&pos, mv);
        if i == 0 && !key.skip_first {
            let _ = best.push_str(san.as_str());
        }
        let mut token: String<24> = String::new();
        let number = pos.fullmove_number();
        let _ = match pos.side_to_move() {
            Color::White => write!(token, "{number}.{san}"),
            Color::Black if i == 0 => write!(token, "{number}...{san}"),
            Color::Black => write!(token, "{san}"),
        };
        let sep = usize::from(!line.is_empty());
        if line.len() + sep + token.len() > PV_LINE_CHARS {
            break;
        }
        if sep == 1 {
            let _ = line.push(' ');
        }
        let _ = line.push_str(&token);
        pos = pos.make_move(mv);
    }
    debug_assert!(best.len() <= BEST_MOVE_CHARS);
    (line, best)
}

pub(crate) fn clip<const N: usize>(text: &str, max: usize) -> String<N> {
    let mut out: String<N> = String::new();
    let count = text.chars().count();
    for (i, c) in text.chars().enumerate() {
        if count > max && i + 1 >= max {
            let _ = out.push('.');
            break;
        }
        if out.push(c).is_err() {
            break;
        }
    }
    out
}

pub(crate) fn draw_text<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    text: &str,
    at: Point,
    font: &MonoFont<'_>,
    color: Rgb565,
    alignment: Alignment,
) -> Result<(), D::Error> {
    let style = MonoTextStyle::new(font, color);
    let text_style = TextStyleBuilder::new()
        .alignment(alignment)
        .baseline(Baseline::Top)
        .build();
    Text::with_text_style(text, at, style, text_style).draw(target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::parse_uci;

    fn key_for(root: &Position, ucis: &[&str], skip_first: bool) -> PvKey {
        let mut moves = [Move::default(); chess_hub::PV_LEN];
        let mut pos = root.clone();
        for (i, uci) in ucis.iter().enumerate() {
            moves[i] = parse_uci(&pos, uci).unwrap();
            pos = pos.make_move(moves[i]);
        }
        PvKey {
            game: 1,
            ply: 0,
            skip_first,
            moves,
            len: ucis.len() as u8,
        }
    }

    #[test]
    fn pv_text_with_move_numbers_and_best_move() {
        let start = Position::start();
        let key = key_for(&start, &["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"], false);
        let (line, best) = pv_texts(&start, &key);
        assert_eq!(line.as_str(), "1.e4 e5 2.Nf3 Nc6 3.Bb5");
        assert_eq!(best.as_str(), "e4");
    }

    #[test]
    fn pv_continuation_after_played_move_starts_with_black() {
        // Demo between moves: the first variation move is already played, the best move is not shown.
        let start = Position::start();
        let key = key_for(&start, &["e2e4", "e7e5", "g1f3"], true);
        let after = start.make_move(key.moves[0]);
        let (line, best) = pv_texts(&after, &key);
        assert_eq!(line.as_str(), "1...e5 2.Nf3");
        assert!(best.is_empty());
    }

    #[test]
    fn pv_text_fits_the_line_and_stops_at_illegal_move() {
        let start = Position::start();
        let key = key_for(
            &start,
            &[
                "g1f3", "g8f6", "b1c3", "b8c6", "f3g1", "f6g8", "g1f3", "g8f6", "c3b1", "c6b8",
                "b1c3", "b8c6",
            ],
            false,
        );
        let (line, _) = pv_texts(&start, &key);
        assert!(line.len() <= PV_LINE_CHARS, "{line}");
        assert!(line.starts_with("1.Nf3 Nf6 2.Nc3 Nc6"));
        // Variation from another position (e.g. after a takeback) → nothing invalid is printed.
        let other = Position::from_fen("8/8/8/8/8/8/8/K6k w - - 0 1").unwrap();
        let (line, best) = pv_texts(&other, &key);
        assert!(line.is_empty() && best.is_empty());
    }
}
