//! Waveshare ESP32-C6-LCD-1.3 board description and display initialisation.
//!
//! Display wiring (taken from the official Waveshare demo code):
//!
//! | Display signal | GPIO | Meaning                                 |
//! |----------------|------|-----------------------------------------|
//! | SCLK           | 7    | SPI clock                               |
//! | MOSI           | 6    | data from the chip to the display       |
//! | CS             | 14   | chip select                             |
//! | DC             | 15   | 0 = sending a command, 1 = sending data |
//! | RST            | 21   | display controller reset                |
//! | BL             | 22   | backlight (1 = on)                      |
//!
//! The display controller is an ST7789V2, 240×240, RGB565 colours.
//! The panel has inverted colours, hence `ColorInversion::Inverted`.

use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    peripherals::{GPIO6, GPIO7, GPIO14, GPIO15, GPIO21, GPIO22, SPI2},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};
use mipidsi::{Builder, interface::SpiInterface, models::ST7789, options::ColorInversion};

/// Renderer for this display (240×240).
pub type Renderer = chess_display::Renderer;

/// SPI bus frequency. The ST7789 handles 40 MHz safely.
const SPI_FREQUENCY_MHZ: u32 = 40;

/// Peripherals (pins and SPI controller) the display needs.
///
/// The struct shows in one place everything the display uses.
/// It is created by the [`display_pins!`] macro.
pub struct DisplayPins<'d> {
    pub spi: SPI2<'d>,
    pub sck: GPIO7<'d>,
    pub mosi: GPIO6<'d>,
    pub cs: GPIO14<'d>,
    pub dc: GPIO15<'d>,
    pub rst: GPIO21<'d>,
    pub backlight: GPIO22<'d>,
}

/// Takes the display pins out of `peripherals`, e.g. `display_pins!(peripherals)`.
///
/// It is a macro (not a function) because only some fields must be moved out of
/// `peripherals` and the rest (e.g. `WIFI`) left for other code.
macro_rules! display_pins {
    ($p:ident) => {
        $crate::board::DisplayPins {
            spi: $p.SPI2,
            sck: $p.GPIO7,
            mosi: $p.GPIO6,
            cs: $p.GPIO14,
            dc: $p.GPIO15,
            rst: $p.GPIO21,
            backlight: $p.GPIO22,
        }
    };
}
pub(crate) use display_pins;

/// SPI device of the display = SPI bus + its own CS pin.
type DisplaySpi<'d> = ExclusiveDevice<Spi<'d, Blocking>, Output<'d>, Delay>;

/// Display type. Implements `DrawTarget<Color = Rgb565>`, so
/// `chess_display::Renderer` can draw to it.
pub type Display<'d> =
    mipidsi::Display<SpiInterface<'d, DisplaySpi<'d>, Output<'d>>, ST7789, Output<'d>>;

/// Display including the backlight.
pub struct Screen<'d> {
    /// The display to draw to.
    pub display: Display<'d>,
    /// The backlight pin must be kept, otherwise it would be released when dropped
    /// and the backlight could turn off.
    _backlight: Output<'d>,
}

/// Initialises the SPI bus and the display controller and turns on the backlight.
///
/// `buffer` is work memory for sending pixels over SPI (a few hundred bytes are enough;
/// a larger buffer = somewhat faster drawing).
///
/// An initialisation error panics – there is no point continuing without the display,
/// and the error shows up on the serial console.
pub fn init_display<'d>(pins: DisplayPins<'d>, buffer: &'d mut [u8]) -> Screen<'d> {
    // Keep the backlight off until the display is set up (no flashing garbage).
    let mut backlight = Output::new(pins.backlight, Level::Low, OutputConfig::default());

    // SPI mode 0 (CPOL=0, CPHA=0) – same as the Waveshare demo code.
    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(SPI_FREQUENCY_MHZ))
        .with_mode(Mode::_0);
    let spi = Spi::new(pins.spi, spi_config)
        .expect("invalid SPI configuration")
        .with_sck(pins.sck)
        .with_mosi(pins.mosi);

    // Display control pins.
    let cs = Output::new(pins.cs, Level::High, OutputConfig::default());
    let dc = Output::new(pins.dc, Level::Low, OutputConfig::default());
    let rst = Output::new(pins.rst, Level::High, OutputConfig::default());

    // ExclusiveDevice = an SPI bus with a single device (the display).
    // It drives CS before and after every transfer itself.
    let spi_device = ExclusiveDevice::new(spi, cs, Delay::new()).expect("cannot set up the CS pin");
    let interface = SpiInterface::new(spi_device, dc, buffer);

    let mut delay = Delay::new();
    let display = Builder::new(ST7789, interface)
        .display_size(chess_display::WIDTH as u16, chess_display::HEIGHT as u16)
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(rst)
        .init(&mut delay)
        .expect("display initialisation failed");

    backlight.set_high();

    Screen {
        display,
        _backlight: backlight,
    }
}
