#!/usr/bin/env bash
# Builds the firmware, flashes it to the board and opens the serial console (Ctrl+C = quit).
#   scripts/unix/flash.sh                  normal firmware (ESP32-C6 with a display)
#   CHIP=esp32c3 scripts/unix/flash.sh     ESP32-C3 / CHIP=esp32s3 … ESP32-S3 (no display)
#   BOARD=tdongle scripts/unix/flash.sh    LilyGO T-Dongle-S3 (ESP32-S3 + 160×80 display)
#   FEATURES=uci …                         extra firmware features (uci = UCI engine over USB)
#   NO_MONITOR=1 scripts/unix/flash.sh     no console
#   SKIP_BUILD=1 scripts/unix/flash.sh     flash the last built firmware
#   PORT=/dev/cu.usbmodem1101 …            specific port
#   FLASH_FREQ=80mhz …                     SPI flash frequency (dio mode only, the boards cannot do QIO)
source "$(dirname "$0")/common.sh"
need espflash
[[ -n "${SKIP_BUILD:-}" ]] || build_firmware "${FEATURES:-}"
[[ -f "$FIRMWARE_ELF" ]] || die "Firmware $FIRMWARE_ELF does not exist – run without SKIP_BUILD."
port="$(find_port)"
set_flash_args
step "Flashing firmware via $port"
espflash flash "${FLASH_ARGS[@]}" --port "$port" "$FIRMWARE_ELF"
if [[ -z "${NO_MONITOR:-}" ]]; then
    step "Serial console (Ctrl+C = quit)"
    exec espflash monitor --chip "$CHIP" --elf "$FIRMWARE_ELF" --port "$port"
fi
