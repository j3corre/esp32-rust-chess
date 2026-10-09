//! Colors (RGB565). The board uses the same colors as the web app (lichess style).

use embedded_graphics::pixelcolor::Rgb565;

/// Converts a 24-bit color (0xRRGGBB) – more readable than raw RGB565
/// (5 bits red, 6 bits green, 5 bits blue).
pub const fn rgb(hex: u32) -> Rgb565 {
    let r = ((hex >> 16) & 0xFF) as u8;
    let g = ((hex >> 8) & 0xFF) as u8;
    let b = (hex & 0xFF) as u8;
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

pub const BACKGROUND: Rgb565 = rgb(0x161512);
pub const TEXT: Rgb565 = rgb(0xE8E6E3);
pub const MUTED: Rgb565 = rgb(0x8D8B88);
pub const ACCENT: Rgb565 = rgb(0x7AB834);
pub const WARNING: Rgb565 = rgb(0xE0A030);
pub const DANGER: Rgb565 = rgb(0xCC3333);

pub const LIGHT: Rgb565 = rgb(0xF0D9B5);
pub const DARK: Rgb565 = rgb(0xB58863);
/// Last move – lichess yellow-green blended with the square color (41 %).
pub const LIGHT_LAST: Rgb565 = rgb(0xCDD26A);
pub const DARK_LAST: Rgb565 = rgb(0xAAA23A);
/// King in check.
pub const CHECK: Rgb565 = rgb(0xE84A3A);

pub const PIECE_OUTLINE: Rgb565 = rgb(0x000000);
pub const PIECE_FILL: Rgb565 = rgb(0xFFFFFF);
pub const PIECE_EDGE: Rgb565 = rgb(0x555555);

pub const QR_DARK: Rgb565 = rgb(0x000000);
pub const QR_LIGHT: Rgb565 = rgb(0xFFFFFF);
