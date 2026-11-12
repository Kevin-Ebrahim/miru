#!/usr/bin/env python3
"""Reject ambiguous operations before provider, player, import or state side effects."""
import argparse
import http.server
import itertools
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading

from jobs_smoke import Fixture


def snapshot(root):
    result = {}
    for path in root.rglob("*"):
        info = path.lstat()
        result[str(path.relative_to(root))] = (info.st_ino, info.st_mode, info.st_mtime_ns,
                                             path.read_bytes() if path.is_file() else None)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/miru", type=pathlib.Path)
    options = parser.parse_args()
    binary = options.binary.resolve()
    requests = []

    class Provider(Fixture):
        def do_GET(self):
            requests.append(self.path)
            super().do_GET()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="miru-cli-") as temporary:
            root = pathlib.Path(temporary)
            tools = root / "tools"
            tools.mkdir()
            marker = root / "media-started"
            for name in ("player", "ffmpeg"):
                program = tools / name
                program.write_text(f'#!{sys.executable}\nimport pathlib\npathlib.Path({str(marker)!r}).touch()\n')
                program.chmod(0o700)
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("MIRU_", "ANI_WATCH_"))}
            environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
            state = root / "state"
            state.mkdir()
            library = {"version": 2, "mode": "sub", "quality": "best", "entries": []}
            (state / "library.json").write_text(json.dumps(library))
            source = root / "ani-hsts"
            source.write_text("1\tshow-1\tImported show\n")
            common = [str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/",
                      "--state-dir", str(state), "--cache-dir", str(root / "cache"),
                      "--download-dir", str(root / "downloads"), "--player", str(tools / "player"),
                      "--images", "off", "--no-animation"]

            def rejected(command):
                before = snapshot(root)
                requests.clear()
                result = subprocess.run(command, env=environment, stdin=subprocess.DEVNULL,
                                        capture_output=True, text=True, timeout=10)
                assert result.returncode == 2, (command, result.returncode, result.stdout, result.stderr)
                assert not result.stdout and "cannot be used with" in result.stderr, result.stderr
                assert not requests, f"Invalid operations contacted the provider: {requests}"
                assert not marker.exists(), "Invalid operations started a media tool"
                assert snapshot(root) == before, "Invalid operations mutated state, imports or downloads"

            operations = [("--play",), ("--history",), ("--list",), ("--resolve",),
                          ("--download",), ("--import-history", str(source)), ("--continue",)]
            for first, second in itertools.combinations(operations, 2):
                for left, right in ((first, second), (second, first)):
                    rejected(common + [*left, *right, "Probe"])
            print("PASS: all 42 ordered operation pairs fail before any side effects", flush=True)
            for aliases in (("-c", "--history"), ("-d", "--history"), ("-cd",), ("--play", "-c")):
                rejected(common + [*aliases, "Probe"])
            print("PASS: short and clustered operation aliases share the same validation", flush=True)

            legacy = root / "migration/ani-watch"
            legacy.mkdir(parents=True)
            (legacy / "library.json").write_text(json.dumps(library))
            environment["XDG_STATE_HOME"] = str(legacy.parent)
            rejected([str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/",
                      "--player", str(tools / "player"), "--play", "Probe", "--history"])
            assert not (legacy.parent / "miru").exists(), "Invalid operations migrated history"
            print("PASS: conflicts are rejected before default-state migration", flush=True)

            help_output = subprocess.run([str(binary), "--help"], env=environment,
                                         capture_output=True, text=True, timeout=10)
            assert help_output.returncode == 0 and not help_output.stderr
            assert "mutually exclusive" in help_output.stdout and "Modifiers" in help_output.stdout, help_output.stdout
            print("PASS: help documents operation exclusions and modifier compatibility", flush=True)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
