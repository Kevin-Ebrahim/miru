#!/usr/bin/env python3
"""Real mpv load failures must preserve history; loaded playback records immediately."""
import http.server
import json
import os
import pathlib
import socket
import shutil
import shlex
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
        loading = threading.Event()
        release = threading.Event()
        class Provider(Fixture):
            def do_GET(self):
                if self.server.delay and self.path == "/video.m3u8":
                    loading.set()
                    release.wait(10)
                    self.send_response(404)
                    self.end_headers()
                    return
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
        server.delay = True
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
            ipc = root / "mpv.sock"
            queue = common.copy()
            queue[queue.index("-e") + 1] = "1-2"
            process = subprocess.Popen(queue + [f"--player-arg=--input-ipc-server={ipc}"],
                                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                assert loading.wait(8), "mpv never requested the delayed playlist"
                with socket.socket(socket.AF_UNIX) as client:
                    client.connect(str(ipc))
                    client.sendall(b'{"command":["quit"]}\n')
                _, stderr = process.communicate(timeout=8)
                assert library.read_bytes() == original, "Quitting before playback advanced history"
                assert process.returncode != 0, "Unstarted playback advanced the queue"
                assert "before playback started" in stderr.decode(), stderr
            finally:
                release.set()
                if process.poll() is None:
                    process.terminate()
                    process.communicate(timeout=5)
            server.delay = False
            print("PASS: closing mpv before playback preserves history and stops the queue", flush=True)
            for extra in [[], ["--player-arg=--miru-invalid-option"]]:
                failed = subprocess.run(common + extra, env=env, capture_output=True, text=True, timeout=20)
                assert failed.returncode != 0, failed
                assert library.read_bytes() == original, "Failed playback changed existing history"
            wrapper = root / "traced-player"
            wrapper.write_text('#!/bin/sh\nset -x\nexec ' + shlex.quote(shutil.which("mpv")) + ' "$@"\n')
            wrapper.chmod(0o700)
            traced = common.copy()
            traced[traced.index("--player") + 1] = str(wrapper)
            failed = subprocess.run(traced, env=env, capture_output=True, text=True, timeout=20)
            assert failed.returncode != 0, failed.stderr
            assert library.read_bytes() == original, "Echoed player arguments advanced history"
            print("PASS: traced wrapper arguments do not count as playback", flush=True)
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
            for quiet in [[], ["--player-arg=--really-quiet"], ["--player-arg=--msg-level=all=no"],
                          ["--player-arg=--msg-module=yes", "--player-arg=--msg-time=yes",
                           "--player-arg=--msg-color=yes"]]:
                library.write_bytes(original)
                completed = subprocess.run(common + ["--player-arg=--length=0.3"] + quiet,
                                           env=env, capture_output=True, text=True, timeout=20)
                assert completed.returncode == 0, completed.stderr
                assert json.loads(library.read_text())["entries"][0]["episode"] == "2"
            print("PASS: failed loads preserve history; loaded, cancelled and completed playback record correctly")
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
