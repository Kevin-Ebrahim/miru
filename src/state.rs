use crate::{
    filter::TitleList,
    input::Input,
    model::{Anime, Episode, Mode, SourceId},
};
use anyhow::Result;
use lru::LruCache;
use std::{
    collections::{BTreeSet, VecDeque},
    num::NonZeroUsize,
};

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

pub use crate::jobs::JobKind;

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

pub enum Synopsis {
    Loading,
    Ready(String),
    Failed,
}

#[derive(Clone, Default)]
pub struct SearchState {
    pub query: Input,
    results: TitleList<Anime>,
    pub results_query: String,
    pub selected: usize,
    pub exact: bool,
}

pub struct HistoryState {
    entries: TitleList<crate::persistence::Entry>,
    pub query: Input,
    pub selected: usize,
}

impl SearchState {
    pub fn results(&self) -> &[Anime] {
        self.results.items()
    }

    pub fn replace_results(&mut self, results: Vec<Anime>) {
        self.results.replace(results, |anime| &anime.title);
    }
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            entries: TitleList::new(str::to_lowercase),
            query: Input::default(),
            selected: 0,
        }
    }
}

impl HistoryState {
    pub fn entries(&self) -> &[crate::persistence::Entry] {
        self.entries.items()
    }

    pub fn replace_entries(&mut self, entries: Vec<crate::persistence::Entry>) {
        self.entries.replace(entries, |entry| &entry.anime.title);
    }
}

#[derive(Clone, Default)]
pub struct EpisodesState {
    pub anime: Option<Anime>,
    pub items: Vec<Episode>,
    pub selected: usize,
    pub marked: BTreeSet<usize>,
    pub range: Input,
    pub(crate) continuation: Option<crate::persistence::Entry>,
}

pub struct State {
    pub screen: Screen,
    pub focus: Focus,
    pub search: SearchState,
    pub history: HistoryState,
    pub episodes: EpisodesState,
    pub view: crate::view::Geometry,
    pub art_revision: u64,
    pub mode: Mode,
    pub quality: String,
    pub search_generation: u64,
    pub episode_generation: u64,
    pub loading: bool,
    pub status: String,
    pub error: bool,
    pub request_error: Option<String>,
    pub details: bool,
    pub help: bool,
    pub help_query: Input,
    pub help_filtering: bool,
    pub frame: usize,
    pub sakura: crate::sakura::Motion,
    pub ascii: bool,
    pub colorless: bool,
    pub palette: crate::palette::Palette,
    pub synopses: LruCache<SourceId, Synopsis>,
    pub animate: bool,
    pub activity: Option<Activity>,
}

impl State {
    pub fn request_blocks_list(&self) -> bool {
        self.request_error.is_some()
            && (self.screen != Screen::Episodes || self.episodes.items.is_empty())
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
            search: SearchState {
                query: input,
                exact,
                ..Default::default()
            },
            history: HistoryState::default(),
            episodes: EpisodesState::default(),
            view: crate::view::Geometry::default(),
            art_revision: 0,
            mode,
            quality,
            search_generation: 0,
            episode_generation: 0,
            loading: false,
            status: String::new(),
            error: false,
            request_error: None,
            details: false,
            help: false,
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

    pub fn visible(&self) -> std::sync::Arc<[usize]> {
        self.search.results.visible(
            &self.search.query.text,
            self.search.exact,
            Some(&self.search.results_query),
        )
    }

    pub fn visible_history(&self) -> std::sync::Arc<[usize]> {
        self.history
            .entries
            .visible(self.history.query.text.trim(), false, None)
    }

    pub fn selected_history(&self) -> Option<&crate::persistence::Entry> {
        self.visible_history()
            .get(self.history.selected)
            .and_then(|i| self.history.entries().get(*i))
    }

    pub(crate) fn history_filter_changed(&mut self) {
        self.history.selected = 0;
        self.view.list_offsets[Screen::History as usize] = 0;
    }

    pub fn selected_anime(&self) -> Option<&Anime> {
        match self.screen {
            Screen::Home => None,
            Screen::Search if self.request_error.is_some() => None,
            Screen::Search => self
                .visible()
                .get(self.search.selected)
                .and_then(|i| self.search.results().get(*i)),
            Screen::Episodes => self.episodes.anime.as_ref(),
            Screen::History => self.selected_history().map(|e| &e.anime),
        }
    }

    pub fn current_episode_index(&self) -> Option<usize> {
        let activity = self.activity.as_ref()?;
        if activity.kind != JobKind::Playback
            || !self.episodes.anime.as_ref()?.same_source(&activity.anime)
        {
            return None;
        }
        let episode = &activity.episodes[activity.index];
        self.episodes
            .items
            .iter()
            .position(|e| e.same_source(episode))
    }

    pub fn invalidate_search(&mut self) {
        self.search_generation += 1;
        self.search.selected = 0;
        self.view.list_offsets[Screen::Search as usize] = 0;
        self.loading = !self.search.query.text.trim().is_empty();
        self.error = false;
        self.request_error = None;
        self.status.clear();
        if !self.loading {
            self.search.replace_results(vec![]);
        }
    }

    pub fn accept_search(&mut self, generation: u64, result: Result<Vec<Anime>>) -> bool {
        if generation != self.search_generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(results) => {
                let selected_id = self.selected_anime().map(Anime::source_id);
                self.search.replace_results(results);
                self.search.results_query = self.search.query.text.clone();
                self.search.selected = selected_id
                    .and_then(|id| {
                        self.visible().iter().position(|i| {
                            self.search.results()[*i].provider == id.provider
                                && self.search.results()[*i].id == id.id
                        })
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

    pub(crate) fn message(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.error = error;
    }
}
