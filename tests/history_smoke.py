#!/usr/bin/env python3
"""Real mpv load failures must preserve history; loaded playback records immediately."""
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import time

from jobs_smoke import Fixture


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    with tempfile.TemporaryDirectory(prefix="miru-history-") as temporary:
        root = pathlib.Path(temporary)
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                        "color=size=64x64:rate=10:duration=30", "-c:v", "libx264",
                        "-f", "hls", "-hls_time", "1", "-hls_list_size", "0",
                        str(root / "video.m3u8")], check=True)
        class Provider(Fixture):
            def do_GET(self):
                file = root / self.path.lstrip("/")
                if self.server.available and file.is_file():
                    data = file.read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                else:
                    super().do_GET()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        server.available = False
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("MIRU_", "ANI_WATCH_"))}
        env["XDG_CONFIG_HOME"] = str(root / "config")
        state = root / "state"
        state.mkdir()
        library = state / "library.json"
        previous = {"version": 2, "mode": "sub", "quality": "best", "entries": [{
            "anime": {"id": "probe-1", "provider": "hianime", "title": "Jobs Probe", "cover": None},
            "episode": "0", "mode": "sub", "watched_at": 1}]}
        library.write_text(json.dumps(previous))
        original = library.read_bytes()
        common = [str(binary), "--source", "hianime", "--provider",
                  f"http://127.0.0.1:{server.server_port}/", "--state-dir", str(state),
                  "--sub", "--play", "Probe", "-e", "1", "--player", "mpv",
                  "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null",
                  "--player-arg=--ytdl=no"]
        try:
            for extra in [[], ["--player-arg=--miru-invalid-option"]]:
                failed = subprocess.run(common + extra, env=env, capture_output=True, text=True, timeout=20)
                assert failed.returncode != 0, failed
                assert library.read_bytes() == original, "Failed playback changed existing history"
            server.available = True
            process = subprocess.Popen(common, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if json.loads(library.read_text())["entries"][0]["episode"] == "1":
                        break
                    assert process.poll() is None, process.communicate()
                    time.sleep(0.05)
                assert json.loads(library.read_text())["entries"][0]["episode"] == "1", "Loaded media was not recorded"
                assert process.poll() is None, "History was only saved after exit"
            finally:
                if process.poll() is None:
                    process.terminate()
                process.communicate(timeout=10)
            assert json.loads(library.read_text())["entries"][0]["episode"] == "1"
            common[common.index("-e") + 1] = "2"
            completed = subprocess.run(common + ["--player-arg=--length=0.3"],
                                       env=env, capture_output=True, text=True, timeout=20)
            assert completed.returncode == 0, completed.stderr
            assert json.loads(library.read_text())["entries"][0]["episode"] == "2"
            print("PASS: failed loads preserve history; loaded, cancelled and completed playback record correctly")
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
