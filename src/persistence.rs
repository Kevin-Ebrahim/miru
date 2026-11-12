mod writer;
pub use writer::{Sender, Writer};

use crate::model::{Anime, Episode, Mode, SourceId};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_HISTORY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024;

fn size_limit_error(limit: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("File exceeds the {limit}-byte limit"),
    )
}

pub(crate) fn read_regular(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a regular file",
        ));
    }
    if metadata.len() > limit {
        return Err(size_limit_error(limit));
    }
    let mut bytes = Vec::new();
    // Limit the read too: the opened file can grow after its metadata was checked.
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(size_limit_error(limit));
    }
    Ok(bytes)
}

struct HistoryWriter<'a> {
    file: &'a mut File,
    remaining: u64,
}

impl Write for HistoryWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(size_limit_error(MAX_HISTORY_BYTES));
        }
        let written = self.file.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

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
        let bytes = match read_regular(&self.path, MAX_HISTORY_BYTES) {
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
        let bytes = match read_regular(&old.path, MAX_HISTORY_BYTES) {
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
        {
            let mut writer = HistoryWriter {
                file: temp.as_file_mut(),
                remaining: MAX_HISTORY_BYTES,
            };
            serde_json::to_writer_pretty(&mut writer, &library)
                .context("Could not save history; the existing file was left unchanged")?;
            writer.write_all(b"\n")?;
        }
        temp.as_file().sync_all()?;
        temp.persist(&self.path)
            .context("Could not atomically save history")?;
        File::open(dir)?.sync_all()?;
        Ok(library)
    }
}

pub fn import_file(path: &Path) -> Result<Vec<Entry>> {
    let bytes = read_regular(path, MAX_IMPORT_BYTES)
        .with_context(|| format!("Could not read ani-cli history {}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("Invalid UTF-8 in ani-cli history {}", path.display()))?;
    Ok(import_legacy(text))
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
    use std::os::unix::fs::{MetadataExt, symlink};

    #[test]
    fn regular_symlinks_support_reading_migration_and_atomic_saves() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.json");
        let bytes = br#"{"version":1,"mode":"dub","quality":"720p","entries":[]}"#;
        fs::write(&original, bytes).unwrap();
        let store = Store::new(dir.path());
        symlink(&original, &store.path).unwrap();
        let library = store.load().unwrap();
        assert_eq!(library.version, 2);
        assert_eq!(library.mode, Mode::Dub);
        assert_eq!(library.quality, "720p");
        assert!(store.path.is_symlink());
        let migrated = Store::new(&dir.path().join("migrated"));
        migrated.migrate_from(&store).unwrap();
        assert_eq!(fs::read(&migrated.path).unwrap(), bytes);
        store
            .update(Change::Settings {
                mode: Mode::Sub,
                quality: "best".into(),
            })
            .unwrap();
        assert!(!store.path.is_symlink());
        assert_eq!(store.load().unwrap().mode, Mode::Sub);
        assert_eq!(fs::read(original).unwrap(), bytes);
    }

    #[test]
    fn import_sources_may_be_regular_file_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("ani-hsts");
        let bytes = "2\tshow-1\t日本語\n";
        fs::write(&original, bytes).unwrap();
        let link = dir.path().join("source");
        symlink(&original, &link).unwrap();
        let entries = import_file(&link).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].episode, "2");
        assert_eq!(entries[0].anime.title, "日本語");
        assert!(link.is_symlink());
        assert_eq!(fs::read_to_string(original).unwrap(), bytes);
    }

    #[test]
    fn oversized_history_is_not_read_or_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        File::create(&store.path)
            .unwrap()
            .set_len(MAX_HISTORY_BYTES + 1)
            .unwrap();
        let before = fs::metadata(&store.path).unwrap();
        assert!(format!("{:#}", store.load().unwrap_err()).contains("limit"));
        let error = store
            .update(Change::Settings {
                mode: Mode::Sub,
                quality: "best".into(),
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("limit"));
        let after = fs::metadata(&store.path).unwrap();
        assert_eq!(after.ino(), before.ino());
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    }

    #[test]
    fn an_oversized_save_preserves_the_existing_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store
            .update(Change::Settings {
                mode: Mode::Dub,
                quality: "720p".into(),
            })
            .unwrap();
        let bytes = fs::read(&store.path).unwrap();
        let inode = fs::metadata(&store.path).unwrap().ino();
        let error = store
            .update(Change::Record {
                anime: Anime {
                    title: "x".repeat(MAX_HISTORY_BYTES as usize),
                    ..Default::default()
                },
                episode: "1".into(),
                mode: Mode::Sub,
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("limit"));
        assert_eq!(fs::metadata(&store.path).unwrap().ino(), inode);
        assert_eq!(fs::read(&store.path).unwrap(), bytes);
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
            matches!(
                entry.unwrap().file_name().to_str(),
                Some("library.json" | "library.lock")
            )
        }));
    }

    #[test]
    fn invalid_and_oversized_imports_leave_the_source_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("ani-hsts");
        File::create(&source)
            .unwrap()
            .set_len(MAX_IMPORT_BYTES + 1)
            .unwrap();
        assert!(format!("{:#}", import_file(&source).unwrap_err()).contains("limit"));
        assert_eq!(fs::metadata(&source).unwrap().len(), MAX_IMPORT_BYTES + 1);
        fs::write(&source, [0xff]).unwrap();
        let error = import_file(&source).unwrap_err();
        assert!(format!("{error:#}").contains("UTF-8"));
        assert_eq!(fs::read(source).unwrap(), [0xff]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_regular_files_metadata_cannot_bypass_the_read_budget() {
        let path = Path::new("/proc/self/cmdline");
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
        assert!(
            read_regular(path, 1)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }

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
