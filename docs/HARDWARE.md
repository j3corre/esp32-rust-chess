# Hardware notes

The firmware runs on three ESP32 chips. Any board with one of them and **at least 4 MB of
flash** should work; these are the boards it was developed and measured on.

| | ESP32-C6 | ESP32-C3 | ESP32-S3 | LilyGO T-Dongle-S3 |
|---|---|---|---|---|
| Tested board | [Waveshare ESP32-C6-LCD-1.3](https://docs.waveshare.com/ESP32-C6-LCD-1.3) (4 MB flash) | ESP32-C3 SuperMini (4 MB flash) | board with ESP32-S3FH4R2 (4 MB flash + 2 MB PSRAM in the package) | [T-Dongle-S3](https://lilygo.cc/products/t-dongle-s3) (16 MB flash, no PSRAM) |
| CPU | RISC-V, 160 MHz | RISC-V, 160 MHz, no atomic instructions | 2× Xtensa LX7, 240 MHz, SIMD | as ESP32-S3 |
| Display | 1.3″ ST7789, 240×240 | – (web only) | – (web only) | 0.96″ ST7735 IPS, 160×80 |
| Build | `scripts/unix/flash.sh` | `CHIP=esp32c3` | `CHIP=esp32s3` | `BOARD=tdongle` |
| Engine speed (lab) | ~19,400 nodes/s | ~14,800 nodes/s | ~38,000 nodes/s (~35,800 with the TT in PSRAM) | as S3 without PSRAM |
| Depth in a demo game (10 s) | ~10 plies | ~9 plies | ~10–11 plies | ~10–11 plies |
| Transposition table | 40 kB | 8 kB | 1.75 MB in PSRAM, 16 kB without | 16 kB |
| Strength | ~2590 Elo | ~2460 Elo | ~2740 Elo with PSRAM, ~2650 without | ≈ S3 without PSRAM (not measured separately) |

The lab speed is measured with the same fixed search on every chip and Wi-Fi switched off; in
normal use with Wi-Fi and the display the engine is a few per cent slower.

## Notes per board

**ESP32-C6 (Waveshare ESP32-C6-LCD-1.3)** – the reference board and the default build. The
engine runs as a low-priority thread next to the Wi-Fi and web server; the hot parts of the
search and the NNUE live in fast internal RAM. The display shows the game, the engine's thinking,
QR codes and statistics.

**ESP32-C3 (SuperMini)** – the smallest and cheapest option. It has 112 kB less RAM than the C6,
so the hash table is only 8 kB and fewer web tasks run (2 + 2), which costs some strength: the C6
with a five times larger hash table is about 130 Elo stronger although it is only about 1.3×
faster. The chip has no atomic read-modify-write instructions (`portable-atomic` is used
instead). The small antennas of SuperMini boards are often weak – keep the board in the open,
away from metal.

**ESP32-S3** – the strongest: the engine runs alone on the second core and evaluates the network
with the S3's 128-bit SIMD instructions (PIE). PSRAM is detected from eFuse (`PSRAM_CAP`, the same
field `espefuse summary` shows – `espflash board-info` does not report PSRAM on the S3), so one
firmware serves S3 chips with and without PSRAM (e.g. …R2 / …R8 variants). With PSRAM the hash
table moves there and grows from 16 kB to about 1.75 MB: searching is about 5 % slower, but in a
200-game match against Stockfish at 2650 the engine scored about 100 Elo more than the same board
with the small table. The S3 needs the `esp` toolchain (espup); it is built with
`-C llvm-args=-disable-lsr` to work around an Xtensa LLVM back-end error (and it is faster that
way).

**LilyGO T-Dongle-S3** – a USB stick with an ESP32-S3 without PSRAM (eFuse `PSRAM_CAP = 0`) and an
ST7735 display, built with `BOARD=tdongle` (firmware feature `tdongle`). The 160×80 screen uses
the compact `SmallRenderer`: board, evaluation bar, depth, speed, best move and main line side by
side; QR codes, device info and the games overview have small-screen versions too. Its IPS panel
renders dark tones far too bright and the controller's gamma tables do not fix it, so the
firmware passes every colour through a tone curve measured on the panel before sending it. The
only button is BOOT, which works as on the other boards. There is no RST button – to enter the
bootloader, hold BOOT while plugging it in.

## Good to know

- **Flash mode:** the firmware is flashed in DIO (espflash's default). Some boards – including
  the C6-LCD and the S3FH4R2 – do not boot in QIO mode (the ROM loader keeps resetting via the
  watchdog). Fix: flash again in DIO (enter the bootloader with BOOT + RST first). The Unix
  scripts refuse `FLASH_MODE=qio` unless `ALLOW_QIO=1` is set.
- **Partition table:** the firmware needs `firmware/partitions.csv` (a ~3.9 MB app partition and
  the `chesscfg` partition at 0x3F0000 for the saved Wi-Fi). The scripts pass it automatically.
- **Wi-Fi TX power:** set to 15 dBm (the Wi-Fi driver starts at 5 dBm); the actual value read back
  from the chip is printed to the serial console every 30 s. The board's antenna and position
  matter more than the setting – a board with a weak signal may fail to join the home network
  while its own `ESP-Chess-XXXX` network still works.
- **Temperature:** the built-in sensor (uncalibrated, ± a few °C, measured inside the chip) is
  shown in the web app (Settings → device info), on the display's statistics page and in the
  serial log. The ESP32-S3 measured about 50–60 °C under full load and about 37 °C after a
  restart – well within its rating; boards get warm to the touch, which is normal. The demo game
  pauses 20 s between moves when nobody plays to reduce the load.
- **Main stack:** the main-thread stack must be at least ~16 kB, otherwise the board crashes when
  Wi-Fi starts ("Detected a write to the main stack's guard"). `tools/elf-size` checks this after
  every build by the scripts.
- **LEDs:** the firmware does not drive any LED. A blinking LED on some boards most likely comes
  from the board itself, typically a battery-charging circuit without a battery.
- **Power:** any USB charger or computer port works; the board needs well under 1 W.
- **USB:** all supported chips have a built-in USB serial/JTAG port – no driver is needed. Use a
  cable that carries data.

## Known issues

- Some ESP32-S3 boards stay silent in the bootloader after flashing or `espflash board-info` –
  press RST or run `espflash reset`.
- On the ESP32-S3 the engine once stopped responding completely in a lost position (about once
  in 1,500 games during the strength measurements); only a reset helped. The cause has not been
  found yet.
- In UCI mode, a reset can leave a fragment of old output in the USB buffer; `esp-uci.py`
  filters non-UCI lines and waits for `readyok`.
