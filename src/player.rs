use crate::{
    jobs::Log,
    model::Stream,
    network::{Session, Tool},
};
use anyhow::{Context, Result};
use std::{path::Path, process::Stdio};
use tokio::{process::Child, sync::oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    Mpv,
    Vlc,
}

#[derive(Clone)]
pub struct Player {
    executable: String,
    extra: Vec<String>,
    backend: Backend,
}

impl Player {
    pub fn new(
        executable: impl Into<String>,
        extra: Vec<String>,
        backend: Option<Backend>,
    ) -> Self {
        let executable = executable.into();
        let backend = backend.unwrap_or_else(|| {
            match Path::new(&executable)
                .file_name()
                .and_then(|name| name.to_str())
            {
                Some("vlc" | "cvlc") => Backend::Vlc,
                _ => Backend::Mpv,
            }
        });
        Self {
            executable,
            extra,
            backend,
        }
    }

    fn validate_request(&self, stream: &Stream) -> Result<()> {
        anyhow::ensure!(
            !matches!(self.backend, Backend::Vlc)
                || stream
                    .request
                    .as_ref()
                    .is_none_or(|profile| profile.origin.is_none()),
            "VLC cannot apply the required Origin header; use mpv for this stream"
        );
        Ok(())
    }

    pub fn args(&self, stream: &Stream, quality: &str, title: &str) -> Vec<String> {
        let (variant, _) = stream.select(quality);
        let mut args = self.extra.clone();
        if let Some(profile) = &stream.request {
            if let Some(agent) = &profile.user_agent {
                args.push(format!(
                    "{}={agent}",
                    match self.backend {
                        Backend::Mpv => "--user-agent",
                        Backend::Vlc => "--http-user-agent",
                    }
                ));
            }
            if let Some(origin) = &profile.origin {
                args.push(format!("--http-header-fields=Origin: {origin}"));
            }
        }
        match self.backend {
            Backend::Vlc => {
                // Delegating to a host VLC would escape network policy and cancellation.
                args.extend([
                    "--no-one-instance".into(),
                    "--no-one-instance-when-started-from-file".into(),
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
            }
            Backend::Mpv => {
                // Custom player executables use mpv's argument contract (no shell evaluation).
                args.extend([
                    format!("--referrer={}", stream.referer),
                    format!("--force-media-title={title}"),
                    "--terminal=yes".into(),
                    "--input-terminal=no".into(),
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
        }
        args
    }
}

pub async fn launch(
    player: &Player,
    media: &Session,
    quality: &str,
    title: &str,
    log: &Log,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<Child>> {
    player.validate_request(media.stream())?;
    let output = log.writer()?;
    let executable = &player.executable;
    let mut child = media.command(executable, Tool::Player(player.backend))?
        .args(player.args(media.stream(), quality, title))
        .stdin(Stdio::null())
        .stdout(output.try_clone().context("Could not capture player stdout")?)
        .stderr(output)
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("Could not start protected {executable}. Install bubblewrap and the player, or set --player /path/to/player")
        })?;
    if !media
        .wait_started(&mut child, cancel)
        .await
        .with_context(|| format!("Could not start {executable}; see {}", log.path().display()))?
    {
        return Ok(None);
    }
    Ok(Some(child))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Variant;

    fn stream() -> Stream {
        Stream {
            request: None,
            variants: vec![Variant {
                format: crate::model::MediaFormat::Hls,
                label: None,
                height: 720,
                url: "https://cdn.example/video.m3u8".into(),
                audio: None,
            }],
            referer: "https://example.org/".into(),
            subtitle: None,
        }
    }

    #[test]
    fn request_profiles_apply_user_agent_and_reject_unsupported_vlc_origin() {
        let mut stream = stream();
        stream.request = Some(crate::model::MediaRequest {
            origin: None,
            user_agent: Some("Probe Agent".into()),
        });
        for (name, expected) in [
            ("mpv", "--user-agent=Probe Agent"),
            ("vlc", "--http-user-agent=Probe Agent"),
        ] {
            let player = Player::new(name, vec![], None);
            assert!(player.validate_request(&stream).is_ok());
            assert!(
                player
                    .args(&stream, "best", "Probe")
                    .contains(&expected.to_owned())
            );
        }
        stream.request.as_mut().unwrap().origin = Some("https://embed.example".into());
        assert!(
            Player::new("vlc", vec![], None)
                .validate_request(&stream)
                .is_err()
        );
        assert!(
            Player::new("mpv", vec![], None)
                .args(&stream, "best", "Probe")
                .contains(&"--http-header-fields=Origin: https://embed.example".to_owned())
        );
    }

    #[test]
    fn vlc_parent_directories_do_not_change_mpv_arguments() {
        for player in [
            "mpv",
            "/tmp/vlc-backup/mpv",
            "/tmp/vlc backup with spaces/mpv",
            "/tmp/tools/my-vlc-wrapper",
        ] {
            let arguments =
                Player::new(player, vec![], None).args(&stream(), "best", "Routing Probe");
            assert!(
                arguments.contains(&"--input-terminal=no".into()),
                "{player}: {arguments:?}"
            );
            assert!(arguments.contains(&"--terminal=yes".into()));
            assert!(!arguments.contains(&"--play-and-exit".into()));
        }
    }

    #[test]
    fn vlc_and_cvlc_basenames_keep_the_vlc_argument_contract() {
        for executable in ["vlc", "cvlc", "/usr/bin/vlc", "/tmp/player paths/cvlc"] {
            let arguments = Player::new(executable, vec![], None).args(&stream(), "best", "Probe");
            assert!(arguments.contains(&"--play-and-exit".into()));
            assert!(!arguments.contains(&"--input-terminal=no".into()));
        }
    }

    #[test]
    fn vlc_cannot_delegate_playback_even_when_user_arguments_enable_it() {
        for extra in [
            vec![],
            vec![
                "--one-instance".into(),
                "--one-instance-when-started-from-file".into(),
            ],
        ] {
            let arguments =
                Player::new("vlc", extra.clone(), None).args(&stream(), "best", "Owned playback");
            for flag in [
                "--no-one-instance",
                "--no-one-instance-when-started-from-file",
            ] {
                let position = arguments
                    .iter()
                    .rposition(|argument| argument == flag)
                    .unwrap_or_else(|| panic!("Missing {flag}: {arguments:?}"));
                assert!(
                    position >= extra.len(),
                    "User arguments can override {flag}"
                );
            }
        }
    }

    #[test]
    fn explicit_backends_override_names_and_keep_extra_arguments_literal() {
        let extra = vec![
            "--no-config".into(),
            "--vo=null".into(),
            "--ao=null".into(),
            "--custom=a b;$(literal)".into(),
        ];
        for (executable, backend, expected, excluded) in [
            (
                "/tmp/player paths/vlc",
                Backend::Mpv,
                "--input-terminal=no",
                "--play-and-exit",
            ),
            (
                "/tmp/tools/decoder",
                Backend::Vlc,
                "--play-and-exit",
                "--input-terminal=no",
            ),
        ] {
            let arguments = Player::new(executable, extra.clone(), Some(backend)).args(
                &stream(),
                "best",
                "Probe",
            );
            assert_eq!(&arguments[..extra.len()], extra.as_slice());
            assert!(arguments.iter().any(|argument| argument == expected));
            assert!(arguments.iter().all(|argument| argument != excluded));
        }
    }
}
