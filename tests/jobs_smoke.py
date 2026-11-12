#!/usr/bin/env python3
"""Check complete owned process trees, including detached player descendants.

Uses local provider metadata and disposable mpv/ffmpeg-compatible wrappers.
Actual media decoding/muxing is covered by smoke.py and network_smoke.py.
Player wrappers emit large diagnostics on both streams before becoming ready.
"""
import argparse
import base64
import fcntl
import http.server
import json
import os
import pathlib
import re
import resource
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time


def process_info(pid):
    try:
        fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return fields[19], fields[0], int(fields[1])  # start time, state, parent PID
    except (FileNotFoundError, ProcessLookupError):
        return None


def descendants(pid):
    found = set()
    pending = [pid]
    while pending:
        current = pending.pop()
        for task in pathlib.Path(f"/proc/{current}/task").glob("*/children"):
            try:
                children = {int(value) for value in task.read_text().split()}
            except (FileNotFoundError, ProcessLookupError):
                continue
            pending.extend(children - found)
            found.update(children)
    return found


def identities(pids):
    return {pid: info[0] for pid in pids if (info := process_info(pid)) is not None}


def remaining(owned):
    return {pid: info[1] for pid, start in owned.items()
            if (info := process_info(pid)) is not None and info[0] == start}


def wait_for(predicate, pump=time.sleep, message="Timed out waiting for job"):
    deadline = time.monotonic() + 5
    while not predicate() and time.monotonic() < deadline:
        pump(0.02)
    assert predicate(), message


class Fixture(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        path = self.path.split("?")[0]
        base = f"http://127.0.0.1:{self.server.server_port}"
        if path == "/search":
            body = '<div class="flw-item"><h3 class="film-name"><a title="Jobs Probe" href="/probe-1">Probe</a></h3></div>'
        elif path == "/probe-1":
            body = '<div class="film-description"><div class="text">Job ownership fixture.</div></div>'
        elif path == "/api/theme/episode/list/1":
            body = json.dumps({"html": ''.join(f'<a class="ep-item" data-id="{i}" data-number="{i}" title="OWNED_EPISODE_{i}" href="/watch/probe-1?ep={i}"></a>' for i in (1, 2))})
        elif path == "/api/theme/episode/servers":
            encoded = base64.b64encode(f"{base}/embed".encode()).decode()
            body = json.dumps({"html": f'<div class="server-item" data-type="sub" data-server-name="ZokoAnime" data-hash="{encoded}"></div>'})
        elif path == "/embed":
            config = json.dumps({"src": f"{base}/master.m3u8"}).encode()
            key = b"otaku-embed-v1"
            blob = base64.b64encode(bytes(byte ^ key[i % len(key)] for i, byte in enumerate(config))).decode()
            body = f'<script>window.__P="{blob}";</script>'
        elif path == "/master.m3u8":
            body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=64x64\nvideo.m3u8\n"
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


def make_tools(root):
    tools = root / "tools"
    tools.mkdir()
    (tools / "descendant.py").write_text('''import pathlib, subprocess, sys, time
root = pathlib.Path(sys.argv[2])
token = sys.argv[3]
if sys.argv[1] == "child":
    leaf = subprocess.Popen([sys.executable, __file__, "leaf", str(root), token], start_new_session=True)
    while not (root / ("leaf-" + token)).exists():
        time.sleep(0.01)
    (root / ("child-" + token)).touch()
    leaf.wait()
else:
    (root / ("leaf-" + token)).touch()
    time.sleep(120)
''')
    wrapper = f'#!{sys.executable}\n' + '''import json, os, pathlib, subprocess, sys, time, uuid
if "--sentinel" in sys.argv:
    time.sleep(120)
    sys.exit(0)
root = pathlib.Path(__file__).parent.parent
token = uuid.uuid4().hex
print("MIRU_JOB_START " + token, file=sys.stderr, flush=True)
child = subprocess.Popen([sys.executable, str(root / "tools/descendant.py"), "child", str(root), token], start_new_session=True)
while not (root / ("child-" + token)).exists():
    time.sleep(0.01)
if pathlib.Path(__file__).name == "ffmpeg":
    pathlib.Path(sys.argv[-1]).write_bytes(b"owned partial fixture")
    print("out_time=00:00:00.1", flush=True)
else:
    for stream, label in ((sys.stdout, "STDOUT"), (sys.stderr, "STDERR")):
        stream.write("MIRU_PLAYER_" + label + "_BEGIN\\n" + "x" * (256 * 1024) + "\\nMIRU_PLAYER_" + label + "_END\\n")
        stream.flush()
with (root / "launches").open("a") as log:
    log.write(json.dumps({"token": token, "args": sys.argv[1:], "log": os.readlink("/proc/self/fd/2")}) + "\\n")
while True:
    if (root / ("finish-" + token)).exists():
        print("MIRU_JOB_END " + token, file=sys.stderr, flush=True)
        sys.exit(0)
    if (root / ("fail-" + token)).exists():
        print("MIRU_JOB_END " + token, file=sys.stderr, flush=True)
        sys.exit(2)
    time.sleep(0.01)
'''
    for name in ("player", "ffmpeg"):
        path = tools / name
        path.write_text(wrapper)
        path.chmod(0o700)
    return tools


class Run:
    def __init__(self, binary, base, root, interactive, download, controlling_terminal=True, state_dir=None):
        self.root = root
        self.interactive = interactive
        self.master = self.slave = None
        self.output = bytearray()
        self.pending = bytearray()
        self.owned = {}
        self.tools = make_tools(root)
        environment = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ["PATH"])
        state = state_dir if state_dir is not None else root / "state"
        state.mkdir(exist_ok=True)
        command = [str(binary), "--provider", base, "--state-dir", str(state),
                   "--cache-dir", str(root / "cache"), "--download-dir", str(root / "downloads"),
                   "--player", str(self.tools / "player")]
        if interactive:
            (state / "library.json").write_text(json.dumps({"version": 2, "mode": "sub", "quality": "best", "entries": [{
                "anime": {"id": "probe-1", "title": "Jobs Probe", "cover": None},
                "episode": "0", "mode": "sub", "watched_at": 0,
            }]}))
            self.master, self.slave = os.openpty()
            os.set_blocking(self.master, False)
            fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 960, 800))
            command += ["--continue", "--images", "off", "--theme", "terminal", "--no-animation"]
            streams = dict(stdin=self.slave, stdout=self.slave, stderr=self.slave)
        else:
            command += ["--download" if download else "--play", "Probe"]
            streams = dict(stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        def prepare():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
            if interactive and controlling_terminal:
                fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(command, env=environment, start_new_session=True, preexec_fn=prepare, **streams)
        self.unrelated = subprocess.Popen([str(self.tools / "player"), "--sentinel"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.outside = identities([self.unrelated.pid, os.getppid()])

    def pump(self, duration):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            self.owned.update(identities(descendants(self.process.pid)))
            if self.master is None:
                time.sleep(0.01)
            elif select.select([self.master], [], [], 0.01)[0]:
                try:
                    data = os.read(self.master, 65536)
                except (OSError, BlockingIOError):
                    continue
                self.output.extend(data)
                self.pending.extend(data)
                while b"\x1b[6n" in self.pending:
                    end = self.pending.index(b"\x1b[6n") + 4
                    del self.pending[:end]
                    os.write(self.master, b"\x1b[1;1R")
                self.pending[:] = self.pending[-3:]

    def keys(self, keys):
        os.write(self.master, keys)
        self.pump(0.1)

    def launches(self):
        path = self.root / "launches"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def job(self, number, download):
        wait_for(lambda: len(self.launches()) >= number, self.pump, "Wrapper did not report its descendants ready")
        if not download:
            log = pathlib.Path(self.launches()[number - 1]["log"]).read_bytes()
            for label in (b"STDOUT", b"STDERR"):
                expected = (b"MIRU_PLAYER_" + label + b"_BEGIN\n" + b"x" * (256 * 1024) +
                            b"\nMIRU_PLAYER_" + label + b"_END\n")
                assert expected in log, f"Incomplete {label!r} diagnostic capture"
        executable = str(self.tools / ("ffmpeg" if download else "player"))
        tree = descendants(self.process.pid)
        wrappers = []
        for pid in tree:
            try:
                arguments = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
            except (FileNotFoundError, ProcessLookupError):
                continue
            if len(arguments) > 1 and os.fsdecode(arguments[1]) == executable:
                wrappers.append(pid)
        assert len(wrappers) == 1, f"Expected one live job wrapper, found {wrappers}"
        owned = identities({wrappers[0], *descendants(wrappers[0])})
        assert len(owned) >= 3, "Test wrapper did not create its detached child and grandchild"
        current = wrappers[0]
        for _ in range(16):
            info = process_info(current)
            assert info is not None, "Job exited before its process tree could be checked"
            parent = info[2]
            if parent == self.process.pid:
                break
            assert parent in tree, "Job ownership scope escaped its Miru process"
            owned.update(identities([parent]))
            current = parent
        else:
            raise AssertionError("Could not trace job ownership back to Miru")
        self.owned.update(owned)
        return owned, self.launches()[number - 1]

    def gone(self, owned):
        wait_for(lambda: not remaining(owned), self.pump, f"Owned processes were not terminated and reaped: {remaining(owned)}")
        assert len(remaining(self.outside)) == len(self.outside), "Stopping a job killed an unrelated player or parent shell"
        assert self.unrelated.poll() is None, "Unrelated player exited"

    def exited(self, expected=0):
        wait_for(lambda: self.process.poll() is not None, self.pump, "Miru did not exit")
        if not self.interactive:
            stdout, stderr = self.process.communicate(timeout=1)
            self.output.extend(stdout + stderr)
        assert b"MIRU_PLAYER_" not in self.output, "Player diagnostics leaked into CLI/TUI output"
        assert self.process.returncode == expected, f"Unexpected Miru exit {self.process.returncode}: {bytes(self.output)[-1000:]}"

    def close(self):
        self.owned.update(identities(descendants(self.process.pid)))
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
        for pid in remaining(self.owned):
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        self.unrelated.terminate()
        self.unrelated.wait(timeout=3)
        for descriptor in (self.master, self.slave):
            if descriptor is not None:
                os.close(descriptor)


CASES = ("play-term", "play-int", "play-hup", "download-term", "download-int", "download-hup",
         "play-finish", "play-error", "tui-stop", "tui-download-stop", "tui-replace", "tui-ctrl-c",
         "tui-term", "tui-hup", "tui-terminal-close", "tui-download-terminal-close",
         "tui-terminal-close-no-hup", "tui-download-terminal-close-no-hup",
         "play-concurrent-logs", "download-concurrent-logs")


def check_concurrent_logs(binary, base, download):
    with tempfile.TemporaryDirectory(prefix="miru-concurrent-logs-") as temporary:
        root = pathlib.Path(temporary)
        runs = []
        launched = []
        try:
            for name in ("first", "second"):
                directory = root / name
                directory.mkdir()
                run = Run(binary, base, directory, False, download, state_dir=root / "state")
                runs.append(run)
                _, job = run.job(1, download)
                launched.append(job)
                if name == "first":
                    active_log = pathlib.Path(job["log"])
                    os.utime(active_log, (1, 1))
                    for index in range(40):
                        archived = active_log.with_name(f"player-archived-{index:03}.log")
                        archived.write_text("Archived diagnostic fixture\n")
                        os.utime(archived, (index + 2, index + 2))
            # Finish the newer process first while the original job still owns its writer.
            logs = []
            for run, job in reversed(list(zip(runs, launched))):
                (run.root / ("fail-" + job["token"])).touch()
                run.exited(1)
                run.gone(run.owned)
                match = re.search(rb"[Ss]ee ([^\r\n]+)", run.output)
                assert match, f"Failure omitted the diagnostic path: {run.output!r}"
                path = pathlib.Path(os.fsdecode(match[1]))
                assert path == pathlib.Path(job["log"]), "Reported path does not match the job's writer"
                logs.append((path, job["token"]))
            assert logs[0][0] != logs[1][0], "Concurrent jobs reported the same log path"
            saved = {path: path.read_bytes() for path, _ in logs}
            directory = root / "later"
            directory.mkdir()
            later = Run(binary, base, directory, False, download, state_dir=root / "state")
            runs.append(later)
            _, job = later.job(1, download)
            assert all(path.read_bytes() == content for path, content in saved.items()), "Starting another job changed a reported log"
            (later.root / ("fail-" + job["token"])).touch()
            later.exited(1)
            later.gone(later.owned)
            retained = list(logs[0][0].parent.glob("*.log"))
            assert len(retained) <= 32, f"Finished log retention is not bounded: {len(retained)} retained"
            for path, token in logs:
                contents = path.read_text()
                assert f"MIRU_JOB_START {token}\n" in contents and f"MIRU_JOB_END {token}\n" in contents, contents[-1000:]
                assert all(other == token or other not in contents for _, other in logs), "Diagnostics mixed jobs"
            print(f"PASS: {'download' if download else 'play'} concurrent failures retain separate complete logs", flush=True)
        finally:
            for run in reversed(runs):
                run.close()


def check(binary, base, case):
    with tempfile.TemporaryDirectory(prefix="miru-jobs-") as temporary:
        root = pathlib.Path(temporary)
        interactive = case.startswith("tui-")
        download = "download" in case
        run = Run(binary, base, root, interactive, download,
                  controlling_terminal=not case.endswith("no-hup"))
        try:
            if interactive:
                run.pump(0.3)
                run.keys(b"\r")
                wait_for(lambda: b"OWNED_EPISODE_1" in run.output, run.pump, "Episode list never rendered")
                run.keys(b"d" if download else b"\r")
            first, launched = run.job(1, download)
            if case in ("tui-stop", "tui-download-stop"):
                run.keys(b"x")
                run.gone(first)
                assert run.process.poll() is None, "Stopping a job also quit Miru"
                run.keys(b"\x03")
            elif case == "tui-replace":
                run.keys(b"j\r")
                second, replacement = run.job(2, False)
                assert any("Episode 2" in argument for argument in replacement["args"]), replacement
                run.gone(first)
                assert remaining(second), "Replacement job did not stay alive"
                run.keys(b"x")
                run.gone(second)
                run.keys(b"\x03")
            elif case in ("play-finish", "play-error"):
                (root / (("finish-" if case == "play-finish" else "fail-") + launched["token"])).touch()
            elif case == "tui-ctrl-c":
                run.keys(b"\x03")
            elif "terminal-close" in case:
                os.close(run.master)
                run.master = None
            else:
                stopping = {"term": signal.SIGTERM, "int": signal.SIGINT, "hup": signal.SIGHUP}[case.rsplit("-", 1)[1]]
                run.process.send_signal(stopping)
            run.exited(1 if case == "play-error" else 0)
            run.gone(first)
            if download:
                assert not list((root / "downloads").glob("*")), "Cancellation left partial media behind"
            print(f"PASS: {case} terminates and reaps the whole job, preserving unrelated processes", flush=True)
        finally:
            run.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/miru", type=pathlib.Path)
    parser.add_argument("--case", choices=CASES)
    options = parser.parse_args()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for case in (options.case,) if options.case else CASES:
            base = f"http://127.0.0.1:{server.server_port}/"
            if case.endswith("concurrent-logs"):
                check_concurrent_logs(options.binary.resolve(), base, case.startswith("download"))
            else:
                check(options.binary.resolve(), base, case)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


if __name__ == "__main__":
    main()
