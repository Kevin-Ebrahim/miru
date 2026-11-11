mod writer;
pub use writer::{Sender, Writer};

use crate::model::{Anime, Episode, Mode, SourceId};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
            version: 2,
            entries: vec![],
            mode: Mode::Sub,
            quality: "best".into(),
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pub path: PathBuf,
    lock_timeout: Duration,
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
        source: SourceId,
        mode: Mode,
    },
}
impl Store {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("library.json"),
            lock_timeout: Duration::from_secs(2),
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
        Self::parse_library(&bytes, &self.path)
    }
    fn parse_library(bytes: &[u8], path: &Path) -> Result<Library> {
        let mut library: Library = serde_json::from_slice(bytes).with_context(|| {
            format!(
                "Invalid history in {}. Preserve this file and repair or rename it before retrying",
                path.display()
            )
        })?;
        ensure!(
            matches!(library.version, 1 | 2),
            "Unsupported history version {} in {}",
            library.version,
            path.display()
        );
        // Version 1 omitted provenance. Serde supplies HiAnime; the next write persists v2.
        library.version = 2;
        Ok(library)
    }
    fn lock(&self) -> Result<File> {
        let dir = self.path.parent().context("Missing state directory")?;
        fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("library.lock"))?;
        let deadline = Instant::now() + self.lock_timeout;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(TryLockError::WouldBlock) => {
                    ensure!(
                        Instant::now() < deadline,
                        "History is busy in another process; changes were not saved. Retry after it finishes"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
    pub fn migrate_from(&self, old: &Store) -> Result<()> {
        if self.path.try_exists()? {
            return Ok(());
        }
        let bytes = match fs::read(&old.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not read {}", old.path.display()));
            }
        };
        let _lock = self.lock()?;
        if self.path.try_exists()? {
            return Ok(());
        }
        Self::parse_library(&bytes, &old.path)?;
        let dir = self.path.parent().context("Missing state directory")?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(&self.path) {
            Ok(_) => File::open(dir)?.sync_all()?,
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("Could not migrate history"),
        }
        Ok(())
    }
    pub fn update(&self, change: Change) -> Result<Library> {
        self.update_many(vec![change])
    }
    fn update_many(&self, changes: Vec<Change>) -> Result<Library> {
        let _lock = self.lock()?;
        let dir = self.path.parent().context("Missing state directory")?;
        let mut library = self.load()?;
        for change in changes {
            match change {
                Change::Record {
                    anime,
                    episode,
                    mode,
                } => {
                    library
                        .entries
                        .retain(|e| !e.anime.same_source(&anime) || e.mode != mode);
                    library.entries.insert(
                        0,
                        Entry {
                            anime,
                            episode,
                            mode,
                            watched_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                        },
                    );
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
                            .any(|e| e.anime.same_source(&entry.anime) && e.mode == entry.mode)
                        {
                            library.entries.push(entry);
                        }
                    }
                }
                Change::Remove { source, mode } => library.entries.retain(|e| {
                    (e.anime.provider != source.provider || e.anime.id != source.id)
                        || e.mode != mode
                }),
            }
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
        .position(|e| e.provider == entry.anime.provider && e.number == entry.episode)
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
                provider: Default::default(),
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
    #[test]
    fn playback_preserves_large_imported_history_until_explicit_removal() {
        let text: String = (0..600)
            .map(|i| format!("1\told-{i}\tImported series {i}\n"))
            .collect();
        let entries = import_legacy(&text);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.update(Change::Import(entries.clone())).unwrap();
        assert_eq!(store.load().unwrap().entries.len(), 600);

        let anime = Anime {
            id: "new-1".into(),
            title: "New series".into(),
            ..Default::default()
        };
        for episode in ["1", "2"] {
            store
                .update(Change::Record {
                    anime: anime.clone(),
                    episode: episode.into(),
                    mode: Mode::Sub,
                })
                .unwrap();
            let loaded = store.load().unwrap();
            assert_eq!(loaded.entries.len(), 601);
            assert_eq!(loaded.entries[0].anime, anime);
            assert_eq!(loaded.entries[0].episode, episode);
            for (saved, imported) in loaded.entries[1..].iter().zip(&entries) {
                assert_eq!(saved.anime, imported.anime);
                assert_eq!(saved.episode, imported.episode);
                assert_eq!(saved.mode, imported.mode);
                assert_eq!(saved.watched_at, imported.watched_at);
            }
        }

        store
            .update(Change::Settings {
                mode: Mode::Dub,
                quality: "720p".into(),
            })
            .unwrap();
        store.update(Change::Import(entries.clone())).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.entries.len(), 601);
        assert_eq!(loaded.entries[0].episode, "2");
        assert_eq!(loaded.mode, Mode::Dub);
        assert_eq!(loaded.quality, "720p");

        store
            .update(Change::Remove {
                source: entries[0].anime.source_id(),
                mode: entries[0].mode,
            })
            .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.entries.len(), 600);
        assert!(
            loaded
                .entries
                .iter()
                .all(|entry| !entry.anime.same_source(&entries[0].anime))
        );
    }
    #[test]
    fn v1_history_is_read_without_rewriting_and_the_next_save_uses_v2() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let old = serde_json::json!({"version": 1, "mode": "dub", "quality": "720p",
            "entries": [{"anime": {"id": "series-1", "title": "Series", "cover": null},
                "episode": "1.5", "mode": "sub", "watched_at": 123}]})
        .to_string();
        fs::write(&store.path, &old).unwrap();
        let migrated = store.load().unwrap();
        assert_eq!(migrated.version, 2);
        assert_eq!(migrated.entries[0].anime.provider.to_string(), "hianime");
        assert_eq!(migrated.entries[0].episode, "1.5");
        assert_eq!(migrated.entries[0].watched_at, 123);
        assert_eq!(migrated.mode, Mode::Dub);
        assert_eq!(fs::read_to_string(&store.path).unwrap(), old);
        store
            .update(Change::Settings {
                mode: migrated.mode,
                quality: migrated.quality,
            })
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["entries"][0]["anime"]["provider"], "hianime");
        let future = old.replace("\"version\":1", "\"version\":3");
        fs::write(&store.path, &future).unwrap();
        assert!(store.update(Change::Import(vec![])).is_err());
        assert_eq!(fs::read_to_string(&store.path).unwrap(), future);
    }

    #[test]
    fn identical_ids_on_different_providers_remain_independent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let first = Anime {
            id: "same-1".into(),
            ..Default::default()
        };
        let second = Anime {
            provider: "other".to_owned().try_into().unwrap(),
            ..first.clone()
        };
        for anime in [&first, &second] {
            store
                .update(Change::Record {
                    anime: anime.clone(),
                    episode: "1".into(),
                    mode: Mode::Sub,
                })
                .unwrap();
        }
        assert_eq!(store.load().unwrap().entries.len(), 2);
        let library = store
            .update(Change::Remove {
                source: first.source_id(),
                mode: Mode::Sub,
            })
            .unwrap();
        assert_eq!(library.entries.len(), 1);
        assert!(library.entries[0].anime.same_source(&second));
        let wrong_provider_episode = Episode {
            provider: first.provider,
            id: "ep-1".into(),
            number: "1".into(),
            title: String::new(),
        };
        assert_eq!(
            next_episode(
                &library.entries[0],
                &[
                    wrong_provider_episode,
                    Episode {
                        provider: second.provider.clone(),
                        id: "ep-2".into(),
                        number: "2".into(),
                        title: String::new(),
                    }
                ]
            ),
            None
        );
        let library = store
            .update(Change::Import(library.entries.clone()))
            .unwrap();
        assert_eq!(library.entries.len(), 1);
    }
}
