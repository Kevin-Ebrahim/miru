# Miru guide

See the [README](../README.md) for installation and first use.

## Controls

| Key | Action |
| --- | --- |
| `H` | Return Home, outside text input |
| `/` | Focus search; in History, filter saved entries |
| ↑ / ↓, `j` / `k` | Navigate the current list |
| Enter / → / `l` | Open a selection; Enter plays an episode |
| ← / `h` / Backspace / Esc | Go back, close a panel, or leave text input |
| `i` | Open full cover and series details |
| `s` or Shift+D | Toggle SUB/DUB |
| `Q` | Cycle playback quality |
| Alt+e | Toggle exact title matching |
| Alt+h | Open watch history |
| Tab / Space in Episodes | Mark or unmark an episode |
| `:` in Episodes | Enter an episode range to mark |
| `d` in Episodes | Download marked episodes, or the highlighted episode |
| `n` / `p` / `r` | Play next / previous / replay |
| `x` | Stop the current player or download queue, or dismiss its result |
| F5 / F6 | Retry the current view / retry a failed queue |
| Delete in History | Remove the highlighted history entry |
| `?` / F1 | Open help; `/` filters help by key or action |
| `q` | Close Help or Details |
| Ctrl+C | Quit from anywhere and stop owned jobs |

Printable keys enter text while an input has focus. Inputs support readline
shortcuts such as Ctrl+a/e, Ctrl+w and Ctrl+u/k. F1 lists all bindings.

Ranges use the provider's episode order and can include decimal specials. Use
`1-3`, `1, 5, 8` or `1.5-3`. Use `0--1` for all episodes and `-1` for the last. Miru
validates the whole range before starting a queue. A queue keeps its original
series, episodes, language, and quality as you browse. If it fails, F6 retries
from the failed episode without repeating completed ones.

## Command-line use

```sh
miru frieren
miru --continue
miru --list frieren
miru --list --exact "Frieren: Beyond Journey's End"
miru --resolve frieren -S 1 -e 1
miru --play frieren -e 1-3 -q 720p
miru --download frieren -e 1-3 --download-dir ./downloads
miru --history
```

`--list` and `--history` output JSON arrays; `--resolve` outputs one JSON object
per episode. `-S` selects a one-based search result. `--sub`, `--dub`, and
`--quality` override saved preferences. Select VLC with `--player vlc`, or
supply an mpv-compatible executable path. Repeat `--player-arg=--option=value`
to pass additional player options.

Downloads are saved as Matroska files with the selected video, separate audio
when available, and the default or English subtitle track when available.
Existing files are never overwritten; failed or cancelled downloads do not
leave a completed file or mark episodes as watched.

## Images and appearance

Miru detects Kitty, Sixel, and iTerm2 image protocols and falls back to colored
halfblocks when necessary. Use `--images kitty`, `--images sixel`,
`--images iterm2`, `--images halfblocks`, or `--images off` to choose explicitly.
Kitty has had more local testing than Sixel, iTerm2 or VLC.

By default `--theme auto` follows Omarchy's active palette when available and
uses the terminal palette otherwise. `--theme terminal` always uses terminal
colors. `NO_COLOR` disables color and images, `--ascii` uses ASCII interface
characters, and `--no-animation` keeps the Home artwork still. For example:

```sh
miru --theme terminal --images halfblocks
miru --images off --ascii --no-animation
```

Use `i` for full details when the terminal is too small for a preview.

## Data and configuration

| Data | Default location |
| --- | --- |
| History, language and quality preferences | `$XDG_STATE_HOME/miru/library.json` |
| Last player and download errors | `$XDG_STATE_HOME/miru/player.log` and `download.log` |
| Cover cache | `$XDG_CACHE_HOME/miru/covers/` |

When XDG variables are unset, state defaults to `~/.local/state` and cache to
`~/.cache`. History is saved with a lock and atomic replacement. Lock contention is reported
after two seconds instead of blocking exit indefinitely. Pending preference
changes are coalesced; queued history changes are flushed on shutdown. An invalid
history file is reported and left intact.

| Environment variable | Command-line equivalent | Purpose |
| --- | --- | --- |
| `MIRU_STATE_DIR` | `--state-dir` | Override the state directory |
| `MIRU_CACHE_DIR` | `--cache-dir` | Override the cover cache directory |
| `MIRU_THEME` | `--theme` | Set `auto` or `terminal` |
| `MIRU_PROVIDER` | `--provider` | Override the provider's base URL |

Command-line options take precedence over `MIRU_*` variables. The previous
`ANI_WATCH_STATE_DIR`, `ANI_WATCH_CACHE_DIR`, `ANI_WATCH_THEME`, and
`ANI_WATCH_PROVIDER` variables remain supported as lower-priority fallbacks.

Series and episode identities include a provider namespace. Existing version 1
history is read as HiAnime data and saved in version 2 format on the next change,
preserving titles, languages, episode numbers and timestamps. JSON output retains
the existing `id` strings and adds a `provider` field. Different providers can use
the same ID without sharing history or synopsis cache entries. Source identity
does not automatically match series across providers.

### Moving from ani-watch

On first use, if the default Miru library does not exist, Miru copies and
validates the old `$XDG_STATE_HOME/ani-watch/library.json` into its new state
directory. It leaves the original intact and never overwrites an existing Miru library.
The copies do not stay synchronized. An explicitly configured
state directory is used directly instead of triggering this migration. Covers
are cached again as needed; old cache and log files are left in place.

To import a compatible history file from the separate shell-based ani-cli app:

```sh
miru --import-history ~/.local/state/ani-cli/ani-hsts
```

Import does not modify the source, skips obsolete provider IDs, and can be
repeated without duplicates. Because that format has no language field, imported
entries are treated as SUB.

## Provider and troubleshooting

Miru is an unofficial client for `https://hianime.at/` and its ZokoAnime server.
`--provider` and `MIRU_PROVIDER` change the base URL for a compatible provider;
they do not add support for other sites' APIs. Site challenges, missing language
sources, and changed markup may prevent playback. Errors appear in the affected
view, and F5 retries it. Check the logs in the state directory for player or
download errors.

### Network policy

Miru's HTTP client accepts HTTP(S), rejects URL credentials and HTTPS downgrades,
checks redirects, and rejects local or reserved destinations, including DNS answers.
An explicitly configured local provider permits requests to that provider's host,
so local fixtures and self-hosted compatible providers continue to work.

ffmpeg and mpv receive a network protocol allowlist and a 20-second network I/O
timeout. Encrypted HLS and separate audio/subtitles remain supported. These tools
fetch nested playlist resources themselves; Miru's destination checks apply to
URLs it resolves, not every request made inside an external player. VLC retains
its own transport settings. This policy is not an operating-system sandbox.

## Development

GitHub Actions runs the checks below for pull requests to `main`, pushes to
`main`, and manual runs. The `Build and test` job uses Ubuntu 26.04 with Rust
stable, FFmpeg 8 and mpv. Select `Build and test` as a required status check in
the `main` ruleset after its first successful run.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/rename_smoke.py target/release/miru
python3 tests/smoke.py target/release/miru
python3 tests/terminal_smoke.py target/release/miru
python3 tests/navigation_smoke.py target/release/miru
python3 tests/theme_smoke.py target/release/miru
```

The smoke tests use isolated temporary state and local provider fixtures. The
media and navigation checks need ffmpeg; the media check also needs mpv and
ffprobe. The terminal checks use a pseudo-terminal and do not replace visual
testing in every terminal.

### CodeQL schedule

The `CodeQL weekly` workflow scans Rust, Python and JavaScript/TypeScript on
Mondays at 02:17 UTC (04:17 South African time), with a manual-run option in
Actions. It has no push or pull-request triggers and should not be a required
PR check. Scheduled runs analyze the default branch.

When switching from GitHub's CodeQL Default setup, merge this workflow first,
then disable Default setup under Settings > Advanced Security > CodeQL analysis.
Default setup otherwise continues its own push and PR scans. After switching,
manually run `CodeQL weekly` once to verify analysis and result uploads.
