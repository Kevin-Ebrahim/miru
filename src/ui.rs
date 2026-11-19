use crate::{
    artwork::Artwork,
    palette::Palette,
    state::{Activity, Focus, HistoryRemoval, JobKind, JobPhase, Screen, State, Synopsis},
    view::TextView,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use ratatui_image::Image;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn palette(state: &State) -> Palette {
    if state.colorless {
        Palette::terminal(true)
    } else {
        state.palette
    }
}

fn muted(state: &State) -> Style {
    Style::default().fg(palette(state).secondary)
}

fn key_style(state: &State) -> Style {
    Style::default().fg(palette(state).text).bold()
}

fn shortcut(state: &State, key: &'static str, action: &str) -> Line<'static> {
    let mut line = Line::default();
    for step in key.split(" › ") {
        if !line.spans.is_empty() {
            line.push_span(Span::styled(symbol(state, " › ", " > "), muted(state)));
        }
        line.push_span(Span::styled(step, key_style(state)));
    }
    line.push_span(Span::styled(format!(" {action}"), muted(state)));
    line
}

fn accent(state: &State) -> Style {
    Style::default().fg(palette(state).focus).bold()
}

fn symbol<'a>(state: &State, unicode: &'a str, ascii: &'a str) -> &'a str {
    if state.ascii { ascii } else { unicode }
}

fn episode_refresh_error(state: &State) -> Option<&str> {
    if state.screen == Screen::Episodes && !state.details && !state.episodes.items.is_empty() {
        state.request_error.as_deref()
    } else {
        None
    }
}

fn inline_search(state: &State, width: u16) -> bool {
    width >= 120 && state.screen == Screen::Search && !state.details
}

fn visible_activity(state: &State) -> Option<&Activity> {
    state.activity.as_ref().filter(|activity| {
        activity.phase != JobPhase::Finished
            || (activity.kind == JobKind::Download
                && state.screen == Screen::Episodes
                && !state.details
                && state
                    .episodes
                    .anime
                    .as_ref()
                    .is_some_and(|anime| anime.same_source(&activity.anime)))
    })
}

fn activity_height(state: &State) -> u16 {
    visible_activity(state).map_or(0, |a| {
        if a.phase != JobPhase::Failed || (a.message.is_empty() && a.quality_note.is_empty()) {
            1
        } else {
            2
        }
    })
}

fn scan(state: &State) -> String {
    let step = if state.animate { state.frame % 12 } else { 3 } as i32;
    let head = if step <= 6 { step } else { 12 - step };
    let dir = if step < 6 { -1 } else { 1 };

    (0..7)
        .map(|x| {
            let level = if x == head {
                0
            } else if x == head + dir {
                1
            } else if x == head + 2 * dir {
                2
            } else {
                3
            };
            if state.ascii {
                ['#', '=', '-', '.'][level]
            } else {
                ['█', '▓', '░', '·'][level]
            }
        })
        .collect()
}

fn sections(state: &State, area: Rect) -> [Rect; 6] {
    let gap = u16::from(area.height >= 20);
    let wrap_footer = download_hints(state, area.width).is_some_and(|controls| {
        controls.width() + 1 + footer_hint(state).width() > usize::from(area.width)
    });
    let [header, _, input, body, status, activity, _, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(gap),
        Constraint::Length(
            if !state.details
                && (state.focus == Focus::Range
                    || state.screen == Screen::History
                    || (state.screen == Screen::Search && !inline_search(state, area.width)))
            {
                3
            } else {
                0
            },
        ),
        Constraint::Min(1),
        Constraint::Length(
            if episode_refresh_error(state).is_none()
                && (state.status.is_empty() || state.request_blocks_list())
            {
                0
            } else {
                2
            },
        ),
        Constraint::Length(activity_height(state)),
        Constraint::Length(gap),
        Constraint::Length(1 + u16::from(wrap_footer)),
    ])
    .areas(area);

    [header, input, body, status, activity, footer]
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
        let share = if area.width >= 120 { 35 } else { 45 };
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
    if inner.height < 6 || (inner.width < 72 && inner.height < if state.details { 10 } else { 12 })
    {
        return (Rect::default(), inner);
    }
    if inner.width >= 72 {
        let cover_width = if state.details {
            (inner.width / 3).min(32)
        } else {
            (inner.width / 4).min(20)
        };
        let text_width = inner.width.saturating_sub(cover_width + 3).min(72);
        let left = inner.x + inner.width.saturating_sub(cover_width + text_width + 3) / 2;
        return (
            Rect::new(
                left,
                inner.y,
                cover_width,
                inner.height.min(if state.details { 26 } else { 14 }),
            ),
            Rect::new(left + cover_width + 3, inner.y, text_width, inner.height),
        );
    }
    let image_height = if state.details {
        (inner.height / 3).min(8)
    } else {
        (inner.height * 2 / 5).min(16)
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
    if state.cover_view.is_some() {
        return area.inner(Margin::new(2, 2));
    }
    preview_regions(state, panels(state, area).1).0
}

pub(crate) fn panel(title: String, state: &State, focused: bool) -> Block<'static> {
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
    state.view.list = None;
    state.view.cover = None;
    state.view.help_area = None;
    state.view.source_setting = None;
    state.view.audio_setting = None;
    state.view.quality_setting = None;
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(palette(state).text)
                .bg(palette(state).canvas),
        ),
        area,
    );

    if area.width < 32 || area.height < 12 {
        if state.cover_view.is_none() {
            state.view.text_view = None;
        }
        state.view.text_selection = None;
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw("Miru"),
                Line::raw("Resize to at least 32 x 12"),
                shortcut(state, "Ctrl+C", "quit"),
            ])
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    if state.cover_view.is_some() {
        state.view.text_selection = None;
        render_cover_view(frame, state, art, area);
        return;
    }

    if state.screen == Screen::Home && !state.details {
        state.view.text_view = None;
        state.view.text_selection = None;
        render_home(frame, state, area);
        if state.help {
            render_help(frame, state);
        }
        return;
    }

    let [header, input, _, status, activity, footer] = sections(state, area);
    render_header(frame, state, header);
    if input.height > 0 {
        render_input(frame, state, input);
    }
    {
        let (list, preview) = panels(state, area);
        if list.width > 0 {
            render_list(frame, state, list);
        }
        if preview.width > 0 {
            render_preview(frame, state, art, preview);
        } else {
            state.view.text_view = None;
            state.view.text_selection = None;
        }
    }

    if status.height > 0 {
        let (text, error) = if !state.status.is_empty() {
            (
                format!(" {}{}", if state.error { "! " } else { "" }, state.status),
                state.error,
            )
        } else {
            (
                format!(
                    " ! Refresh failed\n {}",
                    episode_refresh_error(state).unwrap_or_default()
                ),
                true,
            )
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(if error {
                    Style::default().fg(palette(state).error)
                } else {
                    Style::default()
                })
                .wrap(Wrap { trim: true }),
            status,
        );
    }

    render_activity(frame, state, activity);

    render_footer(frame, state, footer);

    if state.help {
        render_help(frame, state);
    }
}

fn wordmark_style(state: &State, color: ratatui::style::Color, strength: u16) -> Style {
    use ratatui::style::Color;
    let style = Style::default().fg(color);
    match (color, palette(state).canvas) {
        (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) => {
            let blend = |ink: u8, background: u8| {
                ((u16::from(ink) * strength + u16::from(background) * (100 - strength)) / 100) as u8
            };
            style.fg(Color::Rgb(blend(r, br), blend(g, bg), blend(b, bb)))
        }
        _ if strength < 100 => style.dim(),
        _ => style,
    }
}

fn footer_hint(state: &State) -> Line<'static> {
    let hint = if state.activity.as_ref().is_some_and(Activity::retryable) {
        shortcut(state, "Alt+r", "Retry queue")
    } else if !state.details
        && (state.request_blocks_list()
            || (state.status.is_empty() && episode_refresh_error(state).is_some()))
    {
        shortcut(state, if state.typing() { "Esc › R" } else { "R" }, "Retry")
    } else {
        shortcut(state, if state.typing() { "Esc › ?" } else { "?" }, "Help")
    };
    if state.screen == Screen::Home && !state.details {
        let mut home = shortcut(state, "Alt+h", "History");
        home.push_span("  ");
        home.spans.extend(hint.spans);
        return home;
    }
    hint
}

fn download_hints(state: &State, width: u16) -> Option<Line<'static>> {
    if state.screen != Screen::Episodes
        || state.focus != Focus::List
        || state.details
        || state.episodes.items.is_empty()
        || !state.downloads_supported()
    {
        return None;
    }
    let gap = if width <= 40 { "  " } else { "   " };
    let mut controls = Line::from(" ");
    controls
        .spans
        .extend(shortcut(state, "Space", "Mark").spans);
    controls.push_span(gap);
    controls.spans.extend(shortcut(state, ":", "Range").spans);
    controls.push_span(gap);
    controls
        .spans
        .extend(shortcut(state, "d", "Download").spans);
    Some(controls)
}

fn history_footer(state: &State, width: usize) -> Line<'static> {
    let undo = matches!(state.history_removal, Some(HistoryRemoval::Removed(_)))
        .then(|| shortcut(state, "u", "Undo"));
    let available = width.saturating_sub(undo.as_ref().map_or(0, |hint| hint.width() + 2));
    let mut line = Line::default();
    if let Some(notice) = &state.history.notice {
        if available >= notice.action.len() {
            line.push_span(Span::styled(notice.action, muted(state)));
            let title_width = available.saturating_sub(line.width() + 1);
            if title_width > 0 {
                line.push_span(" ");
                line.push_span(ellipsize(&notice.title, title_width, state));
            }
        }
    } else {
        line = match &state.history_removal {
            Some(HistoryRemoval::Removing(_)) => Line::styled("Removing...", muted(state)),
            Some(HistoryRemoval::Restoring(_)) => Line::styled("Restoring...", muted(state)),
            _ if state.selected_history().is_some() => shortcut(state, "Delete", "Remove"),
            _ => Line::default(),
        };
    }
    if let Some(undo) = undo {
        if line.width() == 0 || line.width() > available {
            return undo;
        }
        line.push_span("  ");
        line.spans.extend(undo.spans);
    }
    line
}

fn render_footer(frame: &mut Frame, state: &State, area: Rect) {
    let hint = footer_hint(state);
    let hint_width = (hint.width() as u16).min(area.width);
    if let Some(controls) = download_hints(state, area.width) {
        frame.render_widget(Paragraph::new(controls), area);
    } else if state.screen == Screen::History
        && state.focus == Focus::List
        && !state.details
        && !state.help
    {
        let width = area.width.saturating_sub(hint_width + 1);
        frame.render_widget(
            Paragraph::new(history_footer(state, usize::from(width))),
            area,
        );
    } else if matches!(state.screen, Screen::Episodes | Screen::Search)
        && !state.details
        && !state.downloads_supported()
    {
        let width = area.width.saturating_sub(hint_width + 1);
        frame.render_widget(
            Paragraph::new("Playback only").style(muted(state)),
            Rect::new(area.x, area.y, width, 1),
        );
    }
    frame.render_widget(
        Paragraph::new(hint),
        Rect::new(
            area.right() - hint_width,
            area.bottom().saturating_sub(1),
            hint_width,
            1,
        ),
    );
}

fn home_sections(state: &State, area: Rect) -> [Rect; 5] {
    let hint_width = footer_hint(state).width() as u16;
    let progress_width = area.width.saturating_sub(hint_width + 2);
    let footer_download = progress_width >= 60
        && visible_activity(state)
            .is_some_and(|activity| activity.kind == JobKind::Download && activity.active());
    let activity_height = if footer_download {
        0
    } else {
        activity_height(state)
    };
    let [space, status, mut activity, gap, mut footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(if state.status.is_empty() {
            0
        } else if area.height < 18 {
            1
        } else {
            2
        }),
        Constraint::Length(activity_height),
        Constraint::Length(u16::from(activity_height > 0 && area.height >= 18)),
        Constraint::Length(1),
    ])
    .areas(area);
    if footer_download {
        activity = Rect::new(footer.x, footer.y, progress_width, 1);
        footer = Rect::new(footer.right() - hint_width, footer.y, hint_width, 1);
    }
    [space, status, activity, gap, footer]
}

pub fn home_scene_visible(state: &State, area: Rect) -> bool {
    state.screen == Screen::Home
        && !state.help
        && !state.details
        && state.cover_view.is_none()
        && crate::sakura::fits(home_sections(state, area)[0])
}

fn render_home(frame: &mut Frame, state: &mut State, area: Rect) {
    let [space, status, activity, _, footer] = home_sections(state, area);
    let width = space.width.saturating_sub(4).min(64);
    let big_logo = !state.ascii && space.width >= 60 && space.height >= 20;
    let large_logo = big_logo && space.width >= 72 && space.height >= 24;
    let logo_height = if large_logo {
        8
    } else if big_logo {
        4
    } else {
        u16::from(space.height >= 10)
    };
    let gap = u16::from(space.height >= 16);
    let box_height = if space.height >= 12 { 5 } else { 4 };
    let height = logo_height + gap + box_height;
    let x = space.x + (space.width - width) / 2;
    let y = space.y + space.height.saturating_sub(height) * 2 / 5;
    if !state.help {
        frame.render_widget(
            crate::sakura::Sakura {
                palette: palette(state),
                protected: Rect::new(x.saturating_sub(2), y, width + 4, height),
                motion: &state.sakura,
                animate: state.animate,
                ascii: state.ascii,
            },
            space,
        );
    }
    if big_logo {
        let compact = [
            ("      ▀ ", "         "),
            ("█▀█▀█ █ ", "█▀▀▄ █  █"),
            ("█ █ █ █ ", "█    █  █"),
            ("▀ ▀ ▀ ▀ ", "▀    ▀▀▀▀"),
        ];
        let large = [
            ("                       ", "                           "),
            ("                     ██", "                           "),
            ("                         ", "                           "),
            ("███████████    ███  ", " ████████  ████   ████"),
            ("██████████████   █████ ", " ██████████  █████   █████"),
            ("█████ ████ █████   █████ ", " █████        █████   █████"),
            ("█████ ████ █████  █████ ", " █████        █████   █████"),
            ("█████ ████ ██████ █████ ", " █████        ███████████"),
        ];
        let logo: &[(&str, &str)] = if large_logo { &large } else { &compact };
        let lines: Vec<_> = logo
            .iter()
            .copied()
            .enumerate()
            .map(|(i, (left, right))| {
                let strength = if large_logo {
                    match i {
                        4 => 82,
                        5 => 62,
                        6 => 78,
                        _ => 100,
                    }
                } else if i == 2 {
                    82
                } else {
                    100
                };
                let left_width = left.chars().count();
                let spans: Vec<_> = left
                    .chars()
                    .chain(right.chars())
                    .enumerate()
                    .map(|(column, glyph)| {
                        let color = if column < left_width {
                            palette(state).secondary
                        } else {
                            palette(state).text
                        };
                        let style = wordmark_style(state, color, strength);
                        Span::styled(glyph.to_string(), style)
                    })
                    .collect();
                Line::from(spans).centered()
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), Rect::new(x, y, width, logo_height));
    } else if logo_height > 0 {
        frame.render_widget(
            Paragraph::new("miru")
                .centered()
                .style(Style::default().bold()),
            Rect::new(x, y, width, 1),
        );
    }
    let field = Rect::new(x, y + logo_height + gap, width, box_height);
    let source = format!(" {} ", state.search.source.label());
    state.view.source_setting =
        Some(Rect::new(field.x + 1, field.y, source.width() as u16, 1).intersection(field));
    let block = panel(source, state, state.focus != Focus::List)
        .style(Style::default().bg(palette(state).surface));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let prefix = if state.search.exact { "[EXACT] " } else { "" };
    let input_width = inner.width.saturating_sub(prefix.len() as u16);
    let (query, cursor) = input_window(
        &state.search.query.text,
        state.search.query.cursor,
        usize::from(input_width.saturating_sub(1)),
    );
    let input = if state.search.query.text.is_empty() {
        Line::from(vec![
            Span::styled(prefix, accent(state)),
            Span::styled("Search anime...", muted(state)),
        ])
    } else {
        Line::from(vec![Span::styled(prefix, accent(state)), Span::raw(query)])
    };
    frame.render_widget(
        Paragraph::new(input),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if state.focus == Focus::Search && !state.help && !state.hide_input_cursor {
        frame.set_cursor_position((inner.x + prefix.len() as u16 + cursor as u16, inner.y));
    }
    let setting_style = |focus| {
        if state.focus == focus {
            palette(state).selection.bold().underlined()
        } else {
            muted(state).bold()
        }
    };
    let audio = format!(" {} ", state.mode.as_str().to_uppercase());
    let quality = format!(
        " {} ",
        if state.quality == "best" {
            "Best"
        } else {
            &state.quality
        }
    );
    let quality = ellipsize(&quality, usize::from(inner.width.saturating_sub(7)), state);
    let quality_width = quality.width() as u16;
    frame.render_widget(
        Paragraph::new(audio).style(setting_style(Focus::Audio)),
        Rect::new(inner.x, inner.bottom() - 1, 5, 1),
    );
    frame.render_widget(
        Paragraph::new(quality.clone()).style(setting_style(Focus::Quality)),
        Rect::new(
            inner.right() - quality_width,
            inner.bottom() - 1,
            quality_width,
            1,
        ),
    );
    state.view.audio_setting = Some(Rect::new(inner.x, inner.bottom() - 1, 5, 1));
    state.view.quality_setting = Some(Rect::new(
        inner.right() - quality_width,
        inner.bottom() - 1,
        quality_width,
        1,
    ));
    if status.height > 0 {
        frame.render_widget(
            Paragraph::new(state.status.as_str())
                .style(if state.error {
                    Style::default().fg(palette(state).error)
                } else {
                    muted(state)
                })
                .wrap(Wrap { trim: true }),
            status,
        );
    }

    render_activity(frame, state, activity);
    render_footer(frame, state, footer);
}

fn render_header(frame: &mut Frame, state: &mut State, header: Rect) {
    if header.width >= 120 || (state.screen == Screen::Episodes && !state.details) {
        render_wide_header(frame, state, header);
        return;
    }
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
    let settings = format!(
        "{} / {}",
        state.mode.as_str().to_uppercase(),
        state.quality_label()
    );
    let context = format!("   {location}   {settings}");
    let compact = " Miru".width() + context.width() > usize::from(header.width);
    let settings_x = header.x
        + if compact {
            format!(" {location}   ").width() as u16
        } else {
            format!(" Miru   {location}   ").width() as u16
        };
    state.view.audio_setting = Some(Rect::new(settings_x, header.y, 3, 1).intersection(header));
    state.view.quality_setting = Some(
        Rect::new(settings_x + 6, header.y, state.quality.width() as u16, 1).intersection(header),
    );
    let heading = vec![Line::from(if compact {
        vec![
            Span::styled(format!(" {location}"), accent(state)),
            Span::raw(format!("   {settings}")),
        ]
    } else {
        vec![Span::styled(" Miru", accent(state)), Span::raw(context)]
    })];
    frame.render_widget(
        Paragraph::new(heading).style(Style::default().bold()),
        header,
    );
}

fn render_wide_header(frame: &mut Frame, state: &mut State, area: Rect) {
    let bar = Style::default().fg(palette(state).text).bold();
    frame.render_widget(Block::default().style(bar), area);
    let settings = Line::styled(
        format!(
            "{} / {} ",
            state.mode.as_str().to_uppercase(),
            state.quality_label()
        ),
        muted(state),
    );
    let mut nav = vec![Span::raw(" ")];
    if state.details {
        nav.push(Span::styled("Details", accent(state)));
    } else {
        if state.screen == Screen::Search {
            nav.push(Span::styled("Search", accent(state).underlined()));
            nav.push(Span::raw(" "));
        } else if state.screen == Screen::Episodes {
            nav.push(Span::styled("Episodes", accent(state).underlined()));
            if let Some(anime) = &state.episodes.anime {
                let separator = symbol(state, " · ", " / ");
                let available = usize::from(area.width)
                    .saturating_sub(settings.width() + " Episodes".width() + separator.width() + 2);
                if available > 0 {
                    nav.push(Span::styled(separator, muted(state)));
                    nav.push(Span::styled(
                        ellipsize(&anime.title, available, state),
                        Style::default().fg(palette(state).text),
                    ));
                }
            }
        } else if state.screen == Screen::History {
            nav.push(Span::styled("History", accent(state).underlined()));
        }
    }
    if inline_search(state, area.width) {
        let label = state.search.source.label();
        let x = area.x + Line::from(nav.clone()).width() as u16;
        state.view.source_setting = Some(Rect::new(x, area.y, label.width() as u16, 1));
        nav.push(Span::styled(label, muted(state)));
        nav.push(Span::raw("  "));
    }
    let navigation = Line::from(nav);
    let nav_width = navigation.width() as u16;
    let [left, middle, right] = Layout::horizontal([
        Constraint::Length(nav_width),
        Constraint::Min(1),
        Constraint::Length(settings.width() as u16),
    ])
    .areas(Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(navigation), left);
    frame.render_widget(Paragraph::new(settings), right);
    let audio_width = 3;
    let quality_width = state.quality_label().width() as u16;
    state.view.audio_setting =
        Some(Rect::new(right.x, right.y, audio_width, 1).intersection(right));
    state.view.quality_setting =
        Some(Rect::new(right.x + audio_width + 3, right.y, quality_width, 1).intersection(right));
    if inline_search(state, area.width) {
        let label = if state.search.exact { "[EXACT] " } else { "" };
        let field = Rect::new(middle.x, middle.y, middle.width.saturating_sub(2), 1);
        let content = Rect::new(
            field.x + label.len() as u16,
            field.y,
            field.width.saturating_sub(label.len() as u16),
            1,
        );
        frame.render_widget(Paragraph::new(label).style(accent(state)), field);
        let (text, cursor) = input_window(
            &state.search.query.text,
            state.search.query.cursor,
            content.width.saturating_sub(1) as usize,
        );
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(palette(state).text).not_bold()),
            content,
        );
        if state.focus == Focus::Search && !state.help && !state.hide_input_cursor {
            frame.set_cursor_position((content.x + cursor as u16, content.y));
        }
    }
}

fn render_input(frame: &mut Frame, state: &mut State, area: Rect) {
    let title = if state.focus == Focus::Range {
        " Mark episodes / 1-3, 5 "
    } else {
        " Search "
    };
    let title = if state.screen == Screen::Search && !state.details {
        let label = state.search.source.label();
        state.view.source_setting =
            Some(Rect::new(area.x + 11, area.y, label.width() as u16, 1).intersection(area));
        format!(" Search / {label} ")
    } else {
        title.to_owned()
    };
    let block = panel(
        format!(
            "{title}{}",
            if state.search.exact && state.focus != Focus::Range && state.screen != Screen::History
            {
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
        &state.episodes.range
    } else if state.screen == Screen::History {
        &state.history.query
    } else {
        &state.search.query
    };
    let (text, cursor) = input_window(
        &input.text,
        input.cursor,
        inner.width.saturating_sub(1) as usize,
    );
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(text), inner);
    if state.focus != Focus::List && !state.help && !state.hide_input_cursor {
        frame.set_cursor_position((inner.x + cursor as u16, inner.y));
    }
}

fn series_item(title: &str, metadata: &str, width: usize, state: &State) -> ListItem<'static> {
    let lines = wrap_prose(title, width);
    let mut content = vec![Line::styled(
        lines.first().cloned().unwrap_or_default(),
        Style::default().bold(),
    )];
    if lines.len() > 1 {
        content.push(Line::styled(
            if lines.len() == 2 {
                lines[1].clone()
            } else {
                ellipsize_start(title, width, state)
            },
            Style::default().bold(),
        ));
        content.push(Line::styled(
            ellipsize(metadata, width, state),
            muted(state),
        ));
    } else {
        content.push(Line::styled(
            ellipsize(metadata, width, state),
            muted(state),
        ));
        content.push(Line::raw(""));
    }
    ListItem::new(content)
}

fn render_list(frame: &mut Frame, state: &mut State, area: Rect) {
    let visible = state.visible();
    let (label, count, selected) = match state.screen {
        Screen::Home => return,
        Screen::Search => ("Results", visible.len(), state.search.selected),
        Screen::History => (
            "History",
            state.visible_history().len(),
            state.history.selected,
        ),
        Screen::Episodes => (
            "Episodes",
            state.episodes.items.len(),
            state.episodes.selected,
        ),
    };

    let mut title = if state.episodes.marked.is_empty() || state.screen != Screen::Episodes {
        format!(" {label} / {count} ")
    } else {
        format!(
            " {label} / {count} / {} marked ",
            state.episodes.marked.len()
        )
    };

    if state.loading && count > 0 {
        title.push_str(&format!(
            "/ {} {} ",
            scan(state),
            if state.screen == Screen::Search {
                "Searching"
            } else {
                "Loading"
            }
        ));
    }

    if state.request_error.is_some() && !state.request_blocks_list() {
        title.push_str("/ Cached ");
    }

    if state.screen == Screen::History && selected < count && !state.downloads_supported() {
        title.push_str("/ Playback only ");
    }
    let block = panel(title, state, state.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(error) = &state.request_error
        && state.request_blocks_list()
    {
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
        let title = if state.loading {
            if state.screen == Screen::Episodes {
                "Loading episodes..."
            } else {
                "Searching..."
            }
        } else {
            match state.screen {
                Screen::History if state.history.entries().is_empty() => "No history yet",
                Screen::Episodes => "No episodes available",
                Screen::Search if state.search.query.text.trim().is_empty() => "Enter a title",
                Screen::Search if state.search.exact => "No exact matches",
                _ => "No matches",
            }
        };
        let title = if state.loading {
            format!("{} {}", scan(state), title.trim_end_matches('.'))
        } else {
            title.to_owned()
        };
        empty_message(frame, state, inner, &title);
        return;
    }

    let item_width = usize::from(inner.width.saturating_sub(2));
    let playing = state
        .activity
        .as_ref()
        .is_some_and(|activity| {
            activity.kind == JobKind::Playback && activity.phase == JobPhase::Running
        })
        .then(|| state.current_episode_index())
        .flatten();
    let items: Vec<ListItem> = match state.screen {
        Screen::Home => vec![],
        Screen::History => state
            .visible_history()
            .iter()
            .copied()
            .map(|i| {
                let entry = &state.history.entries()[i];
                series_item(
                    &entry.anime.title,
                    &format!(
                        "Last played: Ep {} / {}",
                        entry.episode,
                        entry.mode.as_str().to_uppercase()
                    ),
                    item_width,
                    state,
                )
            })
            .collect(),
        Screen::Search => visible
            .iter()
            .map(|i| {
                let anime = &state.search.results()[*i];
                series_item(&anime.title, &anime.info, item_width, state)
            })
            .collect(),
        Screen::Episodes => state
            .episodes
            .items
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let indicator = if playing == Some(i) {
                    symbol(state, "▶", ">")
                } else if state.episodes.marked.contains(&i) {
                    symbol(state, "✓", "*")
                } else {
                    " "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(indicator, Style::default().fg(palette(state).success)),
                    Span::raw({
                        let prefix = format!(" {:>4}  ", ep.number);
                        format!(
                            "{prefix}{}",
                            ellipsize(
                                &ep.title,
                                item_width.saturating_sub(1 + prefix.width()),
                                state
                            )
                        )
                    }),
                ]))
            })
            .collect(),
    };

    let heights: Vec<_> = items.iter().map(ListItem::height).collect();
    let list = List::new(items)
        .highlight_symbol(if state.screen == Screen::Episodes {
            ""
        } else if state.ascii {
            "> "
        } else {
            "▸ "
        })
        .highlight_style(palette(state).selection);

    let mut selection = ListState::default().with_selected(Some(selected));
    *selection.offset_mut() = state.view.list_offsets[state.screen as usize];
    frame.render_stateful_widget(list, inner, &mut selection);
    state.view.list_offsets[state.screen as usize] = selection.offset();
    let mut y = inner.y;
    let mut rows = Vec::new();
    for (index, height) in heights.into_iter().enumerate().skip(selection.offset()) {
        let height = height as u16;
        if height > inner.bottom().saturating_sub(y) {
            break;
        }
        rows.push((Rect::new(inner.x, y, inner.width, height), index));
        y += height;
    }
    state.view.list = Some(crate::view::ListView { area: inner, rows });
}

fn render_cover_image(frame: &mut Frame, art: &Artwork, area: Rect) -> Option<Rect> {
    let protocol = art.protocol.as_ref()?;
    let size = protocol.size();
    if size.width > area.width || size.height > area.height {
        return None;
    }
    let image = Rect::new(
        area.x + (area.width - size.width) / 2,
        area.y + (area.height - size.height) / 2,
        size.width,
        size.height,
    );
    frame.render_widget(Image::new(protocol), image);
    Some(image)
}

fn render_cover_view(frame: &mut Frame, state: &State, art: &Artwork, area: Rect) {
    let anime = state.cover_view.as_ref().expect("cover viewer is open");
    frame.render_widget(
        Paragraph::new(ellipsize(
            &anime.title,
            area.width.saturating_sub(8) as usize,
            state,
        ))
        .style(accent(state)),
        Rect::new(area.x + 2, area.y, area.width.saturating_sub(8), 1),
    );
    frame.render_widget(
        Paragraph::new("[x]").style(accent(state)),
        Rect::new(area.right() - 4, area.y, 3, 1),
    );
    let cover = cover_area(state, area);
    if art.protocol.is_some() {
        render_cover_image(frame, art, cover);
    } else {
        empty_message(
            frame,
            state,
            cover,
            if art.error.is_some() {
                "Cover unavailable / R Retry"
            } else {
                "Loading cover..."
            },
        );
    }
}

fn render_preview(frame: &mut Frame, state: &mut State, art: &Artwork, area: Rect) {
    let block = panel(
        if state.details { "" } else { " Series " }.into(),
        state,
        state.details,
    );
    frame.render_widget(
        block.style(
            Style::default()
                .bg(palette(state).surface)
                .fg(palette(state).text),
        ),
        area,
    );

    let (cover, metadata) = if art.enabled {
        preview_regions(state, area)
    } else {
        let inner = area.inner(Margin::new(2, 1));
        let width = inner.width.min(72);
        (
            Rect::default(),
            Rect::new(
                inner.x + (inner.width - width) / 2,
                inner.y,
                width,
                inner.height,
            ),
        )
    };
    let Some(anime) = state.selected_anime() else {
        state.view.text_view = None;
        state.view.text_selection = None;
        empty_message(
            frame,
            state,
            area.inner(Margin::new(2, 1)),
            "Choose a series",
        );
        return;
    };

    let cover_hit = if art.protocol.is_some() {
        render_cover_image(frame, art, cover)
    } else if cover.height > 0 {
        let label = if !art.enabled {
            "Covers disabled"
        } else if art.error.is_some() {
            "Cover unavailable"
        } else if anime.cover.is_some() {
            "Loading cover..."
        } else {
            "No cover"
        };
        empty_message(frame, state, cover, label);
        None
    } else {
        None
    };

    let compact_details = state.details && metadata.height < 10;
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
        Screen::History => state.selected_history(),
        _ => state
            .history
            .entries()
            .iter()
            .find(|e| e.anime.same_source(anime) && e.mode == state.mode),
    };
    if let Some(entry) = entry
        && (state.details || state.screen != Screen::Home)
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
        && let Some(ep) = state.episodes.items.get(state.episodes.selected)
    {
        if !compact_details {
            push("", Style::default());
        }
        push(
            &format!("Episode {} / {}", ep.number, ep.title),
            accent(state),
        );
    }
    if !compact_details {
        push("", Style::default());
    }
    push("Synopsis", Style::default().bold());
    if !compact_details && state.details {
        push("", Style::default());
    }

    let synopsis = state.synopses.peek(&anime.source_id());
    let description = if let Some(Synopsis::Ready(text)) = synopsis {
        text.as_str()
    } else {
        anime.description.as_str()
    };
    let prose = synopsis_lines(
        if description.is_empty() {
            if matches!(synopsis, Some(Synopsis::Loading)) {
                ""
            } else {
                "No synopsis"
            }
        } else {
            description
        },
        metadata.width as usize,
        state,
    );
    details.extend(prose);

    let feedback = match synopsis {
        Some(Synopsis::Loading) => Some(("Loading full synopsis...", palette(state).pending)),
        Some(Synopsis::Failed) => Some(("! Synopsis unavailable", palette(state).error)),
        _ => None,
    };
    if let Some((message, color)) = feedback {
        if !compact_details {
            details.push(Line::raw(""));
        }
        details.extend(
            wrap_prose(message, metadata.width as usize)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(color))),
        );
    }

    state.view.cover = cover_hit;
    let text_lines: Vec<_> = details.iter().map(ToString::to_string).collect();
    if state
        .view
        .text_view
        .as_ref()
        .is_none_or(|view| view.area.width != metadata.width || view.lines != text_lines)
    {
        state.view.text_selection = None;
        state.view.preview_scroll = 0;
    }

    if state
        .view
        .text_view
        .as_ref()
        .is_some_and(|view| view.area.x != metadata.x || view.area.y != metadata.y)
    {
        state.view.text_selection = None;
    }
    let max_scroll = details
        .len()
        .saturating_sub(metadata.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = if state.details {
        &mut state.view.details_scroll
    } else {
        &mut state.view.preview_scroll
    };
    *scroll = (*scroll).min(max_scroll);
    let scroll = *scroll;

    let content_length = details.len();
    let offset = usize::from(scroll).min(text_lines.len());
    let height = usize::from(metadata.height).min(text_lines.len() - offset);
    state.view.text_view = (height > 0).then(|| TextView {
        area: Rect::new(metadata.x, metadata.y, metadata.width, height as u16),
        offset,
        lines: text_lines,
    });

    frame.render_widget(Paragraph::new(details).scroll((scroll, 0)), metadata);
    {
        if let (Some(view), Some(selection)) = (&state.view.text_view, state.view.text_selection) {
            let (start, end) = selection.range();
            for row in 0..view.area.height {
                let line = view.offset + usize::from(row);
                if line < start.line || line > end.line {
                    continue;
                }
                let from = if line == start.line { start.column } else { 0 };
                let to = if line == end.line {
                    end.column
                } else {
                    view.lines[line].width()
                };
                for column in from..to.min(view.lines[line].width()) {
                    frame.buffer_mut()[(view.area.x + column as u16, view.area.y + row)]
                        .set_style(palette(state).selection);
                }
            }
        }
        render_scrollbar(frame, state, metadata, content_length, scroll);
    }
}

fn render_scrollbar(frame: &mut Frame, state: &State, content: Rect, length: usize, scroll: u16) {
    if content.height == 0 || length <= usize::from(content.height) {
        return;
    }
    let mut position = ScrollbarState::new(length - usize::from(content.height) + 1)
        .position(usize::from(scroll))
        .viewport_content_length(usize::from(content.height));
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(symbol(state, "│", "|")))
            .thumb_symbol(symbol(state, "┃", "#"))
            .track_style(muted(state))
            .thumb_style(accent(state)),
        Rect::new(content.right(), content.y, 1, content.height),
        &mut position,
    );
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

fn synopsis_lines(text: &str, width: usize, state: &State) -> Vec<Line<'static>> {
    let mut body = text.trim();
    let mut credit = None;
    for (open, close) in [('[', ']'), ('(', ')')] {
        if body.ends_with(close)
            && let Some(start) = body.rfind(open)
        {
            let candidate = &body[start + 1..body.len() - close.len_utf8()];
            let label = candidate.to_ascii_lowercase();
            if label.starts_with("written by ") || label.starts_with("source:") {
                credit = Some(&body[start..]);
                body = body[..start].trim_end();
                break;
            }
        }
    }
    let paragraphs: Vec<_> = body
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut lines = Vec::new();
    for paragraph in &paragraphs {
        let normalized = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
        // Some providers flatten all paragraphs into one text node.
        let chunks = if paragraphs.len() == 1 && normalized.width() > 480 {
            let mut sentences: Vec<String> = Vec::new();
            for sentence in normalized.unicode_sentences() {
                let abbreviated = sentences.last().is_some_and(|previous| {
                    let word = previous.split_whitespace().last().unwrap_or("");
                    word.strip_suffix('.').is_some_and(|initial| {
                        initial.chars().count() == 1 && initial.chars().all(char::is_uppercase)
                    }) || matches!(
                        word,
                        "Mr." | "Mrs." | "Ms." | "Dr." | "Prof." | "St." | "Jr." | "Sr."
                    )
                });
                if abbreviated {
                    sentences.last_mut().unwrap().push_str(sentence);
                } else {
                    sentences.push(sentence.to_owned());
                }
            }
            sentences
                .chunks(2)
                .map(|sentences| sentences.concat().trim().to_owned())
                .collect::<Vec<_>>()
        } else {
            vec![normalized]
        };
        for chunk in chunks {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.extend(wrap_prose(&chunk, width).into_iter().map(Line::raw));
        }
    }
    if let Some(credit) = credit {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.extend(
            wrap_prose(credit, width)
                .into_iter()
                .map(|line| Line::styled(line, muted(state))),
        );
    }
    lines
}

fn render_activity(frame: &mut Frame, state: &State, area: Rect) {
    let Some(activity) = visible_activity(state) else {
        return;
    };
    if activity.kind == JobKind::Download && activity.phase != JobPhase::Failed {
        render_download_activity(frame, state, activity, area);
        return;
    }
    let phase = match activity.phase {
        JobPhase::Resolving => "Resolving",
        JobPhase::Running => "Playing",
        JobPhase::Finished => "Finished",
        JobPhase::Failed => "Failed",
    };
    let spinner = scan(state);
    let (indicator, color) = match activity.phase {
        JobPhase::Resolving => (spinner.as_str(), palette(state).pending),
        JobPhase::Running => (symbol(state, "▶", ">"), palette(state).success),
        JobPhase::Finished => (symbol(state, "✓", "+"), palette(state).success),
        JobPhase::Failed => ("!", palette(state).error),
    };
    let episode = &activity.episodes[activity.index];
    let compact = area.width < 60;
    let single_line = activity.kind == JobKind::Playback && activity.phase != JobPhase::Failed;
    let quality = if single_line && !activity.quality_note.is_empty() {
        format!(" / {}", activity.quality_note)
    } else {
        String::new()
    };
    let queue = if activity.remaining.is_empty() {
        String::new()
    } else if compact {
        format!(" / {} left", activity.remaining.len())
    } else {
        format!(" / {} queued", activity.remaining.len())
    };
    let width = usize::from(area.width.saturating_sub(1));
    let heading = if compact {
        let phase = match activity.phase {
            JobPhase::Resolving => "Resolve",
            JobPhase::Running => "Play",
            JobPhase::Finished => "Done",
            JobPhase::Failed => "Failed",
        };
        let metadata = format!(
            " Ep {} {}{quality}{queue}",
            episode.number,
            activity.mode.as_str().to_uppercase()
        );
        let phase = ellipsize(
            phase,
            width.saturating_sub(indicator.width() + 1 + metadata.width()),
            state,
        );
        let summary = format!("{indicator} {phase}{metadata}");
        let title_width = width.saturating_sub(summary.width() + 3);
        if title_width > 0 {
            format!(
                "{summary} / {}",
                ellipsize(&activity.anime.title, title_width, state)
            )
        } else {
            summary
        }
    } else {
        let prefix = format!("{indicator} {phase}: ");
        let suffix = format!(
            " / Ep {} / {}{quality}{queue}",
            episode.number,
            activity.mode.as_str().to_uppercase()
        );
        let title = ellipsize(
            &activity.anime.title,
            width.saturating_sub(prefix.width() + suffix.width()),
            state,
        );
        format!("{prefix}{title}{suffix}")
    };
    let message = if activity.quality_note.is_empty() || activity.phase == JobPhase::Failed {
        activity.message.clone()
    } else if activity.message.is_empty() {
        activity.quality_note.clone()
    } else {
        format!("{} / {}", activity.quality_note, activity.message)
    };
    let mut lines = vec![Line::raw(format!(" {heading}"))];
    if !single_line && !message.is_empty() {
        lines.push(Line::raw(format!(" {}", ellipsize(&message, width, state))));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(color)),
        area,
    );
}

fn render_download_activity(frame: &mut Frame, state: &State, activity: &Activity, area: Rect) {
    let finished = activity.phase == JobPhase::Finished;
    let indicator = if finished {
        symbol(state, "✓", "+").to_owned()
    } else {
        scan(state)
    };
    let width = usize::from(area.width.saturating_sub(1));
    let compact = area.width < 60;
    let separator = symbol(state, " · ", " / ");
    let queue = if activity.remaining.is_empty() {
        String::new()
    } else if compact {
        format!(" / {} left", activity.remaining.len())
    } else {
        format!("{separator}{} queued", activity.remaining.len())
    };
    let mut metadata = format!(
        "Ep {} {}",
        activity.episodes[activity.index].number,
        activity.mode.as_str().to_uppercase()
    );
    if !compact {
        if !activity.quality_note.is_empty() {
            metadata.push_str(&format!("{separator}{}", activity.quality_note));
        }
        if activity.phase == JobPhase::Running
            && let Some(time) = activity.message.strip_prefix("Downloaded through ")
        {
            let time = time.split('.').next().unwrap_or(time);
            let time = time.strip_prefix("00:").unwrap_or(time);
            metadata.push_str(&format!("{separator}{time}"));
        }
    }
    metadata.push_str(&queue);
    let label = if finished { "Saved " } else { "" };
    let title_width = width.saturating_sub(
        indicator.width() + 1 + label.width() + separator.width() + metadata.width(),
    );
    let title = if title_width > 0 {
        format!(
            "{}{separator}",
            ellipsize(&activity.anime.title, title_width, state)
        )
    } else {
        String::new()
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {indicator} "),
            Style::default().fg(if finished {
                palette(state).success
            } else {
                palette(state).pending
            }),
        ),
        Span::raw(format!("{label}{title}")),
        Span::styled(metadata, muted(state)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn ellipsize(text: &str, width: usize, state: &State) -> String {
    if text.width() <= width {
        return text.into();
    }
    let marker = symbol(state, "…", "...");
    if width < marker.width() {
        return ".".repeat(width);
    }
    let (prefix, _) = input_window(text, 0, width - marker.width());
    format!("{prefix}{marker}")
}

fn ellipsize_start(text: &str, width: usize, state: &State) -> String {
    if text.width() <= width {
        return text.into();
    }
    let marker = symbol(state, "…", "...");
    if width < marker.width() {
        return ".".repeat(width);
    }
    let mut used = marker.width();
    let mut start = text.len();
    for (i, grapheme) in text.grapheme_indices(true).rev() {
        if used + grapheme.width() > width {
            break;
        }
        used += grapheme.width();
        start = i;
    }
    let suffix = &text[start..];
    let suffix = if start > 0
        && !text[..start].ends_with(char::is_whitespace)
        && !suffix.starts_with(char::is_whitespace)
    {
        suffix
            .split_once(char::is_whitespace)
            .map_or(suffix, |(_, rest)| rest)
    } else {
        suffix.trim_start()
    };
    format!("{marker}{suffix}")
}

const HELP_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("Up/Down or k/j", "Move or scroll"),
            ("PgUp/PgDn", "Jump ten rows"),
            ("Home/End", "First or last row"),
            ("Enter", "Search, open a series or play an episode"),
            ("Esc", "Go back, close a panel or leave input"),
            ("Left/Right", "Browse back or forward"),
            ("i", "View series details"),
            ("y", "Copy panel text"),
            ("H", "Return Home"),
        ],
    ),
    (
        "Search & history",
        &[
            ("/", "Search or filter the current list"),
            ("Alt+p", "Change search provider"),
            ("a", "Find this title on the next provider"),
            ("Alt+e", "Toggle exact search"),
            ("Alt+h", "Open history"),
            ("Delete", "Remove the selected history entry"),
            ("u", "Undo the last removal"),
        ],
    ),
    (
        "Playback & downloads",
        &[
            ("Alt+a", "Switch SUB/DUB audio"),
            ("Alt+q", "Change quality"),
            ("Space", "Mark an episode"),
            (":", "Mark an episode range"),
            ("d", "Download marked or current episodes"),
            ("n/p/r", "Next episode / previous / replay"),
            ("x", "Stop or dismiss the queue"),
            ("R", "Refresh the current view"),
            ("Alt+r", "Retry a failed queue"),
        ],
    ),
    (
        "Settings",
        &[
            (",", "Open Settings"),
            ("Tab/Shift+Tab", "Move focus on Home"),
            ("Left/Right", "Change the focused value"),
            ("Enter", "Edit or save a field"),
            ("R", "Restore the selected default"),
            ("Delete", "Remove a player argument"),
        ],
    ),
    (
        "Text editing",
        &[
            ("Ctrl+Left/Right", "Previous / next word"),
            ("Ctrl+u/k", "Cut to start / end of line"),
            ("Ctrl+w", "Cut to the previous space"),
            ("Alt+Backspace/d", "Cut previous / next word"),
            ("Ctrl+y", "Restore last cut"),
        ],
    ),
    (
        "App controls",
        &[
            ("?", "Help (Esc › ? while typing)"),
            ("Ctrl+c", "Quit from anywhere"),
        ],
    ),
];

fn help_matches(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|term| text.contains(term))
}

fn help_key_aliases(keys: &str) -> String {
    let mut aliases = String::new();
    for group in keys.split(" / ") {
        let mut modifier = "";
        for key in group.split('/') {
            let key = key.trim();
            if let Some((prefix, _)) = key.rsplit_once('+') {
                modifier = &key[..prefix.len() + 1];
                aliases.push_str(key);
            } else {
                aliases.push_str(modifier);
                aliases.push_str(key);
            }
            aliases.push(' ');
        }
    }
    aliases
}

fn help_lines(state: &State, width: usize) -> Vec<Line<'static>> {
    let query = state.help_query.text.to_lowercase().replace('>', "›");
    let mut lines = Vec::new();
    let key_style = key_style(state);
    for (group, entries) in HELP_GROUPS {
        let group = if *group == "Playback & downloads" && !state.downloads_supported() {
            "Playback"
        } else {
            *group
        };
        let entries: Vec<_> = entries
            .iter()
            .filter(|(key, action)| {
                (*key != "d" || state.downloads_supported())
                    && help_matches(
                        &query,
                        &format!("{group} {key} {action} {}", help_key_aliases(key)),
                    )
            })
            .collect();
        if entries.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group, accent(state)));
        for (key, action) in entries {
            let action = action.replace('›', symbol(state, "›", ">"));
            if width >= 50 {
                for (index, description) in wrap_prose(&action, width - 20).into_iter().enumerate()
                {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:<20}", if index == 0 { key } else { "" }),
                            key_style,
                        ),
                        Span::styled(description, muted(state)),
                    ]));
                }
            } else {
                lines.extend(
                    wrap_prose(key, width)
                        .into_iter()
                        .map(|text| Line::styled(text, key_style)),
                );
                lines.extend(
                    wrap_prose(&action, width.saturating_sub(2))
                        .into_iter()
                        .map(|text| Line::styled(format!("  {text}"), muted(state))),
                );
            }
        }
    }
    if !state.downloads_supported() && help_matches(&query, "downloads playback only provider") {
        lines.push(Line::default());
        lines.extend(
            wrap_prose("Downloads are unavailable with this provider.", width)
                .into_iter()
                .map(|line| Line::styled(line, muted(state))),
        );
    }
    if lines.is_empty() {
        lines.push(Line::styled("No matching shortcuts", muted(state)));
    }
    lines
}

fn render_help(frame: &mut Frame, state: &mut State) {
    state.view.list = None;
    state.view.cover = None;
    state.view.source_setting = None;
    state.view.audio_setting = None;
    state.view.quality_setting = None;
    let area = frame.area();
    let base = Style::default()
        .fg(palette(state).text)
        .bg(palette(state).canvas);
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(base), area);
    let width = area
        .width
        .saturating_sub(if area.width >= 40 { 4 } else { 0 })
        .min(96);
    let margin_y = u16::from(area.height >= 20);
    let modal = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + margin_y,
        width,
        area.height.saturating_sub(margin_y * 2),
    );
    let block = panel(String::new(), state, true).style(base);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    let [header, filter, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    state.view.help_area = Some(body);
    frame.render_widget(
        Paragraph::new("Keybindings").style(Style::default().bold()),
        header,
    );
    let close = shortcut(
        state,
        "Esc",
        if state.help_filtering {
            "finish filter"
        } else {
            "close"
        },
    );
    if header.width >= 26 {
        let close_width = close.width() as u16;
        frame.render_widget(
            Paragraph::new(close).style(Style::default().bg(palette(state).surface)),
            Rect::new(header.right() - close_width, header.y, close_width, 1),
        );
    }
    if state.help_filtering || !state.help_query.text.is_empty() {
        let (query, cursor) = input_window(
            &state.help_query.text,
            state.help_query.cursor,
            usize::from(filter.width.saturating_sub(3)),
        );
        frame.render_widget(
            Paragraph::new(format!("/ {query}")).style(accent(state)),
            filter,
        );
        if state.help_filtering && filter.width > 2 && filter.height > 0 && !state.hide_input_cursor
        {
            frame.set_cursor_position((filter.x + 2 + cursor as u16, filter.y));
        }
    } else {
        frame.render_widget(
            Paragraph::new(shortcut(state, "/", "Filter by key or action")),
            filter,
        );
    }
    let lines = help_lines(state, usize::from(body.width));
    let content_length = lines.len();
    state.view.help_scroll = state.view.help_scroll.min(
        content_length
            .saturating_sub(usize::from(body.height))
            .min(usize::from(u16::MAX)) as u16,
    );
    frame.render_widget(
        Paragraph::new(lines).scroll((state.view.help_scroll, 0)),
        body,
    );
    render_scrollbar(frame, state, body, content_length, state.view.help_scroll);
}

fn empty_message(frame: &mut Frame, state: &State, area: Rect, title: &str) {
    let caption = Paragraph::new(title)
        .style(accent(state))
        .centered()
        .wrap(Wrap { trim: true });
    let height = caption.line_count(area.width) as u16;
    let y = area.y + area.height.saturating_sub(height) / 2;
    frame.render_widget(caption, Rect::new(area.x, y, area.width, area.bottom() - y));
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
    use crate::state::Activity;
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
    };
    use ratatui_image::picker::Picker;

    #[test]
    fn disabled_covers_reclaim_space_for_the_selected_series() {
        for (width, height) in [(32, 12), (40, 16), (80, 24), (120, 40)] {
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
                    id: "series".into(),
                    title: "A series".into(),
                    description: "A synopsis".into(),
                    ..Default::default()
                }]),
            );
            state.focus = Focus::List;
            let text = render_text(&mut state, width, height);
            assert!(text.contains("A series"), "{text}");
            state.details = true;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
            terminal.draw(|f| render(f, &mut state, &art)).unwrap();
            let view = state.view.text_view.as_ref().unwrap();
            let body = panels(&state, Rect::new(0, 0, width, height)).1;
            assert_eq!(view.area.y, body.y + 1);
            assert_eq!(view.area.width, body.width.saturating_sub(4).min(72));
            assert!(state.view.cover.is_none());
        }
    }

    #[test]
    fn download_actions_follow_episode_provider_not_search_source() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(Anime {
            provider: crate::provider::Source::AnimeGg.id(),
            title: "Probe".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: crate::provider::Source::AnimeGg.id(),
            id: "one".into(),
            number: "1".into(),
            title: "One".into(),
        }];
        for width in [32, 40, 80, 120] {
            let text = render_text(&mut state, width, 24);
            assert!(!text.contains("Download"), "{text}");
            assert!(text.contains("Playback only"), "{text}");
            let footer = text.lines().last().unwrap();
            assert!(
                footer.contains("Playback only") && footer.ends_with("? Help"),
                "{text}"
            );
        }
        assert!(
            !help_lines(&state, 80)
                .iter()
                .any(|line| line.to_string().contains("Download marked"))
        );
        state.search.source = crate::provider::Source::AnimeGg;
        state.episodes.anime.as_mut().unwrap().provider = crate::provider::Source::HiAnime.id();
        assert!(!render_text(&mut state, 80, 24).contains("Playback only"));
        assert!(
            help_lines(&state, 80)
                .iter()
                .any(|line| line.to_string().contains("Download marked"))
        );
    }

    #[test]
    fn history_shows_remove_only_for_a_selected_list_entry() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::History;
        state.focus = Focus::List;
        assert!(!render_text(&mut state, 32, 12).contains("Delete Remove"));

        state
            .history
            .replace_entries(vec![crate::persistence::Entry {
                anime: Anime {
                    provider: crate::provider::Source::AnimeGg.id(),
                    title: "Mushi-Shi".into(),
                    ..Default::default()
                },
                episode: "1".into(),
                mode: Mode::Sub,
                watched_at: 1,
            }]);
        for width in [32, 100] {
            let text = render_text(&mut state, width, 12);
            assert!(text.contains("Mushi-Shi"), "{text}");
            assert_eq!(text.matches("Delete Remove").count(), 1, "{text}");
            assert!(
                text.lines().last().unwrap().starts_with("Delete Remove"),
                "{text}"
            );
            assert!(
                text.contains("Playback only") && text.ends_with("? Help"),
                "{text}"
            );
        }
        state.focus = Focus::Search;
        assert!(!render_text(&mut state, 32, 12).contains("Delete Remove"));
        state.focus = Focus::List;
        state.details = true;
        assert!(!render_text(&mut state, 32, 12).contains("Delete Remove"));
        state.details = false;
        state.history.query.set("missing".into());
        assert!(!render_text(&mut state, 32, 12).contains("Delete Remove"));
    }

    #[test]
    fn history_feedback_shares_the_footer_with_undo_without_shifting_the_list() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::History;
        state.focus = Focus::List;
        let entry = crate::persistence::Entry {
            anime: Anime {
                title: "Cowboy Bebop".into(),
                ..Default::default()
            },
            episode: "1".into(),
            mode: Mode::Sub,
            watched_at: 1,
        };
        state.history_removal = Some(HistoryRemoval::Removed(entry.clone()));
        state.confirm_history_change("Removed", &entry.anime.title);
        for width in [32, 100] {
            let text = render_text(&mut state, width, 12);
            let footer = text.lines().last().unwrap();
            assert!(
                footer.starts_with("Removed Cowboy") && footer.contains("  u Undo"),
                "{text}"
            );
            assert!(
                footer.ends_with("? Help") && text.contains("No history yet"),
                "{text}"
            );
            assert_eq!(sections(&state, Rect::new(0, 0, width, 12))[3].height, 0);
        }
        state.history.notice = None;
        assert!(
            render_text(&mut state, 32, 12)
                .lines()
                .last()
                .unwrap()
                .starts_with("u Undo")
        );
        state.focus = Focus::Search;
        assert!(!render_text(&mut state, 32, 12).contains("u Undo"));
        state.focus = Focus::List;
        state.history_removal = None;
        state.confirm_history_change("Restored", &entry.anime.title);
        state.history.replace_entries(vec![entry]);
        let text = render_text(&mut state, 32, 12);
        assert!(
            text.lines()
                .last()
                .unwrap()
                .starts_with("Restored Cowboy Bebop"),
            "{text}"
        );
        assert!(!text.contains("u Undo"));
    }

    #[test]
    fn provider_selector_is_visible_and_clickable_on_compact_and_wide_search() {
        for source in [
            crate::provider::Source::HiAnime,
            crate::provider::Source::AnimeGg,
            crate::provider::Source::Kaa,
        ] {
            for screen in [Screen::Home, Screen::Search] {
                for width in [32, 40, 80, 119, 120, 160] {
                    let mut state = State::new(
                        "Probe".into(),
                        Mode::Dub,
                        "1080p".into(),
                        false,
                        false,
                        false,
                    );
                    state.screen = screen;
                    state.search.source = source;
                    let text = render_text(&mut state, width, 24);
                    let area = state.view.source_setting.expect("provider selector");
                    let visible: String = text
                        .lines()
                        .nth(area.y as usize)
                        .unwrap()
                        .chars()
                        .skip(area.x as usize)
                        .take(area.width as usize)
                        .collect();
                    assert!(
                        visible.trim() == source.label(),
                        "{width} {screen:?}: {visible:?}\n{text}"
                    );
                    assert!(text.contains("DUB") && text.contains("1080p"), "{text}");
                }
            }
        }
    }

    fn render_text(state: &mut State, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), true);
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
    fn setting_hit_areas_match_visible_labels_across_layouts() {
        for screen in [
            Screen::Home,
            Screen::Search,
            Screen::History,
            Screen::Episodes,
        ] {
            for width in [32, 40, 80, 120] {
                for focus in [Focus::Search, Focus::List] {
                    for details in [false, true] {
                        let mut state = State::new(
                            String::new(),
                            Mode::Sub,
                            "best".into(),
                            false,
                            false,
                            false,
                        );
                        state.screen = screen;
                        state.focus = focus;
                        state.details = details;
                        for quality in ["best", "1080p"] {
                            state.quality = quality.into();
                            let text = render_text(&mut state, width, 24);
                            for (area, label) in [
                                (state.view.audio_setting.unwrap(), "sub"),
                                (state.view.quality_setting.unwrap(), quality),
                            ] {
                                let visible: String = text
                                    .lines()
                                    .nth(area.y as usize)
                                    .unwrap()
                                    .chars()
                                    .skip(area.x as usize)
                                    .take(area.width as usize)
                                    .collect();
                                assert!(
                                    visible.to_lowercase().contains(label),
                                    "{screen:?}/{focus:?}/{details}/{width}: {visible:?} should contain {label}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scan_sweeps_and_respects_terminal_modes() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.animate = true;
        for step in 0..24 {
            state.frame = step;
            let frame = scan(&state);
            assert_eq!(frame.width(), 7);
            let head = frame.chars().position(|c| c == '█').unwrap();
            assert_eq!(
                head,
                if step % 12 <= 6 {
                    step % 12
                } else {
                    12 - step % 12
                }
            );
        }
        state.animate = false;
        let still = scan(&state);
        state.frame += 5;
        assert_eq!(scan(&state), still);
        state.ascii = true;
        assert!(scan(&state).is_ascii());
    }

    #[test]
    fn episode_row_and_playback_status_are_compact() {
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
        state.episodes.anime = Some(Anime {
            id: "series".into(),
            title: "Cowboy Bebop".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "1".into(),
            number: "1".into(),
            title: "Asteroid Blues".into(),
        }];
        state.episodes.marked.insert(0);
        state.activity = Some(Activity {
            anime: state.episodes.anime.clone().unwrap(),
            episodes: state.episodes.items.clone(),
            index: 0,
            remaining: Default::default(),
            mode: Mode::Sub,
            quality: "1080p".into(),
            quality_note: "1080p".into(),
            kind: JobKind::Playback,
            phase: JobPhase::Running,
            message: String::new(),
        });
        let text = render_text(&mut state, 140, 40);
        let row = text.lines().find(|l| l.contains("Asteroid Blues")).unwrap();
        assert!(row.contains('▶') && !row.contains('✓') && !row.contains('▸'));
        let status = text.lines().find(|l| l.contains("Playing:")).unwrap();
        assert!(status.contains("1080p") && status.contains("SUB"));
        assert_eq!(activity_height(&state), 1);
        let footer = text.lines().last().unwrap();
        assert!(footer.contains("Space Mark   : Range   d Download") && footer.ends_with("? Help"));
    }

    #[test]
    fn responsive_screens_keep_help_and_cover_inside_terminal() {
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
                state.episodes.anime = Some(Anime {
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
                        assert!(text.contains("? Help"));
                    } else {
                        assert!(text.contains("Resize"));
                    }
                }
            }
        }
    }

    #[test]
    fn miru_home_keeps_search_settings_and_cursor_visible_at_supported_sizes() {
        for (width, height) in [(32, 12), (40, 18), (80, 24), (160, 50)] {
            for ascii in [false, true] {
                for focus in [Focus::Search, Focus::Audio, Focus::Quality, Focus::List] {
                    let mut state = State::new(
                        String::new(),
                        Mode::Dub,
                        "1080p".into(),
                        false,
                        ascii,
                        false,
                    );
                    state.focus = focus;
                    {
                        let mut entries = state.history.entries().to_vec();
                        entries.push(crate::persistence::Entry {
                            anime: Anime {
                                title: "History stays in History".into(),
                                ..Default::default()
                            },
                            episode: "1".into(),
                            mode: Mode::Sub,
                            watched_at: 1,
                        });
                        state.history.replace_entries(entries);
                    }
                    let text = render_text(&mut state, width, height);
                    for label in ["Search anime...", "DUB", "1080p", "? Help"] {
                        assert!(text.contains(label), "{width}x{height} {focus:?}: {text}");
                    }
                    let hint = match (focus, ascii) {
                        (Focus::Search, true) => "Esc > ? Help",
                        (Focus::Search, false) => "Esc › ? Help",
                        _ => "? Help",
                    };
                    assert_eq!(
                        text.lines().last().unwrap().trim(),
                        format!("Alt+h History  {hint}")
                    );
                    assert!(!text.contains("History stays in History"));
                    assert!(!text.contains("Ctrl+c Quit"));
                    assert!(!text.contains("/ Search  s Audio  Q Quality"));
                    assert!(state.view.text_view.is_none());
                    assert_eq!(
                        cover_area(&state, Rect::new(0, 0, width, height)),
                        Rect::default()
                    );
                    state.search.query.set("日本語 e\u{301} query ".repeat(20));
                    state.search.exact = true;
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
                    terminal.draw(|f| render(f, &mut state, &art)).unwrap();
                    if focus == Focus::Search {
                        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
                        assert!(cursor.x < width - 2 && cursor.y < height - 2);
                    }
                }
            }
        }
    }

    #[test]
    fn miru_settings_focus_survives_no_color_and_long_quality_values() {
        for colorless in [false, true] {
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.colorless = colorless;
            let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
            let mut terminal = Terminal::new(TestBackend::new(32, 12)).unwrap();
            for (focus, expected) in [(Focus::Audio, " SUB "), (Focus::Quality, " Best ")] {
                state.focus = focus;
                terminal.draw(|f| render(f, &mut state, &art)).unwrap();
                let focused: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .filter(|cell| cell.modifier.contains(ratatui::style::Modifier::UNDERLINED))
                    .map(|cell| cell.symbol())
                    .collect();
                assert_eq!(focused, expected);
            }
            state.quality = "日本語".repeat(100);
            let text = render_text(&mut state, 32, 12);
            assert!(
                text.contains("SUB")
                    && text.contains('日')
                    && text.contains('語')
                    && text.contains("Search anime..."),
                "{text}"
            );
        }
    }

    #[test]
    fn help_keeps_the_desktop_canvas_and_text_after_clearing() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.palette = Palette::themed(
            &crate::theme::Theme::parse(
                "background = '#faf0e0'\nforeground = '#202030'\naccent = '#604080'",
            )
            .unwrap(),
            false,
        );
        state.help = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
        terminal.draw(|f| render(f, &mut state, &art)).unwrap();
        let cell = &terminal.backend().buffer()[(4, 2)];
        assert_eq!(cell.bg, state.palette.canvas);
        assert_eq!(cell.fg, state.palette.text);
    }

    #[test]
    fn wide_previews_prioritize_reading_and_keep_art_outside_selectable_text() {
        let anime = Anime {
            id: "series-1".into(),
            title: "A series with a long title and a second season".into(),
            info: "TV / 24m / SUB 24 / DUB 12".into(),
            description: "A readable synopsis with room below the title.".into(),
            ..Default::default()
        };
        let mut state = State::new(
            "series".into(),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.accept_search(0, Ok(vec![anime.clone()]));
        state.focus = Focus::List;
        {
            let mut entries = state.history.entries().to_vec();
            entries.push(crate::persistence::Entry {
                anime,
                episode: "1".into(),
                mode: Mode::Sub,
                watched_at: 1,
            });
            state.history.replace_entries(entries);
        }
        for screen in [Screen::Search, Screen::History] {
            state.screen = screen;
            for (w, h) in [(120, 40), (180, 45)] {
                let text = render_text(&mut state, w, h);
                let area = Rect::new(0, 0, w, h);
                let (list, preview) = panels(&state, area);
                let cover = cover_area(&state, area);
                let reading = state.view.text_view.as_ref().unwrap().area;
                assert!(preview.width > list.width);
                assert!(cover.width <= 20);
                assert_eq!(cover.y, reading.y);
                assert!(cover.right() < reading.x);
                assert!((50..=72).contains(&reading.width));
                assert!(text.contains("Synopsis"));
                assert!(!text.contains("Enter Episodes"));
            }
        }
        state.screen = Screen::Search;
        let text = render_text(&mut state, 78, 18);
        assert!(text.contains("A readable synopsis"));
        assert_eq!(cover_area(&state, Rect::new(0, 0, 78, 18)), Rect::default());
    }

    #[test]
    fn episode_lists_show_mark_range_and_download_together_at_all_sizes() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(Anime {
            title: "A series".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        for (w, h) in [(32, 12), (40, 18), (52, 12), (71, 24), (72, 12), (80, 24)] {
            for marked in [false, true] {
                state.episodes.marked.clear();
                if marked {
                    state.episodes.marked.insert(0);
                }
                let text = render_text(&mut state, w, h);
                assert!(text.contains("The beginning"), "{w}x{h}: {text}");
                assert_eq!(text.contains("1 marked"), marked, "{text}");
                let mut footer = text.lines().rev();
                let last = footer.next().unwrap();
                assert!(last.ends_with("? Help"), "{text}");
                let controls = if w == 32 {
                    footer.next().unwrap()
                } else {
                    last
                };
                let words: Vec<_> = controls.split_whitespace().take(6).collect();
                assert_eq!(
                    words,
                    ["Space", "Mark", ":", "Range", "d", "Download"],
                    "{text}"
                );
                for hint in ["i Details", "Enter Play"] {
                    assert!(!text.contains(hint), "{w}x{h}: {text}");
                }
            }
        }
        state.focus = Focus::Range;
        assert!(!render_text(&mut state, 80, 24).contains("Space Mark"));
        state.focus = Focus::List;
        state.details = true;
        assert!(!render_text(&mut state, 80, 24).contains("d Download"));
        state.details = false;
        state.screen = Screen::Search;
        state.search.query.set("series".into());
        state.accept_search(0, Ok(vec![state.episodes.anime.clone().unwrap()]));
        let text = render_text(&mut state, 32, 12);
        assert_eq!(text.lines().last().unwrap().trim(), "? Help");
        assert!(
            !text.contains("/ Edit") && !text.contains("i Details"),
            "{text}"
        );
    }

    #[test]
    fn activity_keeps_episode_queue_and_failure_reason_visible() {
        for ascii in [false, true] {
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, ascii, false);
            state.activity = Some(Activity {
                anime: Anime {
                    title: "A very long 日本語 series title that would otherwise hide the queue"
                        .into(),
                    ..Default::default()
                },
                episodes: (1..=3)
                    .map(|n| Episode {
                        provider: Default::default(),
                        id: n.to_string(),
                        number: n.to_string(),
                        title: String::new(),
                    })
                    .collect(),
                index: 1,
                remaining: [2].into(),
                mode: Mode::Dub,
                quality: "720p".into(),
                quality_note: String::new(),
                kind: JobKind::Download,
                phase: JobPhase::Failed,
                message: format!(
                    "File already exists: /{}/episode.mkv",
                    "long-directory/".repeat(10)
                ),
            });
            for phase in [JobPhase::Resolving, JobPhase::Running, JobPhase::Failed] {
                state.activity.as_mut().unwrap().phase = phase;
                for (w, h) in [(32, 12), (40, 18), (80, 24), (120, 40)] {
                    let text = render_text(&mut state, w, h);
                    for label in ["Ep 2", "DUB"] {
                        assert!(
                            text.contains(label),
                            "{phase:?} {w}x{h}, missing {label}: {text}"
                        );
                    }
                    assert!(
                        text.contains(if w < 60 { "1 left" } else { "1 queued" }),
                        "{text}"
                    );
                    assert!(!text.contains("Stop/Dismiss"), "{text}");
                    assert!(!text.contains("Esc Controls"), "{text}");
                    if phase == JobPhase::Failed {
                        assert!(text.contains("File already exists"), "{text}");
                        assert!(text.contains("Alt+r Retry"), "{text}");
                    } else {
                        assert_eq!(activity_height(&state), 1);
                        assert!(!text.contains("File already exists"), "{text}");
                    }
                }
            }
            {
                let mut entries = state.history.entries().to_vec();
                entries.push(crate::persistence::Entry {
                    anime: state.activity.as_ref().unwrap().anime.clone(),
                    episode: "1".into(),
                    mode: Mode::Sub,
                    watched_at: 1,
                });
                state.history.replace_entries(entries);
            }
            state.status = "Audio applies to the next queue.".into();
            state.focus = Focus::Audio;
            let text = render_text(&mut state, 32, 12);
            for label in ["Search anime...", "Ep 2", "Alt+r Retry queue"] {
                assert!(text.contains(label), "{text}");
            }
            state.status.clear();
            let activity = state.activity.as_mut().unwrap();
            activity.episodes = (1..=1400)
                .map(|n| Episode {
                    provider: Default::default(),
                    id: n.to_string(),
                    number: n.to_string(),
                    title: String::new(),
                })
                .collect();
            activity.index = 1233;
            activity.remaining = (1234..1400).collect();
            activity.phase = JobPhase::Running;
            let text = render_text(&mut state, 32, 12);
            for label in ["Ep 1234", "DUB", "166 left"] {
                assert!(text.contains(label), "{text}");
            }
            state.activity.as_mut().unwrap().quality_note = "720p (1080p unavailable)".into();
            state.activity.as_mut().unwrap().message = "Saved /tmp/episode.mkv".into();
            state.activity.as_mut().unwrap().phase = JobPhase::Finished;
            state.screen = Screen::Episodes;
            state.episodes.anime = Some(state.activity.as_ref().unwrap().anime.clone());
            for (w, h) in [(32, 12), (80, 24)] {
                let text = render_text(&mut state, w, h);
                if w >= 60 {
                    assert!(text.contains("720p (1080p unavailable)"), "{text}");
                }
                state.help = true;
                state.view.help_scroll = u16::MAX;
                let text = render_text(&mut state, w, h);
                assert!(!text.contains("1080p unavailable"), "{text}");
                assert!(!text.contains("Saved /tmp/episode.mkv"), "{text}");
                state.help = false;
            }
        }
    }

    #[test]
    fn downloads_use_one_scan_line_and_keep_saved_confirmation_on_its_episode_screen() {
        for ascii in [false, true] {
            let mut state =
                State::new(String::new(), Mode::Sub, "1080p".into(), false, ascii, true);
            let anime = Anime {
                id: "mushi-shi".into(),
                title: "Mushi-Shi".into(),
                ..Default::default()
            };
            state.activity = Some(Activity {
                anime: anime.clone(),
                episodes: (1..=3)
                    .map(|n| Episode {
                        provider: Default::default(),
                        id: n.to_string(),
                        number: n.to_string(),
                        title: String::new(),
                    })
                    .collect(),
                index: 0,
                remaining: [1, 2].into(),
                mode: Mode::Sub,
                quality: "1080p".into(),
                quality_note: "1080p".into(),
                kind: JobKind::Download,
                phase: JobPhase::Running,
                message: "Downloaded through 00:02:02.38".into(),
            });
            for animate in [false, true] {
                state.animate = animate;
                for frame in [0, 1, 6, 11] {
                    state.frame = frame;
                    for width in [32, 40, 59, 60, 80, 120] {
                        let text = render_text(&mut state, width, 24);
                        let status = text.lines().find(|line| line.contains("Ep 1 SUB")).unwrap();
                        assert!(status.contains(&scan(&state)), "{text}");
                        assert!(status.contains(if width < 60 { "2 left" } else { "2 queued" }));
                        if width >= 60 {
                            assert!(
                                status.contains("1080p") && status.contains("02:02"),
                                "{text}"
                            );
                        }
                        assert_eq!(activity_height(&state), 1);
                        assert!(
                            !text.contains("Downloading:") && !text.contains("Downloaded through")
                        );
                    }
                }
            }
            for history in [false, true] {
                state.history.replace_entries(if history {
                    vec![crate::persistence::Entry {
                        anime: anime.clone(),
                        episode: "1".into(),
                        mode: Mode::Sub,
                        watched_at: 1,
                    }]
                } else {
                    vec![]
                });
                for width in [80, 120, 180] {
                    for phase in [JobPhase::Resolving, JobPhase::Running] {
                        state.activity.as_mut().unwrap().phase = phase;
                        let text = render_text(&mut state, width, 24);
                        let footer = text.lines().last().unwrap();
                        assert!(footer.ends_with("? Help"), "{text}");
                        assert!(footer.contains("Alt+h History"), "{text}");
                        assert_eq!(text.matches("Ep 1 SUB").count(), 1, "{text}");
                        let progress = text.lines().find(|line| line.contains("Ep 1 SUB")).unwrap();
                        assert!(
                            progress.contains(&scan(&state)) && progress.contains("2 queued"),
                            "{text}"
                        );
                        assert_eq!(footer.contains("Ep 1 SUB"), width >= 120, "{text}");
                    }
                }
            }
            let activity = state.activity.as_mut().unwrap();
            activity.phase = JobPhase::Finished;
            activity.remaining.clear();
            activity.message = "Saved /downloads/Mushi-Shi _ Episode 1 SUB.mkv".into();
            state.screen = Screen::Episodes;
            state.episodes.anime = Some(anime);
            for width in [32, 80, 120] {
                let text = render_text(&mut state, width, 24);
                let status = text.lines().find(|line| line.contains("Saved")).unwrap();
                assert!(status.contains("Ep 1 SUB"), "{text}");
                assert!(
                    !text.contains(".mkv") && !text.contains("Finished:"),
                    "{text}"
                );
                assert_eq!(activity_height(&state), 1);
            }
            for screen in [Screen::Home, Screen::Search, Screen::History] {
                state.screen = screen;
                let text = render_text(&mut state, 80, 24);
                assert!(
                    !text.contains("Saved") && !text.contains("x Dismiss"),
                    "{text}"
                );
                assert_eq!(activity_height(&state), 0);
            }
            state.screen = Screen::Episodes;
            state.details = true;
            assert_eq!(activity_height(&state), 0);
            state.details = false;
            state.episodes.anime.as_mut().unwrap().id = "other-series".into();
            assert_eq!(activity_height(&state), 0);
        }
    }

    #[test]
    fn help_finds_individual_shortcut_aliases() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        for (query, expected) in [
            ("Ctrl+k", "Cut to start / end of line"),
            ("Alt+d", "Cut previous / next word"),
            ("Ctrl+Right", "Previous / next word"),
            ("F2", "No matching shortcuts"),
            ("Ctrl+p", "No matching shortcuts"),
        ] {
            state.help_query.set(query.into());
            let text = help_lines(&state, 92)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains(expected), "{query}: {text}");
        }
    }

    #[test]
    fn help_filters_keys_actions_and_groups_at_all_sizes() {
        for (width, height) in [(32, 12), (80, 24), (160, 50)] {
            let mut state =
                State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
            state.help = true;
            state.help_filtering = true;
            for (query, expected) in [
                ("sUb/DuB", "Alt+a"),
                ("Ctrl+y", "Restore last cut"),
                ("playback quality", "Change quality"),
            ] {
                state.help_query.set(query.into());
                state.view.help_scroll = u16::MAX;
                let text = render_text(&mut state, width, height);
                assert!(text.contains(expected), "{text}");
                assert!(
                    text.contains("Keybindings") && text.contains("Esc finish filter"),
                    "{text}"
                );
                assert!(!text.contains("Terminal options"));
                assert_eq!(state.view.help_scroll, 0);
            }
            state.help_query.set("no-such-shortcut".into());
            assert!(render_text(&mut state, width, height).contains("No matching shortcuts"));
        }
    }

    #[test]
    fn help_reaches_last_shortcuts_and_clamps_after_resize() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.help = true;
        state.view.help_scroll = u16::MAX;
        let text = render_text(&mut state, 32, 12);
        assert!(text.contains("Quit from anywhere"), "{text}");
        assert!(state.view.help_scroll > 40 && state.view.help_scroll < u16::MAX);
        let text = render_text(&mut state, 120, 80);
        assert!(text.contains("App controls"));
        for removed in [
            "Terminal options",
            "Printable keys",
            "Browsing keeps",
            "History records",
            "Diagnostics",
            "Covers:",
        ] {
            assert!(!text.contains(removed), "{text}");
        }
        assert_eq!(state.view.help_scroll, 0);
    }

    #[test]
    fn long_titles_keep_season_suffixes_visible_in_results() {
        let title = "As a Reincarnated Aristocrat, I’ll Use My Appraisal Skill";
        for ascii in [false, true] {
            let mut state = State::new(
                "appraisal".into(),
                Mode::Sub,
                "best".into(),
                false,
                ascii,
                false,
            );
            state.accept_search(
                0,
                Ok(vec![
                    Anime {
                        id: "first".into(),
                        title: title.into(),
                        info: "TV / SUB 24".into(),
                        ..Default::default()
                    },
                    Anime {
                        id: "second".into(),
                        title: format!("{title} Season 2"),
                        info: "TV / SUB 12".into(),
                        ..Default::default()
                    },
                ]),
            );
            state.focus = Focus::List;
            for width in [117, 80, 52, 32] {
                state.search.selected = 0;
                let text = render_text(&mut state, width, 28);
                assert!(text.contains("Season 2"), "{width}: {text}");
                assert!(text.contains("TV / SUB 12"));
                let mut results = state.search.results().to_vec();
                results[1].title = format!(
                    "{} Season 2",
                    "A very long 日本語 title e\u{301} ".repeat(15)
                );
                state.search.replace_results(results);
                let text = render_text(&mut state, width, 28);
                assert!(text.contains("Season 2"), "{width}: {text}");
                assert!(text.contains(if ascii { "..." } else { "…" }));
                let mut results = state.search.results().to_vec();
                results[1].title = format!("{title} Season 2");
                state.search.replace_results(results);
            }
        }
    }

    #[test]
    fn resizing_keeps_a_single_footer_line_and_hides_finished_playback() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.focus = Focus::List;
        state.episodes.anime = Some(Anime {
            id: "series".into(),
            title: "Series".into(),
            description: "A synopsis.".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "1".into(),
            number: "1".into(),
            title: "Opening".into(),
        }];
        state.activity = Some(Activity {
            anime: state.episodes.anime.clone().unwrap(),
            episodes: state.episodes.items.clone(),
            index: 0,
            remaining: Default::default(),
            mode: Mode::Sub,
            quality: "best".into(),
            quality_note: String::new(),
            kind: JobKind::Playback,
            phase: JobPhase::Finished,
            message: String::new(),
        });
        for width in [120, 133, 160] {
            state.view.text_view = None;
            let text = render_text(&mut state, width, 40);
            let footer = text.lines().last().unwrap();
            assert!(
                footer.contains("Space Mark   : Range   d Download") && footer.ends_with("? Help")
            );
            assert_eq!(sections(&state, Rect::new(0, 0, width, 40))[5].height, 1);
            assert!(!text.contains("Finished"), "{text}");
            assert!(!text.contains("x Dismiss"), "{text}");
            assert_eq!(sections(&state, Rect::new(0, 0, width, 40))[4].height, 0);
        }
    }

    #[test]
    fn wide_search_reclaims_rows_and_keeps_editing_cursor_in_query() {
        let mut state = State::new(
            "日本語 ".repeat(30),
            Mode::Sub,
            "best".into(),
            false,
            false,
            false,
        );
        state.focus = Focus::Search;
        state.search.exact = true;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
        terminal.draw(|f| render(f, &mut state, &art)).unwrap();
        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
        assert_eq!(cursor.y, 0);
        assert!(cursor.x > 20 && cursor.x < 108, "{cursor:?}");
        assert_eq!(sections(&state, Rect::new(0, 0, 120, 30))[1].height, 0);
        assert_eq!(sections(&state, Rect::new(0, 0, 119, 30))[1].height, 3);
        let text = render_text(&mut state, 120, 30);
        let top = text.lines().next().unwrap();
        assert!(
            top.contains("[EXACT]") && top.contains("SUB / Best"),
            "{text}"
        );
        assert!(!top.contains("H Home") && !top.contains("Q Best"));
        assert!(!text.contains("Enter Results") && !text.contains("Esc Done"));
        state.focus = Focus::List;
        let text = render_text(&mut state, 120, 30);
        let top = text.lines().next().unwrap();
        assert!(top.contains("SUB / Best") && !top.contains("Q Best"));
    }

    #[test]
    fn minimum_size_headers_keep_language_and_quality_visible() {
        for screen in [Screen::Search, Screen::Episodes, Screen::History] {
            for details in [false, true] {
                for quality in ["best", "1080p", "720p", "480p", "360p", "worst"] {
                    let mut state = State::new(
                        String::new(),
                        Mode::Dub,
                        quality.into(),
                        false,
                        false,
                        false,
                    );
                    state.screen = screen;
                    state.details = details;
                    let text = render_text(&mut state, 32, 12);
                    assert!(
                        text.lines()
                            .next()
                            .unwrap()
                            .contains(&format!("DUB / {}", state.quality_label())),
                        "{screen:?}, {details}: {text}"
                    );
                }
            }
        }
    }

    #[test]
    fn compact_details_show_synopsis_without_blank_separators() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.details = true;
        let anime = Anime {
            id: "series".into(),
            title: "As a Reincarnated Aristocrat, I’ll Use My Appraisal Skill".into(),
            info: "TV / SUB 24".into(),
            description: "Opening sentence.".into(),
            ..Default::default()
        };
        state.episodes.anime = Some(anime.clone());
        {
            let mut entries = state.history.entries().to_vec();
            entries.push(crate::persistence::Entry {
                anime,
                episode: "5".into(),
                mode: Mode::Sub,
                watched_at: 1,
            });
            state.history.replace_entries(entries);
        }
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "6".into(),
            number: "6".into(),
            title: "Arrival".into(),
        }];
        let text = render_text(&mut state, 52, 12);
        for content in [
            "Last played: Ep 5",
            "Episode 6 / Arrival",
            "Synopsis",
            "Opening sentence.",
        ] {
            assert!(text.contains(content), "{text}");
        }
        let view = state.view.text_view.as_ref().unwrap();
        let line = view
            .lines
            .iter()
            .position(|line| line.contains("Episode 6 / Arrival"))
            .unwrap();
        assert_eq!(
            view.selected_text(crate::view::TextSelection {
                anchor: crate::view::TextPosition { line, column: 0 },
                end: crate::view::TextPosition { line, column: 19 },
                dragging: false,
            }),
            "Episode 6 / Arrival"
        );
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
        for (width, height) in [(32, 12), (80, 24), (120, 40)] {
            let text = render_text(&mut state, width, height);
            assert!(
                text.contains("Search unavailable") && text.contains("Service unavailable"),
                "{width}x{height}: {text}"
            );
            assert_eq!(text.lines().last().unwrap().trim(), "Esc › R Retry");
            assert_eq!(text.matches("Retry").count(), 1);
            assert!(!text.contains("Enter Results") && !text.contains("Esc Done"));
            assert!(!text.contains("No matches"));
            assert!(!text.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
        }
    }

    #[test]
    fn failed_episode_refresh_keeps_cached_selection_visible() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(Anime {
            title: "Series".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        state.episodes.marked.insert(0);
        state.request_error = Some("Service unavailable".into());
        for (w, h) in [(80, 24), (40, 18), (32, 12)] {
            let text = render_text(&mut state, w, h);
            assert!(text.contains("The beginning"), "{w}x{h}: {text}");
            assert!(text.contains("R Retry"));
            assert!(text.contains("1 marked"));
            assert!(!text.contains("Episodes unavailable"));
        }
        state.episodes.items.clear();
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Episodes unavailable"));
        assert!(!text.contains("Enter Play"));
    }

    #[test]
    fn refresh_errors_do_not_mask_validation_or_follow_the_user_into_details() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.anime = Some(Anime {
            id: "series".into(),
            title: "A series".into(),
            ..Default::default()
        });
        state.episodes.items = vec![Episode {
            provider: Default::default(),
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        state.request_error = Some("Persistent provider failure".into());
        state.status = "Range must go forward".into();
        state.error = true;
        state.focus = Focus::Range;
        state.episodes.range.set("3-1".into());
        for (w, h) in [(80, 24), (32, 12)] {
            let text = render_text(&mut state, w, h);
            assert!(text.contains("Range must go forward"), "{text}");
            assert!(text.contains("Cached"));
        }
        state.status.clear();
        state.focus = Focus::List;
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Refresh failed"));
        assert_eq!(text.matches("R Retry").count(), 1);
        state.details = true;
        let text = render_text(&mut state, 80, 24);
        assert!(!text.contains("Refresh failed"));
        state.help = true;
        state.view.help_scroll = u16::MAX;
        let text = render_text(&mut state, 120, 60);
        assert!(!text.contains("Persistent provider failure"), "{text}");
    }

    #[test]
    fn history_previews_selected_series_and_episode_selection_keeps_scroll_position() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::History;
        state.focus = Focus::List;
        {
            let mut entries = state.history.entries().to_vec();
            entries.push(crate::persistence::Entry {
                anime: Anime {
                    id: "1".into(),
                    title: "Frieren".into(),
                    ..Default::default()
                },
                episode: "6".into(),
                mode: Mode::Sub,
                watched_at: 0,
            });
            state.history.replace_entries(entries);
        }
        let text = render_text(&mut state, 120, 40);
        assert!(text.contains("History"));
        assert!(text.contains("Series"));
        assert!(text.contains("No cover") || text.contains("Covers disabled"));
        assert!(!text.contains("Your next story starts here"));
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.episodes.items = (1..=100)
            .map(|n| Episode {
                provider: Default::default(),
                id: n.to_string(),
                number: n.to_string(),
                title: format!("Title {n}"),
            })
            .collect();
        state.episodes.selected = 60;
        render_text(&mut state, 80, 24);
        let offset = state.view.list_offsets[Screen::Episodes as usize];
        state.episodes.selected = 59;
        render_text(&mut state, 80, 24);
        assert_eq!(state.view.list_offsets[Screen::Episodes as usize], offset);
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
            "{}THE_END",
            "The journey continues across a changing world. ".repeat(100)
        );
        state.synopses.put(
            state.selected_anime().unwrap().source_id(),
            Synopsis::Ready(full),
        );
        for (w, h) in [(180, 45), (80, 24), (60, 40), (40, 18), (32, 12)] {
            state.view.details_scroll = 0;
            let text = render_text(&mut state, w, h);
            assert!(text.contains("Synopsis"), "{w}x{h}");
            assert!(!text.contains("truncated snippet"));
            let (_, reading) = preview_regions(&state, panels(&state, Rect::new(0, 0, w, h)).1);
            assert!(reading.width <= 78);
            assert_eq!(
                text.lines()
                    .nth(reading.y as usize)
                    .unwrap()
                    .chars()
                    .nth(reading.right() as usize),
                Some('┃')
            );
            state.view.details_scroll = u16::MAX;
            let end = render_text(&mut state, w, h);
            assert!(end.contains("THE_END"), "{w}x{h}: {end}");
            assert!(state.view.details_scroll < u16::MAX);
            assert_eq!(
                end.lines()
                    .nth(reading.bottom() as usize - 1)
                    .unwrap()
                    .chars()
                    .nth(reading.right() as usize),
                Some('┃')
            );
        }
        state.synopses.put(
            state.selected_anime().unwrap().source_id(),
            Synopsis::Ready("Short synopsis.".into()),
        );
        let text = render_text(&mut state, 80, 24);
        let (_, reading) = preview_regions(&state, panels(&state, Rect::new(0, 0, 80, 24)).1);
        assert_eq!(
            text.lines()
                .nth(reading.y as usize)
                .unwrap()
                .chars()
                .nth(reading.right() as usize),
            Some(' ')
        );
        assert_eq!(state.view.details_scroll, 0);
    }

    #[test]
    fn text_selection_resets_when_the_reading_area_moves_without_rewrapping() {
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
                id: "1".into(),
                title: "A series title".into(),
                description: "A short synopsis.".into(),
                ..Default::default()
            }]),
        );
        state.details = true;
        render_text(&mut state, 120, 40);
        let area = state.view.text_view.as_ref().unwrap().area;
        let lines = state.view.text_view.as_ref().unwrap().lines.clone();
        state.view.text_selection = Some(crate::view::TextSelection {
            anchor: crate::view::TextPosition { line: 0, column: 0 },
            end: crate::view::TextPosition { line: 0, column: 4 },
            dragging: true,
        });
        render_text(&mut state, 140, 40);
        let view = state.view.text_view.as_ref().unwrap();
        assert_eq!(view.area.width, area.width);
        assert_eq!(view.lines, lines);
        assert_ne!(view.area.x, area.x);
        assert!(
            state.view.text_selection.is_none(),
            "Moving the reading area kept an old drag alive"
        );
    }

    #[test]
    fn text_selection_includes_metadata_and_resets_when_wrapping_changes() {
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
                id: "1".into(),
                title: "A series title".into(),
                info: "TV / SUB 12".into(),
                description: "The first paragraph.\n\nThe second paragraph. [Written by Someone]"
                    .into(),
                ..Default::default()
            }]),
        );
        state.details = true;
        render_text(&mut state, 120, 40);
        let view = state.view.text_view.as_ref().unwrap();
        let text = view.lines.join("\n");
        assert!(text.contains("The first paragraph.") && text.contains("[Written by Someone]"));
        assert!(
            text.contains("Synopsis")
                && text.contains("A series title")
                && text.contains("TV / SUB")
        );
        state.view.text_selection = Some(crate::view::TextSelection {
            anchor: crate::view::TextPosition { line: 0, column: 0 },
            end: crate::view::TextPosition { line: 0, column: 3 },
            dragging: false,
        });
        render_text(&mut state, 120, 40);
        assert!(state.view.text_selection.is_some());
        assert_eq!(
            state
                .view
                .text_view
                .as_ref()
                .unwrap()
                .selected_text(state.view.text_selection.unwrap()),
            "A s"
        );
        render_text(&mut state, 32, 12);
        assert!(state.view.text_selection.is_none());
        state.details = false;
        render_text(&mut state, 120, 40);
        assert!(state.view.text_view.is_some());
        render_text(&mut state, 20, 8);
        assert!(state.view.text_view.is_none());
        let anime = state.search.results()[0].clone();
        state.episodes.anime = Some(anime.clone());
        state
            .history
            .replace_entries(vec![crate::persistence::Entry {
                anime,
                episode: "1".into(),
                mode: Mode::Sub,
                watched_at: 1,
            }]);
        for screen in [Screen::Search, Screen::Episodes, Screen::History] {
            state.screen = screen;
            render_text(&mut state, 120, 40);
            let view = state.view.text_view.as_ref().unwrap();
            assert!(
                view.lines.join("\n").contains("A series title"),
                "{screen:?}"
            );
        }
    }

    #[test]
    fn synopsis_formatting_preserves_paragraphs_and_distinguishes_credits() {
        let state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        let lines = synopsis_lines(
            "An unexpected message arrives. The journey begins.\n\nA second chapter follows. [Written by MAL Rewrite]",
            72,
            &state,
        );
        let text: Vec<_> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(
            text,
            [
                "An unexpected message arrives. The journey begins.",
                "",
                "A second chapter follows.",
                "",
                "[Written by MAL Rewrite]",
            ]
        );
        assert_eq!(lines.last().unwrap().style, muted(&state));
        let initials = format!("{}Written by Chiaki J. Konaka, this story follows a difficult journey. The story continues.",
            "A long journey continues through unfamiliar places and brings new questions for the characters. ".repeat(5));
        let text = synopsis_lines(&initials, 72, &state)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("J.\n\n"), "{text}");
        let original = format!("{} [Written by MAL Rewrite]",
            "日本語の物語。 A message arrives from an old friend. The journey continues through unfamiliar places. ".repeat(8));
        for width in [28, 48, 72] {
            let lines = synopsis_lines(&original, width, &state);
            assert!(lines.iter().any(|line| line.to_string().is_empty()));
            let rendered = lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            let without_space = |text: &str| {
                text.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
            };
            assert_eq!(without_space(&rendered), without_space(&original));
            assert!(lines.iter().all(|line| line.width() <= width));
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

    #[test]
    fn synopsis_cache_separates_matching_ids_from_different_providers() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Search;
        let first = crate::model::Anime {
            id: "same-1".into(),
            title: "First source".into(),
            ..Default::default()
        };
        let second = crate::model::Anime {
            provider: "other".to_owned().try_into().unwrap(),
            title: "Second source".into(),
            ..first.clone()
        };
        state
            .synopses
            .put(first.source_id(), Synopsis::Ready("ALPHA_SYNOPSIS".into()));
        state
            .synopses
            .put(second.source_id(), Synopsis::Ready("BETA_SYNOPSIS".into()));
        state.search.replace_results(vec![first, second]);
        let text = render_text(&mut state, 120, 30);
        assert!(text.contains("ALPHA_SYNOPSIS") && !text.contains("BETA_SYNOPSIS"));
        state.search.selected = 1;
        let text = render_text(&mut state, 120, 30);
        assert!(text.contains("BETA_SYNOPSIS") && !text.contains("ALPHA_SYNOPSIS"));
    }
}
