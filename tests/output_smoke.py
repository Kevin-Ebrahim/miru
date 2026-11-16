#!/usr/bin/env python3
"""Offline CLI output checks for closed consumers and real write failures."""
import argparse
import http.server
import json
import os
import pathlib
import select
import subprocess
import tempfile
import threading

from jobs_smoke import Fixture


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
        with tempfile.TemporaryDirectory(prefix="miru-output-") as temporary:
            root = pathlib.Path(temporary)
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("MIRU_", "ANI_WATCH_"))}
            entries = [{"anime": {"provider": "hianime", "id": f"show-{i}", "title": "日本語 " + "x" * 4096,
                                  "cover": None, "description": "", "info": ""},
                        "episode": "1", "mode": "sub", "watched_at": i} for i in range(500)]
            source = root / "ani-hsts"
            source.write_text("1\tshow-1\tImported show\n")
            source_bytes = source.read_bytes()

            def invoke(command, stdout=subprocess.PIPE):
                requests.clear()
                return subprocess.run(command, env=environment, stdin=subprocess.DEVNULL,
                                      stdout=stdout, stderr=subprocess.PIPE, text=True, timeout=15)

            for operation in ("history", "list", "resolve", "import"):
                state = root / operation
                state.mkdir()
                (state / "library.json").write_text(json.dumps({
                    "version": 2, "mode": "sub", "quality": "best", "entries": entries if operation == "history" else []
                }, ensure_ascii=False))
                command = [str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/", "--state-dir", str(state)]
                command += {"history": ["--history"], "list": ["--list", "Probe"],
                            "resolve": ["--resolve", "Probe", "-e", "1-2"],
                            "import": ["--import-history", str(source)]}[operation]
                normal = invoke(command)
                assert normal.returncode == 0 and not normal.stderr, normal.stderr
                if operation == "history":
                    assert json.loads(normal.stdout) == entries
                elif operation == "list":
                    assert json.loads(normal.stdout)[0]["title"] == "Jobs Probe"
                elif operation == "resolve":
                    lines = normal.stdout.splitlines()
                    assert len(lines) == 2 and all(json.loads(line)["variants"] for line in lines)
                else:
                    assert normal.stdout == "History now contains 1 entries. Source file unchanged.\n"

                reader, writer = os.pipe()
                os.close(reader)
                try:
                    closed = invoke(command, stdout=writer)
                finally:
                    os.close(writer)
                assert closed.returncode == 0 and not closed.stderr, f"{operation}: exit={closed.returncode}, stderr={closed.stderr}"
                if operation == "resolve":
                    assert sum(path.startswith("/api/theme/episode/servers") for path in requests) == 1, "Closed output did not stop further resolutions"
                print(f"PASS: {operation} quietly stops when stdout is closed", flush=True)

                with open("/dev/full", "wb") as full:
                    failed = invoke(command, stdout=full)
                assert failed.returncode != 0 and failed.returncode != 101, (operation, failed.stderr)
                assert "Could not write command output" in failed.stderr and "panicked" not in failed.stderr, failed.stderr
                if operation == "import":
                    assert len(json.loads((state / "library.json").read_text())["entries"]) == 1
                    assert source.read_bytes() == source_bytes, "Output failure modified the import source"
                print(f"PASS: {operation} reports non-pipe output errors", flush=True)

            command = [str(binary), "--state-dir", str(root / "history"), "--history"]
            process = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                assert select.select([process.stdout], [], [], 5)[0], "History output never started"
                assert process.stdout.read(1), "History did not write its prefix"
                process.stdout.close()
                process.stdout = None
                _, stderr = process.communicate(timeout=10)
                assert process.returncode == 0 and not stderr, (process.returncode, stderr)
                print("PASS: history stops quietly when its consumer closes after a prefix", flush=True)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=3)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
