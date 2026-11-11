use crate::{
    artwork::Artwork,
    model::{Anime, Episode, Mode, Stream, normalized},
    player,
    provider::Provider,
    ui,
};
use anyhow::Result;
use crossterm::SynchronizedUpdate;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::{clipboard::CopyToClipboard, execute};
use lru::LruCache;
use ratatui::layout::{Position, Rect};
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
    Audio,
    Quality,
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
    pub quality_note: String,
    pub kind: JobKind,
    pub phase: JobPhase,
    pub message: String,
}

impl Activity {
    pub fn active(&self) -> bool {
        matches!(self.phase, JobPhase::Resolving | JobPhase::Running)
    }

    pub fn retryable(&self) -> bool {
        self.phase == JobPhase::Failed
    }
}

#[derive(Clone, Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
    killed: String,
}
impl Input {
    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }
    fn previous_character(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_character(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }

    fn previous_word(&self, whitespace_only: bool) -> usize {
        let mut start = self.cursor;
        let mut in_word = false;
        for (i, g) in self.text[..self.cursor].grapheme_indices(true).rev() {
            let word = if whitespace_only {
                !g.chars().all(char::is_whitespace)
            } else {
                g.chars().any(char::is_alphanumeric)
            };
            if in_word && !word {
                break;
            }
            in_word |= word;
            start = i;
        }
        start
    }

    fn next_word(&self) -> usize {
        let mut end = self.cursor;
        let mut in_word = false;
        for (i, g) in self.text[self.cursor..].grapheme_indices(true) {
            let word = g.chars().any(char::is_alphanumeric);
            if in_word && !word {
                break;
            }
            in_word |= word;
            end = self.cursor + i + g.len();
        }
        end
    }

    fn kill(&mut self, start: usize, end: usize) -> bool {
        if start == end {
            return false;
        }
        self.killed = self.text[start..end].to_owned();
        self.text.replace_range(start..end, "");
        self.cursor = start;
        true
    }

    pub fn edit(&mut self, mut key: KeyEvent) -> bool {
        if ![
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ]
        .contains(&key.modifiers)
        {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Left => {
                    self.cursor = self.previous_word(false);
                    return false;
                }
                KeyCode::Right => {
                    self.cursor = self.next_word();
                    return false;
                }
                KeyCode::Char('a') => {
                    self.cursor = 0;
                    return false;
                }
                KeyCode::Char('e') => {
                    self.cursor = self.text.len();
                    return false;
                }
                KeyCode::Char('b') => {
                    self.cursor = self.previous_character();
                    return false;
                }
                KeyCode::Char('f') => {
                    self.cursor = self.next_character();
                    return false;
                }
                KeyCode::Char('u') => return self.kill(0, self.cursor),
                KeyCode::Char('k') => return self.kill(self.cursor, self.text.len()),
                KeyCode::Char('w') => return self.kill(self.previous_word(true), self.cursor),
                KeyCode::Char('y') => {
                    if self.killed.is_empty() || self.text.len() + self.killed.len() > 512 {
                        return false;
                    }
                    self.text.insert_str(self.cursor, &self.killed);
                    self.cursor += self.killed.len();
                    return true;
                }
                KeyCode::Char('h') => key.code = KeyCode::Backspace,
                KeyCode::Char('d') => key.code = KeyCode::Delete,
                _ => return false,
            }
        } else if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('b') => {
                    self.cursor = self.previous_word(false);
                    return false;
                }
                KeyCode::Char('f') => {
                    self.cursor = self.next_word();
                    return false;
                }
                KeyCode::Char('d') => return self.kill(self.cursor, self.next_word()),
                KeyCode::Backspace => return self.kill(self.previous_word(false), self.cursor),
                _ => return false,
            }
        }
        match key.code {
            KeyCode::Char(c) if !c.is_control() => {
                if self.text.len() + c.len_utf8() > 512 {
                    return false;
                }
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let start = self.previous_character();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text
                    .replace_range(self.cursor..self.next_character(), "");
            }
            KeyCode::Left => {
                self.cursor = self.previous_character();
                return false;
            }
            KeyCode::Right => {
                self.cursor = self.next_character();
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
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct TextSelection {
    pub anchor: TextPosition,
    pub end: TextPosition,
    pub dragging: bool,
}

impl TextSelection {
    pub fn range(&self) -> (TextPosition, TextPosition) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }
}

pub struct TextView {
    pub area: Rect,
    pub offset: usize,
    pub lines: Vec<String>,
}

impl TextView {
    pub fn position(&self, column: u16, row: u16) -> TextPosition {
        use unicode_width::UnicodeWidthStr;
        let line =
            self.offset + usize::from(row.clamp(self.area.y, self.area.bottom() - 1) - self.area.y);
        let requested =
            usize::from(column.saturating_sub(self.area.x)).min(usize::from(self.area.width));
        let mut position = 0;
        for grapheme in self.lines[line].graphemes(true) {
            if position + grapheme.width() > requested {
                break;
            }
            position += grapheme.width();
        }
        TextPosition {
            line,
            column: position,
        }
    }

    pub fn selected_text(&self, selection: TextSelection) -> String {
        use unicode_width::UnicodeWidthStr;
        let (start, end) = selection.range();
        let mut selected = Vec::new();
        for (i, line) in self
            .lines
            .iter()
            .enumerate()
            .take(end.line + 1)
            .skip(start.line)
        {
            let left = if i == start.line { start.column } else { 0 };
            let right = if i == end.line {
                end.column
            } else {
                line.width()
            };
            let mut column = 0;
            let text: String = line
                .graphemes(true)
                .filter(|grapheme| {
                    let matches = column < right && column + grapheme.width() > left;
                    column += grapheme.width();
                    matches
                })
                .collect();
            selected.push(text);
        }
        selected.join("\n").trim_end().to_owned()
    }
}

pub struct State {
    pub screen: Screen,
    pub focus: Focus,
    pub query: Input,
    pub results: Vec<Anime>,
    pub results_query: String,
    pub history: Vec<crate::persistence::Entry>,
    pub history_selected: usize,
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
    pub preview_scroll: u16,
    pub text_view: Option<TextView>,
    pub text_selection: Option<TextSelection>,
    pub help: bool,
    pub help_scroll: u16,
    pub help_query: Input,
    pub help_filtering: bool,
    pub frame: usize,
    pub sakura: crate::sakura::Motion,
    pub ascii: bool,
    pub colorless: bool,
    pub palette: crate::palette::Palette,
    pub synopses: LruCache<String, Synopsis>,
    pub animate: bool,
    pub activity: Option<Activity>,
}

impl State {
    pub fn request_blocks_list(&self) -> bool {
        self.request_error.is_some()
            && (self.screen != Screen::Episodes || self.episodes.is_empty())
    }

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
            focus: Focus::Search,
            query: input,
            results: vec![],
            results_query: String::new(),
            history: vec![],
            history_selected: 0,
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
            preview_scroll: 0,
            text_view: None,
            text_selection: None,
            help: false,
            help_scroll: 0,
            help_query: Input::default(),
            help_filtering: false,
            frame: 0,
            sakura: crate::sakura::Motion::default(),
            ascii,
            colorless: false,
            palette: crate::palette::Palette::terminal(false),
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
            Screen::Home => None,
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

    pub fn invalidate_search(&mut self) {
        self.search_generation += 1;
        self.selected = 0;
        self.list_offsets[Screen::Search as usize] = 0;
        self.loading = !self.query.text.trim().is_empty();
        self.error = false;
        self.request_error = None;
        self.status.clear();
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
                self.status.clear();
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
    synopsis_target: Option<String>,
    synopsis_due: Option<Instant>,
    play_task: Option<JoinHandle<()>>,
    search_due: Option<Instant>,
    job: u64,
    quit: bool,
    copy_requested: bool,
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
            synopsis_target: None,
            synopsis_due: None,
            play_task: None,
            search_due,
            job: 0,
            quit: false,
            copy_requested: false,
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
        self.state.message("", false);
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
            Screen::History => self.state.history.get(self.state.history_selected),
            _ => None,
        }
        .cloned();
        if let Some(entry) = &entry {
            self.state.mode = entry.mode;
        }
        let same_series = self.state.anime.as_ref().is_some_and(|a| a.id == anime.id);
        if entry.is_some() || !same_series {
            self.state.marked.clear();
        }
        if !same_series {
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
                self.state.message("", false);
            } else if let Some(i) = self
                .state
                .episodes
                .iter()
                .position(|e| e.number == entry.episode)
            {
                self.state.episode_selected = i;
                self.state.message(
                    "No newer episode. Enter replays the last played episode.",
                    false,
                );
            } else {
                self.state.episode_selected = 0;
                self.state.message(
                    "Last played episode is unavailable. Select an episode.",
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
        abort(&mut self.play_task);
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
        if index >= self.state.episodes.len() {
            self.state.message("No more episodes.", false);
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
        activity.message.clear();
        activity.quality_note.clear();
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
                        self.state.request_error = Some(format!("{err:#}"));
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
                let (variant, fallback) = stream.select(&quality);
                activity.quality_note = if fallback {
                    format!("{}p ({quality} unavailable)", variant.height)
                } else {
                    format!("{}p", variant.height)
                };
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
                    activity.message.clear();
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
                        activity.message.clear();
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
                    let previous = self.state.history_selected;
                    let key = self
                        .state
                        .history
                        .get(previous)
                        .map(|e| (e.anime.id.clone(), e.mode));
                    self.state.history = library.entries;
                    self.state.history_selected = key
                        .and_then(|(id, mode)| {
                            self.state
                                .history
                                .iter()
                                .position(|e| e.anime.id == id && e.mode == mode)
                        })
                        .unwrap_or_else(|| {
                            previous.min(self.state.history.len().saturating_sub(1))
                        });
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

    fn copy_text(&self) {
        if let Some(view) = &self.state.text_view {
            let text = self
                .state
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
                    self.state.help_scroll = self.state.help_scroll.saturating_add(3)
                }
                MouseEventKind::ScrollUp => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(3)
                }
                _ => {}
            }
            return None;
        }
        if !self.state.details && self.state.text_view.is_none() {
            return None;
        }
        match event.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = event.kind == MouseEventKind::ScrollDown;
                let over_text =
                    self.state.text_view.as_ref().is_some_and(|view| {
                        view.area.contains(Position::new(event.column, event.row))
                    });
                if self.state.details || over_text {
                    let scroll = if self.state.details {
                        &mut self.state.details_scroll
                    } else {
                        &mut self.state.preview_scroll
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
                self.state.text_selection = self
                    .state
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
                if let Some(selection) = &mut self.state.text_selection
                    && selection.dragging
                    && let Some(view) = &self.state.text_view
                {
                    selection.end = view.position(event.column, event.row);
                    if event.kind == MouseEventKind::Up(MouseButton::Left) {
                        selection.dragging = false;
                        let text = view.selected_text(*selection);
                        if !text.is_empty() {
                            return Some(text);
                        }
                        self.state.text_selection = None;
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
            &mut self.state.range
        } else {
            &mut self.state.query
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
                self.state.help_scroll = 0;
            }
            return;
        }
        if changed && self.state.focus == Focus::Search && self.state.screen == Screen::Search {
            self.state.invalidate_search();
            abort(&mut self.search_task);
            self.search_due = Some(Instant::now() + Duration::from_millis(300));
        }
    }

    fn open_help(&mut self) {
        self.state.help = true;
        self.state.help_scroll = 0;
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
            self.state.exact = !self.state.exact;
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
                if !self.state.query.text.trim().is_empty() {
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
                self.state.query.edit(key);
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
                        self.state.message(
                            if self.state.activity.as_ref().is_some_and(Activity::active) {
                                "Audio applies to the next queue."
                            } else {
                                ""
                            },
                            false,
                        );
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
                    self.state.help_scroll = self
                        .state
                        .help_scroll
                        .saturating_add(if key.code == KeyCode::Down { 1 } else { 10 });
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.state.help_filtering = false;
                    self.state.help_scroll = self
                        .state
                        .help_scroll
                        .saturating_sub(if key.code == KeyCode::Up { 1 } else { 10 });
                }
                _ => {
                    if self.state.help_query.edit(key) {
                        self.state.help_scroll = 0;
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
                        && self.state.focus != Focus::Range =>
                {
                    self.state.exact = !self.state.exact;
                    self.state.selected = 0;
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
                    self.state.help_scroll = self.state.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.state.help_scroll = self.state.help_scroll.saturating_add(10)
                }
                KeyCode::PageUp => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(10)
                }
                KeyCode::Home | KeyCode::Char('g') => self.state.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.state.help_scroll = u16::MAX,
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
            self.state.message(
                format!(
                    "Audio: {}{}",
                    self.state.mode.as_str().to_uppercase(),
                    if self.state.activity.as_ref().is_some_and(Activity::active) {
                        " / applies to the next queue"
                    } else {
                        ""
                    }
                ),
                false,
            );
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
                KeyCode::Home | KeyCode::Char('g') => self.state.details_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.state.details_scroll = u16::MAX,
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
                    match crate::model::episode_range(&self.state.range.text, &self.state.episodes)
                    {
                        Ok(indices) => {
                            self.state.marked = indices.into_iter().collect();
                            self.state.focus = Focus::List;
                            self.state.message("", false);
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
            KeyCode::Char('y') if self.state.text_view.is_some() => self.copy_requested = true,
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('/') => {
                if self.state.screen != Screen::Home {
                    self.navigate(Screen::Search);
                }
                self.state.focus = Focus::Search;
            }
            KeyCode::Esc => self.back(),
            KeyCode::Char('H') => self.home(),
            KeyCode::Char('i') if self.state.selected_anime().is_some() => {
                self.state.message("", false);
                self.state.details = true;
                self.state.details_scroll = 0;
                self.state.text_view = None;
                self.state.text_selection = None;
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
                    self.state.message("First episode.", false);
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
                    Screen::Home => return,
                    Screen::Search => self.state.visible().len(),
                    Screen::History => self.state.history.len(),
                    Screen::Episodes => self.state.episodes.len(),
                };
                let selected = if self.state.screen == Screen::Search {
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
                        self.state.text_selection = None;
                        self.state.text_view = None;
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
                if std::mem::take(&mut self.copy_requested) {
                    self.copy_text();
                }
                let capture = self.state.help || self.state.text_view.is_some();
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
                    if self.state.animate && animation.elapsed() >= Duration::from_millis(crate::scene::FRAME_INTERVAL_MS) {
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
    async fn mado_home_submits_explicitly_and_settings_never_edit_the_query() {
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
        assert_eq!(app.state.query.text, "sQh 日本語");
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
        assert_eq!(app.state.query.text, "sQh 日本語");
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
        assert_eq!(app.state.query.text, "sQh 日本語");
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
        assert_eq!(app.state.query.text, "anime query");
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
        assert_eq!(app.state.help_scroll, 1);
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.state.help && !app.quit);
        app.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.state.help_query.text.is_empty());
        assert_eq!(app.state.help_scroll, 0);
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
                state.anime = Some(anime("series", "A series"));
                state.history.push(crate::persistence::Entry {
                    anime: state.anime.clone().unwrap(),
                    episode: last.into(),
                    mode: Mode::Dub,
                    watched_at: 1,
                });
                state.episodes = (1..=3)
                    .map(|n| Episode {
                        id: n.to_string(),
                        number: n.to_string(),
                        title: String::new(),
                    })
                    .collect();
                state.marked = [0, 1].into();
                let mut app = test_app(state, dir.path());
                let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
                app.key(enter);
                assert!(app.state.marked.is_empty());
                if last == "9" {
                    assert!(app.state.status.contains("unavailable"));
                    assert!(!app.state.status.contains("replays"));
                }
                assert_eq!(
                    app.state.episodes[app.state.episode_selected].number,
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
        state.text_view = Some(TextView {
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
        assert!(app.state.text_selection.is_none());
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
        app.state.text_view = Some(TextView {
            area: Rect::new(10, 5, 12, 1),
            offset: 0,
            lines: vec!["A日e\u{301} B".into()],
        });
        app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 11, 5));
        let copied = app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 14, 5));
        assert_eq!(copied.as_deref(), Some("日e\u{301}"));
        app.state.details = false;
        app.mouse(mouse(MouseEventKind::ScrollDown, 11, 5));
        assert_eq!(app.state.preview_scroll, 3);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.episodes = (1..=3)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.mouse(mouse(MouseEventKind::ScrollDown, 0, 5));
        assert_eq!(app.state.episode_selected, 1);
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
        app.state.selected = 1;
        assert!(app.prepare_synopsis());
        assert!(app.state.synopses.peek("first").is_none());
        assert!(matches!(
            app.state.synopses.peek("second"),
            Some(Synopsis::Loading)
        ));
        app.state
            .synopses
            .put("second".into(), Synopsis::Ready("Full synopsis".into()));
        app.state.selected = 0;
        app.prepare_synopsis();
        app.state.selected = 1;
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
            assert_eq!(app.state.query.text, "fox");
            assert_eq!(app.state.search_generation, generation);
        }
        app.state.help = false;
        app.state.details = false;
        app.state.focus = Focus::Search;
        app.paste(" 日本\n語");
        assert_eq!(app.state.query.text, "fox 日本 語");
        assert!(app.state.search_generation > generation);
        app.state.screen = Screen::Episodes;
        app.state.focus = Focus::List;
        app.state.focus = Focus::Range;
        app.state.episodes = (1..=5)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.paste("1-3,\n5\t");
        assert_eq!(app.state.range.text, "1-3, 5 ");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.state.marked.iter().copied().collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        assert!(app.state.activity.is_none());
        app.state.focus = Focus::Range;
        app.state.range.set(String::new());
        app.state.episodes = (1..=13)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.paste("1\n3");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.state.marked.iter().copied().collect::<Vec<_>>(), [0, 2]);
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
        state.anime = Some(anime("series", "Series"));
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "First".into(),
        }];
        let mut app = test_app(state, dir.path());
        app.start_selection(true);
        abort(&mut app.play_task);
        let job = app.job;
        app.message(Message::Resolved(
            job,
            Ok(Stream {
                variants: vec![crate::model::Variant {
                    height: 720,
                    url: "http://127.0.0.1:9/video".into(),
                }],
                referer: String::new(),
                audio: None,
                subtitle: None,
            }),
        ));
        assert_eq!(
            app.state.activity.as_ref().unwrap().quality_note,
            "720p (1080p unavailable)"
        );
        app.message(Message::Progress(job, "Downloaded through 00:00:01".into()));
        assert!(
            app.state
                .activity
                .as_ref()
                .unwrap()
                .quality_note
                .contains("1080p unavailable")
        );
        app.message(Message::Downloaded(job, Ok(dir.path().join("episode.mkv"))));
        let activity = app.state.activity.as_ref().unwrap();
        assert_eq!(activity.phase, JobPhase::Finished);
        assert_eq!(activity.quality_note, "720p (1080p unavailable)");
        assert!(activity.message.contains("Saved"));
    }

    #[tokio::test]
    async fn history_deletion_keeps_the_nearest_row_and_updates_preserve_identity() {
        for removed in [0, 2, 3] {
            let dir = tempfile::tempdir().unwrap();
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.history = (0..4)
                .map(|n| crate::persistence::Entry {
                    anime: anime(&n.to_string(), &format!("Series {n}")),
                    episode: "1".into(),
                    mode: Mode::Sub,
                    watched_at: n,
                })
                .collect();
            state.history_selected = removed;
            let mut entries = state.history.clone();
            entries.remove(removed);
            let mut app = test_app(state, dir.path());
            app.message(Message::Library(Ok(crate::persistence::Library {
                entries: entries.clone(),
                ..Default::default()
            })));
            assert_eq!(app.state.history_selected, removed.min(2));
            let selected = app.state.history[app.state.history_selected]
                .anime
                .id
                .clone();
            entries.reverse();
            app.message(Message::Library(Ok(crate::persistence::Library {
                entries,
                ..Default::default()
            })));
            assert_eq!(
                app.state.history[app.state.history_selected].anime.id,
                selected
            );
            app.message(Message::Library(Ok(crate::persistence::Library::default())));
            assert_eq!(app.state.history_selected, 0);
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
        state.focus = Focus::List;
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
    #[tokio::test]
    async fn episode_forward_navigation_opens_details_without_starting_playback() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.anime = Some(anime("series", "Series"));
        state.episodes = vec![Episode {
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
            assert!(app.state.status.contains("Audio: SUB"));
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
        app.state.episodes = (1..=30)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.key(ctrl('d'));
        assert_eq!(app.state.episode_selected, 10);
        app.key(ctrl('n'));
        assert_eq!(app.state.episode_selected, 11);
        app.key(ctrl('p'));
        assert_eq!(app.state.episode_selected, 10);
        app.key(ctrl('u'));
        assert_eq!(app.state.episode_selected, 0);
        app.key(ctrl('r'));
        for code in [KeyCode::Enter, KeyCode::Char('D'), KeyCode::Char('d')] {
            app.key(KeyEvent::new(code, KeyModifiers::ALT | KeyModifiers::SHIFT));
        }
        assert_eq!(app.state.mode, Mode::Sub);
        assert!(app.state.activity.is_none());
        app.state.details = true;
        app.key(key('G'));
        assert_eq!(app.state.details_scroll, u16::MAX);
        app.key(key('g'));
        assert_eq!(app.state.details_scroll, 0);
        app.key(key('q'));
        assert!(!app.state.details && !app.quit);
        app.state.help = true;
        app.key(key('q'));
        assert!(!app.state.help && !app.quit);
        app.key(key('/'));
        app.state.query.set("hello world".into());
        app.key(ctrl('a'));
        app.key(ctrl('e'));
        assert_eq!(app.state.query.cursor, 11);
        assert!(!app.state.exact);
        app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT));
        assert!(app.state.exact);
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
