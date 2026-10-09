#!/usr/bin/env bash
# Builds the firmware (release). Argument = extra features, e.g.: build.sh uci
# Chip: CHIP=esp32c3 / CHIP=esp32s3 (default esp32c6), board: BOARD=tdongle.
source "$(dirname "$0")/common.sh"
build_firmware "${1:-}"
step "Done: $FIRMWARE_ELF"
