use crate::{
    model::Stream,
    network::{Session, Tool},
};
use anyhow::{Context, Result};
use std::{path::Path, process::Stdio};
use tokio::{process::Child, sync::oneshot};

pub fn player_args(player: &str, stream: &Stream, quality: &str, title: &str) -> Vec<String> {
    let (variant, _) = stream.select(quality);
    let mut args = Vec::new();
    if player.contains("vlc") {
        args.extend([
            "--play-and-exit".into(),
            format!("--http-referrer={}", stream.referer),
            format!("--meta-title={title}"),
        ]);
        let slaves = [variant.audio_url(), stream.subtitle.as_deref()]
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
            "--network-timeout=20".into(),
            format!(
                "--demuxer-lavf-o-append=protocol_whitelist={}",
                crate::network::MEDIA_PROTOCOLS
            ),
            "--demuxer-lavf-o-append=rw_timeout=20000000".into(),
        ]);
        if let Some(url) = variant.audio_url() {
            args.push(format!("--audio-file={url}"));
        }
        if let Some(url) = &stream.subtitle {
            args.push(format!("--sub-file={url}"));
        }
        args.push(variant.url.clone());
    }
    args
}

pub async fn launch(
    player: &str,
    extra: &[String],
    media: &Session,
    quality: &str,
    title: &str,
    log: &Path,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<Child>> {
    let output = std::fs::File::create(log).context("Could not create player log")?;
    let mut child = media.command(player, Tool::Player)?
        .args(extra)
        .args(player_args(player, media.stream(), quality, title))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(output)
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("Could not start protected {player}. Install bubblewrap and the player, or set --player /path/to/player")
        })?;
    if !media
        .wait_started(&mut child, cancel)
        .await
        .with_context(|| format!("Could not start {player}; see {}", log.display()))?
    {
        return Ok(None);
    }
    Ok(Some(child))
}
