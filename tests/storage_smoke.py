#!/usr/bin/env python3
"""Offline regression checks for unsupported history and import storage objects."""
import argparse
import fcntl
import json
import os
import pathlib
import signal
import socket
import subprocess
import tempfile
import time


def snapshot(path):
    info = path.lstat()
    return info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/miru", type=pathlib.Path)
    options = parser.parse_args()
    binary = options.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="miru-storage-") as temporary:
        root = pathlib.Path(temporary)
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("MIRU_", "ANI_WATCH_"))}
        environment.update(XDG_STATE_HOME=str(root / "default-state"), XDG_CACHE_HOME=str(root / "cache"))

        def invoke(arguments):
            process = subprocess.Popen([str(binary), *arguments], env=environment,
                                       stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                try:
                    stdout, stderr = process.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    process.send_signal(signal.SIGTERM)
                    try:
                        process.communicate(timeout=1)
                    except subprocess.TimeoutExpired as error:
                        raise AssertionError("Storage blocked startup and ignored SIGTERM") from error
                    raise AssertionError("Unsupported storage did not fail promptly")
                return process.returncode, stdout, stderr
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=3)

        for operation in ("history", "import", "migration"):
            for kind in ("fifo", "fifo-link", "directory", "device", "socket", "oversized"):
                directory = root / f"{operation}-{kind}"
                directory.mkdir()
                state = directory / "state"
                state.mkdir()
                source = directory / "source"
                target = state / "library.json"
                if operation == "history":
                    source = target
                    arguments = ["--state-dir", str(state), "--history"]
                elif operation == "import":
                    target.write_text(json.dumps({"version": 2, "mode": "sub", "quality": "best", "entries": []}))
                    arguments = ["--state-dir", str(state), "--import-history", str(source)]
                else:
                    environment["XDG_STATE_HOME"] = str(directory)
                    source = directory / "ani-watch/library.json"
                    source.parent.mkdir()
                    target = directory / "miru/library.json"
                    arguments = ["--history"]
                listener = None
                if kind == "fifo":
                    os.mkfifo(source)
                elif kind == "fifo-link":
                    fifo = directory / "input-fifo"
                    os.mkfifo(fifo)
                    source.symlink_to(fifo)
                elif kind == "directory":
                    source.mkdir()
                elif kind == "device":
                    source.symlink_to("/dev/zero")
                elif kind == "socket":
                    listener = socket.socket(socket.AF_UNIX)
                    working_directory = pathlib.Path.cwd()
                    try:
                        os.chdir(source.parent)
                        listener.bind(source.name)
                    finally:
                        os.chdir(working_directory)
                else:
                    with source.open("wb") as output:
                        output.truncate((8 if operation == "import" else 32) * 1024 * 1024 + 1)
                before = snapshot(source)
                saved = target.read_bytes() if operation == "import" else None
                try:
                    code, stdout, stderr = invoke(arguments)
                    assert code != 0 and str(source) in stderr, (code, stdout, stderr)
                    if kind not in ("socket", "oversized"):
                        assert "regular file" in stderr.lower(), stderr
                    if kind == "oversized":
                        assert "limit" in stderr.lower(), stderr
                    assert not stdout and snapshot(source) == before, "Rejected storage was modified"
                    if operation == "import":
                        assert target.read_bytes() == saved, "Rejected import changed history"
                    elif operation == "migration":
                        assert not target.exists(), "Rejected migration created history"
                    print(f"PASS: {operation} rejects {kind} storage without modifying it", flush=True)
                finally:
                    if listener:
                        listener.close()

        for stop_signal in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            directory = root / f"cancel-import-{stop_signal}"
            directory.mkdir()
            source = directory / "ani-hsts"
            source.write_text("1\tshow-1\tImported show\n")
            lock_path = directory / "library.lock"
            with lock_path.open("w") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                process = subprocess.Popen([str(binary), "--state-dir", str(directory), "--import-history", str(source)],
                                           env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    deadline = time.monotonic() + 5
                    opened = False
                    while process.poll() is None and time.monotonic() < deadline:
                        for descriptor in pathlib.Path(f"/proc/{process.pid}/fd").glob("*"):
                            try:
                                opened = os.readlink(descriptor) == str(lock_path)
                            except FileNotFoundError:
                                continue
                            if opened:
                                break
                        if opened:
                            break
                        time.sleep(0.01)
                    assert opened, "Import did not reach the contended history lock"
                    process.send_signal(stop_signal)
                    process.communicate(timeout=1)
                    assert process.returncode == -stop_signal, "Offline command did not retain normal signal termination"
                    assert not (directory / "library.json").exists(), "Cancelled import changed history"
                    assert source.read_text() == "1\tshow-1\tImported show\n"
                    print(f"PASS: offline import remains terminable by {stop_signal.name}", flush=True)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.communicate(timeout=3)


if __name__ == "__main__":
    main()
