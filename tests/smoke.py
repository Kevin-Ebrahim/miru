#!/usr/bin/env python3
"""Offline end-to-end smoke test with real HTTP, mpv, and ffmpeg.

Run after cargo build: python tests/smoke.py [path/to/miru]
Everything, including history and downloaded media, lives in a temporary directory.
"""
import base64
import functools
import hashlib
import http.server
import json
import os
import pathlib
import signal
import shutil
import subprocess
import sys
import tempfile
import threading
import urllib.parse


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, text=True, timeout=40, **kwargs)


def check_download_destinations(common, root):
    home = root / "download-home"
    config_home = home / "config"
    config_home.mkdir(parents=True)
    config = config_home / "miru/config.json"
    working = root / "working"
    working.mkdir()
    env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(config_home)}
    download = common + ["--download", "Smoke"]

    def saved_in(folder, args=()):
        result = run(download + list(args), cwd=working, env=env)
        media = list(folder.glob("*.mkv"))
        assert media and all(file.stat().st_size > 0 for file in media), result.stdout
        return result

    run(common + ["--list", "Smoke"], cwd=working, env=env)
    assert not (home / "Downloads").exists(), "Browsing created a download directory"
    assert not config.exists(), "Browsing wrote a config file"
    saved_in(home / "Downloads/miru")
    assert not list(working.iterdir()), "Default downloads leaked into the working directory"

    relocated = home / "Téléchargements 日本語"
    (config_home / "user-dirs.dirs").write_text('XDG_DOWNLOAD_DIR="$HOME/Téléchargements 日本語"\n')
    saved_in(relocated / "miru")
    config.parent.mkdir()
    config.write_text("{}")
    saved_in(relocated / "miru", ["-e", "2"])

    config.write_text(json.dumps({"download_dir": "~/Anime library/日本語"}))
    original = config.read_bytes()
    custom = home / "Anime library/日本語"
    saved_in(custom)
    saved_in(working / "one-run", ["--download-dir", "./one-run"])
    assert config.read_bytes() == original, "A one-run override changed the saved preference"
    elsewhere = root / "another-working-directory"
    elsewhere.mkdir()
    run(download + ["-e", "2"], cwd=elsewhere, env=env)
    assert len(list(custom.glob("*.mkv"))) == 2, "The saved default was not reused on the next run"
    assert not list(elsewhere.iterdir()), "The saved path changed with the working directory"
    saved_in(working, ["--download-dir", "."])
    assert config.read_bytes() == original

    config.write_text("{invalid json")
    bad = subprocess.run(download, cwd=working, env=env, text=True, capture_output=True, timeout=10)
    assert bad.returncode != 0 and str(config) in bad.stderr, bad.stderr
    saved_in(root / "override-invalid-config", ["--download-dir", str(root / "override-invalid-config")])
    assert config.read_text() == "{invalid json", "Invalid config was overwritten"
    blocked = home / "blocked"
    blocked.write_text("This is a file")
    config.write_text(json.dumps({"download_dir": str(blocked)}))
    bad = subprocess.run(download, cwd=working, env=env, text=True, capture_output=True, timeout=10)
    assert bad.returncode != 0 and "Could not create download directory" in bad.stderr, bad.stderr
    assert blocked.read_text() == "This is a file"
    print("PASS: automatic/fallback/XDG download folders, saved config, tilde/Unicode paths, one-run overrides, next-run persistence, invalid/unusable destinations")


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    for command in ("ffmpeg", "ffprobe", "mpv"):
        if not shutil.which(command):
            raise SystemExit(f"Install {command} to run the media smoke test")
    with tempfile.TemporaryDirectory(prefix="miru-smoke-") as temporary:
        root = pathlib.Path(temporary)
        os.environ["XDG_CONFIG_HOME"] = str(root / "isolated-config")
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
             "testsrc=size=128x96:rate=12", "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
             "-f", "hls", "-hls_playlist_type", "vod", str(root / "video.m3u8")])
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
             "sine=frequency=440:sample_rate=44100", "-t", "1", "-c:a", "aac",
             "-f", "hls", "-hls_playlist_type", "vod", str(root / "audio.m3u8")])
        (root / "subtitle.vtt").write_text("WEBVTT\n\n00:00.000 --> 00:00.800\nSmoke test subtitle\n")
        (root / "master.m3u8").write_text(
            '#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",DEFAULT=YES,URI="audio.m3u8"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=128x96,AUDIO="a"\nvideo.m3u8\n')

        slow = threading.Event()
        segment_requested = threading.Event()
        release_segment = threading.Event()

        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                path = urllib.parse.urlparse(self.path).path
                if path.endswith(".ts") and slow.is_set():
                    segment_requested.set()
                    release_segment.wait(timeout=10)
                base = f"http://127.0.0.1:{self.server.server_port}"
                if path == "/search":
                    body = '<div class="film_list-wrap"><div class="flw-item"><h3 class="film-name"><a title="Smoke &amp; Test" href="/smoke-1">Smoke</a></h3></div></div>'
                elif path == "/api/theme/episode/list/1":
                    body = json.dumps({"html": ''.join(
                        f'<a class="ep-item" data-number="{i}" data-id="{i}" title="Part {i}" href="/watch/smoke-1?ep={i}"></a>' for i in (1, 2))})
                elif path == "/api/theme/episode/servers":
                    encoded = base64.b64encode(f"{base}/embed".encode()).decode()
                    body = json.dumps({"html": f'<div class="server-item" data-type="sub" data-server-name="ZokoAnime" data-hash="{encoded}"></div>'})
                elif path == "/embed":
                    config = json.dumps({"src": f"{base}/master.m3u8", "subtitles": [{"default": True, "src": f"{base}/subtitle.vtt"}]}).encode()
                    key = b"otaku-embed-v1"
                    blob = base64.b64encode(bytes(b ^ key[i % len(key)] for i, b in enumerate(config))).decode()
                    body = f'<script>window.__P="{blob}";</script>'
                else:
                    try:
                        return super().do_GET()
                    except (BrokenPipeError, ConnectionResetError):
                        return
                data = body.encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(root)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        common = [str(binary), "--provider", f"http://127.0.0.1:{server.server_port}/", "--state-dir", str(root / "state")]
        try:
            found = json.loads(run(common + ["--list", "--exact", "  SMOKE  &  Test  "]).stdout)
            assert found[0]["id"] == "smoke-1"
            streams = [json.loads(line) for line in run(common + ["--resolve", "Smoke", "-e", "0--1"]).stdout.splitlines()]
            assert len(streams) == 2 and streams[0]["audio"] and streams[0]["subtitle"]
            run(common + ["--play", "Smoke", "-e", "1-2", "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"])
            history = json.loads(run(common + ["--history"]).stdout)
            assert len(history) == 1 and history[0]["episode"] == "2"
            download_args = common + ["--download", "Smoke", "-e", "1", "--download-dir", str(root / "downloads")]
            run(download_args)
            media = next((root / "downloads").glob("*.mkv"))
            info = json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(media)]).stdout)
            assert {s["codec_type"] for s in info["streams"]} == {"audio", "video", "subtitle"}
            before = hashlib.sha256(media.read_bytes()).digest()
            repeated = subprocess.run(download_args, capture_output=True, text=True, timeout=40)
            assert repeated.returncode != 0 and "already exists" in repeated.stderr
            assert hashlib.sha256(media.read_bytes()).digest() == before
            assert not list((root / "downloads").glob("*.part"))
            missing = subprocess.run(common + ["--resolve", "--dub", "Smoke"], capture_output=True, text=True, timeout=40)
            assert missing.returncode != 0 and "No DUB source" in missing.stderr
            assert json.loads(run(common + ["--history"]).stdout) == history
            check_download_destinations(common, root)
            # Check nested protocol restrictions without breaking encrypted HLS.
            key = root / "encrypt.key"
            key.write_bytes(os.urandom(16))
            key_info = root / "key-info"
            key_info.write_text(f"encrypt.key\n{key}\n")
            run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                 "color=size=128x96:rate=12", "-t", "1", "-c:v", "libx264", "-f", "hls",
                 "-hls_key_info_file", str(key_info), str(root / "encrypted.m3u8")])
            playlist = root / "video.m3u8"
            original_playlist = playlist.read_bytes()
            playlist.write_bytes((root / "encrypted.m3u8").read_bytes())
            run(common + ["--download", "Smoke", "--download-dir", str(root / "encrypted")])
            run(common + ["--play", "Smoke", "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"])
            playlist.write_text("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n" + (root / "video0.ts").as_uri() + "\n#EXT-X-ENDLIST\n")
            denied = subprocess.run(common + ["--download", "Smoke", "--download-dir", str(root / "denied")], capture_output=True, text=True, timeout=10)
            assert denied.returncode != 0, "Remote HLS was allowed to read a local file"
            assert "not on whitelist" in (root / "state/download.log").read_text()
            assert not list((root / "denied").iterdir()), "Rejected HLS left partial media"
            playlist.write_bytes(original_playlist)
            slow.set()
            for operation in ("--download", "--play"):
                for stop_signal in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                    segment_requested.clear()
                    release_segment.clear()
                    cancel_dir = root / f"cancelled-{operation[2:]}-{stop_signal}"
                    args = [operation, "Smoke"]
                    if operation == "--download":
                        args += ["--download-dir", str(cancel_dir)]
                    else:
                        args += ["--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"]
                    cancelling = subprocess.Popen(common + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    children = []
                    try:
                        assert segment_requested.wait(timeout=10), "Job never reached media transfer"
                        children = run(["ps", "-o", "pid=", "--ppid", str(cancelling.pid)]).stdout.split()
                        assert children, "Media child was not running"
                        cancelling.send_signal(stop_signal)
                        _, stderr = cancelling.communicate(timeout=5)
                        assert cancelling.returncode == 0, stderr
                        if operation == "--download":
                            assert "cancelled" in stderr
                            assert not list(cancel_dir.iterdir()), "Cancellation left partial media behind"
                        assert not any(pathlib.Path("/proc", pid).exists() for pid in children), "Cancellation did not reap its child"
                    finally:
                        release_segment.set()
                        if cancelling.poll() is None:
                            cancelling.kill()
                            cancelling.wait()
                        for pid in children:
                            try:
                                os.kill(int(pid), signal.SIGKILL)
                            except ProcessLookupError:
                                pass
            print("PASS: INT/TERM/HUP cleanup for playback and downloads, HTTP provider, exact search, ranges, mpv, history, ffmpeg audio/subtitle mux, encrypted HLS, protocol restrictions, no-clobber, missing DUB")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    main()
