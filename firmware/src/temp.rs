//! Chip temperature from the built-in sensor (shown in the web settings and on the display stats
//! page).
//!
//! - ESP32-C3/C6: the `esp_hal::tsens` driver.
//! - ESP32-S3: esp-hal 1.2 has no driver – the `SENS` registers directly, like
//!   `temperature_sensor_ll` in ESP-IDF (clock, software-forced power-up, "dump out").
//!
//! Conversion per ESP-IDF: °C = 0.4386 · raw − 27.88 · offset − 20.52, where the offset
//! depends on the range (DAC). C3/C6: default range, offset −1 (like esp-hal). S3: the range
//! is set to −10…80 °C (offset 0). No eFuse calibration (accuracy ± a few °C). The sensor
//! measures inside the chip – it reads higher than the air around the board.

#[cfg(not(feature = "esp32s3"))]
use esp_hal::tsens::{Config, TemperatureSensor};

/// ESP32-S3: sensor DAC value for the −10…80 °C range (offset 0 in the ESP-IDF formula).
#[cfg(feature = "esp32s3")]
const S3_TSENS_DAC: u8 = 15;

/// Plausible range – outside it the value is most likely wrong (e.g. a bad conversion on the S3).
const VALID_C: core::ops::RangeInclusive<f32> = -20.0..=125.0;

pub struct ChipTemp {
    #[cfg(not(feature = "esp32s3"))]
    sensor: TemperatureSensor<'static>,
    #[cfg(feature = "esp32s3")]
    _sens: esp_hal::peripherals::SENS<'static>,
}

impl ChipTemp {
    #[cfg(not(feature = "esp32s3"))]
    pub fn new(tsens: esp_hal::peripherals::TSENS<'static>) -> Option<ChipTemp> {
        let sensor = TemperatureSensor::new(tsens, Config::default()).ok()?;
        Some(ChipTemp { sensor })
    }

    #[cfg(feature = "esp32s3")]
    pub fn new(sens: esp_hal::peripherals::SENS<'static>) -> Option<ChipTemp> {
        // Sensor range (DAC in the SAR ADC analog block via regi2c) – like the ESP-IDF default
        // range −10…80 °C (value 15, offset 0). With the reset default and offset −1 (as on
        // the C3/C6) the S3 read ~30 °C too high (81 °C instead of 51 °C under load).
        // esp-hal has regi2c only internally → use the chip's ROM functions.
        unsafe extern "C" {
            fn rom_i2c_readReg_Mask(block: u8, host_id: u8, reg_add: u8, msb: u8, lsb: u8) -> u8;
            fn rom_i2c_writeReg_Mask(
                block: u8,
                host_id: u8,
                reg_add: u8,
                msb: u8,
                lsb: u8,
                data: u8,
            );
        }
        // SAFETY: ESP32-S3 ROM functions (esp32s3.rom.ld); SAR ADC block 0x69, host 1,
        // register 6 bits 3..0 = I2C_SARADC_TSENS_DAC (esp-hal soc/esp32s3/regi2c.rs).
        let before = unsafe { rom_i2c_readReg_Mask(0x69, 1, 6, 3, 0) };
        unsafe { rom_i2c_writeReg_Mask(0x69, 1, 6, 3, 0, S3_TSENS_DAC) };
        log::info!("Temperature sensor: DAC {before} → {S3_TSENS_DAC} (range −10…80 °C)");

        let regs = esp_hal::peripherals::SENS::regs();
        regs.sar_peri_clk_gate_conf()
            .modify(|_, w| w.tsens_clk_en().set_bit());
        // SAFETY: xpd_force is a two-bit field, 1 = forced on by software (like ESP-IDF).
        regs.sar_tsens_ctrl2()
            .modify(|_, w| unsafe { w.sar_tsens_xpd_force().bits(1) });
        regs.sar_tsens_ctrl().modify(|_, w| {
            w.sar_tsens_power_up_force()
                .set_bit()
                .sar_tsens_power_up()
                .set_bit()
        });
        Some(ChipTemp { _sens: sens })
    }

    /// Temperature in tenths of °C, `None` if the sensor did not respond or the value is
    /// implausible.
    pub fn read_dc(&mut self) -> Option<i16> {
        let celsius = self.read_celsius()?;
        VALID_C
            .contains(&celsius)
            .then(|| (celsius * 10.0 + if celsius < 0.0 { -0.5 } else { 0.5 }) as i16)
    }

    #[cfg(not(feature = "esp32s3"))]
    fn read_celsius(&mut self) -> Option<f32> {
        Some(self.sensor.get_temperature().to_celsius())
    }

    #[cfg(feature = "esp32s3")]
    fn read_celsius(&mut self) -> Option<f32> {
        let ctrl = esp_hal::peripherals::SENS::regs().sar_tsens_ctrl();
        ctrl.modify(|_, w| w.sar_tsens_dump_out().set_bit());
        // A conversion takes tens of µs; the wait is bounded so the main loop cannot get stuck.
        let ready = (0..20_000).any(|_| ctrl.read().sar_tsens_ready().bit_is_set());
        let raw = ctrl.read().sar_tsens_out().bits();
        ctrl.modify(|_, w| w.sar_tsens_dump_out().clear_bit());
        // Offset 0 (DAC = S3_TSENS_DAC): °C = 0.4386 · raw − 20.52.
        ready.then(|| f32::from(raw) * 0.4386 - 20.52)
    }
}
