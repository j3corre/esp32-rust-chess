//! LilyGO T-Dongle-S3 board description and display initialisation (feature `tdongle`).
//!
//! Same interface as `board.rs` (Waveshare ESP32-C6-LCD-1.3) – `main.rs` and `ui.rs` pick the
//! module by feature. Wiring from the official LilyGO example (examples/lcd):
//!
//! | Display signal | GPIO | Meaning                                 |
//! |----------------|------|-----------------------------------------|
//! | SCLK           | 5    | SPI clock                               |
//! | MOSI           | 3    | data from the chip to the display       |
//! | CS             | 4    | chip select                             |
//! | DC             | 2    | 0 = sending a command, 1 = sending data |
//! | RST            | 1    | display controller reset                |
//! | BL             | 38   | backlight (0 = on)                      |
//!
//! ST7735 controller, 0.96″ IPS 80×160 panel (132×162 in controller memory with a 26/1
//! offset), BGR colours, inverted. Drawn in landscape (160×80, `chess_display::SmallRenderer`).
//! The board has no PSRAM (eFuse PSRAM_CAP = 0) – the engine uses a small TT, see `psram.rs`.

use embedded_graphics::{
    Pixel,
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, OriginDimensions, Size},
    primitives::Rectangle,
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    peripherals::{GPIO1, GPIO2, GPIO3, GPIO4, GPIO5, GPIO38, SPI2},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};
use mipidsi::{
    Builder,
    interface::SpiInterface,
    models::ST7735s,
    options::{ColorInversion, ColorOrder, Orientation, Rotation},
};

/// SPI frequency. The ST7735 is slower than the ST7789; LilyGO uses 27 MHz as well.
const SPI_FREQUENCY_MHZ: u32 = 27;

/// Renderer for this display (160×80).
pub type Renderer = chess_display::SmallRenderer;

/// Peripherals (pins and SPI controller) the display needs.
pub struct DisplayPins<'d> {
    pub spi: SPI2<'d>,
    pub sck: GPIO5<'d>,
    pub mosi: GPIO3<'d>,
    pub cs: GPIO4<'d>,
    pub dc: GPIO2<'d>,
    pub rst: GPIO1<'d>,
    pub backlight: GPIO38<'d>,
}

/// Takes the display pins out of `peripherals`, e.g. `display_pins!(peripherals)`.
macro_rules! display_pins {
    ($p:ident) => {
        $crate::board::DisplayPins {
            spi: $p.SPI2,
            sck: $p.GPIO5,
            mosi: $p.GPIO3,
            cs: $p.GPIO4,
            dc: $p.GPIO2,
            rst: $p.GPIO1,
            backlight: $p.GPIO38,
        }
    };
}
pub(crate) use display_pins;

type DisplaySpi<'d> = ExclusiveDevice<Spi<'d, Blocking>, Output<'d>, Delay>;

type Panel<'d> =
    mipidsi::Display<SpiInterface<'d, DisplaySpi<'d>, Output<'d>>, ST7735s, Output<'d>>;

/// Display type (`DrawTarget<Color = Rgb565>`).
pub type Display<'d> = Corrected<Panel<'d>>;

/// Panel tone correction: the panel brightens dark tones a lot (40 % grey is almost white,
/// the #161512 background is mid grey) and the controller's gamma tables cannot fix it (the
/// LilyGO driver sends the same ones as mipidsi, the Adafruit tables are worse). Colours are
/// therefore darkened before sending, following the response measured from photos
/// (brightness ≈ 1 − e^(−v/0.2); the tables are its inverse for 5 and 6 bits).
pub struct Corrected<D>(D);

const TONE_5: [u8; 32] = [
    0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 11, 12, 14, 16,
    20, 31,
];
const TONE_6: [u8; 64] = [
    0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 6, 6, 6, 7, 7, 7, 8, 8, 8,
    9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 16, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    27, 29, 31, 34, 37, 41, 48, 63,
];

fn tone(c: Rgb565) -> Rgb565 {
    Rgb565::new(
        TONE_5[usize::from(c.r())],
        TONE_6[usize::from(c.g())],
        TONE_5[usize::from(c.b())],
    )
}

impl<D: OriginDimensions> OriginDimensions for Corrected<D> {
    fn size(&self) -> Size {
        self.0.size()
    }
}

impl<D: DrawTarget<Color = Rgb565> + OriginDimensions> DrawTarget for Corrected<D> {
    type Color = Rgb565;
    type Error = D::Error;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), D::Error> {
        self.0
            .draw_iter(pixels.into_iter().map(|Pixel(p, c)| Pixel(p, tone(c))))
    }

    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(
        &mut self,
        area: &Rectangle,
        colors: I,
    ) -> Result<(), D::Error> {
        self.0.fill_contiguous(area, colors.into_iter().map(tone))
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), D::Error> {
        self.0.fill_solid(area, tone(color))
    }

    fn clear(&mut self, color: Rgb565) -> Result<(), D::Error> {
        self.0.clear(tone(color))
    }
}

/// Display including the backlight.
pub struct Screen<'d> {
    pub display: Display<'d>,
    _backlight: Output<'d>,
}

/// Initialises SPI and the display controller and turns on the backlight (panics on error – see
/// `board.rs`).
pub fn init_display<'d>(pins: DisplayPins<'d>, buffer: &'d mut [u8]) -> Screen<'d> {
    // Backlight off (logic 1) until the display is set up.
    let mut backlight = Output::new(pins.backlight, Level::High, OutputConfig::default());

    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(SPI_FREQUENCY_MHZ))
        .with_mode(Mode::_0);
    let spi = Spi::new(pins.spi, spi_config)
        .expect("invalid SPI configuration")
        .with_sck(pins.sck)
        .with_mosi(pins.mosi);

    let cs = Output::new(pins.cs, Level::High, OutputConfig::default());
    let dc = Output::new(pins.dc, Level::Low, OutputConfig::default());
    let rst = Output::new(pins.rst, Level::High, OutputConfig::default());

    let spi_device = ExclusiveDevice::new(spi, cs, Delay::new()).expect("cannot set up the CS pin");
    let interface = SpiInterface::new(spi_device, dc, buffer);

    // Landscape: swap the axes + mirror the rows (= Deg270 in mipidsi), like the LilyGO example
    // (swap_xy + mirror_y, offset 1/26). The offset is given in the panel's native orientation
    // (80×160).
    let mut delay = Delay::new();
    let display = Builder::new(ST7735s, interface)
        .display_size(80, 160)
        .display_offset(26, 1)
        .orientation(Orientation::new().rotate(Rotation::Deg270))
        .color_order(ColorOrder::Bgr)
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(rst)
        .init(&mut delay)
        .expect("display initialisation failed");

    backlight.set_low();

    Screen {
        display: Corrected(display),
        _backlight: backlight,
    }
}
