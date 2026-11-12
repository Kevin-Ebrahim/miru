#!/usr/bin/env python3
"""Process-level regression for hostile provider HTML and terminal shutdown."""
import fcntl
import http.server
import json
import os
import pathlib
import resource
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    depth = 8
    stage = "synopsis"
    served = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            path = self.path.split("?")[0]
            nested = "<div>" * depth + "PARSER_PROBE_COMPLETE" + "</div>" * depth
            if path == "/probe-1":
                body = '<div class="film-description"><div class="text">' + nested + '</div></div>'
            elif path == "/search":
                card = '<div class="flw-item"><h3 class="film-name"><a title="Parser Probe" href="/probe-1">Probe</a></h3></div>'
                if stage == "expensive-search":
                    body = ''.join(f'<div class="flw-item"><h3 class="film-name"><a title="Series {i}" href="/series-{i}">Probe</a></h3>' for i in range(800))
                    body += '<div class="description">' + "x" * (1024 * 1024) + '</div>' + '</div>' * 800
                else:
                    body = nested + card if stage == "search" else card
            elif path == "/api/theme/episode/list/1":
                episode = '<a class="ep-item" data-id="1" data-number="1" href="/watch/probe-1?ep=1"></a>'
                body = json.dumps({"html": nested + episode if stage == "episodes" else episode})
            else:
                self.send_response(404)
                self.end_headers()
                return
            data = body.encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            try:
                self.wfile.write(data)
                if path == "/probe-1":
                    served.set()
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for depth in (8, 100_000, 8_000):
            for stopping in ("ctrl-c", signal.SIGTERM, signal.SIGHUP):
                with tempfile.TemporaryDirectory(prefix="miru-parser-") as temporary:
                    root = pathlib.Path(temporary)
                    (root / "library.json").write_text(json.dumps({
                        "version": 2, "mode": "sub", "quality": "best", "entries": [{
                            "anime": {"id": "probe-1", "title": "Parser Probe", "cover": None},
                            "episode": "0", "mode": "sub", "watched_at": 0,
                        }],
                    }))
                    master, slave = os.openpty()
                    before = termios.tcgetattr(slave)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 960, 800))
                    os.set_blocking(master, False)
                    served.clear()
                    process = subprocess.Popen([
                        str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/",
                        "--state-dir", str(root), "--cache-dir", str(root / "cache"),
                        "--continue", "--images", "off", "--theme", "terminal", "--no-animation",
                    ], stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
                        preexec_fn=lambda: resource.setrlimit(resource.RLIMIT_CORE, (0, 0)))
                    output = bytearray()
                    pending = bytearray()

                    def pump(duration):
                        deadline = time.monotonic() + duration
                        while time.monotonic() < deadline:
                            if select.select([master], [], [], 0.01)[0]:
                                try:
                                    data = os.read(master, 65536)
                                except BlockingIOError:
                                    continue
                                output.extend(data)
                                pending.extend(data)
                                while b"\x1b[6n" in pending:
                                    end = pending.index(b"\x1b[6n") + 4
                                    del pending[:end]
                                    os.write(master, b"\x1b[1;1R")
                                pending[:] = pending[-3:]

                    try:
                        deadline = time.monotonic() + 5
                        while not served.is_set() and time.monotonic() < deadline:
                            pump(0.02)
                        assert served.is_set(), "Synopsis request never started"
                        pump(0.35)
                        if depth == 8:
                            assert b"PARSER_PROBE_COMPLETE" in output, "Normal synopsis stopped rendering"
                        if stopping == "ctrl-c":
                            os.write(master, b"\x03")
                        else:
                            process.send_signal(stopping)
                        deadline = time.monotonic() + 3
                        while process.poll() is None and time.monotonic() < deadline:
                            pump(0.02)
                        assert process.poll() == 0, f"depth={depth}, stop={stopping}: parser blocked shutdown (exit={process.poll()})"
                        pump(0.05)
                        assert b"\x1b[?1049l" in output, "Alternate screen was not restored"
                        mask = termios.ICANON | termios.ECHO
                        assert termios.tcgetattr(slave)[3] & mask == before[3] & mask, "Terminal stayed in raw mode"
                        print(f"PASS: depth={depth} restores terminal and exits on {stopping}", flush=True)
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait(timeout=3)
                        os.close(master)
                        os.close(slave)

        depth = 100_000
        for stage, operation in (("search", "--list"), ("episodes", "--resolve")):
            with tempfile.TemporaryDirectory(prefix="miru-parser-cli-") as temporary:
                result = subprocess.run([
                    str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/",
                    "--state-dir", temporary, operation, "Probe",
                ], capture_output=True, text=True, timeout=5)
                assert result.returncode != 0 and "complexity limit" in result.stderr, result.stderr
                assert not result.stdout, "Rejected provider HTML produced partial JSON"
                print(f"PASS: excessive {stage} markup is rejected without partial output", flush=True)

        # This input fits both byte and markup budgets but duplicates a large
        # descendant description across many results. The process budget must
        # still prevent runaway CPU or memory use in parsing/result extraction.
        stage = "expensive-search"
        with tempfile.TemporaryDirectory(prefix="miru-parser-budget-") as temporary:
            result = subprocess.run([
                str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/",
                "--state-dir", temporary, "--list", "Probe",
            ], capture_output=True, text=True, timeout=5)
            assert result.returncode == 1, result.stderr
            assert "resource limits" in result.stderr or "time limit" in result.stderr, result.stderr
            assert not result.stdout, "A failed worker produced partial search output"
            print("PASS: expensive below-threshold HTML is bounded without crashing Miru", flush=True)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
