use crate::{model::Mode, player::Backend};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy)]
pub enum Setting {
    DownloadFolder,
    Audio,
    Quality,
    Player,
    PlayerArguments,
    PlayerBackend,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub download_dir: Option<PathBuf>,
    pub audio: Option<Mode>,
    pub quality: Option<String>,
    pub player: Option<String>,
    pub player_args: Vec<String>,
    pub player_backend: Option<Backend>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match crate::persistence::read_regular(path, MAX_CONFIG_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        };

        serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid config in {}", path.display()))
    }

    pub fn saved_download_dir(&self) -> Result<Option<PathBuf>> {
        let Some(path) = &self.download_dir else {
            return Ok(None);
        };

        let path = if let Ok(relative) = path.strip_prefix("~") {
            dirs::home_dir()
                .context("Could not find the home directory to expand download_dir")?
                .join(relative)
        } else {
            path.clone()
        };

        ensure!(
            path.is_absolute(),
            "download_dir must be an absolute path or start with ~/"
        );

        Ok(Some(path))
    }
}

pub const QUALITIES: [&str; 6] = ["best", "1080p", "720p", "480p", "360p", "worst"];

pub fn path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Could not find config directory")?
        .join("miru/config.json"))
}

impl Config {
    /// Resolve legacy preferences without writing them until an explicit edit.
    pub fn inherit(&mut self, library: &crate::persistence::Library) {
        self.audio.get_or_insert(library.mode);
        self.quality.get_or_insert_with(|| library.quality.clone());
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(path) = self.saved_download_dir()? {
            let mut parent = path.as_path();
            while !parent.try_exists()? {
                parent = parent
                    .parent()
                    .context("Download folder has no existing parent")?;
            }
            ensure!(parent.is_dir(), "Download folder or its parent is a file");
            tempfile::NamedTempFile::new_in(parent)
                .context("Download folder parent is not writable; choose another folder")?;
        }
        if let Some(quality) = &self.quality {
            ensure!(
                QUALITIES.contains(&quality.as_str())
                    || quality.trim_end_matches('p').parse::<u32>().is_ok(),
                "Choose a listed video quality"
            );
        }
        if let Some(player) = &self.player {
            ensure!(
                !player.trim().is_empty() && !player.contains('\0'),
                "Enter a player executable name or path"
            );
            use std::os::unix::fs::PermissionsExt;
            let executable = |path: &Path| {
                path.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            };
            let found = if player.contains('/') {
                executable(Path::new(player))
            } else {
                std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|dir| executable(&dir.join(player)))
                })
            };
            ensure!(
                found,
                "Player was not found or is not executable; enter an executable path or install it first"
            );
        }
        ensure!(
            self.player_args.iter().all(|arg| !arg.contains('\0')),
            "Player arguments cannot contain NUL characters"
        );
        Ok(())
    }

    pub fn save(&self, path: &Path, previous: &Self, edited: Setting) -> Result<Self> {
        self.validate()?;
        let parent = path.parent().context("Missing config directory")?;
        std::fs::create_dir_all(parent)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(parent.join("config.lock"))?;
        lock.try_lock()
            .context("Settings are being saved by another process; retry shortly")?;
        // Reject malformed existing configuration instead of silently replacing it.
        let mut latest = Self::load(path)?;
        // Migrate legacy defaults only when no other instance has saved them.
        latest.audio = latest.audio.or(previous.audio);
        latest.quality = latest.quality.or_else(|| previous.quality.clone());
        match edited {
            Setting::DownloadFolder => latest.download_dir = self.download_dir.clone(),
            Setting::Audio => latest.audio = self.audio,
            Setting::Quality => latest.quality = self.quality.clone(),
            Setting::Player => latest.player = self.player.clone(),
            Setting::PlayerArguments => latest.player_args = self.player_args.clone(),
            Setting::PlayerBackend => latest.player_backend = self.player_backend,
        }
        let bytes = serde_json::to_vec_pretty(&latest)?;
        ensure!(
            bytes.len() as u64 <= MAX_CONFIG_BYTES,
            "Settings exceed the size limit"
        );
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(path).context("Could not save settings")?;
        Ok(latest)
    }
}

pub fn automatic_download_dir() -> Result<PathBuf> {
    let downloads = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .filter(|path| path.is_absolute())
        .context("Could not find the Downloads directory; use --download-dir to choose a folder")?;

    Ok(downloads.join("miru"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn concurrent_edits_merge_and_preserve_legacy_quality() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for quality in ["720", "1440p"] {
            let previous = Config {
                quality: Some(quality.into()),
                audio: Some(Mode::Sub),
                ..Default::default()
            };
            let mut first = previous.clone();
            first.download_dir = Some(dir.path().join("downloads"));
            first
                .save(&path, &previous, Setting::DownloadFolder)
                .unwrap();
            let mut second = previous.clone();
            second.audio = Some(Mode::Dub);
            let saved = second.save(&path, &previous, Setting::Audio).unwrap();
            assert_eq!(saved.download_dir, first.download_dir);
            assert_eq!(saved.audio, Some(Mode::Dub));
            assert_eq!(saved.quality.as_deref(), Some(quality));
            std::fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn optional_config_and_saved_absolute_download_folder() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        assert!(
            Config::load(&path)
                .unwrap()
                .saved_download_dir()
                .unwrap()
                .is_none()
        );
        for json in ["{}", r#"{"download_dir": null}"#] {
            fs::write(&path, json).unwrap();
            assert!(
                Config::load(&path)
                    .unwrap()
                    .saved_download_dir()
                    .unwrap()
                    .is_none()
            );
        }
        let folder = root.path().join("Anime 日本語");
        let json = serde_json::json!({ "download_dir": folder }).to_string();
        fs::write(&path, &json).unwrap();
        assert_eq!(
            Config::load(&path).unwrap().saved_download_dir().unwrap(),
            Some(folder)
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), json);
    }

    #[test]
    fn invalid_config_is_reported_without_overwriting_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        for json in [
            "{invalid",
            r#"{"download_dir": 7}"#,
            r#"{"download_directory": "/tmp/anime"}"#,
        ] {
            fs::write(&path, json).unwrap();
            let error = Config::load(&path).err().unwrap();
            assert!(error.to_string().contains(&path.display().to_string()));
            assert_eq!(fs::read_to_string(&path).unwrap(), json);
        }
        for folder in ["", ".", "relative/anime", "~someone/anime"] {
            let config = Config {
                download_dir: Some(folder.into()),
                ..Default::default()
            };
            assert!(config.saved_download_dir().is_err());
        }
    }

    #[test]
    fn command_line_download_folder_can_be_relative() {
        for path in [".", "./one-run", "/tmp/one-run"] {
            assert_eq!(
                crate::settings::Settings::new(
                    Config::default(),
                    crate::settings::Overrides {
                        download: Some(path.into()),
                        ..Default::default()
                    },
                    PathBuf::new()
                )
                .download()
                .unwrap(),
                PathBuf::from(path)
            );
        }
        assert!(
            crate::settings::Settings::new(
                Config::default(),
                crate::settings::Overrides {
                    download: Some(PathBuf::new()),
                    ..Default::default()
                },
                PathBuf::new()
            )
            .download()
            .is_err()
        );
    }
}
