#!/usr/bin/env python3
"""Offline TUI regression: browsing cannot replace or cancel an active queue.

Uses a local provider, a controllable player, and a generated one-second clip.
Real mpv and ffmpeg integration is covered by smoke.py.
"""
import base64
import fcntl
import http.server
import json
import os
import pathlib
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import urllib.parse


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/ani-watch").resolve()
    with tempfile.TemporaryDirectory(prefix="ani-watch-navigation-") as directory:
        root = pathlib.Path(directory)
        resolving = threading.Event()
        release = threading.Event()
        requests = []
        media_requested = threading.Event()
        release_media = threading.Event()
        fail_refresh = threading.Event()
        refresh_failed = threading.Event()
        fail_episode_once = threading.Event()
        clip = root / "video.ts"
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                        "color=size=128x96:rate=12", "-t", "1", "-c:v", "libx264",
                        "-f", "mpegts", str(clip)], check=True, timeout=20)

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parsed = urllib.parse.urlparse(self.path)
                path = parsed.path
                base = f"http://127.0.0.1:{self.server.server_port}"
                requests.append(self.path)
                if path == "/video.ts":
                    media_requested.set()
                    release_media.wait(timeout=10)
                    data = clip.read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    try:
                        self.wfile.write(data)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    return
                if path == "/search":
                    body = '<div class="flw-item"><h3 class="film-name"><a title="Other Series" href="/other-2">Other Series</a></h3></div>'
                elif path == "/original-1":
                    body = '<div class="film-description"><div class="text">A complete synopsis beyond the search snippet.</div></div>'
                elif path == "/other-2":
                    body = '<div class="film-description"><div class="text">The second series has a complete synopsis beyond its search result.</div></div>'
                elif path.startswith("/api/theme/episode/list/"):
                    if fail_refresh.is_set():
                        self.send_response(503)
                        self.end_headers()
                        refresh_failed.set()
                        return
                    series = path.rsplit("/", 1)[-1]
                    body = json.dumps({"html": ''.join(
                        f'<a class="ep-item" data-number="{n}" data-id="{series}{n}" title="Part {n}" href="/watch/{"original-1" if series == "1" else "other-2"}?ep={series}{n}"></a>'
                        for n in (1, 2, 3))})
                elif path == "/api/theme/episode/servers":
                    episode = urllib.parse.parse_qs(parsed.query)["episodeId"][0]
                    if episode == "12" and fail_episode_once.is_set():
                        fail_episode_once.clear()
                        self.send_response(503)
                        self.end_headers()
                        return
                    if episode == "11":
                        resolving.set()
                        release.wait(timeout=10)
                    encoded = base64.b64encode(f"{base}/embed/{episode}".encode()).decode()
                    body = json.dumps({"html": f'<div class="server-item" data-type="sub" data-server-name="ZokoAnime" data-hash="{encoded}"></div>'})
                elif path.startswith("/embed/"):
                    episode = path.rsplit("/", 1)[-1]
                    config = json.dumps({"src": f"{base}/{episode}.m3u8"}).encode()
                    key = b"otaku-embed-v1"
                    encoded = base64.b64encode(bytes(b ^ key[i % len(key)] for i, b in enumerate(config))).decode()
                    body = f'<script>window.__P="{encoded}";</script>'
                elif path == "/media.m3u8":
                    body = '#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\nvideo.ts\n#EXT-X-ENDLIST\n'
                elif path.endswith(".m3u8"):
                    body = '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=1280x720\nmedia.m3u8\n'
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
                except (BrokenPipeError, ConnectionResetError):
                    pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        player = root / "player"
        player.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
with (root / "launches").open("a") as log:
    log.write(json.dumps({"pid": os.getpid(), "args": sys.argv[1:]}) + "\\n")
while not (root / str(os.getpid())).exists():
    time.sleep(0.03)
''')
        player.chmod(0o755)
        (root / "library.json").write_text(json.dumps({
            "version": 1, "mode": "sub", "quality": "best", "entries": [{
                "anime": {"id": "original-1", "title": "Original Series", "cover": None},
                "episode": "0", "mode": "sub", "watched_at": 1,
            }, {
                "anime": {"id": "history-2", "title": "Second Series", "cover": None},
                "episode": "0", "mode": "sub", "watched_at": 0,
            }],
        }))
        master, slave = os.openpty()
        os.set_blocking(master, False)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 960, 800))
        process = subprocess.Popen([
            str(binary), "--images", "off", "--no-animation", "--state-dir", str(root),
            "--cache-dir", str(root / "cache"), "--player", str(player),
            "--download-dir", str(root / "downloads"),
            "--provider", f"http://127.0.0.1:{server.server_port}/",
        ], stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        pending = bytearray()
        output = bytearray()

        def pump(seconds=0.15):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    try:
                        chunk = os.read(master, 65536)
                        pending.extend(chunk)
                        output.extend(chunk)
                    except (BlockingIOError, OSError):
                        continue
                    while b"\x1b[6n" in pending:
                        end = pending.index(b"\x1b[6n") + 4
                        del pending[:end]
                        os.write(master, b"\x1b[1;1R")
                    pending[:] = pending[-3:]

        def send(keys):
            os.write(master, keys)
            pump()

        def wait_for(predicate):
            deadline = time.monotonic() + 5
            while not predicate() and time.monotonic() < deadline:
                pump()
            log = root / "download.log"
            assert predicate(), "Timed out waiting for TUI action: " + (log.read_text() if log.exists() else "no download log")

        def copied_text(keys=b"y"):
            output.clear()
            send(keys)
            payloads = re.findall(rb"\x1b\]52;[^;]*;([A-Za-z0-9+/=]+)(?:\x07|\x1b\\)", output)
            assert payloads, "Copy should emit a clipboard payload"
            return base64.b64decode(payloads[-1]).decode()

        def launches():
            path = root / "launches"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        def alive(pid):
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False

        try:
            pump(0.5)
            wait_for(lambda: requests.count("/original-1") == 1)
            wait_for(lambda: "A complete synopsis beyond the search snippet." in copied_text())
            copied = copied_text(b"jy")
            assert "Second Series" in copied and "Original Series" not in copied, "Queued navigation+copy must use the newly selected panel"
            copied = copied_text(b"yk")
            assert "Second Series" in copied and "Original Series" not in copied, "Queued copy+navigation must copy before moving"
            send(b"i")
            copied = copied_text()
            assert all(text in copied for text in ["Original Series", "Synopsis", "A complete synopsis beyond the search snippet."])
            assert "F1 Help" not in copied and "Enter Episodes" not in copied
            send(b"\x1b")
            send(b"i")
            assert requests.count("/original-1") == 1, "Reopening details should reuse the full synopsis"
            send(b"\x1b[15~")  # F5 explicitly refreshes the synopsis.
            wait_for(lambda: requests.count("/original-1") == 2)
            send(b"\x1b")
            send(b"\r")  # Home -> episodes; last played 0 selects episode 1.
            wait_for(lambda: "/api/theme/episode/list/1" in requests)
            pump()
            send(b" \x1b[B \r")  # Mark episodes 1 and 2, then play the queue.
            wait_for(resolving.is_set)
            send(b"/other")
            pump(0.5)
            send(b"\r")  # Focus the search results without opening Info.
            wait_for(lambda: "The second series has a complete synopsis beyond its search result." in " ".join(copied_text().split()))
            assert requests.count("/other-2") == 1, "Search previews should fetch full synopses without opening Info"
            send(b"\r")  # Browse a different series while the first stream resolves.
            wait_for(lambda: "/api/theme/episode/list/2" in requests)
            release.set()
            wait_for(lambda: len(launches()) == 1)
            first = launches()[0]
            assert "--force-media-title=Original Series / Episode 1" in first["args"], first
            send(b"Hh/\x1b\x1b")  # Home, history, search, and back must leave the player alive.
            assert alive(first["pid"]), "Browsing stopped the player"
            send(b"DQ")  # New defaults cannot mutate the active queue.
            (root / str(first["pid"])).touch()
            wait_for(lambda: len(launches()) == 2)
            second = launches()[1]
            assert "--force-media-title=Original Series / Episode 2" in second["args"], second
            assert any("episodeId=12" in url for url in requests), requests
            assert not any("episodeId=2" in url for url in requests), requests
            send(b"x")
            wait_for(lambda: not alive(second["pid"]))
            history = json.loads((root / "library.json").read_text())["entries"]
            assert history[0]["anime"]["id"] == "original-1" and history[0]["episode"] == "2", history
            assert history[0]["mode"] == "sub", history
            send(b"/")
            send(b"\r\r")  # Open the other series from the retained search.
            pump()
            send(b"D \x1b[B d")  # SUB, mark two episodes, download.
            wait_for(media_requested.is_set)
            send(b"Hh/")
            release_media.set()
            wait_for(lambda: len(list((root / "downloads").glob("*.mkv"))) == 2)
            assert all("Other Series" in file.name for file in (root / "downloads").glob("*.mkv"))
            assert json.loads((root / "library.json").read_text())["entries"] == history
            send(b"\x1b")
            send(b"H\r")
            pump()
            fail_refresh.set()
            output.clear()
            send(b"\x1b[15~")  # F5: a failed refresh keeps the cached episodes usable.
            wait_for(refresh_failed.is_set)
            pump()
            fail_refresh.clear()
            fail_episode_once.set()
            output.clear()
            send(b":1-3\rd")
            wait_for(lambda: not fail_episode_once.is_set())
            pump()
            originals = list((root / "downloads").glob("Original*.mkv"))
            assert len(originals) == 1, originals
            saved = originals[0].stat()
            first_requests = sum("episodeId=11" in url for url in requests)
            # Retry while searching elsewhere, after changing the next queue's defaults.
            send(b"DQ/other")
            pump(0.5)
            send(b"\x1b[17~")  # F6 retries only the failed and pending episodes.
            wait_for(lambda: len(list((root / "downloads").glob("Original*.mkv"))) == 3)
            originals = list((root / "downloads").glob("Original*.mkv"))
            assert all(" SUB.mkv" in file.name for file in originals), originals
            assert sum("episodeId=11" in url for url in requests) == first_requests
            assert any("episodeId=13" in url for url in requests)
            assert (root / "downloads" / "Original Series _ Episode 1 SUB.mkv").stat().st_mtime_ns == saved.st_mtime_ns
            send(b"\x03")
            assert process.wait(timeout=3) == 0
            print("PASS: synopsis/cache/retry, navigation, queue ownership, fixed settings, history, stop, cached refresh failure, partial download recovery without repeating saved episodes")
        finally:
            release.set()
            release_media.set()
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                pump()
                process.wait(timeout=3)
            os.close(master)
            os.close(slave)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
