#!/usr/bin/env python3
"""Offline end-to-end smoke test with real HTTP, mpv, and ffmpeg.

Run after cargo build: python tests/smoke.py [path/to/miru]
Everything, including history and downloaded media, lives in a temporary directory.
"""
import argparse
import array
import base64
import functools
import hashlib
import html
import http.server
import json
import math
import os
import pathlib
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import threading
import urllib.parse
import wave


def run(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, text=True, timeout=40, **kwargs)


def error_log(stderr):
    match = re.search(r"[Ss]ee ([^\r\n]+)", stderr)
    assert match, f"Failure omitted the diagnostic path: {stderr}"
    return pathlib.Path(match[1])


def snapshot(path):
    metadata = path.stat()
    return hashlib.sha256(path.read_bytes()).digest(), metadata.st_ino, metadata.st_mtime_ns


def assert_tone(path, expected):
    """Distinguish the fixture's two tones after real decoding/muxing/playback."""
    with wave.open(str(path), "rb") as recording:
        assert recording.getnchannels() == 1 and recording.getsampwidth() == 2
        rate = recording.getframerate()
        samples = array.array("h", recording.readframes(recording.getnframes()))
    if sys.byteorder != "little":
        samples.byteswap()
    samples = samples[rate // 10:rate * 3 // 5]
    assert len(samples) >= rate // 4, f"Audio recording is incomplete: {path}"

    def energy(frequency):
        step = 2 * math.pi * frequency / rate
        real = sum(sample * math.cos(step * index) for index, sample in enumerate(samples))
        imaginary = sum(sample * math.sin(step * index) for index, sample in enumerate(samples))
        return real * real + imaginary * imaginary

    other = 880 if expected == 440 else 440
    assert energy(expected) > 10 * energy(other), f"Wrong audio group in {path}: expected {expected} Hz"


def check_optional_audio_default(common, root):
    master = root / "master.m3u8"
    original = master.read_text()
    try:
        cases = (
            ("yes", (("DEFAULT=YES,AUTOSELECT=YES", "audio.m3u8"),)),
            ("omitted", (("AUTOSELECT=YES", "audio.m3u8"),)),
            ("no", (("DEFAULT=NO,AUTOSELECT=YES", "audio.m3u8"),)),
            ("autoselect", (("AUTOSELECT=NO", "audio-low.m3u8"), ("AUTOSELECT=YES", "audio.m3u8"))),
            ("default-priority", (("AUTOSELECT=YES", "audio-low.m3u8"), ("DEFAULT=YES,AUTOSELECT=YES", "audio.m3u8"))),
            ("first-autoselect", (("AUTOSELECT=YES", "audio.m3u8"), ("AUTOSELECT=YES", "audio-low.m3u8"))),
        )
        for label, renditions in cases:
            audio = ''.join(
                f'#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="Track {index}",{flags},URI="{uri}"\n'
                for index, (flags, uri) in enumerate(renditions))
            master.write_text(
                '#EXTM3U\n' + audio +
                '#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=128x96,AUDIO="a"\nvideo.m3u8\n')
            destination = root / f"optional-default-{label}"
            run(common + ["--download", "Smoke", "--download-dir", str(destination)])
            media = next(destination.glob("*.mkv"))
            info = json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(media)]).stdout)
            kinds = {stream["codec_type"] for stream in info["streams"]}
            assert {"video", "audio"} <= kinds, f"Audio {label}: successful download contains {kinds}"
            decoded = root / f"optional-default-{label}.wav"
            run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(media),
                 "-vn", "-ac", "1", "-ar", "8000", "-c:a", "pcm_s16le", str(decoded)])
            assert_tone(decoded, 440)
            played = root / f"optional-default-{label}-playback.wav"
            run(common + ["--play", "Smoke", "--player-arg=--no-config", "--player-arg=--vo=null",
                          "--player-arg=--ao=pcm", "--player-arg=--audio-format=s16",
                          "--player-arg=--audio-channels=mono", f"--player-arg=--ao-pcm-file={played}"])
            assert_tone(played, 440)
            print(f"PASS: audio {label} retains expected audio in downloads and mpv playback", flush=True)
    finally:
        master.write_text(original)


def download_video(common, destination, quality, size):
    downloaded = subprocess.run(common + ["--download", "Smoke", "--quality", quality, "--download-dir", str(destination)],
                                capture_output=True, text=True, timeout=40)
    assert downloaded.returncode == 0, f"{quality}: {downloaded.stderr}\n{error_log(downloaded.stderr).read_text()}"
    media = next(destination.glob("*.mkv"))
    info = json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(media)]).stdout)
    video = next(stream for stream in info["streams"] if stream["codec_type"] == "video")
    assert (video["width"], video["height"]) == size, info
    return media


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/miru", type=pathlib.Path)
    parser.add_argument("--audio-fallback-only", action="store_true",
                        help="Check optional HLS DEFAULT flags using real downloads and playback")
    options = parser.parse_args()
    binary = options.binary.resolve()
    for command in ("ffmpeg", "ffprobe", "mpv"):
        if not shutil.which(command):
            raise SystemExit(f"Install {command} to run the media smoke test")
    with tempfile.TemporaryDirectory(prefix="miru-smoke-") as temporary:
        root = pathlib.Path(temporary)
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
             "testsrc=size=128x96:rate=12", "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
             "-f", "hls", "-hls_playlist_type", "vod", str(root / "video.m3u8")])
        for frequency, name in ((440, "audio"), (880, "audio-low")):
            run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                 f"sine=frequency={frequency}:sample_rate=44100", "-t", "1", "-c:a", "aac",
                 "-f", "hls", "-hls_playlist_type", "vod", str(root / f"{name}.m3u8")])
        (root / "subtitle.vtt").write_text("WEBVTT\n\n00:00.000 --> 00:00.800\nSmoke test subtitle\n")
        (root / "master.m3u8").write_text(
            '#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",DEFAULT=YES,URI="audio.m3u8"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=128x96,AUDIO="a"\nvideo.m3u8\n')

        slow = threading.Event()
        segment_requested = threading.Event()
        release_segment = threading.Event()
        series = {"id": "smoke-1", "title": "Smoke & Test"}
        dub_available = threading.Event()

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
                    body = f'<div class="film_list-wrap"><div class="flw-item"><h3 class="film-name"><a title="{html.escape(series["title"])}" href="/{series["id"]}">Smoke</a></h3></div></div>'
                elif path == f'/api/theme/episode/list/{series["id"].rsplit("-", 1)[1]}':
                    body = json.dumps({"html": ''.join(
                        f'<a class="ep-item" data-number="{i}" data-id="{i}" title="Part {i}" href="/watch/{series["id"]}?ep={i}"></a>' for i in (1, 2))})
                elif path == "/api/theme/episode/servers":
                    encoded = base64.b64encode(f"{base}/embed".encode()).decode()
                    modes = ("sub", "dub") if dub_available.is_set() else ("sub",)
                    body = json.dumps({"html": ''.join(f'<div class="server-item" data-type="{mode}" data-server-name="ZokoAnime" data-hash="{encoded}"></div>' for mode in modes)})
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
            check_optional_audio_default(common, root)
            if options.audio_fallback_only:
                return
            found = json.loads(run(common + ["--list", "--exact", "  SMOKE  &  Test  "]).stdout)
            assert found[0]["id"] == "smoke-1"
            streams = [json.loads(line) for line in run(common + ["--resolve", "Smoke", "-e", "0--1"]).stdout.splitlines()]
            assert len(streams) == 2 and streams[0]["audio"] and streams[0]["subtitle"]
            played = run(common + ["--play", "Smoke", "-e", "1-2", "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"])
            assert not played.stdout and not played.stderr, "Successful playback leaked player output"
            invalid = subprocess.run(common + ["--play", "Smoke", "-e", "2", "--player-arg=--no-config",
                                               "--player-arg=--miru-review-invalid-option"],
                                     capture_output=True, text=True, timeout=10)
            assert invalid.returncode != 0, invalid
            log = error_log(invalid.stderr)
            assert "Error parsing option miru-review-invalid-option" in log.read_text(), "Missing mpv startup diagnostic"
            assert not invalid.stdout and "Error parsing option" not in invalid.stderr, "Player diagnostics leaked into CLI output"
            for path, backend in (("vlc-backup with spaces/mpv", None), ("wrapper names/my-vlc-wrapper", None),
                                  ("literal $(touch marker) vlc/mpv", None), ("overrides/vlc", "mpv")):
                alias = root / path
                alias.parent.mkdir(parents=True, exist_ok=True)
                alias.symlink_to(pathlib.Path(shutil.which("mpv")).resolve())
                arguments = common + ["--play", "Smoke", "-e", "2", "--player", str(alias),
                                      "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"]
                if backend:
                    arguments += ["--player-backend", backend]
                played = subprocess.run(arguments, capture_output=True, text=True, timeout=40, cwd=root)
                assert played.returncode == 0, f"{played.stderr}\n{error_log(played.stderr).read_text()}"
                assert not (root / "marker").exists(), "Player path was evaluated by a shell"
            wrapper_dir = root / "custom vlc wrappers with spaces"
            wrapper_dir.mkdir()
            wrapper = wrapper_dir / "custom-wrapper"
            wrapper.write_text(f'#!{sys.executable}\nimport json, pathlib, sys\npathlib.Path(__file__).with_name("arguments.json").write_text(json.dumps(sys.argv[1:]))\n')
            wrapper.chmod(0o700)
            literal = "--custom=space ; $(touch marker)"
            for name, override, backend in (("vlc", None, "vlc"), ("cvlc", None, "vlc"),
                                            ("custom-wrapper", None, "mpv"), ("custom-wrapper", "vlc", "vlc")):
                program = wrapper_dir / name
                if program != wrapper:
                    program.symlink_to(wrapper)
                arguments = common + ["--play", "Smoke", "-e", "2", "--player", str(program), f"--player-arg={literal}"]
                if override:
                    arguments += ["--player-backend", override]
                run(arguments, cwd=root)
                recorded = json.loads((wrapper_dir / "arguments.json").read_text())
                expected = "--play-and-exit" if backend == "vlc" else "--input-terminal=no"
                excluded = "--input-terminal=no" if backend == "vlc" else "--play-and-exit"
                assert expected in recorded and excluded not in recorded, recorded
                assert literal in recorded and any(argument.startswith("--http-proxy=") for argument in recorded), recorded
                assert not (root / "marker").exists(), "Player argument was evaluated by a shell"
            history = json.loads(run(common + ["--history"]).stdout)
            assert len(history) == 1 and history[0]["episode"] == "2"
            missing_player = subprocess.run(common + ["--play", "Smoke", "--player", str(root / "missing-player")], capture_output=True, text=True, timeout=10)
            assert missing_player.returncode != 0, "Missing player unexpectedly started"
            assert json.loads(run(common + ["--history"]).stdout) == history, "Failed media startup changed history"
            download_args = common + ["--download", "Smoke", "-e", "1", "--download-dir", str(root / "downloads")]
            run(download_args)
            media = next((root / "downloads").glob("*.mkv"))
            info = json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(media)]).stdout)
            assert {s["codec_type"] for s in info["streams"]} == {"audio", "video", "subtitle"}
            before = snapshot(media)
            repeated = subprocess.run(download_args, capture_output=True, text=True, timeout=40)
            assert repeated.returncode != 0 and "already exists" in repeated.stderr
            assert snapshot(media) == before
            assert not list((root / "downloads").glob("*.part"))
            missing = subprocess.run(common + ["--resolve", "--dub", "Smoke"], capture_output=True, text=True, timeout=40)
            assert missing.returncode != 0 and "No DUB source" in missing.stderr
            assert json.loads(run(common + ["--history"]).stdout) == history
            long_dir = root / "long-downloads"
            dub_available.set()
            saved = {}
            for source, title in (("smoke-1", "Long Title " * 20),
                                  ("other-2", "Long Title " * 20 + "Different Series"),
                                  ("unicode-3", "日本語🎬" * 40)):
                series.update(id=source, title=title)
                for mode in ("sub", "dub"):
                    previous = set(long_dir.glob("*.mkv"))
                    long_args = common + ["--download", "Smoke", "-e", "1-2", f"--{mode}", "--download-dir", str(long_dir)]
                    downloaded = subprocess.run(long_args, capture_output=True, text=True, timeout=40)
                    assert downloaded.returncode == 0, downloaded.stderr
                    added = set(long_dir.glob("*.mkv")) - previous
                    assert len(added) == 2, added
                    for episode in (1, 2):
                        assert any(file.name.endswith(f" Episode {episode} {mode.upper()}.mkv") for file in added), added
                    for file in added:
                        assert len(file.stem.encode("utf-8")) <= 180, file.name
                        saved[file] = snapshot(file)
            repeated = subprocess.run(long_args, capture_output=True, text=True, timeout=40)
            assert repeated.returncode != 0 and "already exists" in repeated.stderr, repeated.stderr
            assert set(long_dir.iterdir()) == set(saved), "A repeat renamed files or left partial media"
            assert all(snapshot(file) == original for file, original in saved.items()), "Existing downloads changed"
            series.update(id="smoke-1", title="Smoke & Test")
            dub_available.clear()
            # The two video qualities deliberately use different default audio groups.
            run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                 "testsrc=size=64x48:rate=12", "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
                 "-f", "hls", "-hls_playlist_type", "vod", str(root / "video-low.m3u8")])
            master = root / "master.m3u8"
            original_master = master.read_text()
            master.write_text(
                '#EXTM3U\n'
                '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a1",NAME="First",DEFAULT=YES,AUTOSELECT=YES,URI="audio.m3u8"\n'
                '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a2",NAME="Second",DEFAULT=YES,AUTOSELECT=YES,URI="audio-low.m3u8"\n'
                '#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=128x96,AUDIO="a1"\nvideo.m3u8\n'
                '#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=64x48,AUDIO="a2"\nvideo-low.m3u8\n'
                '#EXT-X-STREAM-INF:BANDWIDTH=65000,CODECS="mp4a.40.2",AUDIO="a1"\naudio.m3u8\n')
            resolved = json.loads(run(common + ["--resolve", "Smoke"]).stdout)
            base = f"http://127.0.0.1:{server.server_port}/"
            assert resolved["audio"] == base + "audio.m3u8", resolved
            assert resolved["variants"][0]["audio"] == {"group": "a1", "url": base + "audio.m3u8"}
            assert resolved["variants"][1]["audio"] == {"group": "a2", "url": base + "audio-low.m3u8"}
            assert len(resolved["variants"]) == 2, "Stream export included an audio-only video candidate"
            for quality, tone, size in (("best", 440, (128, 96)), ("worst", 880, (64, 48)),
                                        ("96p", 440, (128, 96)), ("48p", 880, (64, 48))):
                destination = root / f"grouped-{quality}"
                media = download_video(common, destination, quality, size)
                decoded = root / f"download-{quality}.wav"
                run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(media),
                     "-vn", "-ac", "1", "-ar", "8000", "-c:a", "pcm_s16le", str(decoded)])
                assert_tone(decoded, tone)
                played = root / f"playback-{quality}.wav"
                run(common + ["--play", "Smoke", "--quality", quality, "--player-arg=--no-config",
                              "--player-arg=--vo=null", "--player-arg=--ao=pcm", "--player-arg=--audio-format=s16",
                              "--player-arg=--audio-channels=mono", f"--player-arg=--ao-pcm-file={played}"])
                assert_tone(played, tone)
            unknown_sources = []
            for label, codecs in (("video-codecs", ',CODECS="avc1.4d401f,mp4a.40.2"'), ("no-codecs", "")):
                page = (
                    '#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="Main",DEFAULT=YES,URI="audio.m3u8"\n'
                    f'#EXT-X-STREAM-INF:BANDWIDTH=200000,AUDIO="a"{codecs}\nvideo.m3u8\n')
                unknown_sources.append((label, page))
            unknown_sources.append(("direct-media", (root / "video.m3u8").read_text()))
            for label, page in unknown_sources:
                master.write_text(page)
                exported = json.loads(run(common + ["--resolve", "Smoke"]).stdout)
                assert len(exported["variants"]) == 1 and exported["variants"][0]["height"] == 0, exported
                download_video(common, root / f"unknown-{label}", "worst", (128, 96))
            before_rejected = run(common + ["--history"]).stdout
            master.write_text('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=65000,CODECS="mp4a.40.2"\naudio.m3u8\n')
            unsupported_dir = root / "audio-only"
            for operation in ("--resolve", "--play", "--download"):
                failed = subprocess.run(common + [operation, "Smoke", "--quality", "worst", "--download-dir", str(unsupported_dir),
                                                "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null"],
                                        capture_output=True, text=True, timeout=10)
                assert failed.returncode != 0 and "audio-only" in failed.stderr.lower(), failed.stderr
            assert run(common + ["--history"]).stdout == before_rejected, "Unsupported playback changed history"
            assert not unsupported_dir.exists(), "Unsupported source left a download directory or partial media"
            master.write_text(original_master)
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
            assert "not on whitelist" in error_log(denied.stderr).read_text()
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
                        cancel_dir.mkdir()
                        untouched = {}
                        for name in ("existing.mkv", ".miru-other-job.part"):
                            file = cancel_dir / name
                            file.write_bytes(b"unrelated download")
                            untouched[file] = snapshot(file)
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
                            assert set(cancel_dir.iterdir()) == set(untouched), "Cancellation left partial media or removed unrelated files"
                            assert all(snapshot(file) == original for file, original in untouched.items()), "Cancellation changed unrelated files"
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
            print("PASS: INT/TERM/HUP cleanup for playback and downloads, HTTP provider, exact search, ranges, mpv, history, ffmpeg audio/subtitle mux, encrypted HLS, protocol restrictions, no-clobber, missing DUB, long UTF-8 filenames, distinct episodes/languages and shared prefixes, per-quality audio identity, video selection in mixed masters, unknown resolutions and direct media, clear audio-only rejection")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    main()
