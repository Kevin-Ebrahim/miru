use crate::{
    app::{Activity, Focus, JobKind, JobPhase, Screen, State, Synopsis, TextView},
    artwork::Artwork,
    palette::Palette,
    scene::Dusk,
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

fn shortcut(state: &State, key: &str, action: &str) -> Line<'static> {
    let base = if key == "Enter" {
        palette(state).selection
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(key.to_owned(), base.patch(key_style(state))),
        Span::styled(format!(" {action}"), base.patch(muted(state).not_bold())),
    ])
}

fn hint_message(state: &State, text: &str, style: Style) -> Line<'static> {
    if let Some(action) = text.strip_prefix("Alt+e ") {
        return shortcut(state, "Alt+e", action);
    }
    if let Some((before, after)) = text.split_once("F5 Retry") {
        let mut spans = vec![Span::styled(before.to_owned(), style)];
        spans.extend(shortcut(state, "F5", "Retry").spans);
        spans.push(Span::styled(after.to_owned(), style));
        return Line::from(spans);
    }
    Line::styled(text.to_owned(), style)
}

fn accent(state: &State) -> Style {
    Style::default().fg(palette(state).focus).bold()
}
fn symbol<'a>(state: &State, unicode: &'a str, ascii: &'a str) -> &'a str {
    if state.ascii { ascii } else { unicode }
}

fn episode_refresh_error(state: &State) -> Option<&str> {
    if state.screen == Screen::Episodes && !state.details && !state.episodes.is_empty() {
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
        activity.kind != JobKind::Playback || activity.phase != JobPhase::Finished
    })
}

fn activity_height(state: &State) -> u16 {
    visible_activity(state).map_or(0, |a| {
        if (a.kind == JobKind::Playback && a.phase != JobPhase::Failed)
            || (a.message.is_empty() && a.quality_note.is_empty())
        {
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
        // Preview rendering updates text_view later; reserve its hint now so a resize
        // cannot clip the primary-action row on the first frame.
        Constraint::Length(
            key_hints(
                state,
                area.width as usize,
                (state.details || state.selected_anime().is_some()).then_some("Drag Select"),
            )
            .len() as u16,
        ),
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
        Block::default().style(
            Style::default()
                .fg(palette(state).text)
                .bg(palette(state).canvas),
        ),
        area,
    );
    if area.width < 32 || area.height < 12 {
        state.text_view = None;
        state.text_selection = None;
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
    if state.screen == Screen::Home && !state.details {
        state.text_view = None;
        state.text_selection = None;
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
            state.text_view = None;
            state.text_selection = None;
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
                    " ! Refresh failed / F5 Retry\n {}",
                    episode_refresh_error(state).unwrap_or_default()
                ),
                true,
            )
        };
        frame.render_widget(
            Paragraph::new(
                text.lines()
                    .map(|line| hint_message(state, line, Style::default()))
                    .collect::<Vec<_>>(),
            )
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
    let hints = key_hints(
        state,
        area.width as usize,
        (state.details || state.text_view.is_some()).then_some(
            if state.text_selection.is_some() && (state.details || state.focus == Focus::List) {
                "y Copy"
            } else {
                "Drag Select"
            },
        ),
    );
    let hint_rows = hints.len() as u16;
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().bold()),
        Rect::new(
            footer.x,
            footer.bottom().saturating_sub(hint_rows),
            footer.width,
            hint_rows,
        ),
    );
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

fn home_sections(state: &State, area: Rect) -> [Rect; 5] {
    let activity_height = activity_height(state);
    Layout::vertical([
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
        Constraint::Length(
            if state.activity.as_ref().is_some_and(|a| a.retryable()) && area.width < 54 {
                2
            } else {
                1
            },
        ),
    ])
    .areas(area)
}

pub fn home_scene_visible(state: &State, area: Rect) -> bool {
    state.screen == Screen::Home
        && !state.help
        && !state.details
        && crate::sakura::fits(home_sections(state, area)[0])
}

fn render_home(frame: &mut Frame, state: &State, area: Rect) {
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
    let history = !state.history.is_empty();
    let hint_height = 1;
    let gap = u16::from(space.height >= 16);
    let box_height = if space.height >= 12 { 5 } else { 4 };
    let hint_gap = u16::from(space.height >= 10);
    let height = logo_height + gap + box_height + hint_gap + hint_height;
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
    let block = panel(String::new(), state, state.focus != Focus::List)
        .style(Style::default().bg(palette(state).surface));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let prefix = if state.exact { "[EXACT] " } else { "" };
    let input_width = inner.width.saturating_sub(prefix.len() as u16);
    let (query, cursor) = input_window(
        &state.query.text,
        state.query.cursor,
        usize::from(input_width.saturating_sub(1)),
    );
    let input = if state.query.text.is_empty() {
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
    if state.focus == Focus::Search && !state.help {
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
    let mut hint = Line::default();
    if matches!(state.focus, Focus::Audio | Focus::Quality) {
        hint = shortcut(state, symbol(state, "←/→", "</>"), "Change");
    }
    frame.render_widget(
        Paragraph::new(hint),
        Rect::new(x, field.bottom() + hint_gap, width, hint_height),
    );
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
    let mut footer_hint = Line::default();
    if history {
        footer_hint
            .spans
            .extend(shortcut(state, "Alt+h", "History").spans);
        footer_hint.spans.push(Span::raw("  "));
    }
    footer_hint
        .spans
        .extend(shortcut(state, "F1", "Help").spans);
    frame.render_widget(Paragraph::new(footer_hint).right_aligned(), footer);
    if state.activity.as_ref().is_some_and(|a| a.retryable()) {
        frame.render_widget(
            Paragraph::new(shortcut(state, "F6", "Retry")).centered(),
            if footer.width >= 54 {
                footer
            } else {
                Rect::new(footer.x, footer.y + 1, footer.width, 1)
            },
        );
    }
}

fn render_header(frame: &mut Frame, state: &State, header: Rect) {
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
    let settings = format!("{} / {}", state.mode.as_str().to_uppercase(), state.quality);
    let context = format!("   {location}   {settings}");
    let heading = vec![Line::from(
        if " ani / watch".width() + context.width() > usize::from(header.width) {
            vec![
                Span::styled(format!(" {location}"), accent(state)),
                Span::raw(format!("   {settings}")),
            ]
        } else {
            vec![
                Span::styled(" ani / watch", accent(state)),
                Span::raw(context),
            ]
        },
    )];
    frame.render_widget(
        Paragraph::new(heading).style(Style::default().bold()),
        header,
    );
}

fn render_wide_header(frame: &mut Frame, state: &State, area: Rect) {
    let bar = Style::default().fg(palette(state).text).bold();
    frame.render_widget(Block::default().style(bar), area);
    let shortcuts = state.focus == Focus::List && !state.details;
    let settings = if shortcuts {
        let mut spans = shortcut(state, "s", &state.mode.as_str().to_uppercase()).spans;
        spans.push(Span::raw("   "));
        spans.extend(shortcut(state, "Q", &state.quality).spans);
        spans.push(Span::raw(" "));
        Line::from(spans)
    } else {
        Line::styled(
            format!(
                "{} / {} ",
                state.mode.as_str().to_uppercase(),
                state.quality
            ),
            muted(state),
        )
    };
    let mut nav = vec![Span::raw(" ")];
    if state.details {
        nav.push(Span::styled("Details", accent(state)));
    } else {
        for (screen, key, label) in [(Screen::Search, "/", "Search")] {
            if matches!(state.screen, Screen::History | Screen::Episodes) {
                continue;
            }
            let active = state.screen == screen;
            if shortcuts && !active {
                nav.push(Span::styled(format!("{key} "), key_style(state)));
            }
            nav.push(Span::styled(
                label,
                if active {
                    accent(state).underlined()
                } else {
                    muted(state)
                },
            ));
            nav.push(Span::raw(
                if screen == Screen::Search && inline_search(state, area.width) {
                    " "
                } else {
                    "   "
                },
            ));
        }
        if state.screen == Screen::Episodes {
            nav.push(Span::styled("Episodes", accent(state).underlined()));
            if let Some(anime) = &state.anime {
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
    if inline_search(state, area.width) {
        let label = if state.exact { "[EXACT] " } else { "" };
        let field = Rect::new(middle.x, middle.y, middle.width.saturating_sub(2), 1);
        let content = Rect::new(
            field.x + label.len() as u16,
            field.y,
            field.width.saturating_sub(label.len() as u16),
            1,
        );
        frame.render_widget(Paragraph::new(label).style(accent(state)), field);
        let (text, cursor) = input_window(
            &state.query.text,
            state.query.cursor,
            content.width.saturating_sub(1) as usize,
        );
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(palette(state).text).not_bold()),
            content,
        );
        if state.focus == Focus::Search && !state.help {
            frame.set_cursor_position((content.x + cursor as u16, content.y));
        }
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
            if state.exact && state.focus != Focus::Range && state.screen != Screen::History {
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
    } else if state.screen == Screen::History {
        &state.history_query
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
    let gap = if width < 72 { "  " } else { "   " };
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for hint in hints {
        let separator = if used == 1 { "" } else { gap };
        if used + separator.len() + hint.width() > width {
            continue;
        }
        used += separator.len() + hint.width();
        spans.push(Span::raw(separator));
        let (key, label) = hint.split_once(' ').unwrap_or((hint.as_str(), ""));
        spans.extend(shortcut(state, key, label).spans);
    }
    Line::from(spans)
}

fn key_hints(state: &State, width: usize, selection_hint: Option<&str>) -> Vec<Line<'static>> {
    let compact = width < 72;
    let mut primary: Vec<String> = if state.details {
        vec!["Esc Back".into(), "Up/Down Scroll".into()]
    } else if state.focus == Focus::Search {
        if state.screen == Screen::History {
            vec![]
        } else {
            vec!["Alt+e Exact".into()]
        }
    } else if state.focus == Focus::Range {
        vec!["Enter Mark range".into(), "Esc Cancel".into()]
    } else if state.request_blocks_list() {
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
            "Space Mark".into(),
            ": Range".into(),
            format!("d Download{suffix}"),
            "i Details".into(),
        ]
    } else {
        let mut hints = vec![
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
    if state.screen == Screen::History {
        primary.retain(|hint| hint != "/ Search");
    }
    let mut global: Vec<String> = vec!["F1 Help".into()];
    if let Some(activity) = visible_activity(state) {
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
        global = vec!["F1 Help".into()];
        if state.request_error.is_some() && !state.details {
            global.insert(0, "F5 Retry".into());
        }
    } else if !state.details {
        global.extend(["s SUB/DUB".into(), "Q Quality".into()]);
    }
    if state.activity.as_ref().is_some_and(|a| a.retryable()) {
        global.insert(
            0,
            if compact {
                "F6 Retry"
            } else {
                "F6 Retry remaining"
            }
            .into(),
        );
    }
    if let Some(hint) = selection_hint.filter(|hint| *hint != "Drag Select") {
        global.push(hint.into());
    }
    if width >= 120 {
        global.retain(|hint| {
            !matches!(
                hint.as_str(),
                "H Home" | "Alt+h History" | "s SUB/DUB" | "Q Quality"
            )
        });
        let action = if state.screen == Screen::History
            || matches!(primary[0].as_str(), "Alt+e Exact" | "/ Edit" | "Space Mark")
        {
            None
        } else {
            Some(primary.remove(0))
        };
        let mut secondary = primary;
        secondary.extend(
            global
                .into_iter()
                .filter(|hint| !matches!(hint.as_str(), "F1 Help" | "Ctrl+C Quit")),
        );
        let mut right = shortcut(state, "F1", "Help");
        if let Some(action) = action {
            right.spans.push(Span::raw("    "));
            let (key, label) = action.split_once(' ').unwrap_or((&action, ""));
            right.spans.extend(shortcut(state, key, label).spans);
        }
        right.spans.push(Span::raw("  "));
        let secondary_width = 1
            + secondary.iter().map(|hint| hint.width()).sum::<usize>()
            + secondary.len().saturating_sub(1) * 3;
        let left = fit_hints(&secondary, width, state);
        if secondary_width + 3 + right.width() <= width {
            let gap = width - left.width() - right.width();
            let mut spans = left.spans;
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(right.spans);
            return vec![Line::from(spans)];
        }
        let mut lines = Vec::new();
        let mut row = Vec::new();
        let mut used = 1;
        for hint in secondary {
            let needed = hint.width() + if row.is_empty() { 0 } else { 3 };
            if used + needed > width && !row.is_empty() {
                lines.push(fit_hints(&row, width, state));
                row.clear();
                used = 1;
            }
            used += hint.width() + if row.is_empty() { 0 } else { 3 };
            row.push(hint);
        }
        if !row.is_empty() {
            lines.push(fit_hints(&row, width, state));
        }
        lines.push(right.right_aligned());
        return lines;
    }
    if compact && (state.details || state.focus == Focus::List) {
        global.retain(|hint| hint != "F1 Help");
        global.insert(
            usize::from(state.activity.as_ref().is_some_and(|a| a.retryable())),
            "F1 Help".into(),
        );
    }
    if compact && !state.details && state.focus == Focus::List && !state.request_blocks_list() {
        if state.screen == Screen::Episodes && !state.episodes.is_empty() {
            global.retain(|hint| hint != "F1 Help");
            return vec![
                fit_hints(&["Space Mark".into(), ": Range".into()], width, state),
                fit_hints(
                    &["d Download".into(), "i Details".into(), "F1 Help".into()],
                    width,
                    state,
                ),
                fit_hints(&global, width, state),
            ];
        }
        if state.screen == Screen::Search && state.selected_anime().is_some() {
            let search = if state.screen == Screen::Search {
                "/ Edit"
            } else {
                "/ Search"
            };
            primary.retain(|hint| hint != search);
            let after_help = global.iter().position(|hint| hint == "F1 Help").unwrap() + 1;
            global.insert(after_help, search.into());
        }
    }
    vec![
        fit_hints(&primary, width, state),
        fit_hints(&global, width, state),
    ]
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
        Screen::Search => ("Results", visible.len(), state.selected),
        Screen::History => (
            "History",
            state.visible_history().len(),
            state.history_selected,
        ),
        Screen::Episodes => ("List", state.episodes.len(), state.episode_selected),
    };
    let mut title = if state.marked.is_empty() || state.screen != Screen::Episodes {
        format!(" {label} / {count} ")
    } else {
        format!(" {label} / {count} / {} marked ", state.marked.len())
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
        let (title, hint) = if state.loading {
            if state.screen == Screen::Episodes {
                ("Loading episodes...", "")
            } else {
                ("Searching...", "")
            }
        } else if state.screen == Screen::History {
            if state.history.is_empty() {
                ("No history", "")
            } else {
                ("No matches", "")
            }
        } else if state.screen == Screen::Episodes {
            ("No episodes", "")
        } else if state.query.text.trim().is_empty() {
            ("Enter a title", "")
        } else {
            (
                "No matches",
                if state.exact {
                    "Alt+e Turn off exact match"
                } else {
                    "Try another title"
                },
            )
        };
        let title = if state.loading {
            format!("{} {}", scan(state), title.trim_end_matches('.'))
        } else {
            title.to_owned()
        };
        empty_art(frame, state, inner, &title, hint, false);
        return;
    }
    let item_width = usize::from(inner.width.saturating_sub(2));
    let items: Vec<ListItem> = match state.screen {
        Screen::Home => vec![],
        Screen::History => state
            .visible_history()
            .into_iter()
            .map(|i| {
                let entry = &state.history[i];
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
                let anime = &state.results[*i];
                series_item(&anime.title, &anime.info, item_width, state)
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
                let indicator = if playing {
                    symbol(state, "▶", ">")
                } else if state.marked.contains(&i) {
                    symbol(state, "✓", "*")
                } else {
                    " "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(indicator, Style::default().fg(palette(state).success)),
                    Span::raw(format!(" {:>4}  {}", ep.number, ep.title)),
                ]))
            })
            .collect(),
    };
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
    frame.render_widget(
        block.style(
            Style::default()
                .bg(palette(state).surface)
                .fg(palette(state).text),
        ),
        area,
    );
    let (cover, metadata) = preview_regions(state, area);
    let Some(anime) = state.selected_anime() else {
        state.text_view = None;
        state.text_selection = None;
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
            .iter()
            .find(|e| e.anime.id == anime.id && e.mode == state.mode),
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
        && let Some(ep) = state.episodes.get(state.episode_selected)
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
    let synopsis = state.synopses.peek(&anime.id);
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
        Some(Synopsis::Failed) => Some(("! Synopsis unavailable / F5 Retry", palette(state).error)),
        _ => None,
    };
    if let Some((message, color)) = feedback {
        if !compact_details {
            details.push(Line::raw(""));
        }
        details.extend(
            wrap_prose(message, metadata.width as usize)
                .into_iter()
                .map(|line| hint_message(state, &line, Style::default().fg(color))),
        );
    }
    let text_lines: Vec<_> = details.iter().map(ToString::to_string).collect();
    if state
        .text_view
        .as_ref()
        .is_none_or(|view| view.area.width != metadata.width || view.lines != text_lines)
    {
        state.text_selection = None;
        state.preview_scroll = 0;
    }
    let max_scroll = details
        .len()
        .saturating_sub(metadata.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = if state.details {
        &mut state.details_scroll
    } else {
        &mut state.preview_scroll
    };
    *scroll = (*scroll).min(max_scroll);
    let scroll = *scroll;
    let content_length = details.len();
    let offset = usize::from(scroll).min(text_lines.len());
    let height = usize::from(metadata.height).min(text_lines.len() - offset);
    state.text_view = (height > 0).then(|| TextView {
        area: Rect::new(metadata.x, metadata.y, metadata.width, height as u16),
        offset,
        lines: text_lines,
    });
    frame.render_widget(Paragraph::new(details).scroll((scroll, 0)), metadata);
    {
        if let (Some(view), Some(selection)) = (&state.text_view, state.text_selection) {
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
    let phase = match activity.phase {
        JobPhase::Resolving => "Resolving",
        JobPhase::Running if activity.kind == JobKind::Download => "Downloading",
        JobPhase::Running => "Playing",
        JobPhase::Finished => "Finished",
        JobPhase::Failed => "Failed",
    };
    let spinner = scan(state);
    let (indicator, color) = match activity.phase {
        JobPhase::Resolving => (spinner.as_str(), palette(state).pending),
        JobPhase::Running if activity.kind == JobKind::Download => {
            (symbol(state, "↓", "v"), palette(state).pending)
        }
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
        let phase = match (activity.phase, activity.kind) {
            (JobPhase::Resolving, _) => "Resolve",
            (JobPhase::Running, JobKind::Download) => "Download",
            (JobPhase::Running, JobKind::Playback) => "Play",
            (JobPhase::Finished, _) => "Done",
            (JobPhase::Failed, _) => "Failed",
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
    format!("{marker}{}", &text[start..])
}

const HELP_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Home",
        &[
            ("Tab / Shift+Tab", "Cycle search, audio and quality"),
            ("Left/Right", "Change focused setting"),
            ("Enter", "Return to input / submit search"),
        ],
    ),
    (
        "Navigation",
        &[
            ("H", "Home"),
            ("/", "Search"),
            ("Alt+h", "History"),
            ("j/k / Up/Down", "Move or scroll"),
            ("Ctrl+n/p", "Next / previous item"),
            ("PgUp/PgDn / Ctrl+u/d/f/b", "Jump ten items or lines"),
            ("g/G / Home/End", "First / last item or line"),
            ("Enter", "Open series / play episode"),
            ("l / Right", "Open series / episode details"),
            ("i", "Series details"),
            ("Esc / h / Left", "Back (Esc leaves text input)"),
            ("Alt+e", "Toggle exact search"),
            ("Delete", "Remove highlighted history entry"),
            ("Drag in text / y", "Select and copy / copy panel text"),
        ],
    ),
    (
        "Playback & downloads",
        &[
            ("s / D", "Switch SUB/DUB"),
            ("Q", "Change quality"),
            ("Space / Tab", "Mark episode"),
            (":", "Mark range (1-3, 5 or 0--1)"),
            ("d", "Download marked or current episodes"),
            ("n / p / r", "Next / previous / replay"),
            ("x", "Stop or dismiss activity"),
            ("F5 / R", "Retry request and cover"),
            ("F6", "Retry remaining queue"),
        ],
    ),
    (
        "Text editing",
        &[
            ("Ctrl+a/e / Home/End", "Start / end of line"),
            ("Ctrl+b/f / Left/Right", "Previous / next character"),
            ("Alt+b/f / Ctrl+Left/Right", "Previous / next word"),
            ("Ctrl+u/k", "Cut to start / end of line"),
            ("Ctrl+w", "Cut previous whitespace word"),
            ("Alt+Backspace/d", "Cut previous / next word"),
            ("Ctrl+y", "Restore last cut"),
            ("Ctrl+h/d", "Backspace / delete character"),
            ("Esc / Ctrl+g", "Leave text input"),
            ("Enter / Ctrl+j/m", "Accept input"),
        ],
    ),
    (
        "App controls",
        &[
            ("? / F1", "Help (F1 also works while typing)"),
            ("q", "Close Help/Details"),
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
    let query = state.help_query.text.to_lowercase();
    let mut lines = Vec::new();
    let key_style = key_style(state);
    for (group, entries) in HELP_GROUPS {
        let entries: Vec<_> = entries
            .iter()
            .filter(|(key, action)| {
                help_matches(
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
        lines.push(Line::styled(*group, accent(state)));
        for (key, action) in entries {
            if width >= 64 {
                for (index, description) in wrap_prose(action, width - 29).into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:<29}", if index == 0 { key } else { "" }),
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
                    wrap_prose(action, width.saturating_sub(2))
                        .into_iter()
                        .map(|text| Line::styled(format!("  {text}"), muted(state))),
                );
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled("No matching shortcuts", muted(state)));
    }
    lines
}

fn render_help(frame: &mut Frame, state: &mut State) {
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
    let [header, filter, _, body, _, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(u16::from(inner.height >= 18)),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Keybindings").style(Style::default().bold()),
        header,
    );
    let close = shortcut(
        state,
        if state.help_filtering { "F1" } else { "Esc" },
        "close",
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
        if state.help_filtering && filter.width > 2 && filter.height > 0 {
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
    state.help_scroll = state.help_scroll.min(
        content_length
            .saturating_sub(usize::from(body.height))
            .min(usize::from(u16::MAX)) as u16,
    );
    frame.render_widget(Paragraph::new(lines).scroll((state.help_scroll, 0)), body);
    render_scrollbar(frame, state, body, content_length, state.help_scroll);
    let hints: &[(&str, &str)] = if state.help_filtering {
        if footer.width >= 55 {
            &[
                ("Enter/Esc", "Done"),
                ("Ctrl+u", "Clear"),
                ("Down", "Scroll"),
            ]
        } else {
            &[("Enter/Esc", "Done")]
        }
    } else if footer.width >= 64 {
        &[
            ("/", "Filter"),
            ("j/k or Up/Down", "Scroll"),
            ("Esc", "Close"),
        ]
    } else {
        &[("/", "Filter"), ("j/k", "Move"), ("Esc", "Close")]
    };
    let mut spans = Vec::new();
    for (key, action) in hints {
        if !spans.is_empty() {
            spans.push(Span::raw(if footer.width >= 55 { "   " } else { "  " }));
        }
        spans.extend(shortcut(state, key, action).spans);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), footer);
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
        .map(|(text, style)| hint_message(state, text, style))
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
    use crate::app::Activity;
    use crate::model::{Anime, Episode, Mode};
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
    };
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
        state.anime = Some(Anime {
            id: "series".into(),
            title: "Cowboy Bebop".into(),
            ..Default::default()
        });
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "Asteroid Blues".into(),
        }];
        state.marked.insert(0);
        state.activity = Some(Activity {
            anime: state.anime.clone().unwrap(),
            episodes: state.episodes.clone(),
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
        let positions: Vec<_> = ["Mark", "Range", "Download", "Details"]
            .iter()
            .map(|s| footer.find(s).unwrap())
            .collect();
        assert!(positions.windows(2).all(|p| p[0] < p[1]));
        assert!(!footer.contains("Enter Play") && !footer.contains("Esc Back"));
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
                    state.history.push(crate::persistence::Entry {
                        anime: Anime {
                            title: "History stays in History".into(),
                            ..Default::default()
                        },
                        episode: "1".into(),
                        mode: Mode::Sub,
                        watched_at: 1,
                    });
                    let text = render_text(&mut state, width, height);
                    for label in [
                        "Search anime...",
                        "DUB",
                        "1080p",
                        "Alt+h History",
                        "F1 Help",
                    ] {
                        assert!(text.contains(label), "{width}x{height} {focus:?}: {text}");
                    }
                    assert!(!text.contains("History stays in History"));
                    assert!(!text.contains("Ctrl+c Quit"));
                    assert!(!text.contains("/ Search  s Audio  Q Quality"));
                    assert!(state.text_view.is_none());
                    assert_eq!(
                        cover_area(&state, Rect::new(0, 0, width, height)),
                        Rect::default()
                    );
                    state.query.set("日本語 e\u{301} query ".repeat(20));
                    state.exact = true;
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
        state.history.push(crate::persistence::Entry {
            anime,
            episode: "1".into(),
            mode: Mode::Sub,
            watched_at: 1,
        });
        for screen in [Screen::Search, Screen::History] {
            state.screen = screen;
            for (w, h) in [(120, 40), (180, 45)] {
                let text = render_text(&mut state, w, h);
                let area = Rect::new(0, 0, w, h);
                let (list, preview) = panels(&state, area);
                let cover = cover_area(&state, area);
                let reading = state.text_view.as_ref().unwrap().area;
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
        let text = render_text(&mut state, 78, 19);
        assert!(text.contains("A readable synopsis"));
        assert_eq!(cover_area(&state, Rect::new(0, 0, 78, 19)), Rect::default());
    }

    #[test]
    fn compact_episode_controls_and_details_remain_discoverable() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.anime = Some(Anime {
            title: "A series".into(),
            ..Default::default()
        });
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        for (w, h) in [(32, 12), (40, 18), (52, 12), (71, 24), (72, 12), (80, 24)] {
            for marked in [false, true] {
                state.marked.clear();
                if marked {
                    state.marked.insert(0);
                }
                let text = render_text(&mut state, w, h);
                for label in [
                    "Space Mark",
                    ": Range",
                    "i Details",
                    "d Download",
                    "F1 Help",
                ] {
                    assert!(text.contains(label), "{w}x{h}, missing {label}: {text}");
                }
            }
        }
        state.screen = Screen::Search;
        state.query.set("series".into());
        state.accept_search(0, Ok(vec![state.anime.clone().unwrap()]));
        let text = render_text(&mut state, 32, 12);
        for label in ["i Details", "/ Edit", "F1 Help"] {
            assert!(text.contains(label), "{text}");
        }
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
                    for label in ["Ep 2", "DUB", "File already exists", "F1 Help"] {
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
                        assert!(text.contains("F6 Retry"), "{text}");
                    }
                }
            }
            state.history.push(crate::persistence::Entry {
                anime: state.activity.as_ref().unwrap().anime.clone(),
                episode: "1".into(),
                mode: Mode::Sub,
                watched_at: 1,
            });
            state.status = "Audio applies to the next queue.".into();
            state.focus = Focus::Audio;
            let text = render_text(&mut state, 32, 12);
            for label in [
                "Search anime...",
                "Alt+h History",
                if state.ascii {
                    "</> Change"
                } else {
                    "←/→ Change"
                },
                "Ep 2",
                "F6 Retry",
                "F1 Help",
            ] {
                assert!(text.contains(label), "{text}");
            }
            state.status.clear();
            let activity = state.activity.as_mut().unwrap();
            activity.episodes = (1..=1400)
                .map(|n| Episode {
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
            for (w, h) in [(32, 12), (80, 24)] {
                let text = render_text(&mut state, w, h);
                assert!(text.contains("720p (1080p unavailable)"), "{text}");
                state.help = true;
                state.help_scroll = u16::MAX;
                let text = render_text(&mut state, w, h);
                assert!(!text.contains("1080p unavailable"), "{text}");
                assert!(!text.contains("Saved /tmp/episode.mkv"), "{text}");
                state.help = false;
            }
        }
    }

    #[test]
    fn help_finds_individual_shortcut_aliases() {
        assert!(!help_key_aliases("Ctrl+a/e / Home/End").contains("Ctrl+Home"));
        assert!(!help_key_aliases("Ctrl+b/f / Left/Right").contains("Ctrl+Right"));
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        for (query, expected) in [
            ("Ctrl+e", "Start / end of line"),
            ("Ctrl+f", "Previous / next character"),
            ("Ctrl+p", "Next / previous item"),
            ("Alt+d", "Cut previous / next word"),
            ("Ctrl+Right", "Previous / next word"),
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
                ("sUb/DuB", "s / D"),
                ("Ctrl+y", "Restore last cut"),
                ("playback quality", "Change quality"),
            ] {
                state.help_query.set(query.into());
                state.help_scroll = u16::MAX;
                let text = render_text(&mut state, width, height);
                assert!(text.contains(expected), "{text}");
                assert!(
                    text.contains("Keybindings") && text.contains("Enter/Esc Done"),
                    "{text}"
                );
                assert!(!text.contains("Terminal options"));
                assert_eq!(state.help_scroll, 0);
            }
            state.help_query.set("no-such-shortcut".into());
            assert!(render_text(&mut state, width, height).contains("No matching shortcuts"));
        }
    }

    #[test]
    fn help_reaches_last_shortcuts_and_clamps_after_resize() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.help = true;
        state.help_scroll = u16::MAX;
        let text = render_text(&mut state, 32, 12);
        assert!(text.contains("Quit from anywhere"), "{text}");
        assert!(state.help_scroll > 40 && state.help_scroll < u16::MAX);
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
        assert_eq!(state.help_scroll, 0);
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
                state.selected = 0;
                let text = render_text(&mut state, width, 28);
                assert!(text.contains("Season 2"), "{width}: {text}");
                assert!(text.contains("TV / SUB 12"));
                state.results[1].title = format!(
                    "{} Season 2",
                    "A very long 日本語 title e\u{301} ".repeat(15)
                );
                let text = render_text(&mut state, width, 28);
                assert!(text.contains("Season 2"), "{width}: {text}");
                assert!(text.contains(if ascii { "..." } else { "…" }));
                state.results[1].title = format!("{title} Season 2");
            }
        }
    }

    #[test]
    fn wide_footer_keeps_primary_actions_on_first_frame_after_resize() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.focus = Focus::List;
        state.anime = Some(Anime {
            id: "series".into(),
            title: "Series".into(),
            description: "A synopsis.".into(),
            ..Default::default()
        });
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "Opening".into(),
        }];
        state.activity = Some(Activity {
            anime: state.anime.clone().unwrap(),
            episodes: state.episodes.clone(),
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
            state.text_view = None;
            let text = render_text(&mut state, width, 40);
            for hint in ["F1 Help", "d Download", "Space Mark", ": Range"] {
                assert!(text.contains(hint), "{width} missing {hint}: {text}");
            }
            assert!(!text.contains("Enter Play") && !text.contains("Esc Back"));
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
        state.exact = true;
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
            top.contains("[EXACT]") && top.contains("SUB / best"),
            "{text}"
        );
        assert!(!top.contains("H Home") && !top.contains("Q best"));
        assert!(!text.contains("Enter Results") && !text.contains("Esc Done"));
        state.focus = Focus::List;
        let text = render_text(&mut state, 120, 30);
        let top = text.lines().next().unwrap();
        assert!(!top.contains("H Home") && top.contains("s SUB") && top.contains("Q best"));
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
                            .contains(&format!("DUB / {quality}")),
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
        state.anime = Some(anime.clone());
        state.history.push(crate::persistence::Entry {
            anime,
            episode: "5".into(),
            mode: Mode::Sub,
            watched_at: 1,
        });
        state.episodes = vec![Episode {
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
        let view = state.text_view.as_ref().unwrap();
        let line = view
            .lines
            .iter()
            .position(|line| line.contains("Episode 6 / Arrival"))
            .unwrap();
        assert_eq!(
            view.selected_text(crate::app::TextSelection {
                anchor: crate::app::TextPosition { line, column: 0 },
                end: crate::app::TextPosition { line, column: 19 },
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
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Search unavailable"));
        assert!(text.contains("F5 Retry"));
        assert!(!text.contains("Enter Results") && !text.contains("Esc Done"));
        assert!(!text.contains("? Help"));
        assert!(!text.contains("No matches"));
        assert!(!text.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
    }

    #[test]
    fn failed_episode_refresh_keeps_cached_selection_visible() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.anime = Some(Anime {
            title: "Series".into(),
            ..Default::default()
        });
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        state.marked.insert(0);
        state.request_error = Some("Service unavailable".into());
        for (w, h) in [(80, 24), (40, 18), (32, 12)] {
            let text = render_text(&mut state, w, h);
            assert!(text.contains("The beginning"), "{w}x{h}: {text}");
            assert!(text.contains("F5 Retry"));
            assert!(text.contains("Space Mark"));
            assert!(!text.contains("Episodes unavailable"));
        }
        state.episodes.clear();
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Episodes unavailable"));
        assert!(!text.contains("Enter Play"));
    }

    #[test]
    fn refresh_errors_do_not_mask_validation_or_follow_the_user_into_details() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
        state.anime = Some(Anime {
            id: "series".into(),
            title: "A series".into(),
            ..Default::default()
        });
        state.episodes = vec![Episode {
            id: "1".into(),
            number: "1".into(),
            title: "The beginning".into(),
        }];
        state.request_error = Some("Persistent provider failure".into());
        state.status = "Range must go forward".into();
        state.error = true;
        state.focus = Focus::Range;
        state.range.set("3-1".into());
        for (w, h) in [(80, 24), (32, 12)] {
            let text = render_text(&mut state, w, h);
            assert!(text.contains("Range must go forward"), "{text}");
            assert!(text.contains("Cached"));
        }
        state.status.clear();
        state.focus = Focus::List;
        let text = render_text(&mut state, 80, 24);
        assert!(text.contains("Refresh failed / F5 Retry"));
        state.details = true;
        let text = render_text(&mut state, 80, 24);
        assert!(!text.contains("Refresh failed"));
        state.help = true;
        state.help_scroll = u16::MAX;
        let text = render_text(&mut state, 120, 60);
        assert!(!text.contains("Persistent provider failure"), "{text}");
    }

    #[test]
    fn history_previews_selected_series_and_episode_selection_keeps_scroll_position() {
        let mut state = State::new(String::new(), Mode::Sub, "best".into(), false, false, false);
        state.screen = Screen::History;
        state.focus = Focus::List;
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
        assert!(text.contains("History"));
        assert!(text.contains("Series"));
        assert!(text.contains("No cover") || text.contains("Covers disabled"));
        assert!(!text.contains("Your next story starts here"));
        state.screen = Screen::Episodes;
        state.focus = Focus::List;
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
            "{}THE_END",
            "The journey continues across a changing world. ".repeat(100)
        );
        state.synopses.put("series-1".into(), Synopsis::Ready(full));
        for (w, h) in [(180, 45), (80, 24), (60, 40), (40, 18), (32, 12)] {
            state.details_scroll = 0;
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
            state.details_scroll = u16::MAX;
            let end = render_text(&mut state, w, h);
            assert!(end.contains("THE_END"), "{w}x{h}: {end}");
            assert!(state.details_scroll < u16::MAX);
            assert_eq!(
                end.lines()
                    .nth(reading.bottom() as usize - 1)
                    .unwrap()
                    .chars()
                    .nth(reading.right() as usize),
                Some('┃')
            );
        }
        state
            .synopses
            .put("series-1".into(), Synopsis::Ready("Short synopsis.".into()));
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
        assert_eq!(state.details_scroll, 0);
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
        let view = state.text_view.as_ref().unwrap();
        let text = view.lines.join("\n");
        assert!(text.contains("The first paragraph.") && text.contains("[Written by Someone]"));
        assert!(
            text.contains("Synopsis")
                && text.contains("A series title")
                && text.contains("TV / SUB")
        );
        state.text_selection = Some(crate::app::TextSelection {
            anchor: crate::app::TextPosition { line: 0, column: 0 },
            end: crate::app::TextPosition { line: 0, column: 3 },
            dragging: false,
        });
        render_text(&mut state, 120, 40);
        assert!(state.text_selection.is_some());
        assert_eq!(
            state
                .text_view
                .as_ref()
                .unwrap()
                .selected_text(state.text_selection.unwrap()),
            "A s"
        );
        render_text(&mut state, 32, 12);
        assert!(state.text_selection.is_none());
        state.details = false;
        render_text(&mut state, 120, 40);
        assert!(state.text_view.is_some());
        render_text(&mut state, 20, 8);
        assert!(state.text_view.is_none());
        let anime = state.results[0].clone();
        state.anime = Some(anime.clone());
        state.history = vec![crate::persistence::Entry {
            anime,
            episode: "1".into(),
            mode: Mode::Sub,
            watched_at: 1,
        }];
        for screen in [Screen::Search, Screen::Episodes, Screen::History] {
            state.screen = screen;
            render_text(&mut state, 120, 40);
            let view = state.text_view.as_ref().unwrap();
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
}
