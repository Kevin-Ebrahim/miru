use crate::{
    app::{Focus, JobKind, JobPhase, Screen, State},
    artwork::Artwork,
    scene::Dusk,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
};
use ratatui_image::Image;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const ACCENT: Color = Color::Cyan;
fn muted() -> Style {
    Style::default()
}
fn accent() -> Style {
    Style::default().fg(ACCENT).bold()
}

fn sections(state: &State, area: Rect) -> [Rect; 6] {
    Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(
            if !state.details && (state.screen == Screen::Search || state.focus == Focus::Range) {
                3
            } else {
                0
            },
        ),
        Constraint::Min(1),
        Constraint::Length(
            if state.status.is_empty() || state.request_error.is_some() {
                0
            } else {
                2
            },
        ),
        Constraint::Length(if state.activity.is_some() { 2 } else { 0 }),
        Constraint::Length(2),
    ])
    .areas(area)
}

fn panels(state: &State, area: Rect) -> (Rect, Rect) {
    let body = sections(state, area)[2];
    if state.details {
        return (Rect::default(), body);
    }
    if state.selected_anime().is_none() {
        return (body, Rect::default());
    }
    if area.width >= 72 && body.height >= 12 {
        let share = if state.screen == Screen::Episodes {
            65
        } else {
            55
        };
        let [list, preview] = Layout::horizontal([
            Constraint::Percentage(share),
            Constraint::Percentage(100 - share),
        ])
        .areas(body);
        (list, preview)
    } else if area.width >= 40 && body.height >= 27 {
        let [list, preview] =
            Layout::vertical([Constraint::Min(12), Constraint::Length(13)]).areas(body);
        (list, preview)
    } else {
        (body, Rect::default())
    }
}

fn preview_regions(state: &State, area: Rect) -> (Rect, Rect) {
    if area.width < 8 || area.height < 3 {
        return (Rect::default(), Rect::default());
    }
    let inner = area.inner(Margin::new(2, 1));
    if inner.height < 6 {
        return (Rect::default(), inner);
    }
    let metadata_height = if state.details {
        (inner.height / 2).max(3)
    } else if inner.height >= 25 {
        10
    } else if inner.height >= 16 {
        7
    } else {
        4
    };
    let image_height = inner.height.saturating_sub(metadata_height + 1);
    let image_height = if state.screen == Screen::Episodes && !state.details {
        image_height.min(12)
    } else {
        image_height
    };
    let cover = Rect::new(inner.x, inner.y, inner.width, image_height);
    let metadata = Rect::new(
        inner.x,
        inner.y + image_height + 1,
        inner.width,
        inner.height.saturating_sub(image_height + 1),
    );
    (cover, metadata)
}

pub fn cover_area(state: &State, area: Rect) -> Rect {
    if area.width < 32 || area.height < 12 || state.help {
        return Rect::default();
    }
    preview_regions(state, panels(state, area).1).0
}

fn panel(title: String, ascii: bool, focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(if focused { ACCENT } else { Color::DarkGray }))
        .padding(Padding::horizontal(1))
        .title(Line::from(title).style(if focused {
            accent()
        } else {
            Style::default().bold()
        }));
    if ascii {
        block.border_set(ratatui::symbols::border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        })
    } else {
        block
    }
}

pub fn render(frame: &mut Frame, state: &mut State, art: &Artwork) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().fg(Color::Reset).bg(Color::Reset)),
        area,
    );
    if area.width < 32 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("ani-watch\nResize to at least 32 x 12\nCtrl+C quit")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [header, input, body, status, activity, footer] = sections(state, area);
    let location = match state.screen {
        Screen::Home => "Home",
        Screen::Search => "Search",
        Screen::History => "History",
        Screen::Episodes => "Episodes",
    };
    let subtitle = if state.details {
        "Series details / Esc back".into()
    } else if state.screen == Screen::Episodes {
        state
            .anime
            .as_ref()
            .map(|a| a.title.clone())
            .unwrap_or_default()
    } else {
        match state.screen {
            Screen::Home => "Your watch shelf".into(),
            Screen::Search => "Find your next series".into(),
            _ => "Previously played / Enter opens episodes".into(),
        }
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" ani / watch", accent()),
                Span::raw(format!(
                    "   {location}   {} / {}",
                    state.mode.as_str().to_uppercase(),
                    state.quality
                )),
            ]),
            Line::raw(format!(" {subtitle}")),
        ]),
        header,
    );
    if input.height > 0 {
        render_input(frame, state, input);
    }
    if state.welcome() && !state.details {
        empty_art(
            frame,
            state,
            body.inner(Margin::new(2, 1)),
            "Your next story starts here",
            "/ Search for your first series",
            true,
        );
    } else {
        let (list, preview) = panels(state, area);
        if list.width > 0 {
            render_list(frame, state, list);
        }
        if preview.width > 0 {
            render_preview(frame, state, art, preview);
        }
    }
    if status.height > 0 {
        frame.render_widget(
            Paragraph::new(format!(
                " {}{}",
                if state.error { "! " } else { "" },
                state.status
            ))
            .style(if state.error {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            })
            .wrap(Wrap { trim: true }),
            status,
        );
    }
    render_activity(frame, state, activity);
    let hints = key_hints(state, area.width as usize);
    frame.render_widget(Paragraph::new(hints), footer);
    if state.help {
        render_help(frame, state, art);
    }
}

fn render_input(frame: &mut Frame, state: &State, area: Rect) {
    let title = if state.focus == Focus::Range {
        " Mark episodes / 1-3, 5 "
    } else if state.focus == Focus::Search {
        " Search / typing "
    } else {
        " Search / press / to edit "
    };
    let block = panel(
        format!(
            "{title}{}",
            if state.exact && state.focus != Focus::Range {
                "[EXACT] "
            } else {
                ""
            }
        ),
        state.ascii,
        state.focus != Focus::List,
    );
    let inner = block.inner(area);
    let input = if state.focus == Focus::Range {
        &state.range
    } else {
        &state.query
    };
    let (text, cursor) = input_window(
        &input.text,
        input.cursor,
        inner.width.saturating_sub(1) as usize,
    );
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(text), inner);
    if state.focus != Focus::List && !state.help {
        frame.set_cursor_position((inner.x + cursor as u16, inner.y));
    }
}

fn fit_hints(hints: &[String], width: usize) -> String {
    let mut line = String::from(" ");
    for hint in hints {
        let separator = if line.len() == 1 { "" } else { "   " };
        if line.width() + separator.len() + hint.width() <= width {
            line.push_str(separator);
            line.push_str(hint);
        }
    }
    line
}

fn key_hints(state: &State, width: usize) -> Vec<Line<'static>> {
    let primary: Vec<String> = if state.details {
        vec!["Esc Back".into(), "Up/Down Scroll".into()]
    } else if state.focus == Focus::Search {
        vec![
            "Enter Results".into(),
            "Esc Leave input".into(),
            "Ctrl+E Exact".into(),
        ]
    } else if state.focus == Focus::Range {
        vec!["Enter Mark range".into(), "Esc Cancel".into()]
    } else if state.request_error.is_some() {
        vec!["F5 Retry".into(), "/ Edit search".into(), "Esc Back".into()]
    } else if state.screen == Screen::Episodes {
        let count = state.marked.len();
        let suffix = if count > 0 {
            format!(" {count}")
        } else {
            String::new()
        };
        vec![
            format!("Enter Play{suffix}"),
            format!("d Download{suffix}"),
            "Space Mark".into(),
            "Esc Back".into(),
            ": Range".into(),
            "i Details".into(),
        ]
    } else {
        let mut hints = vec![
            "Enter Episodes".into(),
            "/ Search".into(),
            "i Details".into(),
        ];
        if state.welcome() {
            hints = vec!["/ Search".into()];
        }
        if state.screen == Screen::History {
            hints.push("Delete Remove".into());
        }
        hints
    };
    let mut global = vec!["F1 Help".into(), "Ctrl+C Quit".into()];
    if state.activity.is_some() {
        global.insert(0, "x Stop / dismiss".into());
    }
    // Printable shortcuts are text while editing.
    if matches!(state.focus, Focus::Search | Focus::Range) {
        global = vec!["F1 Help".into(), "F5 Retry".into(), "Ctrl+C Quit".into()];
    } else if !state.details {
        global.extend([
            "H Home".into(),
            "h History".into(),
            "D SUB/DUB".into(),
            "Q Quality".into(),
        ]);
    }
    vec![
        Line::raw(fit_hints(&primary, width)),
        Line::raw(fit_hints(&global, width)),
    ]
}

fn render_list(frame: &mut Frame, state: &mut State, area: Rect) {
    let visible = state.visible();
    let (label, count, selected) = match state.screen {
        Screen::Home => (
            "Recently played",
            state.history.len().min(8),
            state.home_selected,
        ),
        Screen::Search => ("Results", visible.len(), state.selected),
        Screen::History => ("History", state.history.len(), state.history_selected),
        Screen::Episodes => ("Episodes", state.episodes.len(), state.episode_selected),
    };
    let title = if state.marked.is_empty() || state.screen != Screen::Episodes {
        format!(" {label} / {count} ")
    } else {
        format!(" {label} / {count} / {} marked ", state.marked.len())
    };
    let block = panel(title, state.ascii, state.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(error) = &state.request_error {
        let title = if state.screen == Screen::Search {
            "Search unavailable"
        } else {
            "Episodes unavailable"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::styled(title, Style::default().fg(Color::Red).bold()),
                Line::raw(""),
                Line::raw(error.clone()),
                Line::raw(""),
                Line::raw("F5 Retry / F1 Details"),
            ])
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    if count == 0 {
        let (title, hint) = if state.loading {
            if state.screen == Screen::Episodes {
                ("Loading episodes...", "You can browse while this loads.")
            } else {
                ("Searching...", "You can keep typing.")
            }
        } else if state.screen == Screen::History {
            (
                "No recently played series",
                "/ Search for your first series",
            )
        } else if state.screen == Screen::Episodes {
            ("No episodes available", "F5 Refresh / Esc Back")
        } else if state.query.text.trim().is_empty() {
            ("Search the catalogue", "Type an anime title above.")
        } else {
            (
                "No matching series",
                "Edit your search or toggle exact matching with Ctrl+E.",
            )
        };
        empty_art(frame, state, inner, title, hint, false);
        return;
    }
    let items: Vec<ListItem> = match state.screen {
        Screen::Home | Screen::History => state
            .history
            .iter()
            .take(count)
            .map(|entry| {
                ListItem::new(vec![
                    Line::styled(entry.anime.title.clone(), Style::default().bold()),
                    Line::raw(format!(
                        "Last played: {} / {}",
                        entry.episode,
                        entry.mode.as_str().to_uppercase()
                    )),
                    Line::raw(""),
                ])
            })
            .collect(),
        Screen::Search => visible
            .iter()
            .map(|i| {
                let anime = &state.results[*i];
                ListItem::new(vec![
                    Line::styled(anime.title.clone(), Style::default().bold()),
                    Line::raw(anime.info.clone()),
                    Line::raw(""),
                ])
            })
            .collect(),
        Screen::Episodes => state
            .episodes
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let playing =
                    state.activity.as_ref().is_some_and(|a| {
                        a.kind == JobKind::Playback && a.phase == JobPhase::Running
                    }) && state.current_episode_index() == Some(i);
                let mark = if state.marked.contains(&i) {
                    if state.ascii { "*" } else { "✓" }
                } else {
                    " "
                };
                let indicator = if playing {
                    if state.ascii { ">" } else { "▶" }
                } else {
                    " "
                };
                ListItem::new(Line::raw(format!(
                    "{mark}{indicator} {:>4}  {}",
                    ep.number, ep.title
                )))
            })
            .collect(),
    };
    let list = List::new(items)
        .highlight_symbol(if state.ascii { "> " } else { "▸ " })
        .highlight_style(Style::default().reversed().bold());
    let mut selection = ListState::default().with_selected(Some(selected));
    *selection.offset_mut() = state.list_offsets[state.screen as usize];
    frame.render_stateful_widget(list, inner, &mut selection);
    state.list_offsets[state.screen as usize] = selection.offset();
}

fn render_preview(frame: &mut Frame, state: &State, art: &Artwork, area: Rect) {
    let block = panel(
        if state.details {
            " Series details / Esc back "
        } else {
            " Selected series "
        }
        .into(),
        state.ascii,
        state.details,
    );
    frame.render_widget(block, area);
    let (cover, metadata) = preview_regions(state, area);
    let Some(anime) = state.selected_anime() else {
        empty_art(
            frame,
            state,
            area.inner(Margin::new(2, 1)),
            "Choose a series",
            "Its cover and details will appear here.",
            false,
        );
        return;
    };
    if let Some(protocol) = &art.protocol {
        let size = protocol.size();
        if size.width <= cover.width && size.height <= cover.height {
            frame.render_widget(
                Image::new(protocol),
                Rect::new(
                    cover.x + (cover.width - size.width) / 2,
                    cover.y + (cover.height - size.height) / 2,
                    size.width,
                    size.height,
                ),
            );
        }
    } else if cover.height > 0 {
        let label = if !art.enabled {
            "Covers disabled"
        } else if art.error.is_some() {
            "Cover unavailable / F5 Retry"
        } else if anime.cover.is_some() {
            "Loading cover..."
        } else {
            "No cover available"
        };
        empty_art(frame, state, cover, label, "", false);
    }
    let mut details = vec![
        Line::styled(anime.title.clone(), Style::default().bold()),
        Line::raw(anime.info.clone()),
    ];
    let entry = match state.screen {
        Screen::Home => state.history.get(state.home_selected),
        Screen::History => state.history.get(state.history_selected),
        _ => state
            .history
            .iter()
            .find(|e| e.anime.id == anime.id && e.mode == state.mode),
    };
    if let Some(entry) = entry {
        details.push(Line::raw(format!(
            "Last played: episode {} / {}",
            entry.episode,
            entry.mode.as_str().to_uppercase()
        )));
    }
    if state.screen == Screen::Episodes
        && let Some(ep) = state.episodes.get(state.episode_selected)
    {
        details.push(Line::raw(""));
        details.push(Line::styled(
            format!("Episode {} / {}", ep.number, ep.title),
            accent(),
        ));
    }
    if !anime.description.is_empty() {
        details.push(Line::raw(""));
        details.push(Line::raw(anime.description.clone()));
    }
    if !state.details {
        details.push(Line::raw(""));
        details.push(Line::raw("i Full details"));
    }
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).scroll((
            if state.details {
                state.details_scroll
            } else {
                0
            },
            0,
        )),
        metadata,
    );
}

fn render_activity(frame: &mut Frame, state: &State, area: Rect) {
    let Some(activity) = &state.activity else {
        return;
    };
    let phase = match activity.phase {
        JobPhase::Resolving => "Resolving",
        JobPhase::Running if activity.kind == JobKind::Download => "Downloading",
        JobPhase::Running => "Playing",
        JobPhase::Finished => "Finished",
        JobPhase::Failed => "Failed",
    };
    let episode = &activity.episodes[activity.index];
    let queue = if activity.remaining.is_empty() {
        String::new()
    } else {
        format!(" / {} queued", activity.remaining.len())
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(
                    " {phase}: {} / Ep {} / {}{queue}",
                    activity.anime.title,
                    episode.number,
                    activity.mode.as_str().to_uppercase()
                ),
                if activity.phase == JobPhase::Failed {
                    Style::default().fg(Color::Red)
                } else {
                    accent()
                },
            ),
            Line::raw(format!(" {}", activity.message)),
        ]),
        area,
    );
}

fn render_help(frame: &mut Frame, state: &State, art: &Artwork) {
    let area = frame.area();
    let block = panel(" Help / Up,Down scroll / Esc ".into(), state.ascii, true);
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "ANI / WATCH\n\nH                 Home / recently played\n/                 Search the catalogue\nh                 Full watch history\nEnter             Results / episodes / play\nUp/Down or j/k    Navigate; PgUp/PgDn jump\nHome/End or g/G   First / last item\ni                 Full cover and series details\nCtrl+E            Toggle exact title matching\nD / Q             Language / quality for a new queue\nSpace or Tab      Mark episodes\n:                 Mark a range (1-3, 5 or 0--1)\nd                 Download marked/current episodes\nn / p / r         Next / previous / replay\nx                 Stop or dismiss the active job\nF5 or R           Retry request and cover\nDelete            Remove an entry in History\nEsc               Leave input / return to previous screen\nF1                Help, including while typing\nq or Ctrl+C       Quit and stop owned jobs\n\nWhile typing, printable shortcuts are text.\nBrowsing leaves playback and downloads running.\nA queue keeps its series, language and quality.\nHistory records launches, not completion or position.\nUse --no-animation for a still welcome scene.\nUse --images off or --ascii for simpler rendering.";
    let job = state
        .activity
        .as_ref()
        .map(|a| {
            format!(
                "\n\nActivity: {} / Episode {}\n{}",
                a.anime.title, a.episodes[a.index].number, a.message
            )
        })
        .unwrap_or_default();
    let cover = art.error.as_deref().unwrap_or(&art.label);
    frame.render_widget(
        Paragraph::new(format!(
            "{text}\n\nCurrent status:\n{}{job}\n\nCovers: {cover}",
            state.status
        ))
        .wrap(Wrap { trim: false })
        .scroll((state.help_scroll, 0)),
        inner,
    );
}
fn empty_art(
    frame: &mut Frame,
    state: &State,
    area: Rect,
    title: &str,
    hint: &str,
    show_scene: bool,
) {
    let lines = vec![Line::styled(title, accent()), Line::styled(hint, muted())];
    let caption = Paragraph::new(lines).centered().wrap(Wrap { trim: true });
    // These short captions need at most two rows each in a scene-sized panel.
    // Reserve an extra row for each wrapped line without an unstable Ratatui API.
    let caption_height = (2
        + u16::from(title.width() > usize::from(area.width))
        + u16::from(hint.width() > usize::from(area.width)))
    .min(area.height);
    let art_height = area.height.saturating_sub(caption_height + 1).min(24);
    let (min_width, min_height) = if state.ascii || state.colorless {
        (40, 10)
    } else {
        (24, 6)
    };
    let has_scene = show_scene && area.width >= min_width && art_height >= min_height;
    let content_height = caption_height + if has_scene { art_height + 1 } else { 0 };
    let y = area.y + area.height.saturating_sub(content_height) / 2;
    if has_scene {
        frame.render_widget(
            Dusk {
                tick: state.frame,
                animate: state.animate,
                ascii: state.ascii,
                monochrome: state.colorless,
            },
            Rect::new(area.x, y, area.width, art_height),
        );
    }
    let caption_y = y + if has_scene { art_height + 1 } else { 0 };
    frame.render_widget(
        caption,
        Rect::new(
            area.x,
            caption_y,
            area.width,
            area.bottom().saturating_sub(caption_y),
        ),
    );
}

pub fn input_window(text: &str, cursor: usize, width: usize) -> (String, usize) {
    let mut start = 0;
    for (i, g) in text[..cursor].grapheme_indices(true) {
        if text[start..cursor].width() <= width {
            break;
        }
        start = i + g.len();
    }
    let mut shown = String::new();
    for g in text[start..].graphemes(true) {
        if shown.width() + g.width() > width {
            break;
        }
        shown.push_str(g);
    }
    (shown, text[start..cursor].width())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Anime, Episode, Mode};
    use ratatui::{Terminal, backend::TestBackend};
    use ratatui_image::picker::Picker;

    fn render_text(state: &mut State, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
        terminal.draw(|f| render(f, state, &art)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn responsive_screens_keep_primary_actions_and_cover_inside_terminal() {
        for (w, h) in [
            (140, 45),
            (120, 40),
            (80, 24),
            (60, 40),
            (40, 18),
            (32, 12),
            (20, 8),
        ] {
            for screen in [
                Screen::Home,
                Screen::Search,
                Screen::History,
                Screen::Episodes,
            ] {
                let mut state = State::new(
                    "日本語 title".into(),
                    Mode::Sub,
                    "best".into(),
                    false,
                    false,
                    false,
                );
                state.screen = screen;
                state.focus = Focus::List;
                state.anime = Some(Anime {
                    title: "A long series title 日本語".into(),
                    ..Default::default()
                });
                for details in [false, true] {
                    state.details = details;
                    let text = render_text(&mut state, w, h);
                    let cover = cover_area(&state, Rect::new(0, 0, w, h));
                    assert!(
                        cover.right() <= w && cover.bottom() <= h,
                        "{screen:?} {w}x{h}"
                    );
                    if w >= 32 && h >= 12 {
                        assert!(text.contains("F1 Help"));
                    } else {
                        assert!(text.contains("Resize"));
                    }
                }
            }
        }
    }

    #[test]
    fn typing_hints_and_failed_search_describe_the_available_actions() {
        let mut state = State::new(
            "frieren".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.accept_search(0, Err(anyhow::anyhow!("Service unavailable")));
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Search unavailable"));
        assert!(text.contains("F5 Retry"));
        assert!(text.contains("Enter Results"));
        assert!(!text.contains("? Help"));
        assert!(!text.contains("No matching series"));
        assert!(!text.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
    }

    #[test]
    fn home_previews_selected_series_and_episode_selection_keeps_scroll_position() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.history.push(crate::persistence::Entry {
            anime: Anime {
                id: "1".into(),
                title: "Frieren".into(),
                ..Default::default()
            },
            episode: "6".into(),
            mode: Mode::Sub,
            watched_at: 0,
        });
        let text = render_text(&mut state, 120, 40);
        assert!(text.contains("Recently played"));
        assert!(text.contains("Selected series"));
        assert!(text.contains("No cover available") || text.contains("Covers disabled"));
        assert!(!text.contains("Your next story starts here"));
        state.screen = Screen::Episodes;
        state.episodes = (1..=100)
            .map(|n| Episode {
                id: n.to_string(),
                number: n.to_string(),
                title: format!("Title {n}"),
            })
            .collect();
        state.episode_selected = 60;
        render_text(&mut state, 80, 24);
        let offset = state.list_offsets[Screen::Episodes as usize];
        state.episode_selected = 59;
        render_text(&mut state, 80, 24);
        assert_eq!(state.list_offsets[Screen::Episodes as usize], offset);
    }

    #[test]
    fn input_scroll_uses_display_width() {
        let (text, cursor) = input_window("ab日本語", "ab日本語".len(), 5);
        assert_eq!(text, "本語");
        assert_eq!(cursor, 4);
    }
}
