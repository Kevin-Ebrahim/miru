use crate::model::{Anime, Episode, Mode};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub anime: Anime,
    pub episode: String,
    pub mode: Mode,
    pub watched_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Library {
    pub version: u8,
    pub entries: Vec<Entry>,
    pub mode: Mode,
    pub quality: String,
}
impl Default for Library {
    fn default() -> Self {
        Self {
            version: 1,
            entries: vec![],
            mode: Mode::Sub,
            quality: "best".into(),
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pub path: PathBuf,
}
pub enum Change {
    Record {
        anime: Anime,
        episode: String,
        mode: Mode,
    },
    Settings {
        mode: Mode,
        quality: String,
    },
    Import(Vec<Entry>),
    Remove {
        id: String,
        mode: Mode,
    },
}
impl Store {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("library.json"),
        }
    }
    pub fn load(&self) -> Result<Library> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Library::default()),
            Err(e) => {
                return Err(e).with_context(|| format!("Could not read {}", self.path.display()));
            }
        };
        let library: Library = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "Invalid history in {}. Preserve this file and repair or rename it before retrying",
                self.path.display()
            )
        })?;
        ensure!(
            library.version == 1,
            "Unsupported history version {} in {}",
            library.version,
            self.path.display()
        );
        Ok(library)
    }
    pub fn update(&self, change: Change) -> Result<Library> {
        let dir = self.path.parent().context("Missing state directory")?;
        fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("library.lock"))?;
        lock.lock()?;
        let mut library = self.load()?;
        match change {
            Change::Record {
                anime,
                episode,
                mode,
            } => {
                library
                    .entries
                    .retain(|e| e.anime.id != anime.id || e.mode != mode);
                library.entries.insert(
                    0,
                    Entry {
                        anime,
                        episode,
                        mode,
                        watched_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                    },
                );
                library.entries.truncate(500);
            }
            Change::Settings { mode, quality } => {
                library.mode = mode;
                library.quality = quality;
            }
            Change::Import(entries) => {
                for entry in entries {
                    if !library
                        .entries
                        .iter()
                        .any(|e| e.anime.id == entry.anime.id && e.mode == entry.mode)
                    {
                        library.entries.push(entry);
                    }
                }
            }
            Change::Remove { id, mode } => library
                .entries
                .retain(|e| e.anime.id != id || e.mode != mode),
        }
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut temp, &library)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)
            .context("Could not atomically save history")?;
        File::open(dir)?.sync_all()?;
        Ok(library)
    }
}

pub fn import_legacy(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let (episode, id, title) = (fields.next()?, fields.next()?, fields.next()?);
            if !id.contains('-') || title.is_empty() || episode.is_empty() {
                return None;
            }
            Some(Entry {
                anime: Anime {
                    id: id.into(),
                    title: crate::model::clean(title),
                    ..Default::default()
                },
                episode: episode.into(),
                mode: Mode::Sub,
                watched_at: 0,
            })
        })
        .collect()
}

pub fn next_episode(entry: &Entry, episodes: &[Episode]) -> Option<usize> {
    episodes
        .iter()
        .position(|e| e.number == entry.episode)
        .and_then(|i| (i + 1 < episodes.len()).then_some(i + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_writers_do_not_lose_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .update(Change::Record {
                            anime: Anime {
                                id: format!("show-{i}"),
                                ..Default::default()
                            },
                            episode: "1".into(),
                            mode: Mode::Sub,
                        })
                        .unwrap()
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(store.load().unwrap().entries.len(), 8);
    }
    #[test]
    fn continue_handles_specials_finished_and_missing_episodes() {
        let entry = Entry {
            anime: Anime::default(),
            episode: "1".into(),
            mode: Mode::Sub,
            watched_at: 0,
        };
        let episodes: Vec<_> = ["1", "1.5"]
            .iter()
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        assert_eq!(next_episode(&entry, &episodes), Some(1));
        assert_eq!(
            next_episode(
                &Entry {
                    episode: "1.5".into(),
                    ..entry.clone()
                },
                &episodes
            ),
            None
        );
        assert_eq!(
            next_episode(
                &Entry {
                    episode: "99".into(),
                    ..entry
                },
                &episodes
            ),
            None
        );
    }
    #[test]
    fn roundtrip_separates_languages_and_preserves_settings() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store
            .update(Change::Settings {
                mode: Mode::Dub,
                quality: "720p".into(),
            })
            .unwrap();
        for mode in [Mode::Sub, Mode::Dub, Mode::Sub] {
            store
                .update(Change::Record {
                    anime: Anime {
                        id: "show-1".into(),
                        title: "日本語 & title".into(),
                        ..Default::default()
                    },
                    episode: "1.5".into(),
                    mode,
                })
                .unwrap();
        }
        let loaded = store.load().unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.mode, Mode::Dub);
        assert_eq!(loaded.quality, "720p");
        assert_eq!(loaded.entries[0].anime.title, "日本語 & title");
    }
    #[test]
    fn malformed_history_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        fs::write(&store.path, b"broken").unwrap();
        assert!(
            store
                .update(Change::Settings {
                    mode: Mode::Sub,
                    quality: "best".into()
                })
                .is_err()
        );
        assert_eq!(fs::read(&store.path).unwrap(), b"broken");
    }
    #[test]
    fn import_is_read_only_and_deduplicated() {
        let entries = import_legacy("3\tshow-123\tA & B\n2\tlegacy_id\tSkip\nmalformed");
        assert_eq!(entries.len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.update(Change::Import(entries.clone())).unwrap();
        let library = store.update(Change::Import(entries)).unwrap();
        assert_eq!(library.entries.len(), 1);
        assert!(next_episode(&library.entries[0], &[]).is_none());
    }
}
