#!/usr/bin/env python3
"""PTY regression: resize and restore terminal modes on q, Ctrl+C, TERM and HUP.

This emulates cursor replies; it does NOT replace visual Kitty/Herdr testing.
"""
import fcntl
import os
import pathlib
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def check(binary, exit_method, animate=False):
    with tempfile.TemporaryDirectory(prefix="ani-watch-pty-") as directory:
        master, slave = os.openpty()
        before = termios.tcgetattr(slave)
        os.set_blocking(master, False)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 960, 800))
        command = [str(binary), "--images", "off", "--state-dir", directory]
        if not animate:
            command.append("--no-animation")
        process = subprocess.Popen(command,
                                   stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        output = bytearray()
        pending = bytearray()

        def pump(seconds):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.03)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except BlockingIOError:
                        continue
                    output.extend(chunk)
                    pending.extend(chunk)
                    # Ratatui 0.30.2 uses DSR even for clear/autoresize.
                    while b"\x1b[6n" in pending:
                        end = pending.index(b"\x1b[6n") + 4
                        del pending[:end]
                        os.write(master, b"\x1b[1;1R")
                    pending[:] = pending[-3:]

        try:
            pump(0.25)
            assert process.poll() is None, bytes(output)
            for _ in range(20):
                if b"Home" in output:
                    break
                pump(0.1)
            assert b"Home" in output, "Home did not draw"
            pump(0.2)
            drawn = len(output)
            pump(0.4)
            if animate:
                assert len(output) > drawn, "Animation stopped redrawing"
            else:
                assert len(output) == drawn, "Reduced motion kept redrawing"
            for rows, cols in ((18, 40), (8, 20), (40, 120)):
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, cols * 8, rows * 20))
                process.send_signal(signal.SIGWINCH)
                pump(0.2)
            if exit_method == "q":
                os.write(master, b"\x1b")
                pump(0.1)
                os.write(master, b"q")
            elif exit_method == "ctrl+c":
                os.write(master, b"\x03")
            else:
                process.send_signal(exit_method)
            pump(0.3)
            assert process.wait(timeout=3) == 0, bytes(output)
            assert termios.tcgetattr(slave) == before, "Raw terminal attributes were not restored"
            assert b"\x1b[?1049l" in output, "Alternate screen not restored"
            assert b"\x1b[?2004l" in output, "Bracketed paste not disabled"
            assert b"cursor position could not" not in output, "Terminal reader conflict regressed"
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    executable = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ani-watch").resolve()
    for method in ("q", "ctrl+c", signal.SIGTERM, signal.SIGHUP):
        check(executable, method)
    check(executable, "ctrl+c", animate=True)
    print("PASS: animation/reduced motion, PTY resize, q, Ctrl+C, SIGTERM, SIGHUP, raw/alternate-screen/paste restoration")
