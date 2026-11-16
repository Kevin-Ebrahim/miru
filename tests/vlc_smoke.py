#!/usr/bin/env python3
"""Keep real VLC playback owned by Miru despite an existing single-instance VLC.

Uses a private D-Bus session and disposable HTTP, media, config and state fixtures.
Run after cargo build: python3 tests/vlc_smoke.py [path/to/miru]
"""
import http.server
import os
import pathlib
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import threading

from jobs_smoke import Fixture, descendants, identities, remaining, wait_for


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    for program in ("ffmpeg", "vlc", "dbus-daemon", "dbus-send"):
        if not shutil.which(program):
            raise SystemExit(f"Install {program} to run the VLC smoke test")
    with tempfile.TemporaryDirectory(prefix="miru-vlc-") as temporary:
        root = pathlib.Path(temporary)
        video = root / "video.ts"
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
            "color=size=64x64:rate=10", "-t", "30", "-c:v", "libx264", "-f", "mpegts", str(video),
        ], check=True, timeout=15)
        private_requests = []
        media_requests = []

        class Private(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                private_requests.append(self.path)
                data = video.read_bytes()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        private = http.server.ThreadingHTTPServer(("127.0.0.2", 0), Private)

        class Provider(Fixture):
            def do_GET(self):
                path = self.path.split("?")[0]
                if path == "/video.m3u8":
                    segment = f"http://127.0.0.2:{private.server_port}/private.ts" if self.server.denied else "video.ts"
                    data = f"#EXTM3U\n#EXT-X-TARGETDURATION:30\n#EXTINF:30,\n{segment}\n#EXT-X-ENDLIST\n".encode()
                elif path == "/video.ts":
                    media_requests.append(self.path)
                    data = video.read_bytes()
                else:
                    return super().do_GET()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        provider.denied = False
        servers = (provider, private)
        threads = [threading.Thread(target=server.serve_forever, daemon=True) for server in servers]
        for thread in threads:
            thread.start()
        config = root / "config/vlc"
        config.mkdir(parents=True)
        (config / "vlcrc").write_text("[core]\none-instance=1\none-instance-when-started-from-file=1\n")
        environment = dict(os.environ, XDG_CONFIG_HOME=str(config.parent), XDG_CACHE_HOME=str(root / "cache"),
                           no_proxy="*", NO_PROXY="*")
        vlc_args = ["--intf=dummy", "--extraintf=dbus", "--aout=dummy", "--vout=dummy",
                    "--no-media-library", "--no-metadata-network-access", "--no-interact",
                    "--one-instance", "--one-instance-when-started-from-file"]
        bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1",
                                f"--address=unix:path={root / 'bus.sock'}"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        host = None
        try:
            assert select.select([bus.stdout], [], [], 5)[0], "Private D-Bus did not start"
            environment["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().strip()
            with (root / "host-vlc.log").open("w") as host_log:
                host = subprocess.Popen(["vlc", *vlc_args], env=environment,
                                        stdout=host_log, stderr=host_log)

                def host_ready():
                    result = subprocess.run([
                        "dbus-send", "--session", "--print-reply", "--dest=org.freedesktop.DBus",
                        "/org/freedesktop/DBus", "org.freedesktop.DBus.NameHasOwner",
                        "string:org.mpris.MediaPlayer2.vlc",
                    ], env=environment, capture_output=True, text=True, timeout=2)
                    return "boolean true" in result.stdout

                wait_for(host_ready, message="Existing VLC did not register on the private bus")
                base = f"http://127.0.0.1:{provider.server_port}"
                # Prove this real host accepts forwarded media before testing Miru.
                forwarded = subprocess.run(["vlc", *vlc_args, f"{base}/video.ts?control"], env=environment,
                                           capture_output=True, text=True, timeout=10)
                assert forwarded.returncode == 0, forwarded.stderr
                wait_for(lambda: "/video.ts?control" in media_requests,
                         message="Single-instance forwarding control never reached the host VLC")
                assert host.poll() is None, "Forwarding control stopped the existing VLC"
                print("PASS: existing real VLC accepts single-instance forwarding", flush=True)
                common = [str(binary), "--provider", base, "--play", "Probe", "--player", "vlc"]
                common += [f"--player-arg={argument}" for argument in vlc_args]
                provider.denied = True
                denied = subprocess.run(common + ["--state-dir", str(root / "denied-state")], env=environment,
                                        capture_output=True, text=True, timeout=20)
                if denied.returncode == 0:
                    wait_for(lambda: private_requests, message="VLC silently succeeded without enforcing the destination policy")
                assert not private_requests, f"Existing VLC bypassed Miru's policy: {private_requests}"
                assert denied.returncode != 0 and "Media destination blocked" in denied.stderr, denied.stderr
                assert host.poll() is None, "Denied playback stopped the existing VLC"
                print("PASS: nested private VLC requests remain blocked despite single-instance preferences and user flags", flush=True)
                provider.denied = False
                for action in (signal.SIGINT, signal.SIGTERM):
                    media_requests.clear()
                    owned = subprocess.Popen(common + ["--state-dir", str(root / f"owned-{action.name}")],
                                             env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    try:
                        def playing():
                            try:
                                return bool(media_requests) and any(
                                    pathlib.Path(f"/proc/{pid}/comm").read_text().strip() == "vlc"
                                    for pid in descendants(owned.pid))
                            except (FileNotFoundError, ProcessLookupError):
                                return False

                        try:
                            wait_for(playing, message="VLC playback did not remain a live Miru descendant")
                        except AssertionError as error:
                            stop(owned)
                            _, stderr = owned.communicate(timeout=5)
                            logs = "\n".join(path.read_text() for path in (root / f"owned-{action.name}/logs").glob("player-*.log"))
                            raise AssertionError(f"{error}; media={media_requests}\n{stderr}\n{logs}") from error
                        tree = identities(descendants(owned.pid))
                        owned.send_signal(action)
                        owned.communicate(timeout=10)
                        wait_for(lambda: not remaining(tree), message=f"{action.name} left owned VLC processes: {remaining(tree)}")
                        assert host.poll() is None, f"{action.name} killed the unrelated existing VLC"
                        assert not private_requests, f"VLC reached private media: {private_requests}"
                        print(f"PASS: {action.name} terminates owned real VLC playback and preserves the existing VLC", flush=True)
                    finally:
                        stop(owned)
        finally:
            if host is not None:
                stop(host)
            stop(bus)
            for server in servers:
                server.shutdown()
                server.server_close()
            for thread in threads:
                thread.join(timeout=2)


if __name__ == "__main__":
    main()
