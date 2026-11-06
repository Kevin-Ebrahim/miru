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
    enabled: bool,
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
        self.protocol = None;
        self.error = None;
        self.clear = true;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let Some((url, size)) = key else { return };
        let generation = self.generation;
        let picker = self.picker.clone();
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
                    let disk = tokio::fs::read(&path).await.ok();
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
                            temp.persist(path_copy)?;
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
                    picker
                        .new_protocol(
                            (*img).clone(),
                            size,
                            Resize::Scale(Some(FilterType::Lanczos3)),
                        )
                        .map_err(anyhow::Error::from)
                })
                .await?
            }
            .await;
            let _ = tx.send(ArtResult { generation, result });
        }));
    }

    pub fn accept(&mut self, message: ArtResult) {
        if message.generation != self.generation {
            return;
        }
        match message.result {
            Ok(protocol) => {
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
            if self.picker.protocol_type() == ProtocolType::Kitty {
                // The library uses virtual placements. Delete their backing data as well, so
                // old covers cannot survive a redraw, resize, empty search or alternate-screen exit.
                write!(io::stdout(), "\x1b_Ga=d,d=A,q=2\x1b\\")?;
                io::stdout().flush()?;
            }
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
        self.clear = true;
        let _ = self.clear_graphics();
    }
}
