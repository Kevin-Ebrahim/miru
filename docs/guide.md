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
| F2 | Change provider on Home or Search |
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

Click a list item to select it; double-click to open a series or play an episode,
using the same action as Enter. The wheel moves one item at a time over a list
and scrolls text over a synopsis or Help. Click SUB/DUB to switch audio, or the
quality label to cycle playback quality. Keyboard controls remain available.

Click a cover to view it enlarged. Press Esc or click anywhere to close the viewer
and return to the same browsing position.

Dragging preview or details text copies the selected text. Navigation before a
new drag targets the newly selected series. Resizing, moving the reading area,
or opening Help clears unfinished selections.

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
VLC always starts a separate instance, overriding single-instance preferences
and player arguments so playback stays under Miru's network policy and
cancellation. Its libproxy discovery uses Miru's proxy environment even when
inherited `PX_FORCE_CONFIG` settings select desktop or PAC configuration.

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
| Download folder preference | `$XDG_CONFIG_HOME/miru/config.json` |
| Downloaded episodes | `<system Downloads>/miru/` |
| History, language and quality preferences | `$XDG_STATE_HOME/miru/library.json` |
| Player and download diagnostics | `$XDG_STATE_HOME/miru/logs/player-*.log` and `download-*.log` |
| Cover cache | `$XDG_CACHE_HOME/miru/covers/` |

When XDG variables are unset, state defaults to `~/.local/state` and cache to
`~/.cache`. History is saved with a lock and atomic replacement. Lock contention is reported
after two seconds instead of blocking exit indefinitely. Pending preference
changes are coalesced; queued history changes are flushed on shutdown. An invalid
history file is reported and left intact.

History reads and saves are limited to 32 MiB; ani-cli import sources are limited
to 8 MiB and the download-location config to 64 KiB. Unsupported file types and
files over those limits are rejected without replacing existing history. Reads
follow symlinks to regular files. Saving replaces the configured `library.json`
path atomically, leaving an original symlink target unchanged. Startup and offline
commands retain normal signal termination until Miru starts work that needs
coordinated shutdown.

With mpv, history updates when playback starts, not when the player process launches.
Failed loads leave the previous entry unchanged. VLC and custom wrappers that do
not emit mpv's start notification update history only after a successful exit;
cancelling those players does not advance history.

History entries are retained until explicitly removed. Importing history and
recording new playback do not prune older entries.

Each media job gets its own diagnostic log, and failures report that exact path.
Cleanup runs when jobs start and finish, keeping the 32 most recently modified
completed logs across playback and downloads. Logs for active jobs are
always preserved, including those belonging to another Miru instance.

### Download location

The TUI and CLI use the same download folder. Without an override, episodes go
into a `miru` subfolder of your system Downloads folder. On Linux, Miru respects
the Downloads location in XDG `user-dirs.dirs`, including renamed or relocated
folders. If no location is configured, it uses `~/Downloads/miru`. The folder is
created when a download starts.

To save a custom default, create `~/.config/miru/config.json` (or
`$XDG_CONFIG_HOME/miru/config.json` if set):

```json
{
  "download_dir": "~/Videos/anime"
}
```

On macOS, the config file is `~/Library/Application Support/miru/config.json`.
The saved directory must be an absolute path or start with `~/`, so launching
Miru from another directory does not change it. Episodes are saved directly in
the chosen directory. Omit `download_dir`, set it to `null`, or remove the file
to restore the automatic default. Until a settings page is available, edit this
file to change the saved preference.

`--download-dir PATH` overrides the location for one run without modifying the
config file. Relative paths are allowed for this flag; use `--download-dir .`
to save into the current working directory. Downloads previously saved in your
launch directory stay where they are. A destination that cannot be created is
reported as an error; Miru does not switch to another folder.

### Environment overrides

| Environment variable | Command-line equivalent | Purpose |
| --- | --- | --- |
| `MIRU_STATE_DIR` | `--state-dir` | Override the state directory |
| `MIRU_CACHE_DIR` | `--cache-dir` | Override the cover cache directory |
| `MIRU_THEME` | `--theme` | Set `auto` or `terminal` |
| `MIRU_SOURCE` | `--source` | Choose `hianime` (default), `animegg` or `kaa` |
| `MIRU_PROVIDER` | `--provider` | Override the selected source's base URL |

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

Miru supports HiAnime (the default), AnimeGG and KAA. AnimeGG downloads are disabled
because sustained transfers have been unreliable. Its episode view hides the
download action, and `--source animegg --download` fails before network access.
HiAnime and KAA downloads remain available, regardless of the current search source.
Select a source with F2 or by
clicking its name on Home or Search. From the CLI:

```sh
miru --source animegg "Cowboy Bebop"
miru --source animegg --dub --play "Cowboy Bebop" -e 1
```

Switching sources reruns the current search. Running playback, queued episodes
and history retain their original provider. The search source is a per-run choice;
use `MIRU_SOURCE` for your shell default. There is no automatic cross-provider
fallback or history merging.

`--provider URL` / `MIRU_PROVIDER` overrides the selected source's endpoint only.
It must implement that source's API; it does not select an adapter. Explicit CLI
values override environment values, including the older `ANI_WATCH_PROVIDER` alias.
History entries from other sources use their own configured adapter.

AnimeGG and KAA can have different SUB/DUB episode lists. Changing audio reloads that list
and clears marks; selection follows the episode number when available. Existing
queues keep their captured language. AnimeGG combined releases such as `277-278`
remain one item; selecting either covered episode plays that combined release. Missing
individual episodes are not replaced by the nearest list position.

AnimeGG quality labels are advertised values, not measurements of video dimensions.
`--resolve` preserves the existing JSON fields and adds optional `label` and `format`
metadata to variants (`format: "direct"` distinguishes direct media from HLS).
The adapter resolves the site's direct sources; third-party backup embeds are not
supported. Site challenges, missing language sources and changed markup can still
interrupt playback. Short playback tests do not establish full-episode reliability.
Errors appear in the affected view, and F5 retries it. Check state-directory logs
for player failures.

KAA selects Japanese audio for SUB and English audio for DUB from the HLS
manifest, regardless of track order or default flags. If an English episode has
no server, Miru may try the exact same episode number in the same series' Japanese
listing, then require English audio in that manifest. It never substitutes audio
or matches by list position. Missing sources remain explicit errors.

KAA exposes available English subtitles and advertised video dimensions. Its
player properties are parsed in the isolated parser. Media uses a validated shared
request profile for video, separate audio and subtitles; JSON includes optional
`request.origin` and `request.user_agent` alongside the existing `referer`.
mpv and ffmpeg apply these headers to nested media requests. VLC cannot apply a
required Origin header, so KAA playback reports an explicit error asking for mpv.
No player is silently substituted. KAA supports downloads through ffmpeg.
Provider connection or segment errors can still occur; failed transfers remove
the partial file instead of publishing it as a completed episode.

### Search another provider

Press F3 on a series, in its episode view, or after a failed search to search
the same title on the next provider. The footer names the destination provider;
use F2 in the resulting search to choose another source.
The requested SUB/DUB mode is retained. Choose the matching series or season and
then the episode yourself; Miru does not match episodes across providers.
Esc returns to the previous view.

Searching does not replace active or failed playback, change its queue, or merge
history. Starting a replacement uses the normal stop-and-replace behavior and
records the new provider separately. CLI failures identify the provider and stage,
and suggest a `--source` search command. Replace `<title>` in that example with
your quoted search text. Remove any `--provider` / `MIRU_PROVIDER` override
when switching to another site.

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

There are three GitHub Actions workflows:

| Workflow | When it runs | Purpose |
| --- | --- | --- |
| CI / Build and test | Pull requests to `main`, pushes to `main`, or manually | Formatting, Clippy, Rust unit tests and a debug build. |
| Integration tests | Manually | Release build and all Python smoke tests, including real mpv/VLC playback, downloads and terminal behavior. Uses Ubuntu 26.04 for FFmpeg 8. |
| CodeQL weekly | Weekly or manually | Static security analysis. |

Only `Build and test` should be a required PR check. Routine CI reuses Cargo's
debug profile and does not install system packages or compile an optimized
release. Run `Integration tests` from the Actions tab when changing playback,
downloads, storage or terminal behavior, and before a release. Its smoke suites
all run even if another suite fails. The workflow must land on `main` before
GitHub offers its Run workflow button; you can then select the branch to test.

Run the routine checks locally:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
```

Run the integration checks locally (or pass either build to an individual script):

```sh
cargo build --release --locked
for suite in tests/*smoke.py; do
  python3 "$suite" target/release/miru || exit 1
done
```

The smoke tests use isolated temporary state and local provider fixtures. The
media, network and navigation checks need ffmpeg and bubblewrap. The media and
network checks also need mpv, with ffprobe for the media check and OpenSSL for
the network check's temporary TLS fixture. The VLC check needs ffmpeg, bubblewrap,
VLC, `dbus-daemon` and `dbus-send`. It verifies private-request blocking, allowed
playback despite a system proxy resolver choosing direct connections, and player
ownership with an existing VLC on a disposable D-Bus session. Miru supplies its
own session-scoped `proxy` helper to VLC, so libproxy-tools is not required.
The job checks need bubblewrap and use Python player/ffmpeg wrappers and Linux
process identities to verify
descendant termination and reaping. The terminal checks use a pseudo-terminal
and do not replace visual testing in every terminal.

Run just the optional HLS audio-default checks with
`python3 tests/smoke.py target/release/miru --audio-fallback-only`. The fixture
verifies the expected audio in downloads and mpv playback with `DEFAULT=YES`,
`DEFAULT=NO`, and the attribute omitted. It also checks default/autoselect
priority and stable selection among equally ranked renditions.

### CodeQL schedule

The `CodeQL weekly` workflow scans Rust, Python and JavaScript/TypeScript on
Mondays at 02:17 UTC (04:17 South African time), with a manual-run option in
Actions. It has no push or pull-request triggers and should not be a required
PR check. Scheduled runs analyze the default branch.

When switching from GitHub's CodeQL Default setup, merge this workflow first,
then disable Default setup under Settings > Advanced Security > CodeQL analysis.
Default setup otherwise continues its own push and PR scans. After switching,
manually run `CodeQL weekly` once to verify analysis and result uploads.

### Settings

Press F4 to open Settings and Esc to return to the previous screen. Choose a row
with the arrow keys, use Left/Right to change it, Enter to edit, or R to restore its default.
PgUp/PgDn move through settings in short terminals; the selected row stays visible. The Advanced section is always visible and contains the player
executable, compatibility mode, and an argument list; arguments are passed
individually without shell expansion. Use Enter to add/edit an argument and
Delete to remove it.

Preferences are saved in `$XDG_CONFIG_HOME/miru/config.json` (normally
`~/.config/miru/config.json`). Existing audio and quality preferences are carried
forward from history on the first settings save. History entries are preserved.
CLI overrides apply only for the current run and are labelled on the page.
Changes affect new queues; episodes and retries in an existing queue retain its
original audio, quality, player, and download destination.
