<p align="center">
  <img src="assets/branding/logo.svg" alt="Miru" width="480">
</p>

<p align="center">Search, watch and download anime from your terminal.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.90%2B-dea584?style=flat-square" alt="Requires Rust 1.90 or newer">
</p>

Miru has SUB/DUB selection, cover art, watch history and episode queues.
Navigate with arrow keys or Vim bindings. Colours follow your terminal or
Omarchy theme.

## Install

You need Git, [Rust 1.90+ and Cargo](https://rustup.rs/),
and [mpv](https://mpv.io/installation/) for playback.
Install [ffmpeg](https://ffmpeg.org/download.html) if you want downloads.
Playback and downloads require Linux and [bubblewrap](https://github.com/containers/bubblewrap)
(`bwrap`) for media network isolation. Browsing, history and JSON exports do not
require bubblewrap.

```sh
git clone https://github.com/Kevin-Ebrahim/miru.git
cd miru
cargo install --locked --path .
miru
```

If your shell cannot find `miru`, add Cargo's bin directory to your `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

## Watch something

1. Type a title on Home and press Enter.
2. Select an anime and press Enter to open its episodes.
3. Select an episode and press Enter to play it.

Use Tab on Home to select audio or quality, then the arrow keys to change it.

| Key | Action |
| --- | --- |
| ↑ / ↓ or `j` / `k` | Move through a list |
| Esc | Go back or leave text input |
| `s` / Shift+Q | Change audio / quality |
| Alt+h | Open history |
| Space, then `d` | Mark episodes, then download them |
| F1 | Show all controls |
| Ctrl+C | Quit |

History remembers the last episode launched, not playback position or completion.

Downloads go to your system Downloads folder under `miru/` by default. Set a
saved download folder in the [configuration file](docs/guide.md#download-location),
or use `--download-dir PATH` to override it for one run.

## From the command line

```sh
miru frieren
miru --continue
miru --play frieren -e 1-3 -q 720p
miru --download frieren -e 1-3 --download-dir ./downloads
```

Run `miru --help` for all options. See the [guide](docs/guide.md) for controls,
JSON output, configuration, history migration and development checks.

## Troubleshooting

- If covers render incorrectly, try `miru --images halfblocks` or `--images off`.
- For plain output, use `miru --images off --ascii --no-animation`.
- If a request fails, press F5 to retry. Miru is an unofficial HiAnime client;
  provider outages and site changes can interrupt search or playback.

## License and credits

[GPL-3.0-only](LICENSE). Kyoto Dusk by [bas3line](https://github.com/bas3line)
is bundled under its [MIT license](assets/kyoto-dusk/LICENSE).
See the [sakura artwork notes](assets/sakura/README.md) for its reference.
The shell workflow and provider integration draw on
[ani-cli](https://github.com/pystardust/ani-cli).
