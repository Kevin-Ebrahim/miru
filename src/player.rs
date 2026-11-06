use crate::model::Stream;
use anyhow::{Context, Result};
use std::{path::Path, process::Stdio};
use tokio::process::{Child, Command};

pub fn player_args(player: &str, stream: &Stream, quality: &str, title: &str) -> Vec<String> {
    let (variant, _) = stream.select(quality);
    let mut args = Vec::new();
    if player.contains("vlc") {
        args.extend([
            "--play-and-exit".into(),
            format!("--http-referrer={}", stream.referer),
            format!("--meta-title={title}"),
        ]);
        let slaves = [stream.audio.as_deref(), stream.subtitle.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("#");
        args.push(variant.url.clone());
        if !slaves.is_empty() {
            args.push(format!(":input-slave={slaves}"));
        }
    } else {
        // Custom player executables use mpv's argument contract (no shell evaluation).
        args.extend([
            format!("--referrer={}", stream.referer),
            format!("--force-media-title={title}"),
            "--no-terminal".into(),
        ]);
        if let Some(url) = &stream.audio {
            args.push(format!("--audio-file={url}"));
        }
        if let Some(url) = &stream.subtitle {
            args.push(format!("--sub-file={url}"));
        }
        args.push(variant.url.clone());
    }
    args
}

pub fn launch(
    player: &str,
    extra: &[String],
    stream: &Stream,
    quality: &str,
    title: &str,
    log: &Path,
) -> Result<Child> {
    let output = std::fs::File::create(log).context("Could not create player log")?;
    Command::new(player)
        .args(extra)
        .args(player_args(player, stream, quality, title))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(output)
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("Could not start {player}. Install it or set --player /path/to/player")
        })
}
