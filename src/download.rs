use crate::{
    jobs::Log,
    model::{Anime, Episode, Mode},
    network::{Session, Tool},
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{mpsc, oneshot},
};

const MAX_STEM_BYTES: usize = 180;

pub struct Name<'a> {
    pub display_title: &'a str,
    pub anime: &'a Anime,
    pub episode: &'a Episode,
    pub mode: Mode,
}

impl Name<'_> {
    fn filename(&self) -> Result<String> {
        let display = sanitize(self.display_title);
        let display = display.trim_matches(['.', ' ']);
        // Preserve ordinary filenames, including the existing CLI/TUI separators.
        if display.len() <= MAX_STEM_BYTES {
            return Ok(if display.is_empty() {
                "episode"
            } else {
                display
            }
            .into());
        }
        let anime_provider = self.anime.provider.to_string();
        let episode_provider = self.episode.provider.to_string();
        let mut identity = Sha256::new();
        for field in [
            anime_provider.as_str(),
            self.anime.id.as_str(),
            episode_provider.as_str(),
            self.episode.id.as_str(),
            self.episode.number.as_str(),
            self.mode.as_str(),
        ] {
            identity.update((field.len() as u64).to_be_bytes());
            identity.update(field.as_bytes());
        }
        let tag = format!("{:x}", identity.finalize());
        let suffix = format!(
            " [{}] Episode {} {}",
            &tag[..16],
            sanitize(&self.episode.number),
            self.mode.as_str().to_uppercase()
        );
        let budget = MAX_STEM_BYTES
            .checked_sub(suffix.len())
            .context("Episode number is too long for a download filename")?;
        let title = sanitize(&self.anime.title);
        let prefix = truncate(title.trim_matches(['.', ' ']), budget);
        Ok(format!("{prefix}{suffix}").trim_start_matches(' ').into())
    }
}

fn sanitize(title: &str) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect()
}

fn truncate(value: &str, bytes: usize) -> &str {
    let mut end = bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end_matches(['.', ' '])
}

/// Download to a unique temporary file. Cancellation removes only this task's partial output.
/// Never clobber an existing episode, and mux separate audio/subtitles when available.
pub async fn download(
    media: &Session,
    quality: &str,
    name: Name<'_>,
    dir: &Path,
    log: &Log,
    progress: mpsc::UnboundedSender<String>,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<PathBuf>> {
    let stream = media.stream();
    let (variant, _) = stream.select(quality);
    let destination = dir.join(format!("{}.mkv", name.filename()?));
    tokio::fs::create_dir_all(dir)
        .await
        .context("Could not create download directory")?;
    ensure!(
        !destination.exists(),
        "File already exists: {}. Choose another download directory or remove it explicitly",
        destination.display()
    );
    let temporary = tempfile::Builder::new()
        .prefix(".miru-")
        .suffix(".part")
        .tempfile_in(dir)?;
    let mut cmd = media.command("ffmpeg", Tool::Download)?;
    cmd.args([
        "-nostdin",
        "-y",
        "-loglevel",
        "warning",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    cmd.args(crate::network::MEDIA_INPUT_OPTIONS);
    cmd.args([
        "-extension_picky",
        "0",
        "-referer",
        &stream.referer,
        "-i",
        &variant.url,
    ]);
    let mut input = 1;
    let audio_index = variant.audio_url().map(|url| {
        cmd.args(crate::network::MEDIA_INPUT_OPTIONS);
        cmd.args([
            "-extension_picky",
            "0",
            "-referer",
            &stream.referer,
            "-i",
            url,
        ]);
        let i = input;
        input += 1;
        i
    });
    let subtitle_index = stream.subtitle.as_ref().map(|url| {
        cmd.args(crate::network::MEDIA_INPUT_OPTIONS);
        cmd.args(["-referer", &stream.referer, "-i", url]);
        input
    });
    cmd.args([
        "-map",
        "0:v:0",
        "-map",
        &format!("{}:a?", audio_index.unwrap_or(0)),
    ]);
    if let Some(index) = subtitle_index {
        cmd.args(["-map", &format!("{index}:s?"), "-c:s", "srt"]);
    }
    cmd.args(["-c:v", "copy", "-c:a", "copy", "-f", "matroska"])
        .arg(temporary.path());
    let error_log = log.writer()?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(error_log)
        .kill_on_drop(true)
        .spawn()
        .context(
            "Could not launch protected ffmpeg. Install bubblewrap and ffmpeg to download episodes",
        )?;
    if !media
        .wait_started(&mut child, cancel)
        .await
        .with_context(|| format!("Could not start ffmpeg; see {}", log.path().display()))?
    {
        return Ok(None);
    }
    let stdout = child
        .stdout
        .take()
        .context("Missing ffmpeg progress pipe")?;
    let mut lines = BufReader::new(stdout).lines();
    let transfer = async {
        while let Some(line) = lines.next_line().await? {
            if let Some(time) = line.strip_prefix("out_time=") {
                let _ = progress.send(format!(
                    "Downloaded through {}",
                    time.trim_end_matches('0').trim_end_matches('.')
                ));
            }
        }
        child.wait().await
    };
    let status = tokio::select! {
        biased;
        _ = cancel => { child.kill().await?; return Ok(None); },
        result = transfer => match result {
            Ok(status) => status,
            Err(error) => { let _ = child.kill().await; return Err(error.into()); }
        },
    };
    media.check()?;
    ensure!(
        status.success(),
        "Download failed ({status}). See {}",
        log.path().display()
    );
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(&destination)
        .context("Could not save download; destination may already exist")?;
    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anime(title: &str) -> Anime {
        Anime {
            id: "series-1".into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn episode(anime: &Anime, number: &str) -> Episode {
        Episode {
            id: number.into(),
            provider: anime.provider.clone(),
            number: number.into(),
            title: String::new(),
        }
    }

    fn filename(anime: &Anime, number: &str, mode: Mode) -> Result<String> {
        Name {
            display_title: &format!(
                "{} Episode {} {}",
                anime.title,
                number,
                mode.as_str().to_uppercase()
            ),
            anime,
            episode: &episode(anime, number),
            mode,
        }
        .filename()
    }

    #[test]
    fn filenames_cannot_escape_download_directory() {
        for title in ["../../hello", "a/b\\c", "\x1b[31mTitle", "..."] {
            let name = filename(&anime(title), "../1/..", Mode::Sub).unwrap();
            assert_eq!(Path::new(&name).components().count(), 1);
        }
    }

    #[test]
    fn ordinary_filenames_preserve_cli_and_tui_names() {
        let anime = anime("日本語: Smoke & Test");
        assert_eq!(
            filename(&anime, "1", Mode::Sub).unwrap(),
            "日本語_ Smoke & Test Episode 1 SUB"
        );
        assert_eq!(
            Name {
                display_title: "日本語: Smoke & Test / Episode 1 SUB",
                anime: &anime,
                episode: &episode(&anime, "1"),
                mode: Mode::Sub,
            }
            .filename()
            .unwrap(),
            "日本語_ Smoke & Test _ Episode 1 SUB"
        );
    }

    #[test]
    fn long_filenames_preserve_episode_and_language() {
        let anime = anime(&"Long Title ".repeat(20));
        let mut names = std::collections::HashSet::new();
        for mode in [Mode::Sub, Mode::Dub] {
            for number in ["1", "2"] {
                let name = filename(&anime, number, mode).unwrap();
                assert!(name.ends_with(&format!(
                    " Episode {number} {}",
                    mode.as_str().to_uppercase()
                )));
                assert!(name.len() <= 180);
                names.insert(name);
            }
        }
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn multibyte_filenames_fit_the_byte_budget() {
        for title in ["日本語🎬".repeat(40), "é".repeat(100)] {
            let name = filename(&anime(&title), "12.5", Mode::Dub).unwrap();
            assert!(name.len() <= 180);
            assert!(name.starts_with(title.chars().next().unwrap()));
            assert!(name.ends_with(" Episode 12.5 DUB"));
        }
    }

    #[test]
    fn special_episode_numbers_survive_title_shortening() {
        let anime = anime(&"Long Title ".repeat(20));
        for number in [".5", "1.", "OVA 1.5"] {
            let name = filename(&anime, number, Mode::Sub).unwrap();
            assert!(name.ends_with(&format!(" Episode {number} SUB")), "{name}");
        }
    }

    #[test]
    fn truncated_prefixes_distinguish_series_and_providers() {
        let prefix = "Long Title ".repeat(20);
        let first = anime(&format!("{prefix}First"));
        let mut second = anime(&format!("{prefix}Second"));
        second.id = "series-2".into();
        let first_name = filename(&first, "1", Mode::Sub).unwrap();
        assert_ne!(first_name, filename(&second, "1", Mode::Sub).unwrap());
        let mut other_provider = first.clone();
        other_provider.provider = "another-provider".to_owned().try_into().unwrap();
        assert_ne!(
            first_name,
            filename(&other_provider, "1", Mode::Sub).unwrap()
        );
    }

    #[test]
    fn oversized_episode_identity_reports_an_error() {
        let error = filename(&anime("Title"), &"1".repeat(180), Mode::Sub).unwrap_err();
        assert!(error.to_string().contains("Episode number is too long"));
    }
}
