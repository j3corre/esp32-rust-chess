# Third-party components

ESP32 Rust Chess is MIT licensed (see [LICENSE](LICENSE)). It builds on the following work.

## tinychess / Dog chess engine

The search and evaluation in `crates/chess-engine` (and the legacy network format in
`crates/nnue-core`) started as a Rust port of
[tinychess](https://github.com/Timmy6942025/tinychess), a fork of the
[Dog](https://github.com/folkertvanheusden/Dog) chess engine by Folkert van Heusden.

The test data in `crates/nnue-core/tests/data/` include an original tinychess network
(`net128.bin`), used to check that the inference matches the original bit for bit; its license
is also in `crates/nnue-core/tests/data/LICENSE-tinychess`.

```
MIT License

Copyright (c) 2022-2025 folkertvanheusden

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Chess pieces (cburnett)

The piece images in `web/pieces/` – and the display bitmaps generated from them in
`crates/chess-display/src/pieces_data.rs` – are the *cburnett* set by Colin M.L. Burnett,
licensed under [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/).

## NNUE network

The network `crates/chess-engine/nets/h128-v4-i8.bin` was trained for this project in a separate
project and is distributed under this project's MIT license.

## Rust crates

The firmware, libraries and tools depend on crates from crates.io (esp-hal, Embassy, esp-radio,
picoserve, edge-dhcp, edge-captive, embedded-graphics, mipidsi, qrcodegen-no-heap, tokio, resvg, flate2 and
others), each under its own license – mostly MIT and/or Apache-2.0. See `Cargo.lock` and
`firmware/Cargo.lock` for the full list.
