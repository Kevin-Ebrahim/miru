#[cfg(test)]
use crate::input::Input;
use crate::view::TextSelection;
#[cfg(test)]
use crate::view::TextView;
use crate::{
    artwork::Artwork,
    jobs::{self, Jobs},
    model::{Anime, Episode, Mode},
    provider::Provider,
    state::{
        Activity, EpisodesState, Focus, JobKind, JobPhase, Screen, SearchState, State, Synopsis,
    },
    ui,
};
use anyhow::Result;
use crossterm::SynchronizedUpdate;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::{clipboard::CopyToClipboard, execute};
use ratatui::layout::Position;
#[cfg(test)]
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

enum Message {
    Synopsis(u64, String, Result<String>),
    Search(u64, Result<Vec<Anime>>),
    Episodes(u64, Result<Vec<Episode>>),
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
    Search(Box<SearchState>, usize),
    Episodes(Box<EpisodesState>, usize, Mode),
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
    synopsis_target: Option<String>,
    synopsis_due: Option<Instant>,
    jobs: Jobs,
    search_due: Option<Instant>,
    job: u64,
    quit: bool,
    copy_requested: bool,
    back_stack: Vec<Visit>,
    writer: crate::persistence::Writer,
    library_rx: mpsc::UnboundedReceiver<Result<crate::persistence::Library>>,
}

fn abort(task: &mut Option<JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
    }
}

impl App {
    pub fn new(state: State, provider: Provider, options: Options) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (writer, library_rx) =
            crate::persistence::Writer::new(crate::persistence::Store::new(&options.state_dir));
        let search_due = if state.search.query.text.is_empty() || state.screen != Screen::Search {
            None
        } else {
            Some(Instant::now())
        };
        let jobs = Jobs::new(
            provider.clone(),
            jobs::Options {
                player: options.player.clone(),
                player_args: options.player_args.clone(),
                state_dir: options.state_dir.clone(),
                download_dir: options.download_dir.clone(),
                history: writer.sender(),
            },
        );
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
            synopsis_target: None,
            synopsis_due: None,
            jobs,
            search_due,
            job: 0,
            quit: false,
            copy_requested: false,
            back_stack: vec![],
            writer,
            library_rx,
        }
    }

    fn search(&mut self) {
        self.search_due = None;
        abort(&mut self.search_task);
        if self.state.search.query.text.trim().is_empty() {
            return;
        }
        self.state.loading = true;
        self.state.request_error = None;
        self.state.message("", false);
        let generation = self.state.search_generation;
        let query = self.state.search.query.text.clone();
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.search_task = Some(tokio::spawn(async move {
            let result = provider.search(&query).await;
            let _ = tx.send(Message::Search(generation, result));
        }));
    }

    fn persist(&self, change: crate::persistence::Change) {
        if let Err(error) = self.writer.submit(change) {
            let _ = self.tx.send(Message::Library(Err(error)));
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
                Screen::Search => Visit::Search(
                    Box::new(self.state.search.clone()),
                    self.state.view.list_offsets[Screen::Search as usize],
                ),
                Screen::Episodes => Visit::Episodes(
                    Box::new(self.state.episodes.clone()),
                    self.state.view.list_offsets[Screen::Episodes as usize],
                    self.state.mode,
                ),
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
        self.state.focus = if screen == Screen::Home {
            Focus::Search
        } else {
            Focus::List
        };
        self.state.details = false;
        self.state.loading = false;
        self.state.request_error = None;
        self.state.message("", false);
        if screen == Screen::Search
            && !self.state.search.query.text.trim().is_empty()
            && self.state.search.results_query != self.state.search.query.text
        {
            self.search();
        } else if screen == Screen::Episodes && self.state.episodes.items.is_empty() {
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
            Visit::Search(search, offset) => {
                self.state.search = *search;
                self.state.view.list_offsets[Screen::Search as usize] = offset;
                Screen::Search
            }
            Visit::Episodes(episodes, offset, mode) => {
                self.state.episodes = *episodes;
                self.state.view.list_offsets[Screen::Episodes as usize] = offset;
                self.state.mode = mode;
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
            Screen::History => self.state.selected_history(),
            _ => None,
        }
        .cloned();
        if let Some(entry) = &entry {
            self.state.mode = entry.mode;
        }
        let same_series = self
            .state
            .episodes
            .anime
            .as_ref()
            .is_some_and(|a| a.id == anime.id);
        if entry.is_some() || !same_series {
            self.state.episodes.marked.clear();
        }
        if !same_series {
            self.state.episodes.items.clear();
            self.state.episodes.selected = 0;
            self.state.view.list_offsets[Screen::Episodes as usize] = 0;
        }
        self.state.episodes.anime = Some(anime);
        self.state.episodes.continuation = entry;
        self.navigate(Screen::Episodes);
        if !self.state.episodes.items.is_empty() {
            self.select_continuation();
        }
    }

    fn select_continuation(&mut self) {
        if self.state.episodes.items.is_empty() {
            self.state.episodes.continuation = None;
            return;
        }
        if let Some(entry) = self.state.episodes.continuation.take() {
            if let Some(i) = crate::persistence::next_episode(&entry, &self.state.episodes.items) {
                self.state.episodes.selected = i;
                self.state.message("", false);
            } else if let Some(i) = self
                .state
                .episodes
                .items
                .iter()
                .position(|e| e.number == entry.episode)
            {
                self.state.episodes.selected = i;
                self.state.message(
                    "No newer episode. Enter replays the last played episode.",
                    false,
                );
            } else {
                self.state.episodes.selected = 0;
                self.state.message(
                    "Last played episode is unavailable. Select an episode.",
                    false,
                );
            }
        }
    }

    fn load_episodes(&mut self) {
        let Some(anime) = self.state.episodes.anime.clone() else {
            return;
        };
        abort(&mut self.episode_task);
        self.state.episode_generation += 1;
        self.state.loading = true;
        self.state.request_error = None;
        self.state.message("", false);
        let generation = self.state.episode_generation;
        let provider = self.provider.clone();
        let tx = self.tx.clone();
        self.episode_task = Some(tokio::spawn(async move {
            let result = provider.episodes(&anime).await;
            let _ = tx.send(Message::Episodes(generation, result));
        }));
    }

    fn cancel_synopsis(&mut self) {
        abort(&mut self.synopsis_task);
        self.synopsis_generation += 1;
        self.synopsis_due = None;
        if let Some(id) = self.synopsis_target.take()
            && matches!(self.state.synopses.peek(&id), Some(Synopsis::Loading))
        {
            self.state.synopses.pop(&id);
        }
    }

    fn prepare_synopsis(&mut self) -> bool {
        let target = self.state.selected_anime().map(|anime| anime.id.clone());
        if target == self.synopsis_target {
            return false;
        }
        self.cancel_synopsis();
        self.synopsis_target = target.clone();
        if let Some(id) = target
            && !matches!(
                self.state.synopses.get(&id),
                Some(Synopsis::Ready(_) | Synopsis::Failed)
            )
        {
            self.state.synopses.put(id, Synopsis::Loading);
            self.synopsis_due = Some(Instant::now() + Duration::from_millis(180));
        }
        true
    }

    fn load_synopsis(&mut self, force: bool) {
        let Some(anime) = self.state.selected_anime().cloned() else {
            return;
        };
        self.synopsis_due = None;
        if !force
            && matches!(
                self.state.synopses.get(&anime.id),
                Some(Synopsis::Ready(_) | Synopsis::Failed)
            )
        {
            return;
        }
        if !force
            && self.synopsis_target.as_deref() == Some(anime.id.as_str())
            && self.synopsis_task.is_some()
        {
            return;
        }
        self.cancel_synopsis();
        self.synopsis_target = Some(anime.id.clone());
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
        self.load_synopsis(true);
        match self.state.screen {
            Screen::Search => self.search(),
            Screen::Episodes => self.load_episodes(),
            _ => {}
        }
    }

    fn stop(&mut self) {
        self.job += 1;
        self.jobs.stop();
        self.state.activity = None;
    }

    fn retry_activity(&mut self) {
        if let Some(activity) = &self.state.activity
            && activity.retryable()
        {
            self.play(activity.index);
        }
    }

    fn start_selection(&mut self, download: bool) {
        let indices: VecDeque<_> = if self.state.episodes.marked.is_empty() {
            [self.state.episodes.selected].into()
        } else {
            self.state.episodes.marked.iter().copied().collect()
        };
        self.start(indices, download);
    }

    fn start(&mut self, mut indices: VecDeque<usize>, download: bool) {
        let Some(anime) = self.state.episodes.anime.clone() else {
            return;
        };
        let Some(index) = indices
            .pop_front()
            .filter(|i| *i < self.state.episodes.items.len())
        else {
            return;
        };
        self.stop();
        self.state.activity = Some(Activity {
            anime,
            episodes: self.state.episodes.items.clone(),
            index,
            remaining: indices,
            mode: self.state.mode,
            quality: self.state.quality.clone(),
            quality_note: String::new(),
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
        if index >= self.state.episodes.items.len() {
            self.state.message("No more episodes.", false);
            return;
        }
        self.state.episodes.selected = index;
        self.start([index].into(), false);
    }

    fn play(&mut self, index: usize) {
        let Some(activity) = self.state.activity.as_mut() else {
            return;
        };
        let episode = activity.episodes[index].clone();
        activity.index = index;
        activity.phase = JobPhase::Resolving;
        activity.message.clear();
        activity.quality_note.clear();
        self.job += 1;
        let mut title = format!("{} / Episode {}", activity.anime.title, episode.number);
        if activity.kind == JobKind::Download {
            title.push_str(&format!(" {}", activity.mode.as_str().to_uppercase()));
        }
        self.jobs.start(jobs::Request {
            id: self.job,
            episode,
            anime: activity.anime.clone(),
            mode: activity.mode,
            quality: activity.quality.clone(),
            title,
            kind: activity.kind,
        });
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
                        Err(_) => Synopsis::Failed,
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
                            .episodes
                            .marked
                            .iter()
                            .filter_map(|i| self.state.episodes.items.get(*i))
                            .map(|e| e.id.clone())
                            .collect();
                        let selected = self
                            .state
                            .episodes
                            .items
                            .get(self.state.episodes.selected)
                            .map(|e| e.id.clone());
                        self.state.episodes.items = episodes;
                        self.state.episodes.marked = self
                            .state
                            .episodes
                            .items
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| marked.contains(&e.id))
                            .map(|(i, _)| i)
                            .collect();
                        self.state.episodes.selected = selected
                            .and_then(|id| {
                                self.state.episodes.items.iter().position(|e| e.id == id)
                            })
                            .unwrap_or(0);
                        self.state.request_error = None;
                        self.state.message("", false);
                        self.select_continuation();
                    }
                    Err(err) => {
                        self.state.request_error = Some(format!("{err:#}"));
                    }
                }
            }
            Message::Library(result) => match result {
                Ok(library) => {
                    let previous = self.state.history.selected;
                    let key = self
                        .state
                        .selected_history()
                        .map(|e| (e.anime.id.clone(), e.mode));
                    self.state.history.replace_entries(library.entries);
                    self.state.history.selected = key
                        .and_then(|(id, mode)| {
                            self.state.visible_history().iter().position(|i| {
                                let e = &self.state.history.entries()[*i];
                                e.anime.id == id && e.mode == mode
                            })
                        })
                        .unwrap_or_else(|| {
                            previous.min(self.state.visible_history().len().saturating_sub(1))
                        });
                }
                Err(err) => self
                    .state
                    .message(format!("History was not saved: {err:#}"), true),
            },
            _ => {}
        }
    }

    fn job_message(&mut self, event: jobs::Event) {
        if event.id != self.job {
            return;
        }
        let Some(activity) = self.state.activity.as_mut() else {
            return;
        };
        match event.update {
            jobs::Update::Resolved(note) => activity.quality_note = note,
            jobs::Update::Started => {
                activity.phase = JobPhase::Running;
                activity.message.clear();
            }
            jobs::Update::Progress(text) => activity.message = text,
            jobs::Update::Finished(result) => match result {
                Ok(jobs::Output::Playback) => self.finish(Ok(()), "Player closed".into()),
                Ok(jobs::Output::Download(path)) => {
                    self.finish(Ok(()), format!("Saved {}", path.display()))
                }
                Err(error) => self.finish(Err(error), String::new()),
            },
        }
    }

    fn copy_text(&self) {
        if let Some(view) = &self.state.view.text_view {
            let text = self
                .state
                .view
                .text_selection
                .map(|selection| view.selected_text(selection))
                .unwrap_or_else(|| view.lines.join("\n"));
            if !text.is_empty() {
                let _ = execute!(std::io::stdout(), CopyToClipboard::to_clipboard_from(text));
            }
        }
    }

    fn mouse(&mut self, event: MouseEvent) -> Option<String> {
        if self.state.help {
            match event.kind {
                MouseEventKind::ScrollDown => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_add(3)
                }
                MouseEventKind::ScrollUp => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_sub(3)
                }
                _ => {}
            }
            return None;
        }
        if !self.state.details && self.state.view.text_view.is_none() {
            return None;
        }
        match event.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = event.kind == MouseEventKind::ScrollDown;
                let over_text =
                    self.state.view.text_view.as_ref().is_some_and(|view| {
                        view.area.contains(Position::new(event.column, event.row))
                    });
                if self.state.details || over_text {
                    let scroll = if self.state.details {
                        &mut self.state.view.details_scroll
                    } else {
                        &mut self.state.view.preview_scroll
                    };
                    *scroll = if down {
                        scroll.saturating_add(3)
                    } else {
                        scroll.saturating_sub(3)
                    };
                } else {
                    self.key(KeyEvent::new(
                        if down { KeyCode::Down } else { KeyCode::Up },
                        KeyModifiers::NONE,
                    ));
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.state.view.text_selection = self
                    .state
                    .view
                    .text_view
                    .as_ref()
                    .filter(|view| view.area.contains(Position::new(event.column, event.row)))
                    .map(|view| {
                        let point = view.position(event.column, event.row);
                        TextSelection {
                            anchor: point,
                            end: point,
                            dragging: true,
                        }
                    });
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = &mut self.state.view.text_selection
                    && selection.dragging
                    && let Some(view) = &self.state.view.text_view
                {
                    selection.end = view.position(event.column, event.row);
                    if event.kind == MouseEventKind::Up(MouseButton::Left) {
                        selection.dragging = false;
                        let text = view.selected_text(*selection);
                        if !text.is_empty() {
                            return Some(text);
                        }
                        self.state.view.text_selection = None;
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn paste(&mut self, text: &str) {
        if self.state.help && !self.state.help_filtering {
            return;
        }
        if !self.state.help
            && (self.state.details || !matches!(self.state.focus, Focus::Search | Focus::Range))
        {
            return;
        }
        let input = if self.state.help {
            &mut self.state.help_query
        } else if self.state.focus == Focus::Range {
            &mut self.state.episodes.range
        } else if self.state.screen == Screen::History {
            &mut self.state.history.query
        } else {
            &mut self.state.search.query
        };
        let mut changed = false;
        for c in text.chars().take(512) {
            let c = if c.is_whitespace() {
                ' '
            } else if c.is_control() {
                continue;
            } else {
                c
            };
            changed |= input.edit(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        if self.state.help {
            if changed {
                self.state.view.help_scroll = 0;
            }
            return;
        }
        if changed && self.state.screen == Screen::History {
            self.state.history_filter_changed();
        }
        if changed && self.state.focus == Focus::Search && self.state.screen == Screen::Search {
            self.state.invalidate_search();
            abort(&mut self.search_task);
            self.search_due = Some(Instant::now() + Duration::from_millis(300));
        }
    }

    fn open_help(&mut self) {
        self.state.help = true;
        self.state.view.help_scroll = 0;
        self.state.help_filtering = false;
        self.state.help_query.set(String::new());
    }

    fn cycle_quality(&mut self, backwards: bool) {
        let choices = ["best", "1080p", "720p", "480p", "360p", "worst"];
        let index = choices
            .iter()
            .position(|q| *q == self.state.quality)
            .unwrap_or(0);
        let step = if backwards { choices.len() - 1 } else { 1 };
        self.state.quality = choices[(index + step) % choices.len()].into();
        self.settings();
        self.state.message(
            if self.state.activity.as_ref().is_some_and(Activity::active) {
                "Quality applies to the next queue."
            } else {
                ""
            },
            false,
        );
    }

    fn home_key(&mut self, mut key: KeyEvent) -> bool {
        if key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('h') {
            self.history();
            return true;
        }
        if key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('e') {
            self.state.search.exact = !self.state.search.exact;
            return true;
        }
        if key.code == KeyCode::F(5) || key.code == KeyCode::F(6) {
            return false;
        }
        if key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('g') => key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyCode::Char('j' | 'm') => key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let focuses = [Focus::Search, Focus::Audio, Focus::Quality];
                let index = focuses.iter().position(|f| *f == self.state.focus);
                let backwards =
                    key.code == KeyCode::BackTab || key.modifiers == KeyModifiers::SHIFT;
                self.state.focus = index.map_or(Focus::Search, |i| {
                    focuses[(i + if backwards { 2 } else { 1 }) % 3]
                });
            }
            KeyCode::Enter if self.state.focus == Focus::Search => {
                if !self.state.search.query.text.trim().is_empty() {
                    self.navigate(Screen::Search);
                }
            }
            KeyCode::Enter if matches!(self.state.focus, Focus::Audio | Focus::Quality) => {
                self.state.focus = Focus::Search
            }
            KeyCode::Esc if self.state.focus == Focus::Search => self.state.focus = Focus::List,
            KeyCode::Esc if matches!(self.state.focus, Focus::Audio | Focus::Quality) => {
                self.state.focus = Focus::List
            }
            _ if self.state.focus == Focus::Search => {
                self.state.search.query.edit(key);
            }
            _ if matches!(self.state.focus, Focus::Audio | Focus::Quality) => {
                if key.modifiers.is_empty()
                    && matches!(
                        key.code,
                        KeyCode::Left
                            | KeyCode::Right
                            | KeyCode::Up
                            | KeyCode::Down
                            | KeyCode::Char('h' | 'j' | 'k' | 'l')
                    )
                {
                    if self.state.focus == Focus::Audio {
                        self.state.mode.toggle();
                        self.settings();
                        self.state.message("", false);
                    } else {
                        self.cycle_quality(matches!(
                            key.code,
                            KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k')
                        ));
                    }
                }
            }
            _ => return false,
        }
        true
    }

    fn key(&mut self, mut key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if key.code == KeyCode::F(1) {
            if self.state.help {
                self.state.help = false;
            } else {
                self.open_help();
            }
            return;
        }
        if self.state.help && self.state.help_filtering {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.state.help_filtering = false,
                KeyCode::Char('g' | 'j' | 'm') if key.modifiers == KeyModifiers::CONTROL => {
                    self.state.help_filtering = false
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.state.help_filtering = false;
                    self.state.view.help_scroll = self
                        .state
                        .view
                        .help_scroll
                        .saturating_add(if key.code == KeyCode::Down { 1 } else { 10 });
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.state.help_filtering = false;
                    self.state.view.help_scroll = self
                        .state
                        .view
                        .help_scroll
                        .saturating_sub(if key.code == KeyCode::Up { 1 } else { 10 });
                }
                _ => {
                    if self.state.help_query.edit(key) {
                        self.state.view.help_scroll = 0;
                    }
                }
            }
            return;
        }
        if self.state.screen == Screen::Home
            && !self.state.help
            && !self.state.details
            && self.home_key(key)
        {
            return;
        }
        // Modified navigation must not fall through to playback/download shortcuts.
        let editing = !self.state.help && !self.state.details && self.state.focus != Focus::List;
        if editing && key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('g') => key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyCode::Char('j' | 'm') => key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                _ => {}
            }
        }
        if !editing && key.modifiers == KeyModifiers::CONTROL {
            key.code = match key.code {
                KeyCode::Char('n') => KeyCode::Down,
                KeyCode::Char('p') => KeyCode::Up,
                KeyCode::Char('d' | 'f') => KeyCode::PageDown,
                KeyCode::Char('u' | 'b') => KeyCode::PageUp,
                _ => return,
            };
            key.modifiers = KeyModifiers::NONE;
        }
        if key.modifiers == KeyModifiers::ALT {
            match key.code {
                KeyCode::Char('h') if !self.state.help => {
                    self.history();
                    return;
                }
                KeyCode::Char('e')
                    if !self.state.help
                        && !self.state.details
                        && self.state.focus != Focus::Range
                        && self.state.screen != Screen::History =>
                {
                    self.state.search.exact = !self.state.search.exact;
                    self.state.search.selected = 0;
                    return;
                }
                _ if !editing => return,
                _ => {}
            }
        }
        if !editing
            && key.modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            )
        {
            return;
        }
        if key.modifiers == KeyModifiers::SHIFT
            && let KeyCode::Char(c) = key.code
        {
            key.code = KeyCode::Char(c.to_ascii_uppercase());
        }
        if !editing
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            key.code = match key.code {
                KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => KeyCode::Esc,
                KeyCode::Char('l') | KeyCode::Right if !self.state.details && !self.state.help => {
                    if self.state.screen == Screen::Episodes {
                        KeyCode::Char('i')
                    } else {
                        KeyCode::Enter
                    }
                }
                _ => key.code,
            };
        }
        if self.state.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => self.state.help = false,
                KeyCode::Char('q') => self.state.help = false,
                KeyCode::Char('/') => {
                    self.state.help_filtering = true;
                    self.state.help_query.cursor = self.state.help_query.text.len();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_add(10)
                }
                KeyCode::PageUp => {
                    self.state.view.help_scroll = self.state.view.help_scroll.saturating_sub(10)
                }
                KeyCode::Home | KeyCode::Char('g') => self.state.view.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.state.view.help_scroll = u16::MAX,
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::F(5) {
            self.retry();
            return;
        }
        if key.code == KeyCode::F(6) {
            self.retry_activity();
            return;
        }
        if !editing && matches!(key.code, KeyCode::Char('s' | 'D')) {
            self.state.mode.toggle();
            self.settings();
            self.state.message("", false);
            return;
        }
        if self.state.details {
            match key.code {
                KeyCode::Esc | KeyCode::Char('i') => self.state.details = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.state.view.details_scroll =
                        self.state.view.details_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.view.details_scroll =
                        self.state.view.details_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.state.view.details_scroll =
                        self.state.view.details_scroll.saturating_add(10)
                }
                KeyCode::PageUp => {
                    self.state.view.details_scroll =
                        self.state.view.details_scroll.saturating_sub(10)
                }
                KeyCode::Home | KeyCode::Char('g') => self.state.view.details_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.state.view.details_scroll = u16::MAX,
                KeyCode::Char('q') => self.state.details = false,
                KeyCode::Char('x') => self.stop(),
                KeyCode::Char('y') => self.copy_requested = true,
                _ => {}
            }
            return;
        }
        if self.state.focus == Focus::Range {
            match key.code {
                KeyCode::Esc => self.state.focus = Focus::List,
                KeyCode::Enter => {
                    match crate::model::episode_range(
                        &self.state.episodes.range.text,
                        &self.state.episodes.items,
                    ) {
                        Ok(indices) => {
                            self.state.episodes.marked = indices.into_iter().collect();
                            self.state.focus = Focus::List;
                            self.state.message("", false);
                        }
                        Err(err) => self.state.message(err.to_string(), true),
                    }
                }
                _ => {
                    self.state.episodes.range.edit(key);
                }
            }
            return;
        }
        if self.state.focus == Focus::Search && self.state.screen == Screen::History {
            match key.code {
                KeyCode::Enter | KeyCode::Down | KeyCode::Tab | KeyCode::Esc => {
                    self.state.focus = Focus::List
                }
                _ => {
                    if self.state.history.query.edit(key) {
                        self.state.history_filter_changed();
                    }
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
                _ => {
                    if self.state.search.query.edit(key) {
                        self.state.invalidate_search();
                        abort(&mut self.search_task);
                        self.search_due = Some(Instant::now() + Duration::from_millis(300));
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('y') if self.state.view.text_view.is_some() => self.copy_requested = true,
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('/') => {
                if !matches!(self.state.screen, Screen::Home | Screen::History) {
                    self.navigate(Screen::Search);
                }
                self.state.focus = Focus::Search;
            }
            KeyCode::Esc => self.back(),
            KeyCode::Char('H') => self.home(),
            KeyCode::Char('i') if self.state.selected_anime().is_some() => {
                self.state.message("", false);
                self.state.details = true;
                self.state.view.details_scroll = 0;
                self.state.view.text_view = None;
                self.state.view.text_selection = None;
                self.load_synopsis(false);
            }
            KeyCode::Delete if self.state.screen == Screen::History => {
                if let Some(entry) = self.state.selected_history() {
                    self.persist(crate::persistence::Change::Remove {
                        id: entry.anime.id.clone(),
                        mode: entry.mode,
                    });
                }
            }
            KeyCode::Char(':') if self.state.screen == Screen::Episodes => {
                self.state.focus = Focus::Range;
                self.state.episodes.range.set(String::new());
            }
            KeyCode::Char(' ') | KeyCode::Tab if self.state.screen == Screen::Episodes => {
                let i = self.state.episodes.selected;
                if !self.state.episodes.marked.remove(&i) && i < self.state.episodes.items.len() {
                    self.state.episodes.marked.insert(i);
                }
                self.state.message("", false);
            }
            KeyCode::Char('d') if self.state.screen == Screen::Episodes => {
                self.start_selection(true)
            }
            KeyCode::Char('Q') => {
                self.cycle_quality(false);
            }
            KeyCode::Enter if matches!(self.state.screen, Screen::Search | Screen::History) => {
                self.open()
            }
            KeyCode::Enter if self.state.screen == Screen::Episodes => self.start_selection(false),
            KeyCode::Char('x') => {
                let active = self.state.activity.as_ref().is_some_and(Activity::active);
                self.stop();
                self.state
                    .message(if active { "Queue stopped." } else { "" }, false);
            }
            KeyCode::Char('n') if self.state.screen == Screen::Episodes => self.navigate_play(
                self.state
                    .current_episode_index()
                    .unwrap_or(self.state.episodes.selected)
                    + 1,
            ),
            KeyCode::Char('p') if self.state.screen == Screen::Episodes => {
                if let Some(i) = self
                    .state
                    .current_episode_index()
                    .unwrap_or(self.state.episodes.selected)
                    .checked_sub(1)
                {
                    self.navigate_play(i);
                } else {
                    self.state.message("First episode.", false);
                }
            }
            KeyCode::Char('r') if self.state.screen == Screen::Episodes => self.navigate_play(
                self.state
                    .current_episode_index()
                    .unwrap_or(self.state.episodes.selected),
            ),
            KeyCode::Char('R') => self.retry(),
            code => {
                let len = match self.state.screen {
                    Screen::Home => return,
                    Screen::Search => self.state.visible().len(),
                    Screen::History => self.state.visible_history().len(),
                    Screen::Episodes => self.state.episodes.items.len(),
                };
                let selected = if self.state.screen == Screen::Search {
                    &mut self.state.search.selected
                } else if self.state.screen == Screen::History {
                    &mut self.state.history.selected
                } else {
                    &mut self.state.episodes.selected
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
        theme: crate::theme::ThemeSource,
    ) -> Result<()> {
        let result = self.run_loop(terminal, picker, signals, theme).await;
        self.stop();
        abort(&mut self.search_task);
        abort(&mut self.episode_task);
        abort(&mut self.synopsis_task);
        let jobs = self.jobs.shutdown().await;
        let saved = self.writer.shutdown().await;
        result?;
        jobs?;
        saved?;
        while let Ok(result) = self.library_rx.try_recv() {
            result?;
        }
        Ok(())
    }

    async fn run_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        picker: Picker,
        signals: [tokio::signal::unix::Signal; 3],
        theme: crate::theme::ThemeSource,
    ) -> Result<()> {
        self.state.palette = theme.load(self.state.colorless).await;
        let mut theme_tick = tokio::time::interval(Duration::from_secs(1));
        let mut art = Artwork::new(picker, self.options.cache.clone(), self.options.images);
        let (art_tx, mut art_rx) = mpsc::unbounded_channel();
        // Keep all terminal reads on this thread. EventStream races with the cursor
        // queries performed by Ratatui 0.30.2 during clear/autoresize.
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        let mut animation = Instant::now();
        let mut dirty = true;
        let mut art_revision = 0;
        let mut mouse_capture = false;
        let [mut interrupt, mut terminate, mut hangup] = signals;
        while !self.quit {
            dirty |= self.prepare_synopsis();
            if self.synopsis_due.is_some_and(|at| Instant::now() >= at) {
                self.load_synopsis(false);
                dirty = true;
            }
            if self.search_due.is_some_and(|at| Instant::now() >= at) {
                self.search();
                dirty = true;
            }
            while crossterm::event::poll(Duration::ZERO)? {
                let event = crossterm::event::read()?;
                if matches!(
                    event,
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Moved,
                        ..
                    })
                ) {
                    continue;
                }
                dirty = true;
                match event {
                    Event::Key(key) => self.key(key),
                    Event::Paste(text) => self.paste(&text),
                    Event::Mouse(event) => {
                        if let Some(text) = self.mouse(event) {
                            execute!(std::io::stdout(), CopyToClipboard::to_clipboard_from(text))?;
                        }
                    }
                    Event::Resize(_, _) => {
                        self.state.view.text_selection = None;
                        self.state.view.text_view = None;
                    }
                    _ => {}
                }
                if self.copy_requested {
                    break;
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
                    art.request(None, cover.as_size(), self.provider.http(), art_tx.clone());
                    art_revision = self.state.art_revision;
                }
                art.request(url, cover.as_size(), self.provider.http(), art_tx.clone());
                std::io::stdout().sync_update(|_| -> Result<()> {
                    if art.clear {
                        art.clear_graphics()?;
                        terminal.clear()?;
                    }
                    terminal.draw(|frame| ui::render(frame, &mut self.state, &art))?;
                    Ok(())
                })??;
                if std::mem::take(&mut self.copy_requested) {
                    self.copy_text();
                }
                let capture = self.state.help || self.state.view.text_view.is_some();
                if capture != mouse_capture {
                    if capture {
                        execute!(std::io::stdout(), EnableMouseCapture)?;
                    } else {
                        execute!(std::io::stdout(), DisableMouseCapture)?;
                    }
                    mouse_capture = capture;
                }
                dirty = false;
            }
            tokio::select! {
                _ = theme_tick.tick() => {
                    let palette = theme.load(self.state.colorless).await;
                    if palette != self.state.palette {
                        self.state.palette = palette;
                        dirty = true;
                    }
                }
                _ = tick.tick() => {
                    if self.state.animate && animation.elapsed() >= Duration::from_millis(120) {
                        animation = Instant::now();
                        self.state.frame = self.state.frame.wrapping_add(1);
                        dirty |= self.state.loading || self.state.activity.as_ref().is_some_and(Activity::active);
                        let size = terminal.size()?;
                        if ui::home_scene_visible(&self.state, ratatui::layout::Rect::new(0, 0, size.width, size.height))
                        {
                            dirty |= self.state.sakura.advance();
                        }
                    }
                }
                Some(event) = self.jobs.recv() => { self.job_message(event); dirty = true; },
                Some(result) = self.library_rx.recv() => { self.message(Message::Library(result)); dirty = true; },
                Some(message) = self.rx.recv() => { self.message(message); dirty = true; },
                Some(message) = art_rx.recv() => { art.accept(message); dirty = true; },
                _ = terminate.recv() => self.quit = true,
                _ = hangup.recv() => self.quit = true,
                _ = interrupt.recv() => self.quit = true,
            }
        }
        if mouse_capture {
            execute!(std::io::stdout(), DisableMouseCapture)?;
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        abort(&mut self.synopsis_task);
        abort(&mut self.search_task);
        abort(&mut self.episode_task);
        self.jobs.stop();
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
    fn test_app(state: State, dir: &std::path::Path) -> App {
        App::new(
            state,
            Provider::new("http://127.0.0.1:9").unwrap(),
            Options {
                player: "does-not-exist".into(),
                player_args: vec![],
                cache: dir.join("cache"),
                state_dir: dir.into(),
                images: false,
                download_dir: dir.join("downloads"),
            },
        )
    }

    #[tokio::test]
    async fn lowercase_q_does_not_quit_when_changing_quality() {
        let dir = tempfile::tempdir().unwrap();
        for screen in [Screen::Search, Screen::History, Screen::Episodes] {
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.screen = screen;
            state.focus = Focus::List;
            let mut app = test_app(state, dir.path());
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
            assert!(!app.quit);
            assert_eq!(app.state.quality, "best");
            app.key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE));
            assert!(!app.quit);
            assert_ne!(app.state.quality, "best");
            app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
            assert!(app.quit);
        }
    }

    #[tokio::test]
    async fn petals_persist_through_typing_paste_and_search_return() {
        use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
        let render = |state: &State| {
            let area = Rect::new(0, 0, 133, 32);
            let mut buf = Buffer::empty(area);
            crate::sakura::Sakura {
                palette: state.palette,
                protected: Rect::new(32, 6, 68, 17),
                motion: &state.sakura,
                animate: true,
                ascii: false,
            }
            .render(area, &mut buf);
            buf
        };
        let visible = |buf: &Buffer| buf.content.iter().any(|c| c.symbol() == "●");
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(
            State::new(String::new(), Mode::Sub, "best".into(), false, false, true),
            dir.path(),
        );
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(visible(&render(&app.state)));
        for _ in 0..20 {
            app.key(key(KeyCode::Char('a')));
            app.paste("b");
            app.state.sakura.advance();
            assert!(
                visible(&render(&app.state)),
                "typing must not fade out petals"
            );
        }
        let before = render(&app.state);
        app.key(key(KeyCode::Enter));
        assert_eq!(app.state.screen, Screen::Search);
        app.key(key(KeyCode::Esc));
        assert_eq!(app.state.screen, Screen::Home);
        assert_eq!(
            before,
            render(&app.state),
            "return must preserve the visible animation phase"
        );
        for _ in 0..20 {
            app.state.sakura.advance();
            assert!(
                visible(&render(&app.state)),
                "returning Home must not restart a fade"
            );
        }
        assert_ne!(before, render(&app.state));
    }

    #[tokio::test]
    async fn miru_home_submits_explicitly_and_settings_never_edit_the_query() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = test_app(
            State::new(String::new(), Mode::Sub, "best".into(), false, false, false),
            dir.path(),
        );
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.key(key(KeyCode::Enter));
        assert_eq!(app.state.screen, Screen::Home);
        for c in "sQh".chars() {
            app.key(key(KeyCode::Char(c)));
        }
        app.paste(" 日本語");
        assert_eq!(app.state.search.query.text, "sQh 日本語");
        assert!(app.search_due.is_none() && app.search_task.is_none());
        app.key(key(KeyCode::Tab));
        assert_eq!(app.state.focus, Focus::Audio);
        app.key(key(KeyCode::Right));
        assert_eq!(app.state.mode, Mode::Dub);
        app.paste("ignored");
        app.key(key(KeyCode::Tab));
        assert_eq!(app.state.focus, Focus::Quality);
        app.key(key(KeyCode::Left));
        assert_eq!(app.state.quality, "worst");
        app.key(key(KeyCode::Right));
        assert_eq!(app.state.quality, "best");
        app.key(key(KeyCode::Esc));
        assert_eq!(app.state.focus, Focus::List);
        app.key(key(KeyCode::Tab));
        app.key(key(KeyCode::Tab));
        app.key(key(KeyCode::Tab));
        app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.state.focus, Focus::Audio);
        app.key(key(KeyCode::Enter));
        assert_eq!(app.state.focus, Focus::Search);
        assert_eq!(app.state.search.query.text, "sQh 日本語");
        app.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(app.state.screen, Screen::History);
        app.key(key(KeyCode::Esc));
        assert_eq!(app.state.screen, Screen::Home);
        assert_eq!(app.state.focus, Focus::Search);
        app.key(key(KeyCode::Enter));
        assert_eq!(app.state.screen, Screen::Search);
        assert_eq!(app.state.focus, Focus::List);
        assert!(app.search_task.is_some());
        app.key(key(KeyCode::Esc));
        assert_eq!(app.state.screen, Screen::Home);
        assert_eq!(app.state.search.query.text, "sQh 日本語");
        assert_eq!(app.state.focus, Focus::Search);
        assert!(app.search_task.is_none());
        app.key(key(KeyCode::Esc));
        app.key(key(KeyCode::Char('/')));
        assert_eq!(app.state.screen, Screen::Home);
        assert_eq!(app.state.focus, Focus::Search);
    }

    #[tokio::test]
    async fn help_filter_isolates_input_and_preserves_the_underlying_screen() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(
            "anime query".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        let mut app = test_app(state, dir.path());
        let original_focus = app.state.focus;
        app.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "qhsD".chars() {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.paste(" downloads");
        assert_eq!(app.state.help_query.text, "qhsD downloads");
        assert_eq!(app.state.search.query.text, "anime query");
        assert_eq!(app.state.mode, Mode::Sub);
        assert_eq!(app.state.focus, original_focus);
        assert!(!app.quit && app.state.help);
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(app.state.help_query.text.is_empty());
        app.paste("quality");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.state.help && !app.state.help_filtering);
        assert_eq!(app.state.help_query.text, "quality");
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.state.view.help_scroll, 1);
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.state.help && !app.quit);
        app.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.state.help_query.text.is_empty());
        assert_eq!(app.state.view.help_scroll, 0);
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(!app.state.help);
    }

    #[tokio::test]
    async fn history_continuations_play_the_highlighted_episode_without_old_marks() {
        for screen in [Screen::History] {
            for (last, expected) in [("1", "2"), ("3", "3"), ("9", "1")] {
                let dir = tempfile::tempdir().unwrap();
                let mut state =
                    State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
                state.screen = screen;
                state.focus = Focus::List;
                state.episodes.anime = Some(anime("series", "A series"));
                {
                    let mut entries = state.history.entries().to_vec();
                    entries.push(crate::persistence::Entry {
                        anime: state.episodes.anime.clone().unwrap(),
                        episode: last.into(),
                        mode: Mode::Dub,
                        watched_at: 1,
                    });
                    state.history.replace_entries(entries);
                }
                state.episodes.items = (1..=3)
                    .map(|n| Episode {
                        id: n.to_string(),
                        number: n.to_string(),
                        title: String::new(),
                    })
                    .collect();
                state.episodes.marked = [0, 1].into();
                let mut app = test_app(state, dir.path());
                let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
                app.key(enter);
                assert!(app.state.episodes.marked.is_empty());
                if last == "9" {
                    assert!(app.state.status.contains("unavailable"));
                    assert!(!app.state.status.contains("replays"));
                }
                assert_eq!(
                    app.state.episodes.items[app.state.episodes.selected].number,
                    expected
                );
                app.key(enter);
                let activity = app.state.activity.as_ref().unwrap();
                assert_eq!(activity.episodes[activity.index].number, expected);
                assert!(activity.remaining.is_empty());
                assert_eq!(activity.mode, Mode::Dub);
            }
        }
    }

    #[tokio::test]
    async fn synopsis_mouse_selection_clamps_to_prose_and_copies_no_surrounding_ui() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.details = true;
        state.focus = Focus::List;
        state.view.text_view = Some(TextView {
            area: Rect::new(10, 5, 12, 3),
            offset: 0,
            lines: vec!["First line".into(), "".into(), "Second line".into()],
        });
        let mut app = test_app(state, dir.path());
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 5));
        assert!(app.state.view.text_selection.is_none());
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, 5));
        app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 60, 20));
        let copied = app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 60, 20));
        assert_eq!(copied.as_deref(), Some("First line\n\nSecond line"));
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 21, 7));
        let copied = app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));
        assert_eq!(copied.as_deref(), Some("First line\n\nSecond line"));
        app.state.help = true;
        assert!(
            app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 20, 7))
                .is_none()
        );
        app.state.help = false;
        app.state.view.text_view = Some(TextView {
            area: Rect::new(10, 5, 12, 1),
            offset: 0,
            lines: vec!["A日e\u{301} B".into()],
        });
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 11, 5));
        let copied = app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 14, 5));
        assert_eq!(copied.as_deref(), Some("日e\u{301}"));
        app.state.details = false;
        app.mouse(mouse(MouseEventKind::ScrollDown, 11, 5));
        assert_eq!(app.state.view.preview_scroll, 3);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.episodes.items = (1..=3)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        assert_eq!(app.state.episodes.selected, 1);
    }

    #[tokio::test]
    async fn preview_loading_tracks_selection_and_reuses_full_synopsis_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(
            "series".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.accept_search(
            0,
            Ok(vec![
                anime("first", "First series"),
                anime("second", "Second series"),
            ]),
        );
        let mut app = test_app(state, dir.path());
        assert!(app.prepare_synopsis());
        assert!(app.synopsis_due.is_some());
        assert!(matches!(
            app.state.synopses.peek("first"),
            Some(Synopsis::Loading)
        ));
        assert!(!app.prepare_synopsis());
        app.state.search.selected = 1;
        assert!(app.prepare_synopsis());
        assert!(app.state.synopses.peek("first").is_none());
        assert!(matches!(
            app.state.synopses.peek("second"),
            Some(Synopsis::Loading)
        ));
        app.state
            .synopses
            .put("second".into(), Synopsis::Ready("Full synopsis".into()));
        app.state.search.selected = 0;
        app.prepare_synopsis();
        app.state.search.selected = 1;
        app.prepare_synopsis();
        assert!(app.synopsis_due.is_none());
        assert!(
            matches!(app.state.synopses.peek("second"), Some(Synopsis::Ready(text)) if text == "Full synopsis")
        );
    }

    #[tokio::test]
    async fn paste_targets_only_the_visible_input_and_preserves_range_validation() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new("fox".into(), Mode::Sub, "best".into(), false, false, false);
        let mut app = test_app(state, dir.path());
        let generation = app.state.search_generation;
        for (help, details, focus) in [
            (true, false, Focus::Search),
            (false, true, Focus::Search),
            (false, false, Focus::List),
        ] {
            app.state.help = help;
            app.state.details = details;
            app.state.focus = focus;
            app.paste("xyz");
            assert_eq!(app.state.search.query.text, "fox");
            assert_eq!(app.state.search_generation, generation);
        }
        app.state.help = false;
        app.state.details = false;
        app.state.focus = Focus::Search;
        app.paste(" 日本\n語");
        assert_eq!(app.state.search.query.text, "fox 日本 語");
        assert!(app.state.search_generation > generation);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.focus = Focus::Range;
        app.state.episodes.items = (1..=5)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.paste("1-3,\n5\t");
        assert_eq!(app.state.episodes.range.text, "1-3, 5 ");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.state
                .episodes
                .marked
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        assert!(app.state.activity.is_none());
        app.state.focus = Focus::Range;
        app.state.episodes.range.set(String::new());
        app.state.episodes.items = (1..=13)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.paste("1\n3");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.state
                .episodes
                .marked
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [0, 2]
        );
    }

    #[tokio::test]
    async fn download_fallback_remains_available_after_progress_and_completion() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(
            String::new(),
            Mode::Sub,
            "1080p".into(),
            false,
            false,
            false,
        );
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(anime("series", "Series"));
        state.episodes.items = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "First".into(),
        }];
        let mut app = test_app(state, dir.path());
        app.start_selection(true);
        app.jobs.stop();
        let job = app.job;
        app.job_message(jobs::Event {
            id: job,
            update: jobs::Update::Resolved("720p (1080p unavailable)".into()),
        });
        assert_eq!(
            app.state.activity.as_ref().unwrap().quality_note,
            "720p (1080p unavailable)"
        );
        app.job_message(jobs::Event {
            id: job,
            update: jobs::Update::Progress("Downloaded through 00:00:01".into()),
        });
        assert!(
            app.state
                .activity
                .as_ref()
                .unwrap()
                .quality_note
                .contains("1080p unavailable")
        );
        app.job_message(jobs::Event {
            id: job,
            update: jobs::Update::Finished(Ok(jobs::Output::Download(
                dir.path().join("episode.mkv"),
            ))),
        });
        let activity = app.state.activity.as_ref().unwrap();
        assert_eq!(activity.phase, JobPhase::Finished);
        assert_eq!(activity.quality_note, "720p (1080p unavailable)");
        assert!(activity.message.contains("Saved"));
    }

    #[tokio::test]
    async fn history_filter_stays_local_and_opens_the_filtered_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(
            "original search".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.screen = Screen::History;
        state.focus = Focus::List;
        state.history.replace_entries(
            ["Frieren", "Cowboy Bebop", "Cowboy Bebop Movie"]
                .iter()
                .enumerate()
                .map(|(i, title)| crate::persistence::Entry {
                    anime: anime(&i.to_string(), title),
                    episode: "4".into(),
                    mode: Mode::Sub,
                    watched_at: i as u64,
                })
                .collect(),
        );
        let mut app = test_app(state, dir.path());
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.paste("BEBOP");
        assert_eq!(app.state.screen, Screen::History);
        assert_eq!(app.state.search.query.text, "original search");
        assert!(app.search_due.is_none() && app.search_task.is_none());
        assert_eq!(app.state.visible_history().as_ref(), [1, 2]);
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.state.selected_history().unwrap().anime.id, "2");
        let mut entries = app.state.history.entries().to_vec();
        entries.remove(2);
        app.message(Message::Library(Ok(crate::persistence::Library {
            entries,
            ..Default::default()
        })));
        assert_eq!(app.state.selected_history().unwrap().anime.id, "1");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.state.screen, Screen::Episodes);
        assert_eq!(app.state.episodes.anime.as_ref().unwrap().id, "1");
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.state.screen, Screen::History);
        assert_eq!(app.state.history.query.text, "BEBOP");
    }

    #[tokio::test]
    async fn history_deletion_keeps_the_nearest_row_and_updates_preserve_identity() {
        for removed in [0, 2, 3] {
            let dir = tempfile::tempdir().unwrap();
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.history.replace_entries(
                (0..4)
                    .map(|n| crate::persistence::Entry {
                        anime: anime(&n.to_string(), &format!("Series {n}")),
                        episode: "1".into(),
                        mode: Mode::Sub,
                        watched_at: n,
                    })
                    .collect(),
            );
            state.history.selected = removed;
            let mut entries = state.history.entries().to_vec();
            entries.remove(removed);
            let mut app = test_app(state, dir.path());
            app.message(Message::Library(Ok(crate::persistence::Library {
                entries: entries.clone(),
                ..Default::default()
            })));
            assert_eq!(app.state.history.selected, removed.min(2));
            let selected = app.state.history.entries()[app.state.history.selected]
                .anime
                .id
                .clone();
            entries.reverse();
            app.message(Message::Library(Ok(crate::persistence::Library {
                entries,
                ..Default::default()
            })));
            assert_eq!(
                app.state.history.entries()[app.state.history.selected]
                    .anime
                    .id,
                selected
            );
            app.message(Message::Library(Ok(crate::persistence::Library::default())));
            assert_eq!(app.state.history.selected, 0);
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
        assert_eq!(state.visible().as_ref(), [0, 1]);
        state.search.selected = 1;
        state.accept_search(0, Ok(results));
        assert_eq!(state.search.selected, 1);
        state.search.exact = true;
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
        assert!(app.state.episodes.items.is_empty());
        let old_job = app.job;
        app.stop();
        app.job_message(jobs::Event {
            id: old_job,
            update: jobs::Update::Resolved("stale".into()),
        });
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
        state.focus = Focus::List;
        state.focus = Focus::Range;
        state.episodes.items = ["1", "1.5", "2", "3"]
            .iter()
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        state.episodes.range.set("1-2".into());
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
            app.state
                .episodes
                .marked
                .iter()
                .copied()
                .collect::<Vec<_>>(),
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
        app.state.episodes.selected = 1;
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.state.search.query.set("other".into());
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
        assert_eq!(app.state.search.query.text, "other");
        app.key(back);
        assert_eq!(app.state.screen, Screen::Episodes);
        assert_eq!(app.state.episodes.anime.as_ref().unwrap().id, "first-1");
        assert_eq!(
            app.state.episodes.items[app.state.episodes.selected].id,
            "12"
        );
        assert!(app.state.episodes.marked.contains(&1));
        app.key(back);
        assert_eq!(app.state.screen, Screen::Search);
        assert_eq!(app.state.search.query.text, "first");
        assert_eq!(app.state.selected_anime().unwrap().id, "first-1");
    }

    #[test]
    fn stale_search_cannot_overwrite_new_query_or_empty_state() {
        let mut state = State::new("old".into(), Mode::Sub, "best".into(), false, false, false);
        let old = state.search_generation;
        state.search.query.set("new".into());
        state.invalidate_search();
        assert!(!state.accept_search(
            old,
            Ok(vec![Anime {
                title: "old".into(),
                ..Default::default()
            }])
        ));
        assert!(state.search.results().is_empty());
        assert!(state.accept_search(
            state.search_generation,
            Ok(vec![Anime {
                title: "new".into(),
                ..Default::default()
            }])
        ));
        state.search.query.set(String::new());
        state.invalidate_search();
        assert!(!state.accept_search(old + 1, Ok(vec![Anime::default()])));
        assert!(state.search.results().is_empty());
    }
    #[tokio::test]
    async fn episode_forward_navigation_opens_details_without_starting_playback() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(anime("series", "Series"));
        state.episodes.items = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "Opening".into(),
        }];
        let mut app = test_app(state, dir.path());
        for code in [KeyCode::Right, KeyCode::Char('l')] {
            app.state.details = false;
            app.key(KeyEvent::new(code, KeyModifiers::NONE));
            assert!(
                app.state.activity.is_none(),
                "{code:?} must not start media"
            );
            assert!(app.state.details);
        }
    }

    #[tokio::test]
    async fn language_shortcut_accepts_uppercase_and_explicit_shift_events() {
        for key in [
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.screen = Screen::Episodes;
            state.focus = Focus::List;
            let mut app = test_app(state, dir.path());
            app.key(key);
            assert_eq!(app.state.mode, Mode::Dub, "{key:?}");
            assert!(app.state.activity.is_none());
            app.state.details = true;
            app.key(key);
            assert_eq!(app.state.mode, Mode::Sub, "{key:?}");
            assert!(app.state.status.is_empty());
        }
    }

    #[test]
    fn readline_editing_moves_cuts_and_yanks_unicode_without_splitting_it() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let alt = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let mut input = Input::default();
        input.set("one 日本 e\u{301}clair!".into());
        assert!(!input.edit(ctrl('a')));
        assert_eq!(input.cursor, 0);
        input.edit(alt('f'));
        assert_eq!(input.cursor, 3);
        input.edit(alt('f'));
        assert_eq!(&input.text[..input.cursor], "one 日本");
        assert!(input.edit(ctrl('u')));
        assert_eq!(input.text, " e\u{301}clair!");
        assert!(input.edit(ctrl('y')));
        assert_eq!(input.text, "one 日本 e\u{301}clair!");
        input.edit(ctrl('k'));
        assert_eq!(input.text, "one 日本");
        input.edit(ctrl('y'));
        input.edit(alt('b'));
        assert_eq!(&input.text[..input.cursor], "one 日本 ");
        input.edit(ctrl('f'));
        assert_eq!(&input.text[..input.cursor], "one 日本 e\u{301}");
        input.edit(ctrl('b'));
        input.edit(ctrl('d'));
        assert_eq!(input.text, "one 日本 clair!");
        input.edit(ctrl('e'));
        input.edit(ctrl('w'));
        assert_eq!(input.text, "one 日本 ");
        input.edit(ctrl('y'));
        input.edit(alt('b'));
        input.edit(alt('d'));
        assert_eq!(input.text, "one 日本 !");
        input.edit(ctrl('y'));
        input.edit(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(input.text, "one 日本 !");
        input.edit(ctrl('h'));
        assert_eq!(input.text, "one 日本!");
        assert!(!input.edit(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )));
        assert_eq!(input.text, "one 日本!");
        input.edit(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(input.cursor, 4);
        input.edit(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(&input.text[..input.cursor], "one 日本");
    }

    #[tokio::test]
    async fn vim_navigation_and_readline_keys_do_not_trigger_media_actions() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        let mut app = test_app(state, dir.path());
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        app.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(app.state.screen, Screen::History);
        app.key(key('h'));
        assert_eq!(app.state.screen, Screen::Home);
        app.history();
        app.key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT));
        assert_eq!(app.state.screen, Screen::Home);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.episodes.items = (1..=30)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.key(ctrl('d'));
        assert_eq!(app.state.episodes.selected, 10);
        app.key(ctrl('n'));
        assert_eq!(app.state.episodes.selected, 11);
        app.key(ctrl('p'));
        assert_eq!(app.state.episodes.selected, 10);
        app.key(ctrl('u'));
        assert_eq!(app.state.episodes.selected, 0);
        app.key(ctrl('r'));
        for code in [KeyCode::Enter, KeyCode::Char('D'), KeyCode::Char('d')] {
            app.key(KeyEvent::new(code, KeyModifiers::ALT | KeyModifiers::SHIFT));
        }
        assert_eq!(app.state.mode, Mode::Sub);
        assert!(app.state.activity.is_none());
        app.state.details = true;
        app.key(key('G'));
        assert_eq!(app.state.view.details_scroll, u16::MAX);
        app.key(key('g'));
        assert_eq!(app.state.view.details_scroll, 0);
        app.key(key('q'));
        assert!(!app.state.details && !app.quit);
        app.state.help = true;
        app.key(key('q'));
        assert!(!app.state.help && !app.quit);
        app.key(key('/'));
        app.state.search.query.set("hello world".into());
        app.key(ctrl('a'));
        app.key(ctrl('e'));
        assert_eq!(app.state.search.query.cursor, 11);
        assert!(!app.state.search.exact);
        app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT));
        assert!(app.state.search.exact);
        app.key(ctrl('g'));
        assert_eq!(app.state.focus, Focus::List);
        app.state.accept_search(
            app.state.search_generation,
            Ok(vec![anime("series", "hello world")]),
        );
        app.key(key('l'));
        assert_eq!(app.state.screen, Screen::Episodes);
        app.key(key('h'));
        assert_eq!(app.state.screen, Screen::Search);
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.state.screen, Screen::Episodes);
        app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.state.screen, Screen::Search);
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
