# Architecture

## Overview

```
                        ┌──────────────── firmware (ESP32, no_std, esp-hal + Embassy) ───────────────┐
  phone / PC            │                                                                            │
  browser ──Wi-Fi──►    │  Main thread (esp-rtos priority 2) – Embassy executor:                     │
  (web app, polling)    │   ├─ net_runner ×2   embassy-net: AP 192.168.4.1 + STA (DHCP client)       │
                        │   ├─ dhcp_server     edge-dhcp (addresses .50–.200 on the AP)              │
                        │   ├─ captive_dns     edge-captive (every name → 192.168.4.1)               │
                        │   ├─ web_task ×N     picoserve ── chess-web ── chess-hub (N per chip)      │
                        │   ├─ wifi_manager    STA connect, retries, scan, save to flash             │
                        │   ├─ display_task    chess-display → ST7789 / ST7735 (changes only)        │
                        │   ├─ button_task     BOOT: short = next page, 10 s = factory reset         │
                        │   └─ main            hub.tick() every 250 ms, stats every 30 s             │
                        │                                                                            │
                        │  Engine thread (C6/C3: esp-rtos priority 1; S3: alone on the second core): │
                        │   └─ takes an EngineJob from the hub → chess-engine (blocking search)      │
                        └────────────────────────────────────────────────────────────────────────────┘

   sim/ (PC, std): the same chess-web + chess-hub + chess-engine; engine in a std thread,
                   web server on tokio → web development, Playwright tests, UCI mode
```

## Crates and dependencies

```
chess-core ◄── chess-engine ──► nnue-core
    ▲              ▲
    └── chess-hub ─┘   (the hub describes EngineJobs and limits; it never calls the engine)
          ▲
          ├── chess-web      HTTP: the app (one gzipped file) + /api/* → chess_hub::api
          └── chess-display  screens drawn from a DisplayView (a snapshot of the hub)

firmware = chess-web + chess-display + chess-engine + hardware (esp-hal, esp-radio, embassy-net…)
sim      = chess-web + chess-engine + tokio
tools/gen-assets  SVG pieces → display bitmaps (the output is committed)
tools/elf-size    firmware section sizes (flash / static RAM) and main-stack check
```

All `crates/*` are `no_std`, do not use a heap, and are tested on a PC. Hardware-specific
code lives only in `firmware/`.

| Crate | Contents |
|---|---|
| `chess-core` | Bitboards + mailbox, move generation, legality, FEN, SAN, Zobrist hashing, draw rules |
| `chess-engine` | Search, transposition table, SEE, endgame helpers, difficulty levels, `SearchHooks` interface, NNUE glue and the embedded network |
| `nnue-core` | Network file formats (legacy Dog/tinychess and the `ESPNNUE` v1 header), inference, ESP32-S3 SIMD kernels |
| `chess-hub` | Sessions, nicknames, challenges, games, clocks, engine queue, demo game, JSON API, stored Wi-Fi record |
| `chess-display` | `Renderer` (240×240) and `SmallRenderer` (160×80) on top of embedded-graphics, QR codes, piece bitmaps |
| `chess-web` | picoserve routes, the embedded web app, captive-portal probes; the `Platform` trait hides board/PC differences |

## Concurrency

The engine searches synchronously (recursive alpha-beta) for up to 10 s at a time, while the
web server must answer within milliseconds.

**ESP32-C6 and ESP32-C3 (one core):**

- The engine has **its own esp-rtos scheduler thread** (`esp_radio_rtos_driver::task_create`,
  48 kB stack from the heap) with **priority 1**.
- **The main thread with the Embassy executor has priority 2.** As soon as a packet arrives, a
  timer expires or the button is pressed, the scheduler pre-empts the engine. When the Embassy
  tasks have nothing to do, the executor sleeps and the CPU goes back to the engine.
- The network stack therefore runs in an ordinary thread-mode executor, as `esp-radio` and
  `embassy-net` expect – nothing calls the Wi-Fi driver from an interrupt.

**ESP32-S3 (two cores):** the engine runs alone on the second core (APP CPU) with a static
48 kB stack; Wi-Fi, network and web run on the first core, so clients do not slow the engine
down. The SIMD coprocessor (PIE) is enabled on the engine's core only – the scheduler does not
save its registers, so nothing else may use it.

**Shared state:** the `Hub` lives in `Mutex<CriticalSectionRawMutex, RefCell<Option<Hub>>>`;
every operation (a JSON response, a move, an engine query) takes microseconds to a few hundred
microseconds. Engine ↔ hub:

- `next_engine_job()` – the job (position, history for repetition detection, limits),
- `job_obsolete()` – polled during the search (on the board every 128 nodes): the game ended or
  changed, or the demo must make way for a human → the search stops,
- `engine_progress()` / `engine_iteration()` – depth, nodes/s, evaluation and main line (display, web),
- `engine_done()` – the move is played (if the job is still valid).

## Engine queue (hub)

Job order: 1) an engine move in a game against a human – the one that has waited longest;
2) a hint (only in games against the engine – never between people; 1 s but at least depth 8,
hard limit 15 s); 3) the demo game. The level's time per move is divided by the number of
waiting jobs (minimum 300 ms) and, with a clock, limited to about 1/25 of the remaining time.
A demo job is cancelled as soon as a human is waiting. Demo without players: 10 s per move and a
20 s pause between moves (`DEMO_PAUSE_IDLE_MS`, about a third of the load – the chip stays
cooler); with players online 2 s per move and an 8 s pause.

## Engine

The search is a Rust port of tinychess (a fork of Dog by Folkert van Heusden), adapted to the
small RAM of the ESP32.

| Part | Description |
|---|---|
| Position | bitboards + mailbox, Zobrist hash (en passant only when the capture is possible), copy-make |
| Attacks | tables for knights / kings / pawns, hyperbola quintessence for sliders (small tables) |
| Moves | pseudo-legal generator, legality via pins and check masks, one shared move stack (3,072 moves) for all plies |
| NNUE | 768 → 128 (×2 perspectives) → SCReLU → 1 with 8 material output buckets; 8-bit feature-transformer weights (QA = 127); weights stay in flash; accumulator stack with lazy (deferred) updates; SWAR on RISC-V, 128-bit SIMD (PIE) on the ESP32-S3, verified against the scalar code at start-up |
| Search | iterative deepening, aspiration windows, PVS, transposition table (12 B per entry), IIR, mate distance pruning, reverse futility, razoring, null move with verification, futility pruning, LMR (ln·ln table), check / single-move / recapture extensions, quiescence search with SEE; the main line is read from the TT |
| Ordering | TT move, MVV-LVA + capture history, quiet-move history (piece → square, from → to) + continuation history |
| Levels | 1–4: limited depth + evaluation noise (believable mistakes); 5–8: 1 / 2.5 / 5 / 10 s per move |

The default network `crates/chess-engine/nets/h128-v4-i8.bin` (~100 kB) was trained in a
separate project; `crates/chess-engine/build.rs` validates the file and generates the
architecture constants. `NNUE_NET=<path>` at build time embeds another network in the same
format.

Performance work (hot code in internal RAM via the `iram` features, cheaper attack tables,
lazy accumulator updates, the static evaluation stored in the TT, branch-free legality checks,
the S3 SIMD kernels) took the ESP32-C6 from about 8,900 to about 19,400 nodes/s in the lab
benchmark. Every optimisation keeps the search bit-for-bit identical (same moves, scores and
node counts).

On the ESP32-S3 the firmware is built with `-C llvm-args=-disable-lsr`
(`firmware/.cargo/config.toml`): it is faster there, and together with a few `black_box` calls
in `search.rs` it avoids an Xtensa LLVM back-end error ("Cannot select: Constant<…>") in the
search loops.

## Web API

JSON; parameters are passed in the URL query, also for POST.

| Path | Parameters | Description |
|---|---|---|
| `GET /` | | the app (HTML + CSS + JS + pieces, one gzipped file of ~30 kB) |
| `/api/hello` | `nick`, `t` | log in / resume a session / change nickname → `{token, id, nick, owner}` |
| `/api/bye` | `t` | log out (resigns the game, frees the nickname) |
| `/api/lobby` | `t` | me, players online, challenges, games (previews), engine, network, device info |
| `/api/seek` | `min`, `inc`, `color` | challenge for a game between people |
| `/api/cancel` | | cancel the challenge |
| `/api/accept` | `seek` | accept a challenge → `{game}` |
| `/api/engine` | `level`, `color`, `min`, `inc` | game against the engine → `{game}` |
| `/api/game` | `id`, `since` | game state; only moves from `since` (max. 120, then `more`) + legal moves |
| `/api/move` | `id`, `m` (UCI) | make a move |
| `/api/resign`, `/api/draw` (`decline`), `/api/takeback`, `/api/hint`, `/api/rematch` | `id` | game actions |
| `/api/wifi` | `ssid`, `pass` / `forget` | network state; saving / forgetting only for the owner |
| `/api/wifi/scan` | | nearby networks (owner only) |
| `/generate_204`, `/hotspot-detect.html`, … | | 302 to the app (captive portal) |

Errors: `{"ok":false,"err":"code"}`; the web app translates the codes (`err_*` in `web/i18n`).
The app polls the game every 700 ms and the lobby every 1.5 s. Unlike WebSockets, polling does
not keep a TCP socket open, so the same number of sockets serves more players.

The web app (`web/`) is plain HTML, CSS and JavaScript without a framework. The `chess-web`
build script inlines CSS, JS and the SVG pieces into `index.html` and gzips it, so the board
and the simulator serve a byte-identical file.

## Limits (hub)

| Item | Limit |
|---|---|
| sessions (players) | 24 (online = heard from in the last 20 s) |
| games (including finished ones and the demo) | 8 |
| open challenges | 8 |
| game length | 400 plies (then a draw) |
| abandoned game | 3 min without contact → loss (cancelled before the 2nd ply) |
| finished game kept | 10 min |
| nickname | 16 characters |

## Per-chip parameters

Set in `firmware/src/chip.rs`:

| | ESP32-C6 | ESP32-C3 | ESP32-S3 |
|---|---|---|---|
| CPU | RISC-V 160 MHz, 1 core | RISC-V 160 MHz, 1 core, no atomics | 2× Xtensa LX7 240 MHz, SIMD |
| transposition table | 40 kB | 8 kB | 16 kB, or ~1.75 MB in PSRAM |
| static heap (+ 64 kB reclaimed from the bootloader) | 68 kB | 56 kB | 8 kB (engine stack is static on core 1) |
| web tasks (AP + home network) | 4 + 4 | 2 + 2 | 3 + 3 |
| NNUE accumulator stack | 32 plies | 16 plies | 16 plies |
| hot code in RAM | search + NNUE (`iram`) | search + NNUE (`iram`) | NNUE (`nnue-iram`) |

The ESP32-C3 has no atomic read-modify-write instructions; the code uses `portable-atomic`
(esp-hal enables `unsafe-assume-single-core`).

**PSRAM (ESP32-S3):** `firmware/src/psram.rs` reads the PSRAM capacity from eFuse
(`PSRAM_CAP`), so the same firmware runs on S3 chips with and without PSRAM. With PSRAM, the
whole PSRAM except a 256 kB reserve becomes the transposition table; it is not added to the
heap so that the Wi-Fi driver never gets DMA buffers there. The unused small TT is then given to
the heap.

## Memory (ESP32-C6, release build, approximate)

| Item | Size |
|---|---|
| flash image (code + NNUE ~100 kB + web app ~30 kB) | ~1.3 MB of the 3.9 MB app partition |
| static RAM: code in RAM (search, NNUE, Wi-Fi driver) | ~93 kB |
| static RAM: data + bss (TT, search tables, Embassy tasks with web buffers, static heap) | ~340 kB |
| main-thread stack | ~17 kB (must be ≥ 16 kB, checked by `tools/elf-size`) |
| heap | 64 kB reclaimed + 68 kB static – Wi-Fi driver (~55 kB) + engine stack (48 kB) + reserve |

The firmware prints the free heap, web-server statistics, chip temperature and Wi-Fi TX power
to the serial console every 30 s.

## Storage

The `chesscfg` partition (0x3F0000, see `firmware/partitions.csv`) holds a 128-byte
`WifiCredentials` record with a CRC-32 (`chess_hub::stored`, tested on the PC). A factory reset
overwrites it with 0xFF. Without stored Wi-Fi the hub is in *provisioning* mode: the first player
to log in becomes the owner and may configure the Wi-Fi.

Wi-Fi: the radio always runs in AP + STA mode. The board's own network is open (no password) so
that guests can join easily; the home network is joined with WPA2-Personal (mixed WPA2/WPA3
networks work, WPA3-only networks do not). The TX power is set to 15 dBm.

## Display

`chess_display::Renderer` (240×240, Waveshare ESP32-C6-LCD-1.3, ST7789) remembers what it has
drawn and redraws only changes (a move = 2–4 squares of 25×25 px, one SPI transfer per square).
Screens:

- **Game** – board 200×200, evaluation bar on the left, side-to-move dot on the right; below it
  the engine's main line ("23...Rxa4 24.Qb3 Ra8 …"), otherwise the players' names, and the line
  "D12 48.3 kN/s +0.35" with the best move so far on the right (D = depth in plies). The main
  line and best move are shown only while the engine thinks about its own move – never for a
  hint, and in a human game only during the engine's turn, so the display does not help the
  human. In the demo the last line stays visible in grey between moves.
- **Overview** – up to 4 mini boards of running games.
- **QR** – join the board's Wi-Fi (`WIFI:` code), app address on the board's network, app
  address in the home network.
- **Info / statistics** – players, games, uptime, engine, network, web server, memory, chip
  temperature.
- **Reset countdown** and **message** (start-up, connecting…).

`chess_display::SmallRenderer` (160×80, LilyGO T-Dongle-S3, ST7735) draws the same frames with a
compact layout: an 80×80 board (10 px squares), the evaluation bar, and on the right depth,
speed, evaluation, side to move, best move and the wrapped main line; QR, info and overview
have small-screen versions too. The T-Dongle panel renders dark tones far too bright, so
`firmware/src/board_tdongle.rs` passes every colour through a tone curve measured on the panel.

Fonts are ISO-8859-2 (Czech diacritics). SAN is recomputed only when the main line changes. The
firmware's `ui.rs` picks the page (see the BOOT button in the README) and redraws every 200 ms.

## Testing

| Layer | How |
|---|---|
| chess-core | perft on standard positions (deeper runs with `--ignored`), FEN, hashing, SAN, draw rules |
| chess-engine | NNUE incremental = full recomputation, symmetry, SEE, TT, mates, tactics, stop, determinism |
| nnue-core | inference against reference outputs (`tests/data`, an original tinychess network) |
| chess-hub | sessions, challenges, clocks, game endings, engine queue, demo, API, stored Wi-Fi |
| chess-display | drawing into memory, diff redraws, QR (snapshots written to `target/display-snapshots`) |
| chess-web | routing, gzip |
| web (Playwright) | desktop + mobile against the simulator: login, languages, settings, Wi-Fi, engine games, two players, promotion, premoves, spectators |

Run `cargo test --workspace --release` and `npx playwright test` in `web/` (see the README).

## Board pinouts

**Waveshare ESP32-C6-LCD-1.3** (`firmware/src/board.rs`):

| Signal | GPIO | Note |
|---|---|---|
| SCLK / MOSI | 7 / 6 | SPI2, 40 MHz |
| CS / DC / RST | 14 / 15 / 21 | ST7789V2 240×240, inverted colours |
| BL | 22 | backlight (1 = on) |
| BOOT | 9 | input with pull-up, pressed = 0 |

**LilyGO T-Dongle-S3** (`firmware/src/board_tdongle.rs`):

| Signal | GPIO | Note |
|---|---|---|
| SCLK / MOSI | 5 / 3 | SPI, 27 MHz |
| CS / DC / RST | 4 / 2 / 1 | ST7735, 0.96″ IPS 80×160 (offset 26/1 in the 132×162 controller RAM), BGR, inverted; drawn in landscape |
| BL | 38 | backlight (0 = on) |
| BOOT | 0 | the only button |

ESP32-C3 boards use BOOT = GPIO9, other ESP32-S3 boards BOOT = GPIO0; they have no display.
