#!/usr/bin/env python3
"""Exercise AnimeGG selection and protected direct-media playback with local fixtures."""
import functools
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    fixtures = pathlib.Path(__file__).parent / "fixtures/animegg"
    requests = []
    with tempfile.TemporaryDirectory(prefix="miru-providers-") as temporary:
        root = pathlib.Path(temporary)
        media = root / "media"
        media.mkdir()
        subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=size=64x64:rate=10",
                        "-f", "lavfi", "-i", "sine=frequency=440", "-t", "1", "-c:v", "libx264",
                        "-c:a", "aac", "-movflags", "+faststart", str(media / "high.mp4")], check=True, timeout=20)
        (media / "low.mp4").write_bytes((media / "high.mp4").read_bytes())

        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                requests.append((self.path, self.headers.get("Referer")))
                path = self.path.split("?", 1)[0]
                fixture = None
                if path == "/search/":
                    fixture = "search.html"
                elif path == "/series/probe":
                    fixture = "series.html"
                elif path in ("/probe-episode-1", "/probe-special", "/probe-episode-10"):
                    fixture = "episode.html"
                elif path in ("/embed/11", "/embed/22"):
                    fixture = "embed.html"
                if fixture:
                    body = (fixtures / fixture).read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                else:
                    super().do_GET()

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(root)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{server.server_port}/"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("MIRU_", "ANI_WATCH_"))}
        env.update(XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"), MIRU_SOURCE="animegg", MIRU_PROVIDER=base)
        common = [str(binary), "--state-dir", str(root / "state"), "--cache-dir", str(root / "cache")]

        def invoke(*args, success=True, environment=None):
            result = subprocess.run(common + list(args), env=environment or env, text=True, capture_output=True, timeout=40)
            if (result.returncode == 0) != success:
                logs = "\n".join(path.read_text() for path in (root / "state/logs").glob("*.log"))
                raise AssertionError((args, result.stdout, result.stderr, logs))
            return result

        try:
            found = json.loads(invoke("--list", "Probe").stdout)
            assert len(found) == 2 and found[0]["provider"] == "animegg"
            assert requests[-1][0] == "/search/?q=Probe"
            streams = [json.loads(line) for line in invoke("--resolve", "Probe", "-e", "1,1.5,10").stdout.splitlines()]
            assert len(streams) == 3
            assert streams[0]["variants"][0]["label"] == "720p"
            assert streams[0]["variants"][0]["format"] == "direct"
            assert streams[0]["referer"] == base and streams[0]["subtitle"] is None
            assert any(path == "/probe-special" for path, _ in requests)
            requests.clear()
            invoke("--dub", "--resolve", "Probe", "-e", "1")
            assert any(path == "/embed/22" for path, _ in requests)
            assert not any(path == "/embed/11" for path, _ in requests)
            failed = invoke("--dub", "--resolve", "Probe", "-e", "1.5", success=False)
            assert not failed.stdout
            assert "animegg episode selection failed (dub)" in failed.stderr, failed.stderr
            assert "--source hianime --list --dub <title>" in failed.stderr
            failed = invoke("--resolve", "Probe", "-e", "9", success=False)
            assert not failed.stdout and "--source hianime" in failed.stderr
            requests.clear()
            failed = invoke("--source", "hianime", "--list", "Probe", success=False)
            assert not failed.stdout
            assert "hianime catalogue search failed" in failed.stderr
            assert "--source animegg --list --sub <title>" in failed.stderr
            assert requests and requests[0][0].startswith("/search?keyword="), requests
            requests.clear()
            invoke("--source", "unknown", "--list", "Probe", success=False)
            assert not requests
            legacy = dict(env)
            legacy.pop("MIRU_PROVIDER")
            legacy["ANI_WATCH_PROVIDER"] = base
            invoke("--list", "Probe", environment=legacy)
            override = dict(env, MIRU_PROVIDER="http://127.0.0.1:9/")
            invoke("--provider", base, "--list", "Probe", environment=override)
            print("PASS: provider CLI/env precedence, explicit audio, decimal/gapped episodes and JSON export", flush=True)

            invoke("--play", "Probe", "-e", "1", "--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null")
            history = json.loads(invoke("--history").stdout)
            assert history[0]["anime"]["provider"] == "animegg" and history[0]["episode"] == "1"
            assert any(path.startswith("/media/") and referer == base for path, referer in requests)
            assert json.loads(invoke("--source", "hianime", "--history").stdout) == history
            print("PASS: protected AnimeGG playback, referer propagation and provider-qualified history", flush=True)
            requests.clear()
            failed_dir = root / "disabled-downloads"
            failed = invoke("--download", "Probe", "-e", "1", "--download-dir", str(failed_dir), success=False)
            assert "Downloads are unavailable for provider animegg" in failed.stderr
            assert not requests, "Disabled download contacted the provider"
            assert not failed_dir.exists(), "Disabled download created output storage"
            assert json.loads(invoke("--history").stdout) == history
            print("PASS: AnimeGG downloads are rejected before network or output side effects", flush=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
