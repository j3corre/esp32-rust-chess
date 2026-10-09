#!/usr/bin/env bash
# Serial console of the board without flashing (Ctrl+C = quit).
#   LOG=monitor.log scripts/unix/monitor.sh    also save a copy of the output to a file
source "$(dirname "$0")/common.sh"
need espflash
port="$(find_port)"
if [[ -n "${LOG:-}" ]]; then
    step "Console $port (copy: $LOG)"
    espflash monitor --chip "$CHIP" --elf "$FIRMWARE_ELF" --port "$port" 2>&1 | tee "$LOG"
else
    step "Console $port"
    exec espflash monitor --chip "$CHIP" --elf "$FIRMWARE_ELF" --port "$port"
fi
