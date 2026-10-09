//! # chess-display
//!
//! Screens for the 240×240 display (ST7789) of the Waveshare ESP32-C6-LCD-1.3 ([`Renderer`])
//! and the small 160×80 display (ST7735) of the LilyGO T-Dongle-S3 ([`SmallRenderer`]).
//! Drawing goes through `embedded-graphics` into any `DrawTarget<Color = Rgb565>`,
//! so everything can be tested on a PC (drawing into memory).
//!
//! | Screen    | Content |
//! |-----------|---------|
//! | game      | 200×200 board, evaluation bar, names, depth + kN/s, player count |
//! | overview  | number of players and games, up to 4 mini boards |
//! | QR        | QR code with the address (URL or Wi-Fi login) |
//! | reset     | countdown while the BOOT button is held |
//! | message   | startup, connecting to Wi-Fi… |
//!
//! **Only what changed** is redrawn (a move = 2–4 squares), so the display
//! holds up the network and the engine as little as possible.

#![no_std]

mod colors;
mod pieces;
mod pieces_data;
mod qr;
mod render;
mod small;
mod text;

pub use render::{Frame, INFO_LINE_LEN, INFO_LINES, Renderer, Screen};
pub use small::{SMALL_HEIGHT, SMALL_WIDTH, SmallRenderer};
pub use text::{Key, Lang};

/// Display dimensions.
pub const WIDTH: u32 = 240;
pub const HEIGHT: u32 = 240;
