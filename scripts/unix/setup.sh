#!/usr/bin/env bash
# One-time computer setup (macOS or Linux – Arch/CachyOS, Debian/Ubuntu):
# Rust targets for the ESP32-C6/C3, espflash, and the "esp" toolchain for the ESP32-S3
# (WITH_S3=1, CHIP=esp32s3 or BOARD=tdongle).
#   scripts/unix/setup.sh
#   WITH_S3=1 scripts/unix/setup.sh        including the ESP32-S3 toolchain
source "$(dirname "$0")/common.sh"

if [[ "$(uname)" == "Linux" ]]; then
    step "System packages (sudo)"
    # rustup only when missing (it usually comes from rustup.rs into ~/.cargo/bin)
    if command -v pacman >/dev/null; then
        pkgs=(base-devel git pkgconf python)
        command -v rustup >/dev/null || pkgs+=(rustup)
        sudo pacman -S --needed --noconfirm "${pkgs[@]}"
    elif command -v apt-get >/dev/null; then
        sudo apt-get install -y build-essential git pkg-config libudev-dev python3 curl
        command -v rustup >/dev/null || curl https://sh.rustup.rs -sSf | sh -s -- -y
    fi
    # serial ports: group uucp (Arch) / dialout (Debian)
    group=uucp
    getent group uucp >/dev/null || group=dialout
    if ! id -nG | tr ' ' '\n' | grep -qx "$group"; then
        sudo usermod -aG "$group" "$USER"
        warn "Added to group $group – log out and back in (or reboot), otherwise /dev/ttyACM* cannot be opened."
    fi
fi

need rustup
rustup toolchain install stable --profile minimal >/dev/null 2>&1 || true
step "Rust RISC-V targets (ESP32-C6, ESP32-C3)"
rustup target add riscv32imac-unknown-none-elf riscv32imc-unknown-none-elf
rustup component add rust-src clippy rustfmt
if ! command -v espflash >/dev/null; then
    step "Installing espflash"
    cargo install espflash --locked
fi
espflash --version

if [[ "$CHIP" == "esp32s3" ]] || [[ -n "${WITH_S3:-}" ]]; then
    step "\"esp\" toolchain for the ESP32-S3 (Xtensa)"
    command -v espup >/dev/null || cargo install espup --locked
    espup install --targets esp32s3 --export-file "$HOME/export-esp.sh"
fi
step "Done – connect the board and run scripts/unix/flash.sh"
