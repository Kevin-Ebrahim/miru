use crate::{
    download,
    model::{Anime, Episode, Mode},
    player,
    provider::Provider,
};
use anyhow::Result;
use std::{path::PathBuf, process::ExitStatus};
use tokio::{
    process::Child,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Playback,
    Download,
}

#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub episode: Episode,
    pub anime: Anime,
    pub mode: Mode,
    pub quality: String,
    pub title: String,
    pub kind: JobKind,
}

#[derive(Clone)]
pub struct Options {
    pub player: String,
    pub player_args: Vec<String>,
    pub state_dir: PathBuf,
    pub download_dir: PathBuf,
    pub history: crate::persistence::Sender,
}

pub enum Output {
    Playback,
    Download(PathBuf),
}

pub enum Update {
    Resolved(String),
    Started,
    Progress(String),
    Finished(Result<Output>),
}

pub struct Event {
    pub id: u64,
    pub update: Update,
}

/// Serializes replacement so the old child is reaped before another job starts.
pub struct Jobs {
    requests: Option<watch::Sender<Option<Request>>>,
    events: mpsc::UnboundedReceiver<Event>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl Jobs {
    pub fn new(provider: Provider, options: Options) -> Self {
        let (requests, mut incoming) = watch::channel::<Option<Request>>(None);
        let (notify, events) = mpsc::unbounded_channel();
        let worker = tokio::spawn(async move {
            let mut active: Option<(oneshot::Sender<()>, JoinHandle<()>)> = None;

            loop {
                let changed = incoming.changed().await;
                if let Some((cancel, task)) = active.take() {
                    let _ = cancel.send(());
                    task.await?;
                }

                if changed.is_err() || incoming.has_changed().is_err() {
                    break;
                }

                // Read after cleanup: rapid replacements only start the latest request.
                let request = incoming.borrow_and_update().clone();
                if let Some(request) = request {
                    let provider = provider.clone();
                    let options = options.clone();
                    let notify = notify.clone();
                    let (cancel, mut cancelled) = oneshot::channel();
                    let task = tokio::spawn(async move {
                        let id = request.id;
                        let result =
                            run(&provider, &options, request, &notify, &mut cancelled).await;
                        match result {
                            Ok(None) => {}
                            Ok(Some(path)) => {
                                let _ = notify.send(Event {
                                    id,
                                    update: Update::Finished(Ok(path)),
                                });
                            }
                            Err(error) => {
                                let _ = notify.send(Event {
                                    id,
                                    update: Update::Finished(Err(error)),
                                });
                            }
                        }
                    });
                    active = Some((cancel, task));
                }
            }
            Ok(())
        });

        Self {
            requests: Some(requests),
            events,
            worker: Some(worker),
        }
    }

    pub fn start(&self, request: Request) {
        if let Some(sender) = &self.requests {
            sender.send_replace(Some(request));
        }
    }

    pub fn stop(&self) {
        if let Some(sender) = &self.requests {
            sender.send_replace(None);
        }
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            worker.await??;
        }

        Ok(())
    }
}

// Cancelled work emits no completion event, so it cannot advance a queue.
async fn run(
    provider: &Provider,
    options: &Options,
    request: Request,
    notify: &mpsc::UnboundedSender<Event>,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<Output>> {
    let stream = tokio::select! {
        biased;
        _ = &mut *cancel => return Ok(None),
        result = provider.resolve(&request.episode, request.mode) => result?,
    };

    let (variant, fallback) = stream.select(&request.quality);
    let quality_note = if fallback {
        format!("{}p ({} unavailable)", variant.height, request.quality)
    } else {
        format!("{}p", variant.height)
    };

    let _ = notify.send(Event {
        id: request.id,
        update: Update::Resolved(quality_note),
    });

    let log = options
        .state_dir
        .join(if request.kind == JobKind::Download {
            "download.log"
        } else {
            "player.log"
        });

    if request.kind == JobKind::Download {
        let _ = notify.send(Event {
            id: request.id,
            update: Update::Started,
        });
        let (progress, mut updates) = mpsc::unbounded_channel();
        let download = download::download(
            &stream,
            &request.quality,
            &request.title,
            &options.download_dir,
            &log,
            progress,
            cancel,
        );
        tokio::pin!(download);
        let result = loop {
            tokio::select! {
                result = &mut download => break result?,
                Some(text) = updates.recv() => {
                    let _ = notify.send(Event {
                        id: request.id,
                        update: Update::Progress(text),
                    });
                }
            }
        };
        return Ok(result.map(Output::Download));
    }

    let mut child = player::launch(
        &options.player,
        &options.player_args,
        &stream,
        &request.quality,
        &request.title,
        &log,
    )?;
    if let Err(error) = options.history.submit(crate::persistence::Change::Record {
        anime: request.anime,
        episode: request.episode.number,
        mode: request.mode,
    }) {
        child.kill().await?;
        return Err(error);
    }

    let _ = notify.send(Event {
        id: request.id,
        update: Update::Started,
    });

    let Some(status) = wait_child(&mut child, cancel).await? else {
        return Ok(None);
    };
    anyhow::ensure!(
        status.success(),
        "Player exited with {status}. See {}",
        log.display()
    );

    Ok(Some(Output::Playback))
}

async fn wait_child(
    child: &mut Child,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Option<ExitStatus>> {
    tokio::select! {
        biased;
        _ = cancel => {
            child.kill().await?;
            Ok(None)
        },
        result = child.wait() => Ok(Some(result?)),
    }
}

pub fn shutdown_signals() -> Result<[tokio::signal::unix::Signal; 3]> {
    use tokio::signal::unix::{SignalKind, signal};
    Ok([
        signal(SignalKind::interrupt())?,
        signal(SignalKind::terminate())?,
        signal(SignalKind::hangup())?,
    ])
}
