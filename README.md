# ani / watch

A Rust + Ratatui anime browser for the terminal. Live search, native cover images,
episode queues, external playback, watch history, and downloads.

This is a separate application. The shell reference, including the local changes
in `/home/kevin/Dev/ani-cli`, remains intact. Nothing installs over `ani-cli`.

## Run locally

Requires Rust 1.90 or newer and a Unix terminal. Install **mpv** for playback
(or use `--player vlc`) and **ffmpeg** for downloads.

```sh
cargo build --release --locked
./target/release/ani-watch
./target/release/ani-watch frieren
./target/release/ani-watch --continue
```

The app opens on **Home**, showing your eight most recently played series and
the highlighted anime's cover. Enter opens its episodes with the next episode
selected; Enter again plays it. History records launches, not completed viewing.

Press `/` to search. Type a title, press Enter to focus the results, then Enter
to browse episodes. `H` returns Home, `h` opens full history, and Esc retraces
your navigation, preserving queries, selections and scroll positions. `i` opens
full cover/details, including in compact terminals. Highlighting a series loads
its full synopsis automatically in the preview and caches it for Details.
F5 retries the full synopsis along with the current view.
Drag over the text in Details or a series preview to select it; releasing the mouse
copies only that selection. This includes the title, episode, metadata and synopsis.
Outside text input, press `y` to copy the selection again, or the full text panel
when nothing is selected. The mouse wheel over a text panel scrolls its content.
Browsing leaves playback and downloads running; `x` stops the current job or
dismisses its result.
Network requests, cover downloads, image decoding, and image encoding run
outside the terminal input loop.

## Controls

| Key | Action |
| --- | --- |
| `H` | Open Home / recently played |
| `/` | Open or edit catalogue search |
| `i` | Full cover and series details; ↑/↓ scroll, Esc closes |
| Enter | Focus results / browse series / play episode(s) |
| ↑ ↓ or `j` `k` | Navigate |
| Page Up / Page Down | Move ten items, or scroll ten lines in details |
| Home / End or `g` / `G` | First / last item; Home / End also scroll details |
| Ctrl+E | Toggle normalized exact title matching |
| `D` | Toggle SUB / DUB |
| `Q` | Cycle best, 1080p, 720p, 480p, 360p, worst |
| `h` | Open watch history |
| Tab / Space in episodes | Mark or unmark an episode |
| `:` in episodes | Enter a range; Enter marks it |
| `d` in episodes | Download marked episodes, or the highlighted episode |
| `n` / `p` / `r` | Next / previous / replay |
| `x` | Stop the owned player/download queue, or dismiss its result |
| F5 or `R` | Retry the current view and artwork; F5 also works while typing |
| F6 | Retry the failed episode and remaining queue, keeping its original settings |
| Delete in history | Remove the highlighted history entry |
| `?` or F1 | Help; ↑/↓ scroll the guide |
| Esc | Leave input / return to the previous screen / close help or details |
| `q` outside text input, or Ctrl+C anywhere | Quit and stop owned jobs |

While typing a search, printable keys are text. The other single-letter shortcuts
apply after leaving the input. F1, F5, F6 and Ctrl+C also work while typing.
The footer shows the actions available for the current focus; F1 lists all
shortcuts. At 120 columns and above, navigation and language/quality controls
move to the top toolbar, alongside the search input on Search. Secondary actions
sit at the bottom left, with Help, Quit and the primary action on the right.
The footer uses one row when everything fits and wraps when needed. Narrower
terminals retain the separate search field and compact shortcut rows.
Loading and error feedback stays with the affected view.

Ranges follow the provider's episode order, including decimal specials. Examples:
`1-3`, `1, 5, 8`, `1.5-3`, `0--1` (all), and `-1` (last). A range is validated
in full before anything launches. Queued playback waits for each player to exit.
Each queue keeps its selected series, episodes, language and quality while you
browse elsewhere. Language and quality changes apply to your next play/download
selection; they do not change an existing queue. Missing resolutions fall back to
best with a notice. SUB and DUB history are tracked separately.
After a queue fails, F6 retries from the failed episode without repeating completed
episodes. Failed episode refreshes keep the cached list and selection visible.

## Script-friendly operations

```sh
./target/release/ani-watch --list frieren
./target/release/ani-watch --list --exact "Frieren: Beyond Journey's End"
./target/release/ani-watch --resolve frieren -S 1 -e 1
./target/release/ani-watch --play frieren -e 1-3 -q 720p
./target/release/ani-watch --download frieren -e 1-3 --download-dir ./downloads
./target/release/ani-watch --history
./target/release/ani-watch --import-history ~/.local/state/ani-cli/ani-hsts
```

`--list` and `--history` print JSON arrays. `--resolve` prints one JSON stream
record per episode without launching a player. `-S` is a one-based search result
index; `-e` is used with `--play`, `--download`, or `--resolve`. `--sub`, `--dub`,
and `--quality` override saved preferences. Use `--player-arg=--option=value` to
pass an additional argument; repeat for multiple arguments. Custom player paths
use mpv's option contract, and VLC is recognized by its executable name. No
shell interpolation is used for titles, URLs, or player arguments.

Downloads are Matroska files containing the selected video, separate audio when
present, and the default/English subtitle track when present. They are written
to a temporary file and moved into place only on success. Existing files are
never overwritten. Cancelling a download removes its partial output. Downloads
do not mark episodes as watched.

History records the last **launched** episode, like the shell reference; it is
not a playback-position or completion tracker. Continue selects the episode
after that entry and reports when there are no newer episodes. Import reads
compatible tab-separated `ani-hsts` entries, skips obsolete provider IDs, and
does not modify the source. Legacy history has no language field, so imported
entries are SUB. Import is explicit and can be repeated without duplicates.

## Images, themes, and layout

The default is `--images auto`. `ratatui-image` queries the terminal before input
handling starts. Supported native protocols are Kitty, Sixel, and iTerm2, with
colored halfblocks as a fallback. There is no blanket Herdr-to-text downgrade.
Kitty images use the library's encoder and virtual placements with owned image
IDs; cleanup deletes only this application's image data.

```sh
./target/release/ani-watch --images kitty frieren
./target/release/ani-watch --images halfblocks frieren
./target/release/ani-watch --images off --ascii --no-animation
```

`NO_COLOR` disables color and images, because Kitty placeholders require color
codes to identify the image. If your launcher exports it unintentionally, use
`env -u NO_COLOR ./target/release/ani-watch` for native images. `--ascii` changes
interface borders and indicators; it does not transliterate anime titles.

The interface keeps the terminal's font and text size. By default, `--theme auto`
uses Omarchy's active `colors.toml`, including its foreground, background, accent,
surface and status colors. Focus uses the theme accent; selections use a tint of
that accent. It checks `$XDG_STATE_HOME/omarchy/current/theme`
(normally `~/.local/state/omarchy/current/theme`), then the older
`$XDG_CONFIG_HOME/omarchy/current/theme` location. Changes appear within about a
second while the app is open, including when images are disabled. Text and focus
colors are adjusted only where needed for readable contrast.

Without a usable desktop palette, or over SSH, the app uses the terminal's default
foreground/background and ANSI colors. These follow the terminal's own palette;
selection uses reverse video. Use `--theme terminal` or `ANI_WATCH_THEME=terminal`
when the terminal has a different theme from the desktop. This mode leaves panel
backgrounds at the terminal default. `NO_COLOR` takes precedence over both modes.

Body text is regular; headings and focused controls use bold. Heavy panel borders
become double borders in focus. Symbols and labels preserve meaning even in
monochrome themes.
Home and Search show a list on the left and the selected anime's aspect-preserving
cover, title and details on the right. Episodes use a wider list and a smaller
cover, without a persistent search field. A separate activity strip shows the
current playback/download job and its outcome across screens. Selected episodes
and marked batches have distinct indicators.

At 72 columns and above, sufficiently tall terminals show both panels. Narrow,
tall terminals stack them; shorter terminals prioritize the list, with the full
preview available through `i`. Below 32 × 12 the app displays a resize hint.
Full details place the cover beside a reading column on wide screens and above
the text on narrow screens. The synopsis uses the remaining height, wraps to a
readable width, and scrolls with arrows, Page Up/Down, or Home/End.
Help and full series details are scrollable. Provider failures have their own
retry state, separate from a successful search with no matches. Failed or missing
covers leave the selected title and details visible with a quiet placeholder.

An empty Home welcomes you with
[Kyoto Dusk from ascii.rest](https://ascii.rest/kyoto-dusk/): a cherry tree, a
five-storey pagoda, a crescent moon, and a glowing stone lantern reflected in a
pond. The scene belongs only to this welcome state; it never substitutes for an
anime cover, search error or loading result. Coloured Unicode dots preserve its
dusk palette and shading as it scales. `--ascii` uses ASCII shading; `NO_COLOR`
uses monochrome ASCII shading. `--no-animation` holds a still frame. Very small
panels show only the search prompt.

The animation is bundled and plays natively in Rust, without network access or
a JavaScript runtime. Its MIT-licensed source and licence are in
[`assets/kyoto-dusk/`](assets/kyoto-dusk/). To regenerate the compressed cell
frames with Node 22.18 or newer, run `node scripts/generate-dusk.mjs`.

Search is debounced by 300 ms and immediately filters currently loaded results
while new results are pending. Provider alias matches remain visible when the
response arrives. Search generations, episode generations, playback job IDs,
and artwork generations reject stale responses. Image selection has a separate
100 ms delay to avoid fetching everything crossed during fast navigation.

Searches use a 32-entry, ten-minute in-memory cache. Covers use a 12-image decoded
LRU and a disk cache capped at 128 files / 128 MiB, with a 30-day freshness limit.
Corrupt cached images are fetched again. Failed covers leave navigation usable.
Selection, empty searches, help, resizing, and exit clear the previous image.

## Local data and troubleshooting

| Data | Default location |
| --- | --- |
| History and language/quality preferences | `$XDG_STATE_HOME/ani-watch/library.json` |
| Last player / download errors | Same directory, `player.log` / `download.log` |
| Cover cache | `$XDG_CACHE_HOME/ani-watch/covers/` |

XDG fallbacks are `~/.local/state` and `~/.cache`. Use `--state-dir`, `--cache-dir`,
`ANI_WATCH_STATE_DIR`, and `ANI_WATCH_CACHE_DIR` for isolation. History updates
use a file lock and atomic replacement. Malformed history is reported with its
path and preserved, rather than silently discarded.

The provider defaults to `https://hianime.at/`, using its episode API and the
ZokoAnime embed format observed in the shell reference. `--provider` or
`ANI_WATCH_PROVIDER` overrides the base URL for testing. Only the ZokoAnime
server is supported, matching the current reference resolver. HTTP failures,
provider challenges, missing language sources, and changed markup are reported
in the UI; `?` also shows the current error in full. This does not circumvent
provider challenges or guarantee future provider availability.

This version targets local Linux/Unix use. Shell-specific integrations such as
Android intents, Chromecast, Syncplay, ani-skip, release countdowns, rofi/dmenu,
system-log viewing, and self-update are not implemented. VLC command construction
is supported but only mpv was exercised on this machine. Sixel and iTerm2 use the
library backends and have not been visually tested here.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python tests/smoke.py
python tests/terminal_smoke.py
python tests/navigation_smoke.py
```

Unit tests cover HTML parsing, embed decoding, HLS relative URLs/audio/quality,
episode ranges, stale responses, Unicode input, responsive layouts, history
continuation, atomic persistence and concurrent writers. `tests/smoke.py` serves
a local provider fixture and generates a one-second media clip: it runs real mpv
and ffmpeg, validates audio/video/subtitle tracks, checks history, and checks that
an existing download is preserved. It needs Python, mpv, ffmpeg and ffprobe.
`tests/terminal_smoke.py` emulates cursor replies in a PTY and checks resizing and
terminal restoration for q, Ctrl+C, SIGTERM and SIGHUP. It is not a graphics test.
`tests/navigation_smoke.py` uses a local provider, controllable player process and
generated one-second clip (requires ffmpeg) to verify Home continuation, navigation
during resolution/playback/downloads, queue ownership across series, correct
history, fixed queue language, explicit stop, cached refresh failures, and partial
download recovery without repeating saved episodes.

Implementation references: [Ratatui](https://ratatui.rs/),
[ratatui-image](https://docs.rs/ratatui-image/11.1.0/ratatui_image/),
[Kitty graphics](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
[Herdr graphics configuration](https://herdr.dev/tui/docs/configuration/).

Licensed under GPL-3.0; see [LICENSE](LICENSE). The shell behavior and provider
integration were studied from the locally modified
[pystardust/ani-cli](https://github.com/pystardust/ani-cli) checkout.
