use crate::{
    app::{Focus, JobKind, JobPhase, Screen, State, Synopsis},
    artwork::Artwork,
    palette::Palette,
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

fn palette(state: &State) -> Palette {
    Palette::terminal(state.background, state.colorless)
}
fn muted(state: &State) -> Style {
    Style::default().fg(palette(state).secondary)
}
fn accent(state: &State) -> Style {
    Style::default().fg(palette(state).focus).bold()
}
fn symbol<'a>(state: &State, unicode: &'a str, ascii: &'a str) -> &'a str {
    if state.ascii { ascii } else { unicode }
}

fn sections(state: &State, area: Rect) -> [Rect; 6] {
    Layout::vertical([
        Constraint::Length(if state.screen == Screen::Episodes && !state.details {
            2
        } else {
            1
        }),
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
        Constraint::Length(
            state
                .activity
                .as_ref()
                .map_or(0, |a| if a.message.is_empty() { 1 } else { 2 }),
        ),
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
    if inner.height < 6 || (state.details && inner.height < 10 && inner.width < 72) {
        return (Rect::default(), inner);
    }
    if state.details && inner.width >= 72 {
        let cover_width = (inner.width / 3).min(32);
        let text_width = inner.width.saturating_sub(cover_width + 3).min(78);
        let left = inner.x + inner.width.saturating_sub(cover_width + text_width + 3) / 2;
        return (
            Rect::new(left, inner.y, cover_width, inner.height.min(26)),
            Rect::new(left + cover_width + 3, inner.y, text_width, inner.height),
        );
    }
    let image_height = if state.details {
        (inner.height / 3).min(8)
    } else {
        (inner.height * 2 / 5).min(if state.screen == Screen::Episodes {
            12
        } else {
            16
        })
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

fn panel(title: String, state: &State, focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if state.ascii {
            BorderType::Plain
        } else if focused {
            BorderType::Double
        } else {
            BorderType::Thick
        })
        .border_style(if focused {
            accent(state)
        } else {
            Style::default().fg(palette(state).border)
        })
        .padding(Padding::horizontal(1))
        .title(Line::from(title).style(if focused {
            accent(state)
        } else {
            Style::default().bold()
        }));
    if state.ascii {
        block.border_set(ratatui::symbols::border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: if focused { "=" } else { "-" },
            horizontal_bottom: if focused { "=" } else { "-" },
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
    let location = if state.details {
        "Details"
    } else {
        match state.screen {
            Screen::Home => "Home",
            Screen::Search => "Search",
            Screen::History => "History",
            Screen::Episodes => "Episodes",
        }
    };
    let mut heading = vec![Line::from(vec![
        Span::styled(" ani / watch", accent(state)),
        Span::raw(format!(
            "   {location}   {} / {}",
            state.mode.as_str().to_uppercase(),
            state.quality
        )),
    ])];
    if state.screen == Screen::Episodes
        && !state.details
        && let Some(anime) = &state.anime
    {
        heading.push(Line::raw(format!(" {}", anime.title)));
    }
    frame.render_widget(Paragraph::new(heading), header);
    if input.height > 0 {
        render_input(frame, state, input);
    }
    if state.welcome() && !state.details {
        empty_art(frame, state, body.inner(Margin::new(2, 1)), "", "", true);
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
                Style::default().fg(palette(state).error)
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
    } else {
        " Search "
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
        state,
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

fn fit_hints(hints: &[String], width: usize, state: &State) -> Line<'static> {
    let mut line = String::from(" ");
    for hint in hints {
        let separator = if line.len() == 1 { "" } else { "   " };
        if line.width() + separator.len() + hint.width() <= width {
            line.push_str(separator);
            line.push_str(hint);
        }
    }
    let mut spans = vec![Span::raw(" ")];
    for (i, hint) in line.trim().split("   ").enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let (key, label) = hint.split_once(' ').unwrap_or((hint, ""));
        spans.push(Span::styled(key.to_owned(), accent(state)));
        spans.push(Span::raw(format!(" {label}")));
    }
    Line::from(spans)
}

fn key_hints(state: &State, width: usize) -> Vec<Line<'static>> {
    let primary: Vec<String> = if state.details {
        vec!["Esc Back".into(), "Up/Down Scroll".into()]
    } else if state.focus == Focus::Search {
        vec![
            "Enter Results".into(),
            "Esc Done".into(),
            "Ctrl+E Exact".into(),
        ]
    } else if state.focus == Focus::Range {
        vec!["Enter Mark range".into(), "Esc Cancel".into()]
    } else if state.request_error.is_some() {
        vec!["F5 Retry".into(), "/ Search".into(), "Esc Back".into()]
    } else if state.screen == Screen::Episodes && state.episodes.is_empty() {
        let mut hints = vec!["Esc Back".into(), "i Details".into()];
        if !state.loading {
            hints.push("F5 Retry".into());
        }
        hints
    } else if state.screen == Screen::Episodes {
        let count = state.marked.len();
        let suffix = if count > 0 {
            format!(" {count}")
        } else {
            String::new()
        };
        vec![
            format!("Enter Play{suffix}"),
            "Esc Back".into(),
            format!("d Download{suffix}"),
            "Space Mark".into(),
            ": Range".into(),
            "i Details".into(),
        ]
    } else {
        let mut hints = vec![
            "Enter Episodes".into(),
            if state.screen == Screen::Search {
                "/ Edit"
            } else {
                "/ Search"
            }
            .into(),
            "i Details".into(),
        ];
        if state.selected_anime().is_none() {
            hints = vec![
                if state.screen == Screen::Search {
                    "/ Edit"
                } else {
                    "/ Search"
                }
                .into(),
            ];
        }
        if state.screen == Screen::History && !state.history.is_empty() {
            hints.push("Delete Remove".into());
        }
        hints
    };
    let mut global = vec!["F1 Help".into(), "Ctrl+C Quit".into()];
    if let Some(activity) = &state.activity {
        global.insert(
            0,
            if matches!(activity.phase, JobPhase::Finished | JobPhase::Failed) {
                "x Dismiss"
            } else {
                "x Stop"
            }
            .into(),
        );
    }
    // Printable shortcuts are text while editing.
    if matches!(state.focus, Focus::Search | Focus::Range) {
        global = vec!["F1 Help".into(), "Ctrl+C Quit".into()];
        if state.request_error.is_some() {
            global.insert(0, "F5 Retry".into());
        }
    } else if !state.details {
        if state.screen != Screen::Home {
            global.push("H Home".into());
        }
        if state.screen != Screen::History && !state.history.is_empty() {
            global.push("h History".into());
        }
        global.extend(["D SUB/DUB".into(), "Q Quality".into()]);
    }
    vec![
        fit_hints(&primary, width, state),
        fit_hints(&global, width, state),
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
    let mut title = if state.marked.is_empty() || state.screen != Screen::Episodes {
        format!(" {label} / {count} ")
    } else {
        format!(" {label} / {count} / {} marked ", state.marked.len())
    };
    if state.loading && count > 0 {
        title.push_str(if state.screen == Screen::Search {
            "/ Searching... "
        } else {
            "/ Loading... "
        });
    }
    let block = panel(title, state, state.focus == Focus::List);
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
                Line::styled(title, Style::default().fg(palette(state).error).bold()),
                Line::raw(""),
                Line::raw(error.clone()),
            ])
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    if count == 0 {
        let (title, hint) = if state.loading {
            if state.screen == Screen::Episodes {
                ("Loading episodes...", "")
            } else {
                ("Searching...", "")
            }
        } else if state.screen == Screen::History {
            ("No history", "")
        } else if state.screen == Screen::Episodes {
            ("No episodes", "")
        } else if state.query.text.trim().is_empty() {
            ("Enter a title", "")
        } else {
            (
                "No matches",
                if state.exact {
                    "Ctrl+E Turn off exact match"
                } else {
                    "Try another title"
                },
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
                    Line::styled(
                        format!(
                            "Last played: Ep {} / {}",
                            entry.episode,
                            entry.mode.as_str().to_uppercase()
                        ),
                        muted(state),
                    ),
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
                    Line::styled(anime.info.clone(), muted(state)),
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
                ListItem::new(Line::from(vec![
                    Span::styled(mark, Style::default().fg(palette(state).success)),
                    Span::styled(indicator, Style::default().fg(palette(state).success)),
                    Span::raw(format!(" {:>4}  {}", ep.number, ep.title)),
                ]))
            })
            .collect(),
    };
    let list = List::new(items)
        .highlight_symbol(if state.ascii { "> " } else { "▸ " })
        .highlight_style(palette(state).selection);
    let mut selection = ListState::default().with_selected(Some(selected));
    *selection.offset_mut() = state.list_offsets[state.screen as usize];
    frame.render_stateful_widget(list, inner, &mut selection);
    state.list_offsets[state.screen as usize] = selection.offset();
}

fn render_preview(frame: &mut Frame, state: &mut State, art: &Artwork, area: Rect) {
    let block = panel(
        if state.details { "" } else { " Series " }.into(),
        state,
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
            "",
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
            "No cover"
        };
        empty_art(frame, state, cover, label, "", false);
    }
    let mut details = Vec::new();
    let mut push = |text: &str, style: Style| {
        details.extend(
            wrap_prose(text, metadata.width as usize)
                .into_iter()
                .map(|line| Line::styled(line, style)),
        );
    };
    push(&anime.title, Style::default().bold());
    if !anime.info.is_empty() {
        push(&anime.info, muted(state));
    }
    let entry = match state.screen {
        Screen::Home => state.history.get(state.home_selected),
        Screen::History => state.history.get(state.history_selected),
        _ => state
            .history
            .iter()
            .find(|e| e.anime.id == anime.id && e.mode == state.mode),
    };
    if let Some(entry) = entry
        && (state.details || !matches!(state.screen, Screen::Home | Screen::History))
    {
        push(
            &format!(
                "Last played: Ep {} / {}",
                entry.episode,
                entry.mode.as_str().to_uppercase()
            ),
            muted(state),
        );
    }
    if state.details
        && state.screen == Screen::Episodes
        && let Some(ep) = state.episodes.get(state.episode_selected)
    {
        push("", Style::default());
        push(
            &format!("Episode {} / {}", ep.number, ep.title),
            accent(state),
        );
    }
    push("", Style::default());
    push("Synopsis", accent(state));
    let synopsis = state.synopses.peek(&anime.id);
    let description = if let Some(Synopsis::Ready(text)) = synopsis {
        text.as_str()
    } else {
        anime.description.as_str()
    };
    push(
        if description.is_empty() {
            "No synopsis"
        } else {
            description
        },
        Style::default(),
    );
    if state.details {
        match synopsis {
            Some(Synopsis::Loading) => {
                push("", Style::default());
                push(
                    "Loading synopsis...",
                    Style::default().fg(palette(state).pending),
                );
            }
            Some(Synopsis::Failed(_)) => {
                push("", Style::default());
                push(
                    "! Synopsis unavailable / F5 Retry",
                    Style::default().fg(palette(state).error),
                );
            }
            _ => {}
        }
    }
    let max_scroll = details
        .len()
        .saturating_sub(metadata.height as usize)
        .min(u16::MAX as usize) as u16;
    if state.details {
        state.details_scroll = state.details_scroll.min(max_scroll);
    }
    let scroll = if state.details {
        state.details_scroll
    } else {
        0
    };
    frame.render_widget(Paragraph::new(details).scroll((scroll, 0)), metadata);
}

fn wrap_prose(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for grapheme in word.graphemes(true) {
                if grapheme.width() > width {
                    continue;
                }
                if line.width() + grapheme.width() > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(grapheme);
            }
        }
        lines.push(line);
    }
    lines
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
    let (indicator, color) = match activity.phase {
        JobPhase::Resolving => (symbol(state, "…", "..."), palette(state).pending),
        JobPhase::Running if activity.kind == JobKind::Download => {
            (symbol(state, "↓", "v"), palette(state).pending)
        }
        JobPhase::Running => (symbol(state, "▶", ">"), palette(state).success),
        JobPhase::Finished => (symbol(state, "✓", "+"), palette(state).success),
        JobPhase::Failed => ("!", palette(state).error),
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
                    " {indicator} {phase}: {} / Ep {} / {}{queue}",
                    activity.anime.title,
                    episode.number,
                    activity.mode.as_str().to_uppercase()
                ),
                Style::default().fg(color),
            ),
            Line::raw(format!(" {}", activity.message)),
        ]),
        area,
    );
}

fn render_help(frame: &mut Frame, state: &State, art: &Artwork) {
    let area = frame.area();
    let block = panel(" Help / Up,Down scroll / Esc ".into(), state, true);
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "H                 Home\n/                 Search\nh                 History\nEnter             Results / episodes / play\nUp/Down or j/k    Navigate or scroll\nPgUp/PgDn         Jump ten items or lines\nHome/End or g/G   First / last item\ni                 Series details\nCtrl+E            Exact match\nD / Q             Language / quality\nSpace or Tab      Mark episodes\n:                 Mark a range (1-3, 5 or 0--1)\nd                 Download marked/current episodes\nn / p / r         Next / previous / replay\nx                 Stop or dismiss\nF5 or R           Retry request and cover\nDelete            Remove history entry\nEsc               Leave input / go back\nF1                Help, including while typing\nq or Ctrl+C       Quit and stop playback/downloads\n\nPrintable shortcuts are text while typing.\nBrowsing keeps playback and downloads running.\nLanguage and quality changes apply to the next queue.\nHistory records launches, not completion or position.\n--no-animation    Disable animation\n--images off      Hide covers\n--ascii           Plain borders and symbols";
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
    let synopsis_error = state
        .selected_anime()
        .and_then(|a| state.synopses.peek(&a.id))
        .and_then(|synopsis| match synopsis {
            Synopsis::Failed(message) => Some(message.as_str()),
            _ => None,
        })
        .map(|message| format!("\n\nSynopsis: {message}"))
        .unwrap_or_default();
    let status = if state.status.is_empty() {
        String::new()
    } else {
        format!("\n\nStatus: {}", state.status)
    };
    let cover = art.error.as_deref().unwrap_or(&art.label);
    frame.render_widget(
        Paragraph::new(format!(
            "{text}{status}{job}\n\nCovers: {cover}{synopsis_error}"
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
    let lines: Vec<_> = [(title, accent(state)), (hint, muted(state))]
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(text, style)| Line::styled(text, style))
        .collect();
    let caption_height = (lines.len() as u16
        + u16::from(title.width() > usize::from(area.width))
        + u16::from(hint.width() > usize::from(area.width)))
    .min(area.height);
    let caption = Paragraph::new(lines).centered().wrap(Wrap { trim: true });
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
        assert!(!text.contains("No matches"));
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
        assert!(text.contains("Series"));
        assert!(text.contains("No cover") || text.contains("Covers disabled"));
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

    #[test]
    fn details_show_full_synopsis_and_clamp_scroll_across_sizes() {
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
            Ok(vec![Anime {
                id: "series-1".into(),
                title: "A long journey".into(),
                description: "A truncated snippet...".into(),
                ..Default::default()
            }]),
        );
        state.details = true;
        let full = format!(
            "{}THE END",
            "The journey continues across a changing world. ".repeat(36)
        );
        state.synopses.put("series-1".into(), Synopsis::Ready(full));
        for (w, h) in [(180, 45), (80, 24), (60, 40), (40, 18), (32, 12)] {
            state.details_scroll = 0;
            let text = render_text(&mut state, w, h);
            assert!(text.contains("Synopsis"), "{w}x{h}");
            assert!(!text.contains("truncated snippet"));
            let (_, reading) = preview_regions(&state, panels(&state, Rect::new(0, 0, w, h)).1);
            assert!(reading.width <= 78);
            state.details_scroll = u16::MAX;
            let end = render_text(&mut state, w, h);
            assert!(end.contains("THE END"), "{w}x{h}");
            assert!(state.details_scroll < u16::MAX);
        }
    }

    #[test]
    fn prose_wraps_unicode_without_losing_paragraphs_or_splitting_graphemes() {
        assert_eq!(
            wrap_prose("one two\n\n日本語 e\u{301}", 6),
            ["one", "two", "", "日本語", "e\u{301}"]
        );
        assert_eq!(wrap_prose("abcdefgh", 3), ["abc", "def", "gh"]);
        assert!(wrap_prose("日本", 1).iter().all(|line| line.width() <= 1));
    }

    #[test]
    fn focus_has_a_stronger_border_and_plain_mode_keeps_non_color_cues() {
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
            Ok(vec![Anime {
                title: "Series".into(),
                ..Default::default()
            }]),
        );
        state.focus = Focus::List;
        let text = render_text(&mut state, 120, 40);
        assert!(text.contains('╔') && text.contains('═'));
        assert!(text.contains('┏') && text.contains('━'));
        state.ascii = true;
        state.colorless = true;
        let text = render_text(&mut state, 120, 40);
        assert!(text.is_ascii());
        assert!(text.contains("==") && text.contains("--") && text.contains("> Series"));
    }
}
