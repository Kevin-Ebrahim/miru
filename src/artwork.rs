use crate::provider::Provider;
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
    sync::Arc,
};
use tokio::sync::{Mutex, Semaphore, mpsc};

pub struct ArtResult {
    pub generation: u64,
    pub result: Result<Protocol>,
    pub image_id: u32,
}

pub struct Artwork {
    pub picker: Picker,
    pub protocol: Option<Protocol>,
    pub error: Option<String>,
    pub label: String,
    pub generation: u64,
    key: Option<(String, Size)>,
    decoded: Arc<Mutex<LruCache<String, Arc<DynamicImage>>>>,
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
        let label = if enabled {
            format!("{:?}", picker.protocol_type())
        } else {
            "text".into()
        };
        Self {
            picker,
            protocol: None,
            error: None,
            label,
            generation: 0,
            key: None,
            decoded: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(12).unwrap()))),
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
        provider: Provider,
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
        self.task = Some(tokio::spawn(async move {
            // Avoid downloading every item crossed during rapid key repeats.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let result = async {
                let cached = decoded.lock().await.get(&url).cloned();
                let img = if let Some(img) = cached {
                    img
                } else {
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
                    let bytes = match disk {
                        Some(b) => b,
                        None => provider.bytes(&url, None, 12 * 1024 * 1024).await?,
                    };
                    let permit = permits.clone().acquire_owned().await?;
                    let (img, bytes) = tokio::task::spawn_blocking(move || -> Result<_> {
                        let _permit = permit;
                        let mut reader =
                            ImageReader::new(io::Cursor::new(&bytes)).with_guessed_format()?;
                        let mut limits = image::Limits::default();
                        limits.max_image_width = Some(4096);
                        limits.max_image_height = Some(4096);
                        limits.max_alloc = Some(96 * 1024 * 1024);
                        reader.limits(limits);
                        Ok((
                            Arc::new(reader.decode().context("Cover could not be decoded")?),
                            bytes,
                        ))
                    })
                    .await??;
                    if tokio::fs::create_dir_all(&cache).await.is_ok() {
                        // Atomic cache writes; a failed cache write must never hide an image.
                        let path_copy = path.clone();
                        let _ = tokio::task::spawn_blocking(move || -> Result<()> {
                            let mut temp =
                                tempfile::NamedTempFile::new_in(path_copy.parent().unwrap())?;
                            temp.write_all(&bytes)?;
                            temp.persist(&path_copy)?;
                            prune_cache(path_copy.parent().unwrap());
                            Ok(())
                        })
                        .await;
                    }
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
                        let resized = resize.resize(&img, picker.font_size(), fit, None);
                        ratatui_image::protocol::kitty::Kitty::new(
                            resized,
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
