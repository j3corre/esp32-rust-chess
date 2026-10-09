#!/usr/bin/env python3
"""A UCI engine for the PC whose search actually runs on an ESP32 board (firmware with the `uci` feature).

Forwards lines between stdin/stdout (a chess GUI such as Arena or cutechess) and the board's
USB-Serial-JTAG serial port. From the board it passes through only UCI protocol lines (id,
uciok, readyok, info, bestmove, option) – bootloader output and logs are dropped.

    esp-uci.py /dev/cu.usbmodem1101

Standard library only (termios, select) – no pyserial.
"""

import os
import select
import subprocess
import sys
import termios
import time

UCI_PREFIXES = ("id ", "uciok", "readyok", "info ", "bestmove", "option ", "copyprotection", "registration")
LOG = os.environ.get("ESP_UCI_LOG")  # debug log file (everything sent in both directions)


def log(direction, line):
    if LOG:
        with open(LOG, "a") as f:
            f.write(f"{time.time():.3f} {direction} {line}\n")


def open_port(path):
    fd = os.open(path, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    attrs = termios.tcgetattr(fd)
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = attrs
    # raw mode, no echo or character processing; HUPCL off (closing the port does not reset the board)
    iflag = 0
    oflag = 0
    lflag = 0
    cflag = (cflag & ~(termios.CSIZE | termios.PARENB | termios.HUPCL)) | termios.CS8 | termios.CREAD | termios.CLOCAL
    cc[termios.VMIN] = 0
    cc[termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, [iflag, oflag, cflag, lflag, termios.B115200, termios.B115200, cc])
    return fd


def uci_part(line):
    """The UCI line, even when preceded by leftovers of old output (USB buffer); otherwise None."""
    if line.startswith(UCI_PREFIXES):
        return line
    for token in ("id name", "id author", "uciok", "readyok", "bestmove", "info depth", "info string"):
        i = line.find(token)
        if i > 0:
            return line[i:]
    return None


def write_line(fd, line):
    data = (line.strip() + "\n").encode()
    while data:
        try:
            n = os.write(fd, data)
            data = data[n:]
        except BlockingIOError:
            time.sleep(0.001)


def wait_ready(fd, seconds):
    """Sends isready until the board answers readyok (after a reset the firmware takes a moment to start)."""
    deadline = time.time() + seconds
    buf = b""
    while time.time() < deadline:
        write_line(fd, "isready")
        end = time.time() + 0.5
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if fd in r:
                try:
                    buf += os.read(fd, 4096)
                except BlockingIOError:
                    pass
                if b"readyok" in buf:
                    return True
        log("<#", buf.decode(errors="replace").replace("\n", " | ")[-300:])
        buf = b""
    return False


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: esp-uci.py <board serial port>")
    fd = open_port(sys.argv[1])
    # The board may still be searching from last time: stop it, drop old output and wait until
    # it answers isready (after a board reset the firmware takes a moment to start).
    write_line(fd, "stop")
    if not wait_ready(fd, 15):
        # Stuck firmware (e.g. after an engine crash mid-game) – reset the board and retry,
        # so the GUI can carry on.
        os.close(fd)
        sys.stderr.write("esp-uci: board not responding, resetting it\n")
        log("#", "board reset")
        subprocess.run(["espflash", "reset", "--port", sys.argv[1]], capture_output=True, timeout=30)
        time.sleep(2)
        fd = open_port(sys.argv[1])
        if not wait_ready(fd, 15):
            sys.exit("esp-uci: board not responding (is firmware with the uci feature flashed?)")
    time.sleep(0.2)
    try:
        while os.read(fd, 4096):
            pass
    except BlockingIOError:
        pass

    stdin = sys.stdin.fileno()
    board_buf = b""
    stdin_buf = b""
    while True:
        ready, _, _ = select.select([fd, stdin], [], [], 1.0)
        if fd in ready:
            try:
                chunk = os.read(fd, 4096)
            except BlockingIOError:
                chunk = b""
            board_buf += chunk
            while b"\n" in board_buf:
                raw, board_buf = board_buf.split(b"\n", 1)
                text = raw.decode(errors="replace").strip("\r\x00 ")
                line = uci_part(text)
                if line:
                    log("<", line)
                    sys.stdout.write(line + "\n")
                    sys.stdout.flush()
                elif text:
                    log("<#", text)
        if stdin in ready:
            chunk = os.read(stdin, 4096)
            if not chunk:
                write_line(fd, "stop")
                return
            stdin_buf += chunk
            while b"\n" in stdin_buf:
                raw, stdin_buf = stdin_buf.split(b"\n", 1)
                line = raw.decode(errors="replace").strip()
                if not line:
                    continue
                log(">", line)
                if line == "quit":
                    write_line(fd, "stop")
                    return
                write_line(fd, line)


if __name__ == "__main__":
    main()
