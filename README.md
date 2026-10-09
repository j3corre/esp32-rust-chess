# ESP32 Rust Chess

A self-contained chess computer on a few-dollar ESP32 board. The board runs a chess engine
written in Rust with an NNUE neural-network evaluation, a small game server, its own Wi-Fi
network and a lichess-style web app. You play from the browser of any phone, tablet or
computer – nothing to install, no internet connection needed.

The engine started as a Rust port of [tinychess](https://github.com/Timmy6942025/tinychess),
a fork of the [Dog](https://github.com/folkertvanheusden/Dog) chess engine by Folkert van
Heusden, and was then heavily optimised for microcontrollers with 400–512 kB of RAM.

**Website: [esp32.chessbenefits.com](https://esp32.chessbenefits.com/)** – install the firmware
on your board straight from the browser (Chrome/Edge, no toolchain needed), or
[play against the engine in your browser](https://esp32.chessbenefits.com/play/) without any board.

## Features

- **A strong engine on a tiny chip** – alpha-beta search (PVS, transposition table, null move,
  LMR, futility pruning, quiescence search with SEE, …) and a quantised NNUE network
  (768 → 128×2 → 1, 8-bit weights, 8 output buckets, ~100 kB). About **2460–2740 Elo**
  depending on the chip (see [Playing strength](#playing-strength)).
- **Its own Wi-Fi with a captive portal** – the board creates the open network
  `ESP-Chess-XXXX`; join it and the chess app opens by itself (or go to
  `http://192.168.4.1`). The board can also join your home network and keeps its own network
  as a fallback.
- **A lichess-style web app** served by the board – nickname, play the engine at **8 levels**,
  challenge other players on the same board (bullet / blitz / rapid / classical or no clock),
  watch running games, hints (in engine games), take-backs, draw offers, rematches, PGN
  export, light and dark themes, board colours, sound, **English and Czech**.
- **Several players at once** – up to 24 connected players and 8 games (including finished
  ones and the demo game) per board.
- **A demo game** – when nobody plays, the engine plays against itself so there is always
  something to watch; human players always take priority.
- **Display** (Waveshare ESP32-C6-LCD-1.3, LilyGO T-Dongle-S3) – the board, an evaluation bar,
  search depth, speed, the best move and the engine's main line while it thinks, an overview of
  running games, QR codes for joining the Wi-Fi and opening the app, and a statistics page.
- **Device info** in the web app – chip, firmware, playing strength, network, engine speed,
  chip temperature and uptime.
- **A PC simulator** – the same game server, engine and web app run on a PC for development and
  tests, and the engine can be used as a UCI engine.

## Supported boards

| Board | Chip | Display | Engine speed* | Strength |
|---|---|---|---|---|
| [Waveshare ESP32-C6-LCD-1.3](https://docs.waveshare.com/ESP32-C6-LCD-1.3) | ESP32-C6, RISC-V 160 MHz | 1.3″ ST7789, 240×240 | ~19,400 nodes/s | ~2590 Elo |
| ESP32-C3 SuperMini | ESP32-C3, RISC-V 160 MHz | – (web only) | ~14,800 nodes/s | ~2460 Elo |
| ESP32-S3 with PSRAM (tested: ESP32-S3FH4R2, 2 MB PSRAM) | ESP32-S3, 2× Xtensa LX7 240 MHz, SIMD | – (web only) | ~35,800 nodes/s | ~2740 Elo |
| ESP32-S3 without PSRAM | ESP32-S3, 2× Xtensa LX7 240 MHz, SIMD | – (web only) | ~38,000 nodes/s | ~2650 Elo |
| [LilyGO T-Dongle-S3](https://lilygo.cc/products/t-dongle-s3) | ESP32-S3 (no PSRAM) | 0.96″ ST7735, 160×80 | as ESP32-S3 without PSRAM | ≈ ESP32-S3 without PSRAM (not measured separately) |

\* Lab measurement: the same fixed search on every chip, with Wi-Fi switched off. In normal
use with Wi-Fi and the display the engine is a few per cent slower.

Any board with one of these chips and **at least 4 MB of flash** should work. PSRAM on the
ESP32-S3 is detected automatically; the hash table then grows from 16 kB to about 1.75 MB.
Per-board notes (flash mode, PSRAM, Wi-Fi power, temperatures, antennas, pinouts) are in
[docs/HARDWARE.md](docs/HARDWARE.md).

## Playing strength

| Board | Elo (95 % interval) |
|---|---|
| ESP32-S3 with PSRAM | **~2740 ± 50** |
| ESP32-S3 without PSRAM | **~2650 ± 34** |
| ESP32-C6 | **~2590 ± 33** |
| ESP32-C3 | **~2460 ± 33** |

Each board played over USB as a UCI engine (firmware without Wi-Fi) against Stockfish 19
limited with `UCI_LimitStrength` / `UCI_Elo`, at 60 s + 0.6 s per move with an opening book and
colour-reversed game pairs, plus a round robin between the boards – about 1,500 games in total,
rated with one maximum-likelihood fit using the Stockfish levels as anchors. The numbers are
therefore Elo on Stockfish's `UCI_Elo` scale (calibrated against CCRL), not an official
rating. The web app shows the strength rounded down (2700+ / 2600+ / 2550+ / 2400+) because in
normal use the board also serves the web app and drives the display.

## Quick start

No toolchain needed just to use it: the [website](https://esp32.chessbenefits.com/docs/install/)
flashes ready-made firmware for the ESP32-C3, C6, S3 and the T-Dongle-S3 from the browser.
The steps below build it from source.

You need a board from the table above and a USB cable **that carries data** (many cheap cables
only charge).

### Toolchains

- **Rust stable** with the RISC-V targets `riscv32imac-unknown-none-elf` (ESP32-C6) and
  `riscv32imc-unknown-none-elf` (ESP32-C3) and the `rust-src` component
  (see [`firmware/rust-toolchain.toml`](firmware/rust-toolchain.toml)).
- **ESP32-S3 only:** Espressif's `esp` toolchain for Xtensa, installed by
  [espup](https://github.com/esp-rs/espup) (about 1–2 GB).
- [**espflash**](https://github.com/esp-rs/espflash) for flashing and the serial console.

The setup scripts install all of this.

### Linux and macOS

The scripts live in `scripts/unix/` (`scripts/mac` is a symlink to the same directory). Run
them from the repository root:

```bash
scripts/unix/setup.sh                 # one-time: Rust targets, espflash
WITH_S3=1 scripts/unix/setup.sh       # ... plus the "esp" toolchain for the ESP32-S3

scripts/unix/flash.sh                 # build + flash + serial console: ESP32-C6 with display (default)
CHIP=esp32c3 scripts/unix/flash.sh    # ESP32-C3
CHIP=esp32s3 scripts/unix/flash.sh    # ESP32-S3 (with or without PSRAM)
BOARD=tdongle scripts/unix/flash.sh   # LilyGO T-Dongle-S3 (ESP32-S3 + 160×80 display)

scripts/unix/build.sh                 # build only (same CHIP / BOARD variables)
scripts/unix/monitor.sh               # serial console only
```

Useful variables: `PORT=/dev/ttyACM1` selects the port when several boards are connected
(otherwise the first USB serial port is used), `NO_MONITOR=1` skips the serial console and
`SKIP_BUILD=1` flashes the last build. On Linux `setup.sh` adds you to the `uucp` (Arch) or
`dialout` (Debian/Ubuntu) group – log out and back in afterwards.

### Windows

The PowerShell scripts in `scripts/` build and flash the default board (ESP32-C6 with display):

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned   # once, to allow local scripts
.\scripts\setup.ps1       # one-time: Rust, RISC-V target, espflash, Playwright for the web tests
.\scripts\build.ps1       # build the firmware
.\scripts\flash.ps1       # build + flash + serial console (-Port COM5, -NoMonitor, -SkipBuild)
.\scripts\monitor.ps1     # serial console only
```

Building also needs the Visual Studio Build Tools ("Desktop development with C++") for the
host-side build scripts. For the other boards, build and flash by hand in `firmware/`:

```powershell
cd firmware
# ESP32-C3
rustup target add riscv32imc-unknown-none-elf
cargo build --release --no-default-features --features esp32c3 --target riscv32imc-unknown-none-elf
espflash flash --chip esp32c3 --partition-table partitions.csv --port COM6 target\riscv32imc-unknown-none-elf\release\esp32-chess

# ESP32-S3 (once: cargo install espup --locked; espup install --targets esp32s3)
# For the LilyGO T-Dongle-S3 use --features tdongle instead of esp32s3.
cargo +esp build --release -Zbuild-std=core,alloc --no-default-features --features esp32s3 --target xtensa-esp32s3-none-elf
espflash flash --chip esp32s3 --partition-table partitions.csv --port COM7 target\xtensa-esp32s3-none-elf\release\esp32-chess
```

`espflash list-ports` lists the ports. A short checkout path (for example under `C:\esp`)
helps the S3 toolchain on Windows.

### Manual build (any OS)

| Board | Command in `firmware/` |
|---|---|
| ESP32-C6 + display | `cargo build --release` |
| ESP32-C3 | `cargo build --release --no-default-features --features esp32c3 --target riscv32imc-unknown-none-elf` |
| ESP32-S3 | `cargo +esp build --release -Zbuild-std=core,alloc --no-default-features --features esp32s3 --target xtensa-esp32s3-none-elf` |
| T-Dongle-S3 | as ESP32-S3, with `--features tdongle` |

The firmware uses its own partition table, so always flash with
`--partition-table partitions.csv`. The scripts also run `tools/elf-size` after every build to
check that enough RAM is left for the main stack (Wi-Fi start-up needs at least 16 kB).

### Troubleshooting

- **Do not flash in QIO mode.** Several boards (including the C6-LCD and the S3FH4R2) do not
  boot in QIO; the firmware is flashed in DIO (the espflash default). The Unix scripts refuse
  `FLASH_MODE=qio`.
- No port, or flashing fails: try another (data) USB cable or port. Then put the board into
  the bootloader by hand – **hold BOOT, press and release RST, release BOOT** – and flash
  again. The T-Dongle-S3 has no RST button: hold BOOT while plugging it in.
- Some ESP32-S3 boards stay in the bootloader after flashing – press RST once (or run
  `espflash reset`).

## Playing

1. After power-up the board starts a demo game (engine vs engine) and creates the open Wi-Fi
   network **`ESP-Chess-XXXX`** (the last four characters come from its MAC address).
2. Join it with a phone, tablet or laptop. Most devices open the chess app automatically
   (captive portal, like a hotel Wi-Fi login page); otherwise open **`http://192.168.4.1`**.
3. Choose a nickname. In the lobby you can play the engine, create or accept a challenge, or
   watch running games.

**Engine levels:** 1–4 (Beginner, Casual, Club, Keen) limit the search depth and add noise to
the evaluation, so the engine makes human-like mistakes; 5–8 (Strong, Expert, Master, Full
power) play at full strength with 1 / 2.5 / 5 / 10 seconds per move. When several players wait
for the engine at the same time, the thinking time is shared. Hints search at least 8 plies
deep and are not available in games between people (fair play).

**Home Wi-Fi:** in *Settings → Wi-Fi* the board can join your home network, so you can play
from any device at home; its own network stays available. Only the board's *owner* – the first
player who connects while no home network is saved – may change the Wi-Fi settings. On boards
with a display, the address in the home network is also available as a QR code.

**BOOT button** (GPIO9 on ESP32-C6/C3, GPIO0 on ESP32-S3):

| Action | Effect |
|---|---|
| Short press (< 1 s) | Next display page: game (human games take turns, otherwise the demo) → each running game → overview → QR codes (join the board's Wi-Fi, app address, app address in the home network) → statistics. After 2 minutes without a press the display returns to the game. No effect on boards without a display. |
| Hold for 10 s | Factory reset: the saved Wi-Fi is erased and the board restarts (the display shows a countdown). Afterwards the first player to connect becomes the owner. |

**When nobody plays**, the demo game thinks 10 s per move and pauses 20 s between moves to keep
the chip cool; with players online it uses 2 s per move, and it gives way whenever a human
needs the engine.

## Project layout

| Path | Contents |
|---|---|
| `crates/chess-core` | Chess rules: bitboards, move generation, FEN, SAN, Zobrist hashing, draw rules |
| `crates/chess-engine` | The engine: search, transposition table, SEE, difficulty levels, NNUE glue; the network `nets/h128-v4-i8.bin` |
| `crates/nnue-core` | Network file format and inference, including the ESP32-S3 SIMD (PIE) kernels |
| `crates/chess-hub` | Game server: players, sessions, challenges, games, clocks, engine job queue, demo game, JSON API, stored Wi-Fi record |
| `crates/chess-display` | Display screens: the 240×240 renderer and the 160×80 `SmallRenderer`, QR codes, piece bitmaps |
| `crates/chess-web` | HTTP layer (picoserve): embeds the web app as one gzipped file and routes `/api/*` to the hub |
| `web/` | The web app (plain HTML/CSS/JavaScript, English + Czech) and its Playwright tests |
| `firmware/` | ESP32 firmware (esp-hal, Embassy, esp-radio) – a separate Cargo project |
| `sim/` | PC simulator / development server and UCI mode |
| `tools/gen-assets` | Converts the SVG pieces into display bitmaps (the output is committed) |
| `tools/elf-size` | Firmware section sizes (flash / static RAM) and the main-stack check |
| `scripts/unix/` | Linux/macOS: `setup.sh`, `build.sh`, `flash.sh`, `monitor.sh`, `esp-uci.py` |
| `scripts/*.ps1` | Windows: `setup`, `build`, `flash`, `monitor`, `sim`, `test` |
| `docs/` | [Architecture](docs/ARCHITECTURE.md) and [hardware notes](docs/HARDWARE.md) |

All crates under `crates/` are `no_std`, do not use a heap, and are tested on a PC.

## Development

### Tests

```bash
cargo test --workspace --release      # all library tests (release: the engine is slow unoptimised)
cargo clippy --workspace --all-targets -- -D warnings
(cd firmware && cargo clippy --release -- -D warnings)

cd web && npm ci && npx playwright install chromium
npx playwright test                   # web UI tests against the simulator (desktop + mobile)
```

The Playwright configuration starts the simulator by itself. On Windows `.\scripts\test.ps1`
runs all of the above (`-SkipWeb` skips Playwright).

### PC simulator

The simulator runs the same hub, engine and web app on a PC (the engine in a thread, the web
server on tokio) – handy for working on the web app without a board:

```bash
cargo run --release -p chess-sim                # http://localhost:8080
cargo run --release -p chess-sim -- --fast      # the engine thinks 20× shorter
cargo run --release -p chess-sim -- --port 9000 --no-demo --no-provision
cargo run --release -p chess-sim -- --uci       # the engine as a UCI program (Arena, cutechess, …)
```

On Windows: `.\scripts\sim.ps1` (`-Port`, `-Fast`, `-Uci`).

### A board as a UCI engine

The firmware feature `uci` turns a board into a UCI engine over USB (without Wi-Fi, web or
display). `scripts/unix/esp-uci.py` bridges the board's serial port to stdin/stdout, so any UCI
GUI or match runner can use it:

```bash
FEATURES=uci CHIP=esp32c3 scripts/unix/flash.sh   # flash the UCI firmware
scripts/unix/esp-uci.py /dev/ttyACM0               # use this as the engine command
```

Flash the normal firmware again afterwards.

### Another network

The build embeds `crates/chess-engine/nets/h128-v4-i8.bin`. A different network in the same
format can be selected at build time with the environment variable `NNUE_NET=<path>`. The
networks were trained in a separate project that is not part of this repository.

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) – crates, threads and concurrency, the engine,
  web API, limits, memory, storage, display.
- [docs/HARDWARE.md](docs/HARDWARE.md) – per-board notes, pinouts and known issues.

## License and credits

ESP32 Rust Chess is © 2026 Jan Blaha and released under the [MIT License](LICENSE).

- The search and evaluation started as a port of
  [tinychess](https://github.com/Timmy6942025/tinychess) /
  [Dog](https://github.com/folkertvanheusden/Dog) © Folkert van Heusden (MIT).
- Chess pieces: the *cburnett* set by Colin M.L. Burnett (CC BY-SA 3.0).

Details and full license texts: [THIRD_PARTY.md](THIRD_PARTY.md).
