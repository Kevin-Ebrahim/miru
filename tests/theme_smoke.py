#!/usr/bin/env python3
"""Offline PTY checks for desktop palette reload and terminal/NO_COLOR fallbacks."""
import fcntl
import json
import os
import pathlib
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time


DARK = '''background = "#101114"
foreground = "#e8e8e8"
accent = "#d8a0f0"
lighter_background = "#171719"
selection = "#443344"
green = "#8fe8aa"
yellow = "#eeddaa"
red = "#eeaaaa"
'''
LIGHT = '''background = "#f4f2ed"
foreground = "#23272e"
accent = "#004477"
lighter_background = "#e8e8e2"
selection = "#c4d6dd"
'''


def check(binary, mode):
    with tempfile.TemporaryDirectory(prefix="ani-watch-theme-") as directory:
        root = pathlib.Path(directory)
        palette = root / "desktop/omarchy/current/theme/colors.toml"
        palette.parent.mkdir(parents=True)
        palette.write_text(DARK)
        library = root / "app"
        library.mkdir()
        (library / "library.json").write_text(json.dumps({
            "version": 1, "mode": "sub", "quality": "best", "entries": [{
                "anime": {"id": "fixture-1", "title": "Theme fixture", "cover": None},
                "episode": "1", "mode": "sub", "watched_at": 1,
            }],
        }))
        env = dict(os.environ)
        for key in ("NO_COLOR", "ANI_WATCH_THEME", "SSH_CONNECTION"):
            env.pop(key, None)
        env.update(XDG_STATE_HOME=str(root / "desktop"), XDG_CONFIG_HOME=str(root / "config"))
        if mode == "no-color":
            env["NO_COLOR"] = "1"
        if mode == "ssh":
            env["SSH_CONNECTION"] = "fixture"
        command = [str(binary), "--images", "off", "--no-animation", "--state-dir", str(library),
                   "--provider", "http://127.0.0.1:9"]
        if mode == "terminal":
            command += ["--theme", "terminal"]
        master, slave = os.openpty()
        os.set_blocking(master, False)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 960, 800))
        process = subprocess.Popen(command, env=env, stdin=slave, stdout=slave, stderr=slave,
                                   start_new_session=True)
        output, pending = bytearray(), bytearray()

        def pump(seconds=0.1):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except (BlockingIOError, OSError):
                        continue
                    output.extend(chunk)
                    pending.extend(chunk)
                    while b"\x1b[6n" in pending:
                        end = pending.index(b"\x1b[6n") + 4
                        del pending[:end]
                        os.write(master, b"\x1b[1;1R")
                    pending[:] = pending[-3:]

        def wait_for(fragment):
            deadline = time.monotonic() + 4
            while fragment not in output and time.monotonic() < deadline:
                pump()
            assert fragment in output, f"{mode}: missing {fragment!r}"

        try:
            wait_for(b"anime")
            if mode == "auto":
                wait_for(b"38;2;216;160;240")
                wait_for(b"48;2;16;17;20")
                output.clear()
                replacement = palette.with_suffix(".new")
                replacement.write_text(LIGHT)
                replacement.replace(palette)
                wait_for(b"38;2;0;68;119")
                wait_for(b"48;2;244;242;237")
                os.write(master, b"\x1bh")
                wait_for(b"Theme fixture")
                os.write(master, b"i")
                wait_for(b"Details")
            else:
                pump(1.2)
                assert b"38;2;" not in output and b"48;2;" not in output, mode
                if mode == "no-color":
                    assert b"38;5;" not in output and b"48;5;" not in output
            os.write(master, b"\x03")
            pump(0.2)
            assert process.wait(timeout=3) == 0
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ani-watch").resolve()
    for mode in ("auto", "terminal", "no-color", "ssh"):
        check(binary, mode)
    print("PASS: desktop colors, live dark-to-light reload, image-independent theme, terminal override, NO_COLOR and SSH fallbacks")
