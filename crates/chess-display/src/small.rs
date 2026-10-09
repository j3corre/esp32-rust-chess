//! Screens for the small 160×80 display (ST7735, LilyGO T-Dongle-S3) – the same [`Frame`] and
//! [`Screen`] as the large [`crate::Renderer`], just a different layout:
//!
//! ```text
//! ┌──────────┬─┬──────────────┐
//! │          │ │ D10  40.7k   │  depth (plies), thousands of nodes/s
//! │          │e│ +0.35      ● │  evaluation, side to move
//! │ board    │v│ Rxa4         │  current best move
//! │ 80×80    │a│ 23...Rxa4    │  principal variation (wrapped),
//! │ (10 px)  │l│ 24.Qb3 Ra8   │  otherwise player names / result
//! └──────────┴─┴──────────────┘
//! ```
//!
//! Only changes are redrawn (squares, the bar, individual text rows).

use core::fmt::Write;

use chess_core::{Color, Position, Square};
use chess_hub::{DisplayView, GamePreview};
use embedded_graphics::mono_font::MonoFont;
use embedded_graphics::mono_font::iso_8859_2::{
    FONT_4X6, FONT_5X8, FONT_6X10, FONT_7X13, FONT_10X20,
};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::Alignment;
use heapless::String;

use crate::colors;
use crate::pieces;
use crate::qr::Qr;
use crate::render::{
    Frame, PvKey, Screen, chess_engine_mate_threshold, clip, draw_text, pv_key, pv_texts,
};
use crate::text::Key;

/// Small display dimensions (landscape).
pub const SMALL_WIDTH: u32 = 160;
pub const SMALL_HEIGHT: u32 = 80;

const SQUARE: u32 = 10;
const BOARD: u32 = SQUARE * 8;
const EVAL_X: i32 = BOARD as i32 + 1;
const EVAL_W: u32 = 4;
/// Text area to the right of the board.
const TEXT_X: i32 = 88;
const TEXT_W: u32 = SMALL_WIDTH - TEXT_X as u32;
/// How many 5×8 font characters fit in the text area (70 px).
const TEXT_CHARS: usize = 14;
/// Text rows on the game screen: (y, font).
const ROWS: [(i32, &MonoFont<'static>); 7] = [
    (0, &FONT_6X10),
    (11, &FONT_6X10),
    (23, &FONT_7X13),
    (38, &FONT_5X8),
    (48, &FONT_5X8),
    (58, &FONT_5X8),
    (68, &FONT_5X8),
];
/// "Side to move" dot on the right of the evaluation row.
const TURN_DOT: Point = Point::new(SMALL_WIDTH as i32 - 9, 12);

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

const UNKNOWN: u16 = u16::MAX;

/// Small display state – what is currently drawn.
pub struct SmallRenderer {
    kind: Kind,
    squares: [u16; 64],
    eval_split: i32,
    eval_for: Option<(u16, i32)>,
    turn: Option<Color>,
    rows: [String<32>; 7],
    pv_key: Option<PvKey>,
    pv_text: String<64>,
    best_text: String<16>,
    text_key: String<160>,
    reset_seconds: u8,
    info: [String<48>; 12],
    /// How many pixels were drawn last time (for tests and debugging).
    pub pixels_drawn: u32,
}

impl Default for SmallRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SmallRenderer {
    pub const fn new() -> SmallRenderer {
        SmallRenderer {
            kind: Kind::None,
            squares: [UNKNOWN; 64],
            eval_split: -1,
            eval_for: None,
            turn: None,
            rows: [const { String::new() }; 7],
            pv_key: None,
            pv_text: String::new(),
            best_text: String::new(),
            text_key: String::new(),
            reset_seconds: u8::MAX,
            info: [const { String::new() }; 12],
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
            let background = if kind == Kind::Qr {
                colors::QR_LIGHT
            } else {
                colors::BACKGROUND
            };
            self.fill(
                target,
                Rectangle::new(Point::zero(), Size::new(SMALL_WIDTH, SMALL_HEIGHT)),
                background,
            )?;
            self.squares = [UNKNOWN; 64];
            self.eval_split = -1;
            self.turn = None;
            for row in self.rows.iter_mut() {
                row.clear();
            }
            for line in self.info.iter_mut() {
                line.clear();
            }
            self.text_key.clear();
            self.reset_seconds = u8::MAX;
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
            Screen::Reset { seconds } => self.draw_reset(target, seconds, frame),
            Screen::Info { title, lines } => self.draw_info(target, title, lines),
        }
    }

    fn fill<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        area: Rectangle,
        color: Rgb565,
    ) -> Result<(), D::Error> {
        self.pixels_drawn += area.size.width * area.size.height;
        target.fill_solid(&area, color)
    }

    /// A text row in the rectangle `x..x+width`: clears it and draws the text.
    #[allow(clippy::too_many_arguments)]
    fn text_at<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        text: &str,
        x: i32,
        y: i32,
        width: u32,
        font: &MonoFont<'_>,
        color: Rgb565,
        background: Rgb565,
        alignment: Alignment,
    ) -> Result<(), D::Error> {
        self.fill(
            target,
            Rectangle::new(
                Point::new(x, y),
                Size::new(width, font.character_size.height),
            ),
            background,
        )?;
        let at_x = match alignment {
            Alignment::Left => x,
            Alignment::Center => x + width as i32 / 2,
            Alignment::Right => x + width as i32,
        };
        draw_text(target, text, Point::new(at_x, y), font, color, alignment)
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

        // 1) Squares – only the changed ones.
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

        // 2) Evaluation bar to the right of the board.
        let eval = self.eval_for_featured(view, featured);
        let split = eval_split(eval);
        if split != self.eval_split {
            self.fill(
                target,
                Rectangle::new(
                    Point::new(EVAL_X, 0),
                    Size::new(EVAL_W, (BOARD as i32 - split) as u32),
                ),
                colors::rgb(0x404040),
            )?;
            self.fill(
                target,
                Rectangle::new(
                    Point::new(EVAL_X, BOARD as i32 - split),
                    Size::new(EVAL_W, split as u32),
                ),
                colors::rgb(0xF0F0F0),
            )?;
            self.eval_split = split;
        }

        // 3) Engine variation (SAN is computed only on change).
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
        let live = view.engine.thinking
            && featured.is_some_and(|g| view.engine.game == Some(g.id))
            && key.is_some_and(|k| !k.skip_first);

        // 4) Text rows.
        let engine = &view.engine;
        let (depth, nps) = if engine.thinking {
            (engine.depth, engine.nps)
        } else {
            (engine.last_depth, engine.last_nps)
        };
        let mut rows: [String<32>; 7] = Default::default();
        let _ = write!(
            rows[0],
            "{}{:<2} {}.{}k",
            frame.lang.text(Key::Depth),
            depth,
            nps / 1000,
            (nps / 100) % 10
        );
        let sign = if eval < 0 { '-' } else { '+' };
        let _ = write!(
            rows[1],
            "{sign}{}.{:02}",
            eval.unsigned_abs() / 100,
            eval.unsigned_abs() % 100
        );
        let _ = rows[2].push_str(&self.best_text);
        if !self.pv_text.is_empty() {
            for (i, line) in wrap(&self.pv_text, TEXT_CHARS).iter().take(4).enumerate() {
                let _ = rows[3 + i].push_str(line);
            }
        } else if let Some(game) = featured {
            let _ = rows[3].push_str(&clip::<32>(&game.white, TEXT_CHARS));
            let _ = rows[4].push_str("-");
            let _ = rows[5].push_str(&clip::<32>(&game.black, TEXT_CHARS));
            if !game.active {
                let _ = rows[6].push_str(game.result);
            }
        } else {
            let _ = rows[3].push_str(&clip::<32>(frame.lang.text(Key::NoGames), TEXT_CHARS));
        }
        for (i, row) in rows.into_iter().enumerate() {
            if row == self.rows[i] {
                continue;
            }
            let (y, font) = ROWS[i];
            let color = match i {
                0 | 1 if engine.thinking => colors::ACCENT,
                0 | 1 => colors::MUTED,
                _ if live || self.pv_text.is_empty() => colors::TEXT,
                _ => colors::MUTED,
            };
            // The evaluation row ends before the "side to move" dot.
            let width = if i == 1 { TEXT_W - 14 } else { TEXT_W };
            self.text_at(
                target,
                &row,
                TEXT_X,
                y,
                width,
                font,
                color,
                colors::BACKGROUND,
                Alignment::Left,
            )?;
            self.rows[i] = row;
        }

        // 5) Side to move (white / black dot).
        let turn = position.side_to_move();
        if self.turn != Some(turn) && featured.is_some_and(|g| g.active) {
            let fill = match turn {
                Color::White => colors::rgb(0xF0F0F0),
                Color::Black => colors::rgb(0x202020),
            };
            let style = PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(colors::ACCENT)
                .stroke_width(1)
                .build();
            self.fill(
                target,
                Rectangle::new(TURN_DOT - Point::new(1, 1), Size::new(10, 10)),
                colors::BACKGROUND,
            )?;
            Circle::new(TURN_DOT, 8).into_styled(style).draw(target)?;
            self.pixels_drawn += 64;
            self.turn = Some(turn);
        }
        Ok(())
    }

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

    /// One 10×10 square in a single transfer.
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
        let x = sq.file() as i32 * SQUARE as i32;
        let y = (7 - sq.rank() as i32) * SQUARE as i32;
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
    // Other screens
    // ------------------------------------------------------------------

    /// Overview: counts and a list of running games (the display is too small for mini boards).
    fn draw_overview<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        frame: &Frame,
    ) -> Result<(), D::Error> {
        let view = frame.view;
        let mut lines: [String<48>; 12] = Default::default();
        let _ = write!(
            lines[0],
            "{}: {}  {}: {}",
            frame.lang.text(Key::Players),
            view.players_online,
            frame.lang.text(Key::Games),
            view.active_games
        );
        for (i, preview) in view.previews.iter().take(6).enumerate() {
            let _ = write!(
                lines[i + 1],
                "{} - {}  {}",
                clip::<20>(&preview.white, 10),
                clip::<20>(&preview.black, 10),
                preview.ply.div_ceil(2)
            );
        }
        if view.previews.is_empty() {
            let _ = lines[1].push_str(frame.lang.text(Key::NoGames));
        }
        self.draw_lines(target, &lines, 0, 11, &FONT_5X8)
    }

    /// Statistics: a title and up to 10 lines in a small font (4×6 → 40 chars per line).
    fn draw_info<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        lines: &[&str],
    ) -> Result<(), D::Error> {
        let mut all: [String<48>; 12] = Default::default();
        let _ = all[0].push_str(&clip::<48>(title, 32));
        for (i, line) in lines.iter().take(10).enumerate() {
            let _ = all[i + 1].push_str(&clip::<48>(line, 40));
        }
        // Title 5×8, lines 4×6 every 7 px (1 + 10 lines = 79 px).
        if all[0] != self.info[0] {
            self.text_at(
                target,
                &all[0],
                0,
                0,
                SMALL_WIDTH,
                &FONT_5X8,
                colors::ACCENT,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
            self.info[0] = all[0].clone();
        }
        for (i, line) in all.iter().enumerate().take(11).skip(1) {
            if *line == self.info[i] {
                continue;
            }
            self.text_at(
                target,
                line,
                0,
                9 + (i as i32 - 1) * 7,
                SMALL_WIDTH,
                &FONT_4X6,
                colors::TEXT,
                colors::BACKGROUND,
                Alignment::Left,
            )?;
            self.info[i] = line.clone();
        }
        Ok(())
    }

    /// Full-width text rows (only changed ones are redrawn).
    fn draw_lines<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        lines: &[String<48>; 12],
        first_y: i32,
        step: i32,
        font: &MonoFont<'_>,
    ) -> Result<(), D::Error> {
        for (i, line) in lines.iter().enumerate() {
            let y = first_y + i as i32 * step;
            if y + font.character_size.height as i32 > SMALL_HEIGHT as i32 {
                break;
            }
            if *line == self.info[i] {
                continue;
            }
            let color = if i == 0 { colors::ACCENT } else { colors::TEXT };
            self.text_at(
                target,
                line,
                0,
                y,
                SMALL_WIDTH,
                font,
                color,
                colors::BACKGROUND,
                Alignment::Left,
            )?;
            self.info[i] = line.clone();
        }
        Ok(())
    }

    fn draw_message<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        subtitle: &str,
    ) -> Result<(), D::Error> {
        let mut key: String<160> = String::new();
        let _ = write!(key, "{title}\u{1}{subtitle}");
        if key == self.text_key {
            return Ok(());
        }
        self.text_key = key;
        // Knight as a logo (25 px bitmap).
        let knight = chess_core::Piece::new(Color::White, chess_core::PieceKind::Knight);
        let pixels = (0..25u32).flat_map(|y| {
            (0..25u32).map(move |x| pieces::pixel(knight, 25, x, y).unwrap_or(colors::BACKGROUND))
        });
        self.pixels_drawn += 625;
        target.fill_contiguous(
            &Rectangle::new(Point::new(68, 6), Size::new(25, 25)),
            pixels,
        )?;
        self.text_at(
            target,
            &clip::<48>(title, 22),
            0,
            38,
            SMALL_WIDTH,
            &FONT_7X13,
            colors::TEXT,
            colors::BACKGROUND,
            Alignment::Center,
        )?;
        self.text_at(
            target,
            &clip::<64>(subtitle, 32),
            0,
            58,
            SMALL_WIDTH,
            &FONT_5X8,
            colors::MUTED,
            colors::BACKGROUND,
            Alignment::Center,
        )
    }

    /// QR code on the left (80×80), title and texts on the right.
    fn draw_qr<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        title: &str,
        content: &str,
        line1: &str,
        line2: &str,
    ) -> Result<(), D::Error> {
        let mut key: String<160> = String::new();
        let _ = write!(key, "{title}\u{1}{content}\u{1}{line1}\u{1}{line2}");
        if key == self.text_key {
            return Ok(());
        }
        self.text_key = key;
        let bg = colors::QR_LIGHT;
        self.fill(
            target,
            Rectangle::new(Point::zero(), Size::new(SMALL_WIDTH, SMALL_HEIGHT)),
            bg,
        )?;
        if let Some(qr) = Qr::encode(content) {
            // 1-module quiet zone (the display's white border helps the reader).
            let modules = qr.size() + 2;
            let scale = (SMALL_HEIGHT as i32 / modules).max(1);
            let side = (modules * scale) as u32;
            let origin = Point::new(0, (SMALL_HEIGHT as i32 - side as i32) / 2);
            let pixels = (0..side).flat_map(|py| {
                let qr = &qr;
                (0..side).map(move |px| {
                    if qr.module(px as i32 / scale - 1, py as i32 / scale - 1) {
                        colors::QR_DARK
                    } else {
                        colors::QR_LIGHT
                    }
                })
            });
            self.pixels_drawn += side * side;
            target.fill_contiguous(&Rectangle::new(origin, Size::new(side, side)), pixels)?;
        }
        // Texts on the right (x 82..160 = 78 px): title 5×8, address and network 4×6 with wrapping.
        let x = 82;
        let width = SMALL_WIDTH - x as u32;
        let mut y = 4;
        for line in wrap(title, 15).iter().take(2) {
            self.text_at(
                target,
                line,
                x,
                y,
                width,
                &FONT_5X8,
                colors::QR_DARK,
                bg,
                Alignment::Left,
            )?;
            y += 10;
        }
        y += 4;
        for text in [line1, line2] {
            for line in wrap(text, 19).iter().take(3) {
                self.text_at(
                    target,
                    line,
                    x,
                    y,
                    width,
                    &FONT_4X6,
                    colors::QR_DARK,
                    bg,
                    Alignment::Left,
                )?;
                y += 8;
            }
            y += 3;
        }
        Ok(())
    }

    fn draw_reset<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        target: &mut D,
        seconds: u8,
        frame: &Frame,
    ) -> Result<(), D::Error> {
        if seconds == self.reset_seconds {
            return Ok(());
        }
        let first = self.reset_seconds == u8::MAX;
        self.reset_seconds = seconds;
        let lang = frame.lang;
        if seconds == 0 {
            return self.text_at(
                target,
                lang.text(Key::Resetting),
                0,
                34,
                SMALL_WIDTH,
                &FONT_6X10,
                colors::WARNING,
                colors::BACKGROUND,
                Alignment::Center,
            );
        }
        if first {
            self.text_at(
                target,
                &clip::<48>(lang.text(Key::ResetIn), 22),
                0,
                2,
                SMALL_WIDTH,
                &FONT_7X13,
                colors::DANGER,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
            self.text_at(
                target,
                &clip::<48>(lang.text(Key::ReleaseToCancel), 32),
                0,
                70,
                SMALL_WIDTH,
                &FONT_5X8,
                colors::MUTED,
                colors::BACKGROUND,
                Alignment::Center,
            )?;
        }
        let mut number: String<8> = String::new();
        let _ = write!(number, "{seconds} s");
        self.text_at(
            target,
            &number,
            0,
            22,
            SMALL_WIDTH,
            &FONT_10X20,
            colors::TEXT,
            colors::BACKGROUND,
            Alignment::Center,
        )?;
        let done = (10u32.saturating_sub(seconds as u32)).min(10) * 14;
        self.fill(
            target,
            Rectangle::new(Point::new(10, 50), Size::new(140, 8)),
            colors::rgb(0x303030),
        )?;
        self.fill(
            target,
            Rectangle::new(Point::new(10, 50), Size::new(done, 8)),
            colors::DANGER,
        )
    }
}

/// How many pixels of the bar (height 80) are white. ±8 pawns = full bar.
fn eval_split(score_white: i32) -> i32 {
    let size = BOARD as i32;
    let half = size / 2;
    if score_white.abs() > chess_engine_mate_threshold() {
        return if score_white > 0 { size - 1 } else { 1 };
    }
    (half + score_white.clamp(-800, 800) * half / 800).clamp(1, size - 1)
}

/// Wraps text by words into lines of at most `width` chars (a long word is cut).
fn wrap(text: &str, width: usize) -> heapless::Vec<String<32>, 8> {
    let mut lines: heapless::Vec<String<32>, 8> = heapless::Vec::new();
    let mut current: String<32> = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let word_len = word.chars().count();
        let needed = current.chars().count() + usize::from(!current.is_empty()) + word_len;
        if !current.is_empty()
            && needed > width
            && lines.push(core::mem::take(&mut current)).is_err()
        {
            return lines;
        }
        if !current.is_empty() {
            let _ = current.push(' ');
        }
        for c in word.chars().take(width) {
            let _ = current.push(c);
        }
    }
    if !current.is_empty() {
        let _ = lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wraps_by_words() {
        let lines = wrap("23...Rxa4 24.Qb3 Ra8 25.Nc3 Bd7", 14);
        let lines: heapless::Vec<&str, 8> = lines.iter().map(|l| l.as_str()).collect();
        assert_eq!(lines.as_slice(), ["23...Rxa4", "24.Qb3 Ra8", "25.Nc3 Bd7"]);
        let long = wrap("http://192.168.4.1/", 10);
        assert_eq!(long[0].as_str(), "http://192");
    }
}
