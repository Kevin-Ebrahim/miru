#!/usr/bin/env python3
"""KAA catalogue, language selection and protected media integration against local fixtures."""
import html
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import urllib.parse

def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    with tempfile.TemporaryDirectory(prefix="miru-kaa-") as tmp:
        root = pathlib.Path(tmp)
        for name, frequency in [("en", 440), ("ja", 880)]:
            subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                f"sine=frequency={frequency}:duration=6", "-c:a", "aac", "-f", "hls",
                "-hls_time", "1", "-hls_list_size", "0", str(root / f"{name}.m3u8")], check=True)
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
            "color=size=64x64:rate=10:duration=6", "-an", "-c:v", "libx264", "-g", "10",
            "-f", "hls", "-hls_time", "1", "-hls_list_size", "0", str(root / "video.m3u8")], check=True)
        (root / "master.m3u8").write_text(
            '#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="English",LANGUAGE="English",URI="en.m3u8"\n'
            '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="Japanese",LANGUAGE="Japanese",URI="ja.m3u8"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=100000,RESOLUTION=64x64,AUDIO="a"\nvideo.m3u8\n'
        )
        (root / "sub.vtt").write_text("WEBVTT\n\n00:00.000 --> 00:01.000\nHello\n")
        requests = []
        media_failure = [False]
        final_failure = [False]
        faults = {"repeat": False, "wrong": False, "no_servers": False, "movie": False, "empty": False, "chinese": False, "unavailable": False}
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def reply(self, data, status=200):
                if not isinstance(data, bytes): data = json.dumps(data).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                assert body["query"] == "Probe" and body["page"] in (1, 2)
                self.reply({"maxPage": 2, "result": [{"slug": "probe", "title": "Probe", "synopsis": "Story",
                    "poster": {"hq": "probe-hq"}}] + ([{"slug": "second", "title": "Second"}] if body["page"] == 2 else [])})
            def do_GET(self):
                path = urllib.parse.urlparse(self.path)
                query = urllib.parse.parse_qs(path.query)
                requests.append((self.path, self.headers.get("Referer"), self.headers.get("User-Agent")))
                if path.path == "/api/show/probe":
                    self.reply({"slug": "probe", "title": "Probe", "synopsis": "Story", "locales": [] if faults["unavailable"] else ["zh-CN"]})
                elif path.path == "/api/show/probe/episodes":
                    if faults["empty"] or faults["unavailable"] or (faults["chinese"] and query["lang"] == ["ja-JP"]):
                        return self.reply({"current_page": 0, "pages": []})
                    if faults["movie"]:
                        data = {"current_page": 1 if query["ep"] == ["0"] else 0,
                            "pages": [{"number": 1, "eps": ["0"]}]}
                        if query["ep"] == ["0"]:
                            data["result"] = [{"slug": "ja-JP", "episode_string": "0"}]
                        return self.reply(data)
                    first = query["ep"] == ["1"]
                    lang = query["lang"][0]
                    number = "1" if first else "1.5"
                    self.reply({"current_page": 1 if first else 2, "pages": [{"number": 1, "eps": [1]}, {"number": 2, "eps": [1 if faults["repeat"] else 1.5]}],
                        "result": [{"slug": lang, "episode_string": number, "title": "Episode"}]})
                elif path.path.startswith("/api/show/probe/episode/"):
                    route = path.path.rsplit("/", 1)[-1]
                    number = "0" if route.startswith("ep-0-") else "1.5" if route.startswith("ep-1.5-") else "1"
                    lang = "en-US" if route.endswith("en-US") else "zh-CN" if route.endswith("zh-CN") else "ja-JP"
                    self.reply({"show_slug": "wrong" if faults["wrong"] else "probe",
                        "slug": lang, "episode_string": number, "language": lang,
                        "servers": [] if lang == "en-US" or faults["no_servers"] else [
                            {"src": base + "cat-player/player?id=test"}]})
                elif path.path == "/cat-player/player":
                    props = {"manifest": [0, base + "master.m3u8"], "subtitles": [1, [
                        [0, {"language": [0, "en"], "src": [0, base + "sub.vtt"]}]]]}
                    self.reply(('<astro-island props="' + html.escape(json.dumps(props), quote=True) + '"></astro-island>').encode())
                else:
                    if self.headers.get("Referer") != base:
                        return self.reply(b"Missing referer", 403)
                    if not self.headers.get("User-Agent", "").startswith("Mozilla/"):
                        return self.reply(b"Missing user agent", 403)
                    if path.path != "/master.m3u8" and self.headers.get("Origin") != base.rstrip("/"):
                        return self.reply(b"Missing origin", 403)
                    if final_failure[0] and path.path == "/video5.ts":
                        return self.reply(b"Final segment unavailable", 403)
                    if media_failure[0] and path.path.endswith(".ts"):
                        return self.reply(b"Segment unavailable", 403)
                    if faults["chinese"] and path.path == "/master.m3u8":
                        return self.reply((root / "master.m3u8").read_bytes().replace(b'LANGUAGE="Japanese"', b'LANGUAGE="zho"'))
                    file = root / path.path.lstrip("/")
                    self.reply(file.read_bytes() if file.is_file() else b"missing",
                        200 if file.is_file() else 404)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        base = f"http://127.0.0.1:{server.server_port}/"
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env = {k: v for k, v in os.environ.items() if not k.startswith(("MIRU_", "ANI_WATCH_"))}
        env["XDG_CONFIG_HOME"] = str(root / "config")
        common = [str(binary), "--source", "kaa", "--provider", base, "--state-dir", str(root / "state")]
        def invoke(*args, success=True):
            p = subprocess.run(common + list(args), env=env, capture_output=True, text=True, timeout=35)
            assert (p.returncode == 0) == success, (args, p.stderr, [p.read_text() for p in (root / "state/logs").glob("*")])
            return p
        try:
            listed = json.loads(invoke("--list", "Probe").stdout)
            assert [item["id"] for item in listed] == ["probe", "second"]
            faults["movie"] = True
            invoke("--sub", "--resolve", "Probe")
            assert any("ep=0&" in path for path, _, _ in requests)
            faults["movie"] = False
            faults["chinese"] = True
            result = json.loads(invoke("--sub", "--resolve", "Probe").stdout)
            assert result["audio"] == base + "ja.m3u8"
            faults["chinese"] = False
            faults["empty"] = True
            failed = invoke("--dub", "--resolve", "Probe", success=False)
            assert "no episodes in dub" in failed.stderr and "Invalid KAA response" not in failed.stderr
            faults["empty"] = False
            faults["unavailable"] = True
            for mode in ["sub", "dub"]:
                failed = invoke("--" + mode, "--resolve", "Probe", success=False)
                assert "no playable episodes in any language" in failed.stderr
                assert "Try switching audio language" not in failed.stderr
            faults["unavailable"] = False
            for mode, track in [("sub", "ja"), ("dub", "en")]:
                requests.clear()
                result = [json.loads(line) for line in invoke("--" + mode, "--resolve", "Probe", "-e", "1,1.5").stdout.splitlines()]
                assert len(result) == 2 and result[0]["audio"] == base + track + ".m3u8"
                assert result[0]["subtitle"] == base + "sub.vtt"
                assert result[0]["request"]["user_agent"]
                invoke("--" + mode, "--play", "Probe", "-e", "1.5", "--player-arg=--no-config",
                    "--player-arg=--vo=null", "--player-arg=--ao=null")
                paths = [r[0] for r in requests]
                assert f"/{track}0.ts" in paths and ("/en0.ts" if track == "ja" else "/ja0.ts") not in paths
                invoke("--" + mode, "--download", "Probe", "-e", "1", "--download-dir", str(root / mode))
                output = next((root / mode).glob("*.mkv"))
                subprocess.run(["ffmpeg", "-v", "error", "-xerror", "-i", str(output), "-f", "null", "-"], check=True)
                assert json.loads(invoke("--history").stdout)[0]["anime"]["provider"] == "kaa"
            failed = invoke("--sub", "--play", "Probe", "-e", "1", "--player", "vlc", success=False)
            assert "VLC cannot apply the required Origin header" in failed.stderr
            media_failure[0] = True
            invoke("--sub", "--download", "Probe", "-e", "1", "--download-dir", str(root / "failed"), success=False)
            assert not list((root / "failed").iterdir()), "Failed download left partial output"
            media_failure[0] = False
            requests.clear()
            final_failure[0] = True
            failed = invoke("--sub", "--download", "Probe", "-e", "1,1.5", "--download-dir", str(root / "truncated"), success=False)
            assert "Saved" not in failed.stdout
            assert not any("/episode/ep-1.5-" in path for path, _, _ in requests), "Queue advanced after an incomplete download"
            assert not list((root / "truncated").iterdir()), "Truncated download was published"
            final_failure[0] = False
            for fault in ["repeat", "wrong", "no_servers"]:
                faults[fault] = True
                failed = invoke("--resolve", "Probe", "-e", "1", success=False)
                assert not failed.stdout and "kaa" in failed.stderr
                faults[fault] = False
            print("PASS: KAA pagination, decimal episodes, exact alternate lookup, explicit audio, subtitles, headers, protected playback/downloads and failure cases", flush=True)
        finally:
            server.shutdown()
            server.server_close()
if __name__ == "__main__": main()
