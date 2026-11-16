use crate::network::Http;
use anyhow::{Context, Result};
use image::{DynamicImage, ImageReader, imageops::FilterType};
use lru::LruCache;
use ratatui::layout::Size;
use ratatui_image::{
    Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, Semaphore, mpsc};

pub struct ArtResult {
    pub generation: u64,
    pub result: Result<Protocol>,
    pub image_id: u32,
}

const RESIZED_CACHE_BYTES: usize = 16 * 1024 * 1024;

type ResizedKey = (String, u32, u32);

struct ResizedCover {
    source: Weak<DynamicImage>,
    image: Arc<DynamicImage>,
}

struct ResizedCache(LruCache<ResizedKey, ResizedCover>);

impl ResizedCache {
    fn new() -> Self {
        Self(LruCache::new(NonZeroUsize::new(4).unwrap()))
    }

    fn get(&mut self, key: &ResizedKey, source: &Arc<DynamicImage>) -> Option<Arc<DynamicImage>> {
        self.0
            .get(key)
            .filter(|entry| entry.source.ptr_eq(&Arc::downgrade(source)))
            .map(|entry| entry.image.clone())
    }

    fn insert(&mut self, key: ResizedKey, source: &Arc<DynamicImage>, image: Arc<DynamicImage>) {
        let bytes = image.as_bytes().len();
        if bytes > RESIZED_CACHE_BYTES {
            return;
        }
        self.0.pop(&key);
        while self
            .0
            .iter()
            .map(|(_, entry)| entry.image.as_bytes().len())
            .sum::<usize>()
            + bytes
            > RESIZED_CACHE_BYTES
        {
            self.0.pop_lru();
        }
        self.0.put(
            key,
            ResizedCover {
                source: Arc::downgrade(source),
                image,
            },
        );
    }
}

pub struct Artwork {
    pub picker: Picker,
    pub protocol: Option<Protocol>,
    pub error: Option<String>,
    pub generation: u64,
    key: Option<(String, Size)>,
    decoded: Arc<Mutex<LruCache<String, Arc<DynamicImage>>>>,
    resized: Arc<Mutex<ResizedCache>>,
    permits: Arc<Semaphore>,
    cache: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
    pub clear: bool,
    pub enabled: bool,
    active_id: Option<u32>,
    pending_delete: Vec<u32>,
}

impl Artwork {
    pub fn new(picker: Picker, cache: PathBuf, enabled: bool) -> Self {
        Self {
            picker,
            protocol: None,
            error: None,
            generation: 0,
            key: None,
            decoded: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(12).unwrap()))),
            resized: Arc::new(Mutex::new(ResizedCache::new())),
            permits: Arc::new(Semaphore::new(2)),
            cache,
            task: None,
            clear: false,
            enabled,
            active_id: None,
            pending_delete: vec![],
        }
    }

    pub fn request(
        &mut self,
        url: Option<&str>,
        size: Size,
        http: Http,
        tx: mpsc::UnboundedSender<ArtResult>,
    ) {
        let key = url
            .filter(|_| self.enabled && size.width > 1 && size.height > 1)
            .map(|u| (u.to_owned(), size));
        if key == self.key {
            return;
        }

        self.generation += 1;
        self.key = key.clone();
        if let Some(id) = self.active_id.take() {
            self.pending_delete.push(id);
        }

        self.protocol = None;
        self.error = None;
        self.clear = true;
        if let Some(task) = self.task.take() {
            task.abort();
        }

        let Some((url, size)) = key else { return };
        let generation = self.generation;
        let picker = self.picker.clone();
        let seed = format!(
            "{}-{generation}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        );
        let digest = Sha256::digest(seed.as_bytes());
        let image_id = u32::from_be_bytes([0, digest[0], digest[1], digest[2]]).max(1);
        let cache = self.cache.clone();
        let decoded = self.decoded.clone();
        let permits = self.permits.clone();
        let resized_cache = self.resized.clone();

        self.task = Some(tokio::spawn(async move {
            let result = async {
                let cached = decoded.lock().await.get(&url).cloned();
                let img = if let Some(img) = cached {
                    img
                } else {
                    // Debounce misses while keeping decoded covers responsive.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let path = cache.join(format!("{:x}", Sha256::digest(url.as_bytes())));
                    let disk = match tokio::fs::metadata(&path).await {
                        Ok(meta)
                            if meta.len() <= 12 * 1024 * 1024
                                && meta
                                    .modified()
                                    .ok()
                                    .and_then(|t| t.elapsed().ok())
                                    .is_some_and(|age| age.as_secs() < 30 * 86400) =>
                        {
                            tokio::fs::read(&path).await.ok()
                        }
                        _ => None,
                    };
                    let mut disk_hit = false;
                    let (img, bytes) = if let Some(bytes) = disk {
                        match decode_image(bytes, permits.clone()).await {
                            Ok(decoded) => {
                                disk_hit = true;
                                decoded
                            }
                            Err(_) => {
                                let _ = tokio::fs::remove_file(&path).await;
                                let bytes = http.bytes(&url, None, 12 * 1024 * 1024).await?;
                                decode_image(bytes, permits.clone()).await?
                            }
                        }
                    } else {
                        let bytes = http.bytes(&url, None, 12 * 1024 * 1024).await?;
                        decode_image(bytes, permits.clone()).await?
                    };
                    // Cache maintenance is best effort and must not delay presentation.
                    tokio::task::spawn_blocking(move || {
                        let _ = maintain_cache(&path, (!disk_hit).then_some(bytes));
                    });
                    decoded.lock().await.put(url.clone(), img.clone());
                    img
                };

                let permit = permits.acquire_owned().await?;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    // Library handles native graphics and aspect-ratio fitting. Rendering only blits.
                    let resize = Resize::Scale(Some(FilterType::Lanczos3));
                    if picker.protocol_type() == ProtocolType::Kitty {
                        // Use the library encoder with an owned ID so cleanup only deletes our
                        // image, never graphics belonging to the shell or another pane.
                        let fit = resize.size_for(&img, picker.font_size(), size);
                        let font = picker.font_size();
                        let key = (
                            url,
                            u32::from(fit.width) * u32::from(font.width),
                            u32::from(fit.height) * u32::from(font.height),
                        );
                        let cached = resized_cache.blocking_lock().get(&key, &img);
                        let resized = cached.unwrap_or_else(|| {
                            let resized = Arc::new(resize.resize(&img, font, fit, None));
                            resized_cache
                                .blocking_lock()
                                .insert(key, &img, resized.clone());
                            resized
                        });
                        ratatui_image::protocol::kitty::Kitty::new(
                            (*resized).clone(),
                            fit,
                            image_id,
                            picker.tmux_detected(),
                            true,
                        )
                        .map(Protocol::Kitty)
                        .map_err(anyhow::Error::from)
                    } else {
                        picker
                            .new_protocol((*img).clone(), size, resize)
                            .map_err(anyhow::Error::from)
                    }
                })
                .await?
            }
            .await;
            let _ = tx.send(ArtResult {
                generation,
                result,
                image_id,
            });
        }));
    }

    pub fn accept(&mut self, message: ArtResult) {
        if message.generation != self.generation {
            return;
        }

        match message.result {
            Ok(protocol) => {
                if matches!(protocol, Protocol::Kitty(_)) {
                    self.active_id = Some(message.image_id);
                }
                self.protocol = Some(protocol);
                self.error = None;
            }
            Err(err) => {
                self.error = Some(format!("Cover unavailable: {err}"));
            }
        }
    }

    pub fn clear_graphics(&mut self) -> io::Result<()> {
        if self.clear {
            for id in self.pending_delete.drain(..) {
                write!(io::stdout(), "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
            }
            io::stdout().flush()?;
            self.clear = false;
        }
        Ok(())
    }
}

impl Drop for Artwork {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(id) = self.active_id.take() {
            self.pending_delete.push(id);
        }
        self.clear = true;
        let _ = self.clear_graphics();
    }
}

async fn decode_image(
    bytes: Vec<u8>,
    permits: Arc<Semaphore>,
) -> Result<(Arc<DynamicImage>, Vec<u8>)> {
    let permit = permits.acquire_owned().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut reader = ImageReader::new(io::Cursor::new(&bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(96 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().context("Cover could not be decoded")?;
        // Bound the decoded LRU independently of encoded file size.
        let image = if image.width() > 1600 || image.height() > 2400 {
            image.resize(1600, 2400, FilterType::Lanczos3)
        } else {
            image
        };
        Ok((Arc::new(image), bytes))
    })
    .await?
}

fn maintain_cache(path: &std::path::Path, bytes: Option<Vec<u8>>) -> Result<()> {
    if let Some(bytes) = bytes {
        let dir = path
            .parent()
            .context("Cover cache has no parent directory")?;
        std::fs::create_dir_all(dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(&bytes)?;
        temp.persist(path)?;
        prune_cache(dir);
    } else {
        // Preserve sliding expiry and eviction recency without rewriting cached bytes.
        std::fs::File::open(path)?.set_modified(std::time::SystemTime::now())?;
    }
    Ok(())
}

fn prune_cache(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((entry.path(), metadata.len(), metadata.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|(_, _, at)| *at);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    let mut count = files.len();
    for (path, size, _) in files {
        if count <= 128 && total <= 128 * 1024 * 1024 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
            count -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resized_covers_match_source_and_geometry() {
        let mut cache = ResizedCache::new();
        let source = Arc::new(DynamicImage::new_rgb8(2, 3));
        let image = Arc::new(DynamicImage::new_rgb8(4, 6));
        let key = ("cover".to_owned(), 4, 6);
        cache.insert(key.clone(), &source, image.clone());
        assert!(Arc::ptr_eq(&cache.get(&key, &source).unwrap(), &image));
        assert!(cache.get(&("cover".to_owned(), 8, 12), &source).is_none());
        let replacement = Arc::new(DynamicImage::new_rgb8(2, 3));
        assert!(cache.get(&key, &replacement).is_none());
        drop(source);
        assert_eq!(cache.0.peek(&key).unwrap().source.strong_count(), 0);
    }

    #[test]
    fn resized_cache_evicts_oldest_to_bound_memory() {
        let mut cache = ResizedCache::new();
        let source = Arc::new(DynamicImage::new_rgb8(1, 1));
        let image = Arc::new(DynamicImage::new_rgba8(1024, 2048));
        for index in 0..3 {
            cache.insert((index.to_string(), 1024, 2048), &source, image.clone());
        }
        assert_eq!(cache.0.len(), 2);
        assert!(cache.get(&("0".to_owned(), 1024, 2048), &source).is_none());
        let oversized = Arc::new(DynamicImage::new_rgba8(2048, 2049));
        cache.insert(("large".to_owned(), 2048, 2049), &source, oversized);
        assert_eq!(cache.0.len(), 2);
    }

    #[test]
    fn disk_hit_refreshes_age_without_replacing_file() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a".repeat(64));
        maintain_cache(&path, Some(vec![1, 2, 3])).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH;
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        maintain_cache(&path, None).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.ino(), inode);
        assert!(metadata.modified().unwrap() > old);
        assert_eq!(std::fs::read(path).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn stale_art_and_failures_never_replace_current_cover() {
        let mut art = Artwork::new(Picker::halfblocks(), PathBuf::new(), false);
        art.generation = 3;
        art.accept(ArtResult {
            generation: 2,
            result: Err(anyhow::anyhow!("stale failure")),
            image_id: 1,
        });
        assert!(art.error.is_none());
        art.accept(ArtResult {
            generation: 3,
            result: Err(anyhow::anyhow!("failed download")),
            image_id: 2,
        });
        assert!(art.error.is_some());
        assert!(art.protocol.is_none());
    }
}
