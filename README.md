<p align="center">
  <img src="assets/branding/logo.svg" alt="Miru" width="480">
</p>

<p align="center">Search, watch and download anime from your terminal.</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.90%2B-dea584?style=flat-square" alt="Requires Rust 1.90 or newer">
</p>

## Install

Build Miru with Git and [Rust 1.90+ with Cargo](https://rustup.rs/).
Playback and downloads require Linux and [bubblewrap](https://github.com/containers/bubblewrap).
Install [mpv](https://mpv.io/installation/) to watch or [FFmpeg](https://ffmpeg.org/download.html) to download.

```sh
git clone https://github.com/Kevin-Ebrahim/miru.git
cd miru
cargo install --locked --path .
miru
```

If your shell can't find `miru`, add `$HOME/.cargo/bin` to your `PATH`.

## Watch an episode

Type a title and press Enter to search. Select a series and press Enter to see its episodes.
Select an episode and press Enter to play it.

Use Alt+a to switch between SUB and DUB, Alt+q to change quality, and Alt+h
to open watch history. Press `?` for controls (Esc then `?` while typing) or Ctrl+C to quit.

See the [guide](docs/guide.md) for downloads, providers, settings and command-line examples.

## License and credits

Miru uses the [GPL-3.0-only license](LICENSE). The [sakura artwork](assets/sakura/README.md)
draws on [Kyoto Dusk by bas3line](assets/kyoto-dusk/LICENSE).
[ani-cli](https://github.com/pystardust/ani-cli) inspired Miru's shell commands and provider integration.
