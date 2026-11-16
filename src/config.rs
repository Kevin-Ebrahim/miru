use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    download_dir: Option<PathBuf>,
}

impl Config {
    fn load(path: &Path) -> Result<Self> {
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

    fn saved_download_dir(&self) -> Result<Option<PathBuf>> {
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

pub fn download_dir(override_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_dir {
        ensure!(
            !path.as_os_str().is_empty(),
            "--download-dir cannot be empty"
        );
        return Ok(path.to_path_buf());
    }
    let config_path = dirs::config_dir()
        .context(
            "Could not find the config directory; use --download-dir to choose a download folder",
        )?
        .join("miru/config.json");
    if let Some(path) = Config::load(&config_path)?
        .saved_download_dir()
        .with_context(|| format!("Invalid download_dir in {}", config_path.display()))?
    {
        return Ok(path);
    }
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
            };
            assert!(config.saved_download_dir().is_err());
        }
    }

    #[test]
    fn command_line_download_folder_can_be_relative() {
        for path in [".", "./one-run", "/tmp/one-run"] {
            assert_eq!(
                download_dir(Some(Path::new(path))).unwrap(),
                PathBuf::from(path)
            );
        }
        assert!(download_dir(Some(Path::new(""))).is_err());
    }
}
