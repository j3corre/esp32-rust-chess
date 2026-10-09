# Shared paths and functions for the macOS and Linux scripts (sourced: source "$(dirname "$0")/common.sh").
#
# The target chip is selected by CHIP (default esp32c6 = Waveshare ESP32-C6-LCD-1.3 with a display):
#   CHIP=esp32c3 scripts/unix/flash.sh     ESP32-C3 (e.g. SuperMini), no display
#   CHIP=esp32s3 scripts/unix/flash.sh     ESP32-S3, no display ("esp" toolchain from espup)
#   BOARD=tdongle scripts/unix/flash.sh    LilyGO T-Dongle-S3 (ESP32-S3 + 160×80 display)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FIRMWARE_DIR="$PROJECT_ROOT/firmware"
PARTITION_CSV="$FIRMWARE_DIR/partitions.csv"
# A board wired differently from the chip default: BOARD=tdongle (LilyGO T-Dongle-S3 with a display).
BOARD="${BOARD:-}"
if [[ "$BOARD" == "tdongle" ]]; then
    CHIP=esp32s3
fi
CHIP="${CHIP:-esp32c6}"

# cargo and espflash from ~/.cargo/bin, even if the shell did not load the profile
export PATH="$HOME/.cargo/bin:$PATH"

step() { printf '\n\033[32m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "Program '$1' not found. Run scripts/unix/setup.sh"
}

# Build parameters per chip: target, toolchain, default features.
CARGO_TOOLCHAIN=()
BUILD_STD=()
case "$CHIP" in
    esp32c6)
        TARGET=riscv32imac-unknown-none-elf
        CHIP_FEATURES="esp32c6,display"
        ;;
    esp32c3)
        TARGET=riscv32imc-unknown-none-elf
        CHIP_FEATURES="esp32c3"
        ;;
    esp32s3)
        TARGET=xtensa-esp32s3-none-elf
        CHIP_FEATURES="esp32s3"
        [[ "$BOARD" == "tdongle" ]] && CHIP_FEATURES="tdongle"
        CARGO_TOOLCHAIN=(+esp)
        BUILD_STD=(-Zbuild-std=core,alloc)
        # environment of the "esp" toolchain (the build fails without it – setup.sh installs it)
        if [[ -f "$HOME/export-esp.sh" ]]; then
            # shellcheck disable=SC1091
            source "$HOME/export-esp.sh"
        fi
        ;;
    *)
        die "Unknown chip CHIP=$CHIP (esp32c6, esp32c3, esp32s3)"
        ;;
esac
FIRMWARE_ELF="$FIRMWARE_DIR/target/$TARGET/release/esp32-chess"

# All USB serial ports: macOS /dev/cu.usbmodem* (built-in USB-Serial-JTAG),
# /dev/cu.usbserial*… (CP210x/CH34x adapters); Linux /dev/ttyACM* (USB-Serial-JTAG), /dev/ttyUSB*.
list_ports() {
    local p
    for p in /dev/cu.usbmodem* /dev/cu.usbserial* /dev/cu.SLAB_USBtoUART* /dev/cu.wchusbserial* \
        /dev/ttyACM* /dev/ttyUSB*; do
        [[ -e "$p" ]] && echo "$p"
    done
    return 0
}

# Chip on the port according to espflash (esp32c6 / esp32c3 / esp32s3 …), empty if it does not answer.
detect_chip() {
    espflash board-info --port "$1" 2>/dev/null | sed -nE 's/^Chip type: *(esp32[a-z0-9]*).*/\1/p' | head -1
}

# Board port: the PORT variable, otherwise the first USB serial port found.
find_port() {
    if [[ -n "${PORT:-}" ]]; then
        echo "$PORT"
        return
    fi
    local ports=()
    local p
    while IFS= read -r p; do ports+=("$p"); done < <(list_ports)
    if (( ${#ports[@]} == 0 )); then
        die "Board not found (no /dev/cu.usbmodem*, /dev/ttyACM* …). Is it connected with a data USB cable?
On Linux the user must be in the uucp (Arch/CachyOS) or dialout (Debian/Ubuntu) group.
If the firmware crashes and USB keeps disconnecting: hold BOOT, press RST, release BOOT and run the script again."
    fi
    if (( ${#ports[@]} > 1 )); then
        warn "Several ports found (${ports[*]}), using ${ports[0]} – choose another with: PORT=/dev/… $0" >&2
    fi
    echo "${ports[0]}"
}

# Firmware build (release) + main-thread stack check (tools/elf-size).
# Argument = extra comma-separated features (e.g. uci).
build_firmware() {
    need cargo
    local features="$CHIP_FEATURES${1:+,$1}"
    if [[ "$CHIP" == "esp32s3" ]] && ! command -v xtensa-esp32s3-elf-gcc >/dev/null; then
        die "The ESP32-S3 toolchain is missing – run: CHIP=esp32s3 scripts/unix/setup.sh"
    fi
    step "Building firmware for $CHIP (release, features: $features)"
    (cd "$FIRMWARE_DIR" && cargo ${CARGO_TOOLCHAIN[@]+"${CARGO_TOOLCHAIN[@]}"} build --release \
        ${BUILD_STD[@]+"${BUILD_STD[@]}"} --target "$TARGET" --no-default-features --features "$features")
    [[ -f "$FIRMWARE_ELF" ]] || die "The build succeeded, but $FIRMWARE_ELF is missing."
    local report
    report="$(cd "$PROJECT_ROOT" && cargo run -q -p elf-size -- "$FIRMWARE_ELF" 2>&1)"
    echo "$report" | tail -3 | sed 's/^/   /'
    if [[ -z "${SKIP_STACK_CHECK:-}" ]] && grep -q 'WARNING' <<<"$report"; then
        die "The main-thread stack is too small – the firmware would crash when Wi-Fi starts."
    fi
}

# Flash mode parameters (FLASH_MODE / FLASH_FREQ) into the FLASH_ARGS array.
set_flash_args() {
    # Verified: neither the Waveshare ESP32-C6-LCD-1.3 nor the ESP32-S3FH4 boots in QIO (the ROM
    # loader keeps getting reset by the watchdog). Fix: flash again without FLASH_MODE.
    if [[ "${FLASH_MODE:-}" == "qio" && -z "${ALLOW_QIO:-}" ]]; then
        die "QIO does not boot on the tested boards (C6-LCD, S3FH4) – use dio (ALLOW_QIO=1 overrides)."
    fi
    FLASH_ARGS=(--chip "$CHIP" --partition-table "$PARTITION_CSV")
    if [[ -n "${FLASH_MODE:-}" ]]; then FLASH_ARGS+=(--flash-mode "$FLASH_MODE"); fi
    if [[ -n "${FLASH_FREQ:-}" ]]; then FLASH_ARGS+=(--flash-freq "$FLASH_FREQ"); fi
}
