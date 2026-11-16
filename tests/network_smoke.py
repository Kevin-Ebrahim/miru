#!/usr/bin/env python3
"""Exercise media destination restrictions with real players and local fixtures.

Run after cargo build: python tests/network_smoke.py [path/to/miru]
All servers, generated media, state and downloads are disposable.
"""
import base64
import http.server
import json
import os
import pathlib
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import urllib.parse


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    for program in ("ffmpeg", "mpv", "openssl"):
        if not shutil.which(program):
            raise SystemExit(f"Install {program} to run the network smoke test")
    environment = dict(os.environ, no_proxy="*", NO_PROXY="*")
    with tempfile.TemporaryDirectory(prefix="miru-network-") as temporary:
        root = pathlib.Path(temporary)
        certificate = root / "certificate.pem"
        key_file = root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
            "-keyout", str(key_file), "-out", str(certificate),
        ], check=True, capture_output=True, timeout=15)
        # Trust only this fixture's certificate without changing the system store
        # or weakening verification in Miru or the actual media executables.
        actual_ffmpeg = shutil.which("ffmpeg")
        tools = root / "tools"
        tools.mkdir()
        trusted_ffmpeg = tools / "ffmpeg"
        trusted_ffmpeg.write_text(
            f"#!{sys.executable}\nimport os, sys\narguments = []\n"
            "for index, argument in enumerate(sys.argv[1:], 1):\n"
            "    if argument == '-i' and sys.argv[index + 1].startswith('https://'):\n"
            f"        arguments.extend(['-ca_file', {str(certificate)!r}])\n"
            "    arguments.append(argument)\n"
            f"os.execv({actual_ffmpeg!r}, [{actual_ffmpeg!r}, *arguments])\n"
        )
        trusted_ffmpeg.chmod(0o700)
        environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
        video = root / "video.ts"
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
            "color=size=64x64:rate=10", "-t", "0.4", "-c:v", "libx264",
            "-f", "mpegts", str(video),
        ], check=True, timeout=15)
        private_requests = []
        private_connections = []
        case = "allowed"

        class Private(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                private_requests.append(self.path)
                data = bytes(16) if self.path.endswith(".key") else video.read_bytes()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        class PrivateServer(http.server.ThreadingHTTPServer):
            def get_request(self):
                connection = super().get_request()
                private_connections.append(connection[1])
                return connection

        private = PrivateServer(("127.0.0.2", 0), Private)
        private_url = f"http://127.0.0.2:{private.server_port}"

        class Provider(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                path = urllib.parse.urlsplit(self.path).path
                base = f"http://127.0.0.1:{self.server.server_port}"
                if path == "/search":
                    body = '<div class="flw-item"><h3 class="film-name"><a href="/probe-1" title="Network Probe">Probe</a></h3></div>'
                elif path == "/api/theme/episode/list/1":
                    body = json.dumps({"html": '<a class="ep-item" data-id="1" data-number="1" href="/watch/probe-1?ep=1"></a>'})
                elif path == "/api/theme/episode/servers":
                    embed = base64.b64encode(f"{base}/embed".encode()).decode()
                    body = json.dumps({"html": f'<div class="server-item" data-type="sub" data-server-name="ZokoAnime" data-hash="{embed}"></div>'})
                elif path == "/embed":
                    config = json.dumps({"src": f"{base}/master.m3u8"}).encode()
                    key = b"otaku-embed-v1"
                    blob = base64.b64encode(bytes(b ^ key[i % len(key)] for i, b in enumerate(config))).decode()
                    body = f'<script>window.__P="{blob}"</script>'
                elif path == "/master.m3u8":
                    target = f"{private_url}/private.m3u8" if case == "direct" else "video.m3u8"
                    if case.startswith("allowed-https"):
                        target = f"https://127.0.0.1:{secure.server_port}/video.m3u8"
                    body = f"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=64x64\n{target}\n"
                elif path == "/video.m3u8":
                    if case == "playlist-redirect":
                        self.send_response(302)
                        self.send_header("Location", f"{private_url}/private.m3u8")
                        self.end_headers()
                        return
                    if case == "nested-playlist":
                        body = f"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=64x64\n{private_url}/private.m3u8\n"
                    else:
                        segment = {
                            "segment": f"{private_url}/private.ts",
                            "segment-redirect": "redirect.ts",
                        }.get(case, "video.ts")
                        if case == "allowed-https":
                            # ffmpeg does not pass ca_file to HLS segment requests.
                            # This valid mixed-scheme fixture tests its TLS playlist
                            # while mpv additionally gets a fully HTTPS control.
                            segment = f"http://127.0.0.1:{provider.server_port}/video.ts"
                        if case == "https-segment":
                            segment = f"https://127.0.0.2:{private.server_port}/private.ts"
                        scheme = "https" if case == "https-key" else "http"
                        key = f'#EXT-X-KEY:METHOD=AES-128,URI="{scheme}://127.0.0.2:{private.server_port}/private.key",IV=0x00000000000000000000000000000000\n' if case in ("key", "https-key") else ""
                        body = f"#EXTM3U\n#EXT-X-TARGETDURATION:1\n{key}#EXTINF:0.4,\n{segment}\n#EXT-X-ENDLIST\n"
                elif path == "/redirect.ts":
                    self.send_response(302)
                    self.send_header("Location", f"{private_url}/private.ts")
                    self.end_headers()
                    return
                elif path == "/video.ts":
                    body = video.read_bytes()
                else:
                    self.send_response(404)
                    self.end_headers()
                    return
                data = body if isinstance(body, bytes) else body.encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        secure = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(certificate, key_file)
        secure.socket = tls.wrap_socket(secure.socket, server_side=True)
        servers = (provider, private, secure)
        threads = [threading.Thread(target=server.serve_forever, daemon=True) for server in servers]
        for thread in threads:
            thread.start()
        common = [str(binary), "--provider", f"http://127.0.0.1:{provider.server_port}/", "--state-dir", str(root / "state")]

        def invoke(operation, directory):
            arguments = common + [operation, "Probe"]
            if operation == "--download":
                arguments += ["--download-dir", str(directory)]
            else:
                arguments += ["--player-arg=--no-config", "--player-arg=--vo=null", "--player-arg=--ao=null",
                              "--player-arg=--tls-verify=yes", f"--player-arg=--tls-ca-file={certificate}"]
            return subprocess.run(arguments, env=environment, capture_output=True, text=True, timeout=20)

        try:
            for operation in ("--download", "--play"):
                controls = ("allowed", "allowed-https", "allowed-https-segments") if operation == "--play" else ("allowed", "allowed-https")
                for case in controls:
                    allowed = invoke(operation, root / f"{case}-{operation[2:]}")
                    prefix = "download" if operation == "--download" else "player"
                    assert allowed.returncode == 0, allowed.stderr + "\n".join(
                        log.read_text() for log in (root / "state/logs").glob(f"{prefix}-*.log"))
                    print(f"PASS: {operation} keeps {case} media working", flush=True)
                for case in ("direct", "segment", "key", "playlist-redirect", "segment-redirect", "nested-playlist", "https-segment", "https-key"):
                    private_requests.clear()
                    private_connections.clear()
                    destination = root / f"denied-{operation[2:]}-{case}"
                    denied = invoke(operation, destination)
                    assert not private_requests, f"{operation} {case} bypassed destination policy: {private_requests}"
                    assert not private_connections, f"{operation} {case} connected to a private service"
                    assert denied.returncode != 0, f"{operation} {case} silently succeeded"
                    assert not list(destination.glob("*")), f"{operation} {case} left partial output"
                    print(f"PASS: {operation} rejects {case}", flush=True)

            # A player that ignores proxy configuration cannot directly reach host services.
            wrapper = root / "player"
            wrapper.write_text(
                f"#!{sys.executable}\nimport socket\n"
                f"with socket.create_connection(('127.0.0.2', {private.server_port}), timeout=2) as connection:\n"
                "    connection.sendall(b'GET /direct-socket HTTP/1.0\\r\\n\\r\\n')\n"
                "    connection.recv(1024)\n"
            )
            wrapper.chmod(0o700)
            case = "allowed"
            private_requests.clear()
            private_connections.clear()
            ignored_proxy = subprocess.run(common + ["--play", "Probe", "--player", str(wrapper)], env=environment, capture_output=True, text=True, timeout=15)
            assert not private_requests, "Player bypassed isolation by ignoring its proxy"
            assert not private_connections, "Player opened a direct connection to the host"
            assert ignored_proxy.returncode != 0, "A direct host connection unexpectedly succeeded"
            print("PASS: direct sockets cannot bypass the media proxy", flush=True)

            unavailable = root / "unavailable-tools"
            unavailable.mkdir()
            broken_bwrap = unavailable / "bwrap"
            broken_bwrap.write_text("#!/bin/sh\nexit 73\n")
            broken_bwrap.chmod(0o700)
            unavailable_environment = dict(environment, PATH=str(unavailable) + os.pathsep + environment["PATH"])
            for operation in ("--play", "--download"):
                state = root / f"unsupported-state-{operation[2:]}"
                destination = root / "unsupported-output"
                arguments = common[:-1] + [str(state), operation, "Probe"]
                if operation == "--download":
                    arguments += ["--download-dir", str(destination)]
                failed = subprocess.run(arguments, env=unavailable_environment, capture_output=True, text=True, timeout=15)
                assert failed.returncode != 0 and "bubblewrap" in failed.stderr, failed.stderr
                assert not (state / "library.json").exists(), "Failed isolation recorded a player launch"
                assert not list(destination.glob("*")), "Failed isolation left media output"
            print("PASS: isolation startup fails closed without changing history or keeping partial files", flush=True)
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()
            for thread in threads:
                thread.join(timeout=2)


if __name__ == "__main__":
    main()
