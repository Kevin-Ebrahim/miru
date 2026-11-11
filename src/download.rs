use crate::model::Stream;
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
};

pub fn filename(title: &str) -> String {
    let name: String = title
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .scan(0, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= 180).then_some(c)
        })
        .collect();
    let name = name.trim_matches(['.', ' ']);
    if name.is_empty() {
        "episode".into()
    } else {
        name.into()
    }
}

/// Download to a unique temporary file. Cancellation removes only this task's partial output.
/// Never clobber an existing episode, and mux separate audio/subtitles when available.
pub async fn download(
    stream: &Stream,
    quality: &str,
    title: &str,
    dir: &Path,
    log: &Path,
    progress: mpsc::UnboundedSender<String>,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<PathBuf>> {
    tokio::fs::create_dir_all(dir)
        .await
        .context("Could not create download directory")?;
    let destination = dir.join(format!("{}.mkv", filename(title)));
    ensure!(
        !destination.exists(),
        "File already exists: {}. Choose another download directory or remove it explicitly",
        destination.display()
    );
    let temporary = tempfile::Builder::new()
        .prefix(".miru-")
        .suffix(".part")
        .tempfile_in(dir)?;
    let mut cmd = Command::new("ffmpeg");
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
        &stream.select(quality).0.url,
    ]);
    let mut input = 1;
    let audio_index = stream.audio.as_ref().map(|url| {
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
    let error_log = std::fs::File::create(log)?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(error_log)
        .kill_on_drop(true)
        .spawn()
        .context("Could not launch ffmpeg. Install ffmpeg to download episodes")?;
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
    ensure!(
        status.success(),
        "Download failed ({status}). See {}",
        log.display()
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
    #[test]
    fn filenames_cannot_escape_download_directory() {
        for title in ["../../hello", "a/b\\c", "\x1b[31mTitle", "..."] {
            assert_eq!(Path::new(&filename(title)).components().count(), 1);
        }
        assert_eq!(filename("日本語: episode 1"), "日本語_ episode 1");
        assert!(filename(&"日本語".repeat(100)).len() <= 180);
    }
}
