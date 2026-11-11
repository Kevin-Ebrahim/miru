#!/usr/bin/env python3
"""Exercise Miru's renamed command and migration with isolated state."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile


def library(title):
    return {
        "version": 1,
        "mode": "dub",
        "quality": "720p",
        "entries": [{
            "anime": {"id": "show-1", "title": title, "cover": None},
            "episode": "3", "mode": "dub", "watched_at": 1,
        }],
    }


def write_library(directory, title):
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "library.json"
    path.write_text(json.dumps(library(title)))
    return path


def main():
    binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/miru").resolve()
    with tempfile.TemporaryDirectory(prefix="miru-rename-") as temporary:
        root = pathlib.Path(temporary)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("MIRU_", "ANI_WATCH_"))}
        env.update(HOME=str(root), XDG_STATE_HOME=str(root / "state"),
                   XDG_CACHE_HOME=str(root / "cache"))

        def run(*args, extra_env=None):
            return subprocess.run([str(binary), *args], env={**env, **(extra_env or {})},
                                  text=True, capture_output=True, check=False)

        old = write_library(root / "state/ani-watch", "Original history")
        original = old.read_bytes()
        result = run("--history")
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout)[0]["anime"]["title"] == "Original history"
        new = root / "state/miru/library.json"
        assert json.loads(new.read_text()) == library("Original history")
        assert old.read_bytes() == original, "migration modified the original"

        new.unlink()
        processes = [subprocess.Popen([str(binary), "--history"], env=env, text=True,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                     for _ in range(6)]
        for process in processes:
            stdout, stderr = process.communicate(timeout=10)
            assert process.returncode == 0, stderr
            assert json.loads(stdout)[0]["anime"]["title"] == "Original history"
        assert json.loads(new.read_text()) == library("Original history")

        write_library(new.parent, "Existing Miru history")
        assert json.loads(run("--history").stdout)[0]["anime"]["title"] == "Existing Miru history"
        assert old.read_bytes() == original

        legacy_dir = root / "legacy-override"
        new_dir = root / "new-override"
        flag_dir = root / "flag-override"
        for directory, title in ((legacy_dir, "Legacy env"), (new_dir, "New env"),
                                 (flag_dir, "Command line")):
            write_library(directory, title)
        overrides = {"ANI_WATCH_STATE_DIR": str(legacy_dir), "MIRU_STATE_DIR": str(new_dir)}
        assert json.loads(run("--history", extra_env=overrides).stdout)[0]["anime"]["title"] == "New env"
        assert json.loads(run("--history", "--state-dir", str(flag_dir), extra_env=overrides).stdout)[0]["anime"]["title"] == "Command line"
        assert json.loads(run("--history", extra_env={"ANI_WATCH_STATE_DIR": str(legacy_dir)}).stdout)[0]["anime"]["title"] == "Legacy env"

        for old_name, new_name, option, good, bad in (
            ("ANI_WATCH_THEME", "MIRU_THEME", "--theme", "terminal", "invalid"),
            ("ANI_WATCH_PROVIDER", "MIRU_PROVIDER", "--provider", "http://127.0.0.1:9", "invalid"),
        ):
            assert run("--history", extra_env={old_name: bad}).returncode != 0
            assert run("--history", extra_env={old_name: bad, new_name: good}).returncode == 0
            assert run("--history", option, good, extra_env={old_name: bad, new_name: bad}).returncode == 0

        old.write_text("{invalid json")
        assert json.loads(run("--history").stdout)[0]["anime"]["title"] == "Existing Miru history"
        new.unlink()
        result = run("--history")
        assert result.returncode != 0 and "Invalid history" in result.stderr, result.stderr
        assert not new.exists() and old.read_text() == "{invalid json"

        print("PASS: renamed state, safe history migration, existing data and environment precedence")


if __name__ == "__main__":
    main()
