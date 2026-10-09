//! Piece bitmaps (generated from SVG by `tools/gen-assets`).

use chess_core::Piece;
use embedded_graphics::pixelcolor::Rgb565;

use crate::colors;
use crate::pieces_data::{
    PIECE_10_BYTES, PIECE_12_BYTES, PIECE_25_BYTES, PIECES_10, PIECES_12, PIECES_25,
};

/// Bitmap index: white P N B R Q K = 0..6, black = 6..12 (same as `Piece::index`).
fn bitmap_index(piece: Piece) -> usize {
    piece.index()
}

/// Pixel level (0 = transparent, 1 = outline, 2 = fill, 3 = edge) from a 2-bit bitmap.
#[inline]
fn level(data: &[u8], size: usize, x: usize, y: usize) -> u8 {
    let i = y * size + x;
    (data[i / 4] >> (6 - 2 * (i % 4))) & 3
}

/// Color of the piece pixel at (x, y), or `None` = transparent (square shows through).
#[inline]
pub fn pixel(piece: Piece, size: u32, x: u32, y: u32) -> Option<Rgb565> {
    let (x, y) = (x as usize, y as usize);
    let value = match size {
        25 => {
            let data: &[u8; PIECE_25_BYTES] = &PIECES_25[bitmap_index(piece)];
            level(data, 25, x, y)
        }
        10 => {
            let data: &[u8; PIECE_10_BYTES] = &PIECES_10[bitmap_index(piece)];
            level(data, 10, x.min(9), y.min(9))
        }
        _ => {
            let data: &[u8; PIECE_12_BYTES] = &PIECES_12[bitmap_index(piece)];
            level(data, 12, x.min(11), y.min(11))
        }
    };
    match value {
        1 => Some(colors::PIECE_OUTLINE),
        2 => Some(colors::PIECE_FILL),
        3 => Some(colors::PIECE_EDGE),
        _ => None,
    }
}
