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

Choose at most one operation: `--continue`, `--history`, `--import-history`,
`--list`, `--resolve`, `--play`, or `--download`. Conflicting operations, such as
`--play --history`, produce a usage error before network requests, media launches,
imports or state changes. Without an operation flag, Miru opens the TUI;
`--continue` opens its history view.

Modifier flags remain accepted with every operation for compatibility; each
operation uses only applicable settings. `--exact` controls searches, and `-S`
and `-e` select direct resolve/play/download results. Language options apply to
resolution, playback, downloads and the TUI; quality options apply to playback,
downloads and the TUI. Player options apply to playback, `--download-dir` to
downloads, and appearance options to the TUI. `--sub` and `--dub` remain mutually
exclusive. State, provider and cache options remain available in every mode.

`--list` and `--history` output JSON arrays; `--resolve` outputs one JSON object
per episode. `-S` selects a one-based search result. `--sub`, `--dub`, and
`--quality` override saved preferences. Select VLC with `--player vlc`, or
supply an mpv-compatible executable path. Repeat `--player-arg=--option=value`
to pass additional player options.

A closed stdout pipe ends the command quietly with exit status 0 and skips any
remaining episode work. Other stdout write failures are reported on stderr and
return a nonzero exit status. Completed imports and downloads remain saved when
their output consumer closes early.

Player argument style is inferred from the executable's exact basename:
`vlc` and `cvlc` use VLC options; every other name uses mpv options. Parent
directories and symlink targets do not affect this choice. For wrappers or
aliases, override it explicitly, for example
`--player '/path/to/my wrapper' --player-backend vlc` or `--player-backend mpv`.
Executable paths and extra arguments are passed literally, without a shell.

`--resolve` keeps the existing top-level `variants`, `referer`, `subtitle`, and
`audio` fields. The top-level `audio` remains a URL or null and describes the
best video variant for compatibility. Each variant now also includes `audio`:
either null for no declared group, or an object with `group` and `url`. A null
`url` means that rendition's audio is included in the video. Scripts choosing
a particular quality should use that variant's audio rather than the top-level
compatibility field.

`variants` contains video candidates: recognized audio-only entries in a mixed
master are excluded from quality selection and stream export. A master with
only such audio entries reports an unsupported source. Missing resolution or
unrecognized codec metadata alone does not exclude a variant; `height: 0`
continues to mean unknown video resolution. Direct media playlists remain
supported.

Downloads are saved as Matroska files with the selected video, separate audio
when available, and the default or English subtitle track when available.
Playback and downloads use audio from the selected video's
[HLS rendition group](https://www.rfc-editor.org/rfc/rfc8216#section-4.3.4.2).
The group's default rendition is preferred, then its first `AUTOSELECT=YES`
rendition, then its first available rendition. Equal priorities retain playlist
order. A variant without a separate audio rendition uses its embedded audio.
Existing files are never overwritten; failed or cancelled downloads do not
leave a completed file or mark episodes as watched.

Ordinary filenames keep their existing format. When a long title needs
shortening, the name keeps the episode number and SUB/DUB label, plus a stable
tag to distinguish series with the same prefix. Only the series title is
shortened, at a UTF-8 boundary, to keep the filename stem within 180 bytes.
Any remaining collision reports an error and leaves the existing file intact.

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
| Player and download diagnostics | `$XDG_STATE_HOME/miru/logs/player-*.log` and `download-*.log` |
| Cover cache | `$XDG_CACHE_HOME/miru/covers/` |

When XDG variables are unset, state defaults to `~/.local/state` and cache to
`~/.cache`. History is saved with a lock and atomic replacement. Lock contention is reported
after two seconds instead of blocking exit indefinitely. Pending preference
changes are coalesced; queued history changes are flushed on shutdown. An invalid
history file is reported and left intact.

History reads and saves are limited to 32 MiB; ani-cli import sources are limited
to 8 MiB. Unsupported file types and files over those limits are rejected without
replacing existing history. Reads follow symlinks to regular files. Saving replaces
the configured `library.json` path atomically, leaving an original symlink target
unchanged. Startup and offline commands retain normal signal termination until
Miru starts work that needs coordinated shutdown.

History entries are retained until explicitly removed. Importing history and
recording new playback do not prune older entries.

Each media job gets its own diagnostic log, and failures report that exact path.
Cleanup runs when jobs start and finish, keeping the 32 most recently modified
completed logs across playback and downloads. Logs for active jobs are
always preserved, including those belonging to another Miru instance.

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

Provider HTML is parsed in short-lived workers, with at most two active at once.
Each page is limited to 4 MiB and 20,000 markup delimiters; each worker has a
two-second deadline, a CPU limit and a 256 MiB address-space limit. Oversized,
overly complex or slow pages produce an error. Cancelling a request kills its
worker, so quitting does not wait for a busy HTML parser. These workers do not
require bubblewrap.

Playback and downloads require Linux and bubblewrap (`bwrap`). Each job runs in
its own network and PID namespaces, with a temporary HTTP/HTTPS proxy as its
only route to remote media. The proxy applies destination checks to every new
connection, including nested playlists, segments, encryption keys, subtitles and
redirect targets. It connects to the checked DNS answers without resolving the
host again. HTTPS uses CONNECT; certificate validation remains the player's or
ffmpeg's responsibility. The provider HTTP client separately rejects HTTPS
downgrades in its redirects.

ffmpeg and mpv also retain their protocol allowlist and 20-second network I/O
timeout. Encrypted HLS and separate audio/subtitles remain supported. VLC and
mpv-compatible custom players use the same proxy; a player that ignores it has
no direct access to the host network. Ambient proxy and `NO_PROXY` settings are
replaced for media jobs, so upstream corporate proxies and host TCP-based player
control are not supported by this boundary. Filesystem access and Unix sockets
for local display/audio remain available. The temporary proxy is removed when
the job ends, and there is no unprotected fallback if isolation fails.

Search, history, imports and JSON exports do not need bubblewrap. URLs exported
by `--resolve` are ordinary source URLs; consumers launching their own tools
must apply their own policy.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/rename_smoke.py target/release/miru
python3 tests/smoke.py target/release/miru
python3 tests/network_smoke.py target/release/miru
python3 tests/parser_smoke.py target/release/miru
python3 tests/jobs_smoke.py target/release/miru
python3 tests/terminal_smoke.py target/release/miru
python3 tests/navigation_smoke.py target/release/miru
python3 tests/theme_smoke.py target/release/miru
```

The smoke tests use isolated temporary state and local provider fixtures. The
media, network and navigation checks need ffmpeg and bubblewrap. The media and
network checks also need mpv, with ffprobe for the media check and OpenSSL for
the network check's temporary TLS fixture. The job checks need bubblewrap and
use Python player/ffmpeg wrappers and Linux process identities to verify
descendant termination and reaping. The terminal checks use a pseudo-terminal
and do not replace visual testing in every terminal.

Run just the optional HLS audio-default checks with
`python3 tests/smoke.py target/release/miru --audio-fallback-only`. The fixture
verifies the expected audio in downloads and mpv playback with `DEFAULT=YES`,
`DEFAULT=NO`, and the attribute omitted. It also checks default/autoselect
priority and stable selection among equally ranked renditions.
