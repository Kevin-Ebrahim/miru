# Miru guide

[Install Miru and play your first episode](../README.md#install).

## Controls

Press `?` while browsing to open Help. Press `/` there to find a key or action.
Esc leaves a text field or closes a panel.

- Press Alt+h to open history. Miru records an episode when mpv starts playback. For VLC and wrappers without a playback notification, it waits for a successful exit. Opening the series highlights the next episode.
- Press `i` for series details. Press `y` to copy the panel text, or drag to copy a passage.
- In Episodes, press Space to mark individual episodes or `:` to enter a range. Press `d` to download the marked episodes. With none marked, `d` downloads the highlighted episode.
- Press `x` to stop the current queue. If a queue fails, Alt+r retries from the failed episode.

Episode ranges work in the app and with `-e` on the command line. Use `1-3`
for a range, `1,5,8` for individual episodes, `-1` for the last episode and
`0--1` for all episodes. Decimal specials such as `1.5` work too.

## Providers

Miru starts with HiAnime. Press Alt+p on Home or Search to change providers.
If a search returns no matches, press Alt+p to try another provider.
Press `a` on a selected series or after a search request fails
to look it up on the next provider. Pick the matching series and episode there.
Your history and queues stay with their original provider.

| Provider | Playback | Downloads |
| --- | --- | --- |
| HiAnime | mpv or VLC | Yes |
| AnimeGG | mpv or VLC | No |
| KAA | mpv | Yes |

KAA requires a request header that VLC can't send. To choose a provider for
one run, use `miru --source kaa "Cowboy Bebop"`. Set `MIRU_SOURCE` to
`hianime`, `animegg` or `kaa` for a persistent shell default.
Use `--provider URL` or `MIRU_PROVIDER` only for a compatible endpoint of the
selected provider. Clear that override when switching sites.

## Command-line use

```sh
miru --list frieren
miru --play frieren -S 1 -e 1 -q 720p
miru --download frieren -e 1-3 --download-dir ./downloads
miru --resolve frieren -e 1
miru --continue
```

`-S` selects the search result, counting from 1. Use `--exact` to match the
full title. `--continue` opens interactive history. Run `miru --help` for all options.

For scripts, `--list` and `--history` print JSON arrays. `--resolve` prints one
JSON object per episode. Pick a video from `variants` and use that variant's
`audio.url` for separate audio when present. The top-level `audio` describes the
best variant. Use `referer` and any `request.origin` and `request.user_agent`
headers for video, audio and subtitles.

## Settings

Press `,` while browsing. Select a row and use Left or Right to change a choice,
Enter to edit a field, or `R` to restore its default. Changes apply to new
queues. Command-line flags override saved settings for one run.

Miru uses mpv by default. Choose VLC with `--player vlc`. For a wrapper, set
`--player-backend mpv` or `--player-backend vlc` to match the arguments it
accepts. Repeat `--player-arg=--option=value` to pass individual arguments.

### Download location

Miru saves episodes as Matroska files under `miru/` in your system Downloads
folder and leaves existing files intact. To save elsewhere, choose a folder in
Settings. Use an absolute path or one starting with `~/`. For one run,
`--download-dir .` saves directly in the current directory.

## Data and configuration

| Data | Default Linux path |
| --- | --- |
| Settings | `~/.config/miru/config.json` |
| Watch history | `~/.local/state/miru/library.json` |
| Playback and download logs | `~/.local/state/miru/logs/` |
| Cached covers | `~/.cache/miru/covers/` |

Miru respects `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME`.
Use `MIRU_STATE_DIR` or `MIRU_CACHE_DIR` to override Miru's own directories.

## Troubleshooting

- If covers look wrong, try `miru --images halfblocks`. For a plain interface, use `miru --images off --ascii --no-animation`. Use `--theme terminal` to choose terminal colors over the active Omarchy theme.
- If a search or cover fails to load, press `R` while browsing to refresh it. For player and download errors, check the log path in the error message.

### Network policy

Provider requests ignore system proxy settings. Playback and downloads run inside bubblewrap on Linux. Miru checks remote
destinations through its own proxy and blocks local and reserved addresses,
except for an explicitly configured local provider. Media jobs can't use a
corporate proxy or host TCP player controls. Players still have access to local
files and Unix sockets. Browsing, history and JSON exports work without
bubblewrap. If you pass exported URLs to another tool, that tool makes its own
network requests.

## Development

To run the [CI checks](../.github/workflows/ci.yml) locally, use these commands.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
```

To run an offline smoke test, pass it the binary, for example
`python3 tests/cli_smoke.py target/debug/miru`. For the full suite and its
Linux dependencies, see the manual [Integration tests workflow](../.github/workflows/integration.yml).
