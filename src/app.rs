use crate::{
    artwork::Artwork,
    model::{Anime, Episode, Mode, Stream, normalized},
    player,
    provider::Provider,
    ui,
};
use anyhow::Result;
use crossterm::SynchronizedUpdate;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lru::LruCache;
use ratatui_image::picker::Picker;
use std::num::NonZeroUsize;
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Search,
    Episodes,
    History,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Search,
    List,
    Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Playback,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobPhase {
    Resolving,
    Running,
    Finished,
    Failed,
}

pub struct Activity {
    pub anime: Anime,
    pub episodes: Vec<Episode>,
    pub index: usize,
    pub remaining: VecDeque<usize>,
    pub mode: Mode,
    pub quality: String,
    pub kind: JobKind,
    pub phase: JobPhase,
    pub message: String,
}

impl Activity {
    pub fn active(&self) -> bool {
        matches!(self.phase, JobPhase::Resolving | JobPhase::Running)
    }
}

#[derive(Clone, Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}
impl Input {
    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }
    pub fn edit(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control() =>
            {
                if self.text.len() >= 512 {
                    return false;
                }
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let start = self.text[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                let n = self.text[self.cursor..]
                    .graphemes(true)
                    .next()
                    .unwrap()
                    .len();
                self.text.replace_range(self.cursor..self.cursor + n, "");
            }
            KeyCode::Left => {
                self.cursor = self.text[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                return false;
            }
            KeyCode::Right if self.cursor < self.text.len() => {
                self.cursor += self.text[self.cursor..]
                    .graphemes(true)
                    .next()
                    .unwrap()
                    .len();
                return false;
            }
            KeyCode::Home => {
                self.cursor = 0;
                return false;
            }
            KeyCode::End => {
                self.cursor = self.text.len();
                return false;
            }
            _ => return false,
        }
        true
    }
}

pub enum Synopsis {
    Loading,
    Ready(String),
    Failed(String),
}

pub struct State {
    pub screen: Screen,
    pub focus: Focus,
    pub query: Input,
    pub results: Vec<Anime>,
    pub results_query: String,
    pub history: Vec<crate::persistence::Entry>,
    pub history_selected: usize,
    pub home_selected: usize,
    pub list_offsets: [usize; 4],
    pub range: Input,
    pub marked: BTreeSet<usize>,
    pub art_revision: u64,
    pub selected: usize,
    pub anime: Option<Anime>,
    pub episodes: Vec<Episode>,
    pub episode_selected: usize,
    pub mode: Mode,
    pub quality: String,
    pub exact: bool,
    pub search_generation: u64,
    pub episode_generation: u64,
    pub loading: bool,
    pub status: String,
    pub error: bool,
    pub request_error: Option<String>,
    pub details: bool,
    pub details_scroll: u16,
    pub help: bool,
    pub help_scroll: u16,
    pub frame: usize,
    pub ascii: bool,
    pub colorless: bool,
    pub background: Option<(u8, u8, u8)>,
    pub synopses: LruCache<String, Synopsis>,
    pub animate: bool,
    pub activity: Option<Activity>,
}

impl State {
    pub fn new(
        query: String,
        mode: Mode,
        quality: String,
        exact: bool,
        ascii: bool,
        animate: bool,
    ) -> Self {
        let mut input = Input::default();
        let searching = !query.is_empty();
        input.set(query);
        Self {
            screen: if searching {
                Screen::Search
            } else {
                Screen::Home
            },
            focus: if searching {
                Focus::Search
            } else {
                Focus::List
            },
            query: input,
            results: vec![],
            results_query: String::new(),
            history: vec![],
            history_selected: 0,
            home_selected: 0,
            list_offsets: [0; 4],
            range: Input::default(),
            marked: BTreeSet::new(),
            art_revision: 0,
            selected: 0,
            anime: None,
            episodes: vec![],
            episode_selected: 0,
            mode,
            quality,
            exact,
            search_generation: 0,
            episode_generation: 0,
            loading: false,
            status: String::new(),
            error: false,
            request_error: None,
            details: false,
            details_scroll: 0,
            help: false,
            help_scroll: 0,
            frame: 0,
            ascii,
            colorless: false,
            background: None,
            synopses: LruCache::new(NonZeroUsize::new(32).unwrap()),
            animate,
            activity: None,
        }
    }

    pub fn visible(&self) -> Vec<usize> {
        let query = normalized(&self.query.text);
        self.results
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                let title = normalized(&a.title);
                if self.exact {
                    title == query
                } else {
                    query == normalized(&self.results_query) || title.contains(&query)
                }
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_anime(&self) -> Option<&Anime> {
        match self.screen {
            Screen::Home => self.history.get(self.home_selected).map(|e| &e.anime),
            Screen::Search if self.request_error.is_some() => None,
            Screen::Search => self
                .visible()
                .get(self.selected)
                .and_then(|i| self.results.get(*i)),
            Screen::Episodes => self.anime.as_ref(),
            Screen::History => self.history.get(self.history_selected).map(|e| &e.anime),
        }
    }

    pub fn current_episode_index(&self) -> Option<usize> {
        let activity = self.activity.as_ref()?;
        if activity.kind != JobKind::Playback || self.anime.as_ref()?.id != activity.anime.id {
            return None;
        }
        let episode = &activity.episodes[activity.index];
        self.episodes.iter().position(|e| e.id == episode.id)
    }

    pub fn welcome(&self) -> bool {
        self.screen == Screen::Home && self.history.is_empty()
    }

    pub fn invalidate_search(&mut self) {
        self.search_generation += 1;
        self.selected = 0;
        self.list_offsets[Screen::Search as usize] = 0;
        self.loading = !self.query.text.trim().is_empty();
        self.error = false;
        self.request_error = None;
        self.status = if self.loading {
            "Searching after you pause..."
        } else {
            "Type a title to begin."
        }
        .into();
        if !self.loading {
            self.results.clear();
        }
    }

    pub fn accept_search(&mut self, generation: u64, result: Result<Vec<Anime>>) -> bool {
        if generation != self.search_generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(results) => {
                let selected_id = self.selected_anime().map(|a| a.id.clone());
                self.results = results;
                self.results_query = self.query.text.clone();
                self.selected = selected_id
                    .and_then(|id| {
                        self.visible()
                            .iter()
                            .position(|i| self.results[*i].id == id)
                    })
                    .unwrap_or(0);
                self.error = false;
                self.request_error = None;
                self.status = if self.visible().is_empty() {
                    "No matches. Try another title or turn off exact matching (Ctrl+E).".into()
                } else {
                    "Choose a series to browse episodes.".into()
                };
            }
            Err(err) => {
                self.error = true;
                self.status = format!("{err:#}");
                self.request_error = Some(self.status.clone());
            }
        }
        true
    }

    fn message(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.error = error;
    }
}

enum Message {
    Synopsis(u64, String, Result<String>),
    Search(u64, Result<Vec<Anime>>),
    Episodes(u64, Result<Vec<Episode>>),
    Resolved(u64, Result<Stream>),
    Finished(u64, Result<()>),
    Downloaded(u64, Result<PathBuf>),
    Progress(u64, String),
    Library(Result<crate::persistence::Library>),
}

pub struct Options {
    pub player: String,
    pub player_args: Vec<String>,
    pub cache: PathBuf,
    pub state_dir: PathBuf,
    pub images: bool,
    pub download_dir: PathBuf,
}

enum Visit {
    Home,
    History,
    Search {
        query: Input,
        results: Vec<Anime>,
        results_query: String,
        selected: usize,
        offset: usize,
        exact: bool,
    },
    Episodes(Box<EpisodeVisit>),
}

struct EpisodeVisit {
    anime: Option<Anime>,
    episodes: Vec<Episode>,
    selected: usize,
    offset: usize,
    marked: BTreeSet<usize>,
    mode: Mode,
    continuation: Option<crate::persistence::Entry>,
}

pub struct App {
    pub state: State,
    provider: Provider,
    options: Options,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
    search_task: Option<JoinHandle<()>>,
    episode_task: Option<JoinHandle<()>>,
    synopsis_task: Option<JoinHandle<()>>,
    synopsis_generation: u64,
    play_task: Option<JoinHandle<()>>,
    search_due: Option<Instant>,
    job: u64,
    quit: bool,
    back_stack: Vec<Visit>,
    continuation: Option<crate::persistence::Entry>,
    save_tx: Option<mpsc::UnboundedSender<crate::persistence::Change>>,
    save_task: Option<JoinHandle<()>>,
}

fn abort(task: &mut Option<JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
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

impl App {
    pub fn new(state: State, provider: Provider, options: Options) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (save_tx, mut save_rx) = mpsc::unbounded_channel();
        let store = crate::persistence::Store::new(&options.state_dir);
        let notify = tx.clone();
        let save_task = tokio::spawn(async move {
            while let Some(change) = save_rx.recv().await {
                let store = store.clone();
                let result = tokio::task::spawn_blocking(move || store.update(change))
                    .await
                    .unwrap_or_else(|e| Err(e.into()));
                let _ = notify.send(Message::Library(result));
            }
        });
        let search_due = if state.query.text.is_empty() || state.screen != Screen::Search {
            None
        } else {
            Some(Instant::now())
        };
        Self {
            state,
            provider,
            options,
            tx,
            rx,
            search_task: None,
            episode_task: None,
            synopsis_task: None,
            synopsis_generation: 0,
            play_task: None,
            search_due,
            job: 0,
            quit: false,
            back_stack: vec![],
            continuation: None,
            save_tx: Some(save_tx),
            save_task: Some(save_task),
        }
    }

    fn search(&mut self) {
        self.search_due = None;
        abort(&mut self.search_task);
        if self.state.query.text.trim().is_empty() {
            return;
        }
        self.state.loading = true;
        self.state.request_error = None;
        self.state.message("Searching the catalogue...", false);
        let generation = self.state.search_generation;
        let query = self.state.query.text.clone();
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.search_task = Some(tokio::spawn(async move {
            let result = provider.search(&query).await;
            let _ = tx.send(Message::Search(generation, result));
        }));
    }

    fn persist(&self, change: crate::persistence::Change) {
        if let Some(tx) = &self.save_tx {
            let _ = tx.send(change);
        }
    }

    fn settings(&self) {
        self.persist(crate::persistence::Change::Settings {
            mode: self.state.mode,
            quality: self.state.quality.clone(),
        });
    }

    fn navigate(&mut self, screen: Screen) {
        if self.state.screen != screen {
            self.back_stack.push(match self.state.screen {
                Screen::Home => Visit::Home,
                Screen::History => Visit::History,
                Screen::Search => Visit::Search {
                    query: self.state.query.clone(),
                    results: self.state.results.clone(),
                    results_query: self.state.results_query.clone(),
                    selected: self.state.selected,
                    offset: self.state.list_offsets[Screen::Search as usize],
                    exact: self.state.exact,
                },
                Screen::Episodes => Visit::Episodes(Box::new(EpisodeVisit {
                    anime: self.state.anime.clone(),
                    episodes: self.state.episodes.clone(),
                    selected: self.state.episode_selected,
                    offset: self.state.list_offsets[Screen::Episodes as usize],
                    marked: self.state.marked.clone(),
                    mode: self.state.mode,
                    continuation: self.continuation.clone(),
                })),
            });
        }
        self.show(screen);
    }

    fn show(&mut self, screen: Screen) {
        abort(&mut self.search_task);
        abort(&mut self.episode_task);
        self.search_due = None;
        self.state.search_generation += 1;
        self.state.episode_generation += 1;
        self.state.screen = screen;
        self.state.focus = Focus::List;
        self.state.details = false;
        self.state.loading = false;
        self.state.request_error = None;
        self.state.message("", false);
        if screen == Screen::Search
            && !self.state.query.text.trim().is_empty()
            && self.state.results_query != self.state.query.text
        {
            self.search();
        } else if screen == Screen::Episodes && self.state.episodes.is_empty() {
            self.load_episodes();
        }
    }

    fn home(&mut self) {
        self.back_stack.clear();
        self.show(Screen::Home);
    }

    fn back(&mut self) {
        let screen = match self.back_stack.pop().unwrap_or(Visit::Home) {
            Visit::Home => Screen::Home,
            Visit::History => Screen::History,
            Visit::Search {
                query,
                results,
                results_query,
                selected,
                offset,
                exact,
            } => {
                self.state.query = query;
                self.state.results = results;
                self.state.results_query = results_query;
                self.state.selected = selected;
                self.state.list_offsets[Screen::Search as usize] = offset;
                self.state.exact = exact;
                Screen::Search
            }
            Visit::Episodes(visit) => {
                let EpisodeVisit {
                    anime,
                    episodes,
                    selected,
                    offset,
                    marked,
                    mode,
                    continuation,
                } = *visit;
                self.state.anime = anime;
                self.state.episodes = episodes;
                self.state.episode_selected = selected;
                self.state.list_offsets[Screen::Episodes as usize] = offset;
                self.state.marked = marked;
                self.state.mode = mode;
                self.continuation = continuation;
                Screen::Episodes
            }
        };
        self.show(screen);
    }

    fn history(&mut self) {
        self.navigate(Screen::History);
    }

    fn open(&mut self) {
        let Some(anime) = self.state.selected_anime().cloned() else {
            return;
        };
        let entry = match self.state.screen {
            Screen::Home => self.state.history.get(self.state.home_selected),
            Screen::History => self.state.history.get(self.state.history_selected),
            _ => None,
        }
        .cloned();
        if let Some(entry) = &entry {
            self.state.mode = entry.mode;
        }
        let same_series = self.state.anime.as_ref().is_some_and(|a| a.id == anime.id);
        if !same_series {
            self.state.marked.clear();
            self.state.episodes.clear();
            self.state.episode_selected = 0;
            self.state.list_offsets[Screen::Episodes as usize] = 0;
        }
        self.state.anime = Some(anime);
        self.continuation = entry;
        self.navigate(Screen::Episodes);
        if !self.state.episodes.is_empty() {
            self.select_continuation();
        }
    }

    fn select_continuation(&mut self) {
        if self.state.episodes.is_empty() {
            self.continuation = None;
            return;
        }
        if let Some(entry) = self.continuation.take() {
            if let Some(i) = crate::persistence::next_episode(&entry, &self.state.episodes) {
                self.state.episode_selected = i;
                self.state.message(
                    format!(
                        "Next episode: {}. Enter to play.",
                        self.state.episodes[i].number
                    ),
                    false,
                );
            } else {
                self.state.episode_selected = self
                    .state
                    .episodes
                    .iter()
                    .position(|e| e.number == entry.episode)
                    .unwrap_or(0);
                self.state.message(
                    "No newer episode. Enter replays the last played episode.",
                    false,
                );
            }
        }
    }

    fn load_episodes(&mut self) {
        let Some(anime) = self.state.anime.clone() else {
            return;
        };
        abort(&mut self.episode_task);
        self.state.episode_generation += 1;
        self.state.loading = true;
        self.state.request_error = None;
        self.state.message("Loading episodes...", false);
        let generation = self.state.episode_generation;
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.episode_task = Some(tokio::spawn(async move {
            let result = provider.episodes(&anime).await;
            let _ = tx.send(Message::Episodes(generation, result));
        }));
    }

    fn load_synopsis(&mut self, force: bool) {
        let Some(anime) = self.state.selected_anime().cloned() else {
            return;
        };
        if !force
            && matches!(
                self.state.synopses.get(&anime.id),
                Some(Synopsis::Ready(_) | Synopsis::Failed(_))
            )
        {
            return;
        }
        abort(&mut self.synopsis_task);
        self.synopsis_generation += 1;
        let generation = self.synopsis_generation;
        self.state.synopses.put(anime.id.clone(), Synopsis::Loading);
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.synopsis_task = Some(tokio::spawn(async move {
            let result = provider.synopsis(&anime).await;
            let _ = tx.send(Message::Synopsis(generation, anime.id, result));
        }));
    }

    fn retry(&mut self) {
        self.state.art_revision += 1;
        if self.state.details {
            self.load_synopsis(true);
            return;
        }
        match self.state.screen {
            Screen::Search => self.search(),
            Screen::Episodes => self.load_episodes(),
            _ => {}
        }
    }

    fn stop(&mut self) {
        self.job += 1;
        abort(&mut self.play_task);
        self.state.activity = None;
    }

    fn start_selection(&mut self, download: bool) {
        let indices: VecDeque<_> = if self.state.marked.is_empty() {
            [self.state.episode_selected].into()
        } else {
            self.state.marked.iter().copied().collect()
        };
        self.start(indices, download);
    }

    fn start(&mut self, mut indices: VecDeque<usize>, download: bool) {
        let Some(anime) = self.state.anime.clone() else {
            return;
        };
        let Some(index) = indices
            .pop_front()
            .filter(|i| *i < self.state.episodes.len())
        else {
            return;
        };
        self.stop();
        self.state.activity = Some(Activity {
            anime,
            episodes: self.state.episodes.clone(),
            index,
            remaining: indices,
            mode: self.state.mode,
            quality: self.state.quality.clone(),
            kind: if download {
                JobKind::Download
            } else {
                JobKind::Playback
            },
            phase: JobPhase::Resolving,
            message: String::new(),
        });
        self.play(index);
    }

    fn navigate_play(&mut self, index: usize) {
        if index >= self.state.episodes.len() {
            self.state
                .message("There are no more episodes in this direction.", false);
            return;
        }
        self.state.episode_selected = index;
        self.start([index].into(), false);
    }

    fn play(&mut self, index: usize) {
        let Some(activity) = self.state.activity.as_mut() else {
            return;
        };
        let episode = activity.episodes[index].clone();
        activity.index = index;
        activity.phase = JobPhase::Resolving;
        activity.message = "Finding a stream...".into();
        self.job += 1;
        abort(&mut self.play_task);
        let job = self.job;
        let mode = activity.mode;
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.play_task = Some(tokio::spawn(async move {
            let result = provider.resolve(&episode, mode).await;
            let _ = tx.send(Message::Resolved(job, result));
        }));
    }

    fn finish(&mut self, result: Result<()>, success: String) {
        let Some(activity) = self.state.activity.as_mut() else {
            return;
        };
        match result {
            Ok(()) => {
                if let Some(index) = activity.remaining.pop_front() {
                    self.play(index);
                } else {
                    activity.phase = JobPhase::Finished;
                    activity.message = success;
                }
            }
            Err(err) => {
                activity.remaining.clear();
                activity.phase = JobPhase::Failed;
                activity.message = format!("{err:#}");
            }
        }
    }

    fn message(&mut self, message: Message) {
        match message {
            Message::Synopsis(generation, id, result) if generation == self.synopsis_generation => {
                self.state.synopses.put(
                    id,
                    match result {
                        Ok(text) => Synopsis::Ready(text),
                        Err(err) => Synopsis::Failed(format!("{err:#}")),
                    },
                );
            }
            Message::Search(generation, result) => {
                self.state.accept_search(generation, result);
            }
            Message::Episodes(generation, result)
                if generation == self.state.episode_generation
                    && self.state.screen == Screen::Episodes =>
            {
                self.state.loading = false;
                match result {
                    Ok(episodes) => {
                        // Refreshes preserve marks by episode identity, not the old row number.
                        let marked: Vec<_> = self
                            .state
                            .marked
                            .iter()
                            .filter_map(|i| self.state.episodes.get(*i))
                            .map(|e| e.id.clone())
                            .collect();
                        let selected = self
                            .state
                            .episodes
                            .get(self.state.episode_selected)
                            .map(|e| e.id.clone());
                        self.state.episodes = episodes;
                        self.state.marked = self
                            .state
                            .episodes
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| marked.contains(&e.id))
                            .map(|(i, _)| i)
                            .collect();
                        self.state.episode_selected = selected
                            .and_then(|id| self.state.episodes.iter().position(|e| e.id == id))
                            .unwrap_or(0);
                        self.state.request_error = None;
                        self.state.message("", false);
                        self.select_continuation();
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        self.state.request_error = Some(message.clone());
                        self.state.message(message, true);
                    }
                }
            }
            Message::Resolved(job, result) if job == self.job => {
                let stream = match result {
                    Ok(stream) => stream,
                    Err(err) => {
                        self.finish(Err(err), String::new());
                        return;
                    }
                };
                let Some(activity) = self.state.activity.as_mut() else {
                    return;
                };
                let episode = &activity.episodes[activity.index];
                let title = format!("{} / Episode {}", activity.anime.title, episode.number);
                let quality = activity.quality.clone();
                let log = self
                    .options
                    .state_dir
                    .join(if activity.kind == JobKind::Download {
                        "download.log"
                    } else {
                        "player.log"
                    });
                if activity.kind == JobKind::Download {
                    let title = format!("{title} {}", activity.mode.as_str().to_uppercase());
                    let dir = self.options.download_dir.clone();
                    let tx = self.tx.clone();
                    activity.phase = JobPhase::Running;
                    activity.message = "Downloading...".into();
                    self.play_task = Some(tokio::spawn(async move {
                        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
                        let task = crate::download::download(
                            &stream,
                            &quality,
                            &title,
                            &dir,
                            &log,
                            progress_tx,
                        );
                        tokio::pin!(task);
                        let result = loop {
                            tokio::select! {
                                result = &mut task => break result,
                                Some(text) = progress_rx.recv() => { let _ = tx.send(Message::Progress(job, text)); }
                            }
                        };
                        let _ = tx.send(Message::Downloaded(job, result));
                    }));
                    return;
                }
                match player::launch(
                    &self.options.player,
                    &self.options.player_args,
                    &stream,
                    &quality,
                    &title,
                    &log,
                ) {
                    Ok(mut child) => {
                        activity.phase = JobPhase::Running;
                        let (variant, fallback) = stream.select(&quality);
                        activity.message = format!(
                            "{}p{}",
                            variant.height,
                            if fallback {
                                " (requested quality unavailable)"
                            } else {
                                ""
                            }
                        );
                        let change = crate::persistence::Change::Record {
                            anime: activity.anime.clone(),
                            episode: episode.number.clone(),
                            mode: activity.mode,
                        };
                        self.persist(change);
                        let tx = self.tx.clone();
                        self.play_task = Some(tokio::spawn(async move {
                            let result = match child.wait().await {
                                Ok(status) if status.success() => Ok(()),
                                Ok(status) => Err(anyhow::anyhow!(
                                    "Player exited with {status}. See {}",
                                    log.display()
                                )),
                                Err(err) => Err(err.into()),
                            };
                            let _ = tx.send(Message::Finished(job, result));
                        }));
                    }
                    Err(err) => self.finish(Err(err), String::new()),
                }
            }
            Message::Library(result) => match result {
                Ok(library) => {
                    let selected =
                        [self.state.home_selected, self.state.history_selected].map(|i| {
                            self.state
                                .history
                                .get(i)
                                .map(|e| (e.anime.id.clone(), e.mode))
                        });
                    self.state.history = library.entries;
                    let positions = selected.map(|key| {
                        key.and_then(|(id, mode)| {
                            self.state
                                .history
                                .iter()
                                .position(|e| e.anime.id == id && e.mode == mode)
                        })
                        .unwrap_or(0)
                    });
                    self.state.home_selected = positions[0].min(7);
                    self.state.history_selected = positions[1];
                }
                Err(err) => self
                    .state
                    .message(format!("History was not saved: {err:#}"), true),
            },
            Message::Progress(job, text) if job == self.job => {
                if let Some(activity) = &mut self.state.activity {
                    activity.message = text;
                }
            }
            Message::Downloaded(job, result) if job == self.job => match result {
                Ok(path) => self.finish(Ok(()), format!("Saved {}", path.display())),
                Err(err) => self.finish(Err(err), String::new()),
            },
            Message::Finished(job, result) if job == self.job => {
                self.finish(result, "Player closed".into());
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.state.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => self.state.help = false,
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.state.help_scroll = self.state.help_scroll.saturating_add(1).min(40)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.state.help_scroll = self.state.help_scroll.saturating_add(8).min(40)
                }
                KeyCode::PageUp => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(8)
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::F(1) {
            self.state.help = true;
            return;
        }
        if key.code == KeyCode::F(5) {
            self.retry();
            return;
        }
        if self.state.details {
            match key.code {
                KeyCode::Esc | KeyCode::Char('i') => self.state.details = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.state.details_scroll = self.state.details_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.details_scroll = self.state.details_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.state.details_scroll = self.state.details_scroll.saturating_add(10)
                }
                KeyCode::PageUp => {
                    self.state.details_scroll = self.state.details_scroll.saturating_sub(10)
                }
                KeyCode::Home => self.state.details_scroll = 0,
                KeyCode::End => self.state.details_scroll = u16::MAX,
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('x') => self.stop(),
                _ => {}
            }
            return;
        }
        if self.state.focus == Focus::Range {
            match key.code {
                KeyCode::Esc => self.state.focus = Focus::List,
                KeyCode::Enter => {
                    match crate::model::episode_range(&self.state.range.text, &self.state.episodes)
                    {
                        Ok(indices) => {
                            self.state.marked = indices.into_iter().collect();
                            self.state.focus = Focus::List;
                            self.state.message(
                                format!(
                                    "{} episodes marked / Enter play / d download",
                                    self.state.marked.len()
                                ),
                                false,
                            );
                        }
                        Err(err) => self.state.message(err.to_string(), true),
                    }
                }
                _ => {
                    self.state.range.edit(key);
                }
            }
            return;
        }
        if self.state.focus == Focus::Search {
            match key.code {
                KeyCode::Enter | KeyCode::Down | KeyCode::Tab => {
                    if self.search_due.is_some() {
                        self.search();
                    }
                    self.state.focus = Focus::List;
                }
                KeyCode::Esc => self.state.focus = Focus::List,
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.state.exact = !self.state.exact;
                    self.state.selected = 0;
                }
                _ => {
                    if self.state.query.edit(key) {
                        self.state.invalidate_search();
                        abort(&mut self.search_task);
                        self.search_due = Some(Instant::now() + Duration::from_millis(300));
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.state.help = true,
            KeyCode::Char('/') => {
                self.navigate(Screen::Search);
                self.state.focus = Focus::Search;
            }
            KeyCode::Esc => self.back(),
            KeyCode::Char('H') => self.home(),
            KeyCode::Char('h') => self.history(),
            KeyCode::Char('i') if self.state.selected_anime().is_some() => {
                self.state.details = true;
                self.state.details_scroll = 0;
                self.load_synopsis(false);
            }
            KeyCode::Delete if self.state.screen == Screen::History => {
                if let Some(entry) = self.state.history.get(self.state.history_selected) {
                    self.persist(crate::persistence::Change::Remove {
                        id: entry.anime.id.clone(),
                        mode: entry.mode,
                    });
                }
            }
            KeyCode::Char(':') if self.state.screen == Screen::Episodes => {
                self.state.focus = Focus::Range;
                self.state.range.set(String::new());
            }
            KeyCode::Char(' ') | KeyCode::Tab if self.state.screen == Screen::Episodes => {
                let i = self.state.episode_selected;
                if !self.state.marked.remove(&i) && i < self.state.episodes.len() {
                    self.state.marked.insert(i);
                }
                self.state.message(
                    format!(
                        "{} marked / Enter play / d download / : range",
                        self.state.marked.len()
                    ),
                    false,
                );
            }
            KeyCode::Char('d') if self.state.screen == Screen::Episodes => {
                self.start_selection(true)
            }
            KeyCode::Char('D') => {
                self.state.mode.toggle();
                self.settings();
                self.state.message(
                    format!(
                        "{} selected for your next play or download selection.",
                        self.state.mode.as_str().to_uppercase()
                    ),
                    false,
                );
            }
            KeyCode::Char('Q') => {
                let choices = ["best", "1080p", "720p", "480p", "360p", "worst"];
                let index = choices
                    .iter()
                    .position(|q| *q == self.state.quality)
                    .unwrap_or(0);
                self.state.quality = choices[(index + 1) % choices.len()].into();
                self.settings();
                self.state.message(
                    "Quality applies to your next play or download selection.",
                    false,
                );
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.exact = !self.state.exact;
                self.state.selected = 0;
            }
            KeyCode::Enter
                if matches!(
                    self.state.screen,
                    Screen::Home | Screen::Search | Screen::History
                ) =>
            {
                self.open()
            }
            KeyCode::Enter if self.state.screen == Screen::Episodes => self.start_selection(false),
            KeyCode::Char('x') => {
                self.stop();
                self.state
                    .message("Playback / download queue stopped.", false);
            }
            KeyCode::Char('n') if self.state.screen == Screen::Episodes => self.navigate_play(
                self.state
                    .current_episode_index()
                    .unwrap_or(self.state.episode_selected)
                    + 1,
            ),
            KeyCode::Char('p') if self.state.screen == Screen::Episodes => {
                if let Some(i) = self
                    .state
                    .current_episode_index()
                    .unwrap_or(self.state.episode_selected)
                    .checked_sub(1)
                {
                    self.navigate_play(i);
                } else {
                    self.state.message("Already at the first episode.", false);
                }
            }
            KeyCode::Char('r') if self.state.screen == Screen::Episodes => self.navigate_play(
                self.state
                    .current_episode_index()
                    .unwrap_or(self.state.episode_selected),
            ),
            KeyCode::Char('R') => self.retry(),
            code => {
                let len = match self.state.screen {
                    Screen::Home => self.state.history.len().min(8),
                    Screen::Search => self.state.visible().len(),
                    Screen::History => self.state.history.len(),
                    Screen::Episodes => self.state.episodes.len(),
                };
                let selected = if self.state.screen == Screen::Home {
                    &mut self.state.home_selected
                } else if self.state.screen == Screen::Search {
                    &mut self.state.selected
                } else if self.state.screen == Screen::History {
                    &mut self.state.history_selected
                } else {
                    &mut self.state.episode_selected
                };
                *selected = match code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        selected.saturating_add(1).min(len.saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => selected.saturating_sub(1),
                    KeyCode::PageDown => selected.saturating_add(10).min(len.saturating_sub(1)),
                    KeyCode::PageUp => selected.saturating_sub(10),
                    KeyCode::Home | KeyCode::Char('g') => 0,
                    KeyCode::End | KeyCode::Char('G') => len.saturating_sub(1),
                    _ => *selected,
                };
            }
        }
    }

    pub async fn run(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        picker: Picker,
        signals: [tokio::signal::unix::Signal; 3],
    ) -> Result<()> {
        let mut art = Artwork::new(picker, self.options.cache.clone(), self.options.images);
        let (art_tx, mut art_rx) = mpsc::unbounded_channel();
        // Keep all terminal reads on this thread. EventStream races with the cursor
        // queries performed by Ratatui 0.30.2 during clear/autoresize.
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        let mut animation = Instant::now();
        let mut dirty = true;
        let mut art_revision = 0;
        let [mut interrupt, mut terminate, mut hangup] = signals;
        while !self.quit {
            if self.search_due.is_some_and(|at| Instant::now() >= at) {
                self.search();
                dirty = true;
            }
            while crossterm::event::poll(Duration::ZERO)? {
                dirty = true;
                match crossterm::event::read()? {
                    Event::Key(key) => self.key(key),
                    Event::Paste(text) if self.state.focus == Focus::Search => {
                        for c in text.chars().filter(|c| !c.is_control()).take(512) {
                            self.state
                                .query
                                .edit(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                        }
                        self.state.invalidate_search();
                        abort(&mut self.search_task);
                        self.search_due = Some(Instant::now() + Duration::from_millis(300));
                    }
                    _ => {}
                }
            }
            if dirty {
                let size = terminal.size()?;
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let cover = ui::cover_area(&self.state, area);
                let url = if self.state.help {
                    None
                } else {
                    self.state.selected_anime().and_then(|a| a.cover.as_deref())
                };
                if art_revision != self.state.art_revision {
                    art.request(None, cover.as_size(), self.provider.clone(), art_tx.clone());
                    art_revision = self.state.art_revision;
                }
                art.request(url, cover.as_size(), self.provider.clone(), art_tx.clone());
                std::io::stdout().sync_update(|_| -> Result<()> {
                    if art.clear {
                        art.clear_graphics()?;
                        terminal.clear()?;
                    }
                    terminal.draw(|frame| ui::render(frame, &mut self.state, &art))?;
                    Ok(())
                })??;
                dirty = false;
            }
            tokio::select! {
                _ = tick.tick() => {
                    if self.state.animate && animation.elapsed() >= Duration::from_millis(crate::scene::FRAME_INTERVAL_MS) {
                        animation = Instant::now();
                        self.state.frame = self.state.frame.wrapping_add(1);
                        dirty = self.state.loading || self.state.activity.as_ref().is_some_and(Activity::active) || self.state.welcome();
                    }
                }
                Some(message) = self.rx.recv() => { self.message(message); dirty = true; },
                Some(message) = art_rx.recv() => { art.accept(message); dirty = true; },
                _ = terminate.recv() => self.quit = true,
                _ = hangup.recv() => self.quit = true,
                _ = interrupt.recv() => self.quit = true,
            }
        }
        self.stop();
        abort(&mut self.search_task);
        abort(&mut self.episode_task);
        abort(&mut self.synopsis_task);
        self.save_tx.take();
        if let Some(task) = self.save_task.take() {
            task.await?;
        }
        while let Ok(message) = self.rx.try_recv() {
            if let Message::Library(Err(err)) = message {
                return Err(err);
            }
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        abort(&mut self.synopsis_task);
        abort(&mut self.search_task);
        abort(&mut self.episode_task);
        abort(&mut self.play_task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn anime(id: &str, title: &str) -> Anime {
        Anime {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }
    #[test]
    fn provider_alias_matches_remain_visible_and_cached_refresh_keeps_selection() {
        let mut state = State::new(
            "sousou".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        let results = vec![anime("1", "Frieren"), anime("2", "Another English title")];
        state.accept_search(0, Ok(results.clone()));
        assert_eq!(state.visible(), [0, 1]);
        state.selected = 1;
        state.accept_search(0, Ok(results));
        assert_eq!(state.selected, 1);
        state.exact = true;
        assert!(state.visible().is_empty());
    }
    #[tokio::test]
    async fn navigation_discards_stale_episode_playback_and_synopsis_messages() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        let options = Options {
            player: "does-not-exist".into(),
            player_args: vec![],
            cache: dir.path().join("cache"),
            state_dir: dir.path().into(),
            images: false,
            download_dir: dir.path().join("downloads"),
        };
        let mut app = App::new(state, Provider::new("http://127.0.0.1:9").unwrap(), options);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.episode_generation = 3;
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.message(Message::Episodes(
            3,
            Ok(vec![Episode {
                id: "1".into(),
                number: "1".into(),
                title: "stale".into(),
            }]),
        ));
        assert_eq!(app.state.screen, Screen::Home);
        assert!(app.state.episodes.is_empty());
        let old_job = app.job;
        app.stop();
        app.message(Message::Resolved(
            old_job,
            Ok(Stream {
                variants: vec![],
                referer: String::new(),
                audio: None,
                subtitle: None,
            }),
        ));
        assert!(app.state.activity.is_none());
        app.synopsis_generation = 2;
        app.message(Message::Synopsis(1, "old".into(), Ok("Stale text".into())));
        assert!(app.state.synopses.is_empty());
        app.message(Message::Synopsis(
            2,
            "current".into(),
            Ok("Full text".into()),
        ));
        assert!(
            matches!(app.state.synopses.peek("current"), Some(Synopsis::Ready(s)) if s == "Full text")
        );
        assert!(app.state.synopses.peek("old").is_none());
    }
    #[tokio::test]
    async fn range_entry_marks_actual_episode_order_without_launching() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::Range;
        state.episodes = ["1", "1.5", "2", "3"]
            .iter()
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        state.range.set("1-2".into());
        let options = Options {
            player: "does-not-exist".into(),
            player_args: vec![],
            cache: dir.path().join("cache"),
            state_dir: dir.path().into(),
            images: false,
            download_dir: dir.path().join("downloads"),
        };
        let mut app = App::new(state, Provider::new("http://127.0.0.1:9").unwrap(), options);
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.state.marked.iter().copied().collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(app.state.focus, Focus::List);
        assert!(app.state.activity.is_none());
    }
    #[tokio::test]
    async fn nested_back_restores_the_original_series_search_and_marks() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(
            "first".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.accept_search(0, Ok(vec![anime("first-1", "First series")]));
        state.focus = Focus::List;
        let options = Options {
            player: "does-not-exist".into(),
            player_args: vec![],
            cache: dir.path().join("cache"),
            state_dir: dir.path().into(),
            images: false,
            download_dir: dir.path().join("downloads"),
        };
        let mut app = App::new(state, Provider::new("http://127.0.0.1:9").unwrap(), options);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let back = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.key(enter);
        let episodes = vec![
            Episode {
                id: "11".into(),
                number: "1".into(),
                title: "First episode".into(),
            },
            Episode {
                id: "12".into(),
                number: "2".into(),
                title: "Second episode".into(),
            },
        ];
        app.message(Message::Episodes(
            app.state.episode_generation,
            Ok(episodes),
        ));
        app.state.episode_selected = 1;
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.state.query.set("other".into());
        app.state.invalidate_search();
        app.state.accept_search(
            app.state.search_generation,
            Ok(vec![anime("other-2", "Other series")]),
        );
        app.key(enter);
        app.key(enter);
        app.message(Message::Episodes(
            app.state.episode_generation,
            Ok(vec![Episode {
                id: "21".into(),
                number: "1".into(),
                title: "Other episode".into(),
            }]),
        ));
        app.key(back);
        assert_eq!(app.state.screen, Screen::Search);
        assert_eq!(app.state.query.text, "other");
        app.key(back);
        assert_eq!(app.state.screen, Screen::Episodes);
        assert_eq!(app.state.anime.as_ref().unwrap().id, "first-1");
        assert_eq!(app.state.episodes[app.state.episode_selected].id, "12");
        assert!(app.state.marked.contains(&1));
        app.key(back);
        assert_eq!(app.state.screen, Screen::Search);
        assert_eq!(app.state.query.text, "first");
        assert_eq!(app.state.selected_anime().unwrap().id, "first-1");
    }

    #[test]
    fn stale_search_cannot_overwrite_new_query_or_empty_state() {
        let mut state = State::new("old".into(), Mode::Sub, "best".into(), false, false, false);
        let old = state.search_generation;
        state.query.set("new".into());
        state.invalidate_search();
        assert!(!state.accept_search(
            old,
            Ok(vec![Anime {
                title: "old".into(),
                ..Default::default()
            }])
        ));
        assert!(state.results.is_empty());
        assert!(state.accept_search(
            state.search_generation,
            Ok(vec![Anime {
                title: "new".into(),
                ..Default::default()
            }])
        ));
        state.query.set(String::new());
        state.invalidate_search();
        assert!(!state.accept_search(old + 1, Ok(vec![Anime::default()])));
        assert!(state.results.is_empty());
    }
    #[test]
    fn input_edits_graphemes_without_splitting_utf8() {
        let mut input = Input::default();
        input.set("日本e\u{301}".into());
        input.edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.text, "日本");
        input.edit(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        input.edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(input.text, "日a本");
    }
}
