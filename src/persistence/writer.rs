use super::{Change, Library, Store};
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};

const CAPACITY: usize = 256;

#[derive(Default)]
struct Pending {
    changes: VecDeque<Change>,
    closed: bool,
}

#[derive(Clone)]
pub struct Sender {
    pending: Arc<Mutex<Pending>>,
    ready: Arc<Notify>,
}

pub struct Writer {
    pending: Arc<Mutex<Pending>>,
    ready: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn new(store: Store) -> (Self, mpsc::UnboundedReceiver<Result<Library>>) {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let ready = Arc::new(Notify::new());
        let (tx, rx) = mpsc::unbounded_channel();
        let queue = pending.clone();
        let wake = ready.clone();
        let task = tokio::spawn(async move {
            loop {
                wake.notified().await;

                let (changes, closed) = {
                    let mut pending = queue.lock().unwrap();
                    (
                        pending.changes.drain(..).collect::<Vec<_>>(),
                        pending.closed,
                    )
                };

                if !changes.is_empty() {
                    let store = store.clone();
                    let result = tokio::task::spawn_blocking(move || store.update_many(changes))
                        .await
                        .unwrap_or_else(|error| Err(error.into()));
                    let _ = tx.send(result);
                }

                if closed {
                    break;
                }
            }
        });

        (
            Self {
                pending,
                ready,
                task: Some(task),
            },
            rx,
        )
    }

    pub fn sender(&self) -> Sender {
        Sender {
            pending: self.pending.clone(),
            ready: self.ready.clone(),
        }
    }

    pub fn submit(&self, change: Change) -> Result<()> {
        self.sender().submit(change)
    }

    fn close(&self) {
        self.pending.lock().unwrap().closed = true;
        self.ready.notify_one();
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.close();
        if let Some(task) = self.task.take() {
            task.await?;
        }

        Ok(())
    }
}

impl Sender {
    pub fn submit(&self, change: Change) -> Result<()> {
        let mut pending = self.pending.lock().unwrap();
        ensure!(!pending.closed, "History writer has shut down");

        if matches!(change, Change::Settings { .. })
            && matches!(pending.changes.back(), Some(Change::Settings { .. }))
        {
            pending.changes.pop_back();
        }

        ensure!(
            pending.changes.len() < CAPACITY,
            "History writer is busy; this change was not queued. Retry after it finishes"
        );

        pending.changes.push_back(change);
        self.ready.notify_one();

        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anime, Mode};

    #[tokio::test]
    async fn preference_bursts_and_history_are_saved_before_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let (mut writer, mut results) = Writer::new(store.clone());
        for quality in ["best", "720p", "worst"] {
            writer
                .submit(Change::Settings {
                    mode: Mode::Dub,
                    quality: quality.into(),
                })
                .unwrap();
        }
        writer
            .submit(Change::Record {
                anime: Anime::default(),
                episode: "1".into(),
                mode: Mode::Sub,
            })
            .unwrap();
        writer.shutdown().await.unwrap();
        let library = store.load().unwrap();
        assert_eq!(library.quality, "worst");
        assert_eq!(library.mode, Mode::Dub);
        assert_eq!(library.entries.len(), 1);
        assert!(results.recv().await.unwrap().is_ok());
        assert!(results.recv().await.is_none());
    }

    #[tokio::test]
    async fn a_contended_lock_reports_unsaved_changes_and_allows_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path());
        store.lock_timeout = std::time::Duration::from_millis(30);
        let lock = store.lock().unwrap();
        let (mut writer, mut results) = Writer::new(store.clone());
        writer
            .submit(Change::Settings {
                mode: Mode::Dub,
                quality: "worst".into(),
            })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), writer.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(
            results
                .recv()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        assert!(!store.path.exists());
        drop(lock);
    }
}
