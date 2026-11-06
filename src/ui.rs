use crate::{
    app::{Focus, Screen, State},
    artwork::Artwork,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
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
    Style::default().add_modifier(Modifier::DIM)
}
fn accent() -> Style {
    Style::default().fg(ACCENT).bold()
}

fn sections(area: Rect) -> [Rect; 5] {
    Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .areas(area)
}

fn panels(area: Rect) -> (Rect, Rect) {
    let body = sections(area)[2];
    if area.width >= 88 {
        let [list, preview] =
            Layout::horizontal([Constraint::Percentage(43), Constraint::Percentage(57)])
                .areas(body);
        (list, preview)
    } else if area.height >= 32 && area.width >= 40 {
        let [list, preview] =
            Layout::vertical([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(body);
        (list, preview)
    } else {
        (body, Rect::default())
    }
}

pub fn cover_area(area: Rect) -> Rect {
    let (_, panel) = panels(area);
    if panel.width < 8 || panel.height < 10 {
        return Rect::default();
    }
    Rect::new(
        panel.x + 2,
        panel.y + 1,
        panel.width.saturating_sub(4),
        panel.height.saturating_sub(8),
    )
}

fn panel(title: String, ascii: bool, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(if ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(if focused {
            Style::default().fg(ACCENT)
        } else {
            muted()
        })
        .padding(Padding::horizontal(1))
        .title(Line::from(title).style(if focused {
            accent()
        } else {
            Style::default().bold()
        }))
}

pub fn render(frame: &mut Frame, state: &State, art: &Artwork) {
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
    let [header, search, _, status, footer] = sections(area);
    let mode = state.mode.as_str().to_uppercase();
    let header_text = if area.width >= 60 {
        Line::from(vec![
            Span::styled("  ani", accent()),
            Span::styled(" / watch", Style::default().bold()),
            Span::styled("     YOUR NEXT STORY", muted()),
            Span::raw(format!("     {mode}  /  {}", state.quality)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ani / watch", accent()),
            Span::raw(format!("   {mode} / {}", state.quality)),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![
            header_text,
            Line::styled(
                match state.screen {
                    Screen::Search => "  DISCOVER   /   Search the catalogue",
                    Screen::Episodes => "  DISCOVER   /   Series   /   Episodes",
                },
                muted(),
            ),
        ]),
        header,
    );

    let search_title = if state.focus == Focus::Search {
        " Search / typing "
    } else {
        " Search / press / to edit "
    };
    let search_block = panel(
        format!(
            "{search_title}{}",
            if state.exact { "[EXACT] " } else { "" }
        ),
        state.ascii,
        state.focus == Focus::Search,
    );
    let inner = search_block.inner(search);
    let (text, cursor) = input_window(
        &state.query.text,
        state.query.cursor,
        inner.width.saturating_sub(1) as usize,
    );
    frame.render_widget(search_block, search);
    frame.render_widget(
        Paragraph::new(
            if state.query.text.is_empty() && state.focus != Focus::Search {
                "Search anime..."
            } else {
                &text
            },
        ),
        inner,
    );
    if state.focus == Focus::Search && !state.help {
        frame.set_cursor_position((inner.x + cursor as u16, inner.y));
    }

    let (list, preview) = panels(area);
    render_list(frame, state, list);
    if preview.width > 0 {
        render_preview(frame, state, art, preview, cover_area(area));
    }
    let spinner = if state.loading || (state.busy && state.playing.is_none()) {
        [".  ", ".. ", "..."][state.frame / 3 % 3]
    } else if state.error {
        "!  "
    } else {
        "   "
    };
    frame.render_widget(
        Paragraph::new(format!(" {spinner}{}", state.status))
            .style(if state.error {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            })
            .wrap(Wrap { trim: true }),
        status,
    );
    let keys = if state.screen == Screen::Search {
        if area.width >= 72 {
            " / Search   Enter Episodes   D SUB/DUB   Q Quality   ? Help   q Quit"
        } else {
            " / Search  Enter Select  ? Help  q Quit"
        }
    } else if area.width >= 72 {
        " Enter Play   n/p Next/Prev   r Replay   x Stop   Esc Back   ? Help"
    } else {
        " Enter Play  n/p Next/Prev  Esc Back  ? Help"
    };
    let navigation = if state.ascii {
        " Up/Down Navigate"
    } else {
        " ↑↓ Navigate"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(keys, accent()),
            Line::styled(
                format!("{navigation}   Ctrl+C Quit   {} covers", art.label),
                muted(),
            ),
        ]),
        footer,
    );
    if state.help {
        render_help(frame, state);
    }
}

fn render_list(frame: &mut Frame, state: &State, area: Rect) {
    let count = if state.screen == Screen::Search {
        state.visible().len()
    } else {
        state.episodes.len()
    };
    let title = format!(
        " {} / {:02} ",
        if state.screen == Screen::Search {
            "Results"
        } else {
            "Episodes"
        },
        count
    );
    let block = panel(title, state.ascii, state.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if count == 0 {
        let title = if state.loading {
            "Searching for a story"
        } else if state.query.text.is_empty() {
            "A story starts here"
        } else {
            "Nothing here yet"
        };
        let hint = if state.loading {
            "You can keep typing."
        } else if state.query.text.is_empty() {
            "Type a title above."
        } else {
            "Try another title. R retries."
        };
        empty_art(frame, inner, title, hint, state.frame, state.animate);
        return;
    }
    let items: Vec<ListItem> = if state.screen == Screen::Search {
        state
            .visible()
            .iter()
            .map(|i| {
                let anime = &state.results[*i];
                ListItem::new(vec![
                    Line::styled(anime.title.clone(), Style::default().bold()),
                    Line::styled(
                        if anime.info.is_empty() {
                            "Series".into()
                        } else {
                            anime.info.clone()
                        },
                        muted(),
                    ),
                    Line::raw(""),
                ])
            })
            .collect()
    } else {
        state
            .episodes
            .iter()
            .enumerate()
            .map(|(i, ep)| {
                let mark = if Some(i) == state.playing {
                    if state.ascii { "> " } else { "▶ " }
                } else {
                    "  "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{mark}{:>5}  ", ep.number), accent()),
                    Span::raw(ep.title.clone()),
                ]))
            })
            .collect()
    };
    let selected = if state.screen == Screen::Search {
        state.selected
    } else {
        state.episode_selected
    };
    let list = List::new(items)
        .highlight_symbol(if state.ascii { "> " } else { "▸ " })
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
    let mut selection = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, inner, &mut selection);
}

fn render_preview(frame: &mut Frame, state: &State, art: &Artwork, panel_area: Rect, cover: Rect) {
    frame.render_widget(
        panel(" Cover / selected series ".into(), state.ascii, false),
        panel_area,
    );
    if let Some(protocol) = &art.protocol {
        let size = protocol.size();
        if size.width <= cover.width && size.height <= cover.height {
            let centered = Rect::new(
                cover.x + (cover.width - size.width) / 2,
                cover.y + (cover.height - size.height) / 2,
                size.width,
                size.height,
            );
            frame.render_widget(Image::new(protocol), centered);
        }
    } else if cover.height > 0 {
        let hint = if art.error.is_some() {
            "Browsing is still available."
        } else if state.selected_anime().is_some_and(|a| a.cover.is_some()) {
            "Preparing cover..."
        } else {
            "A little space for imagination."
        };
        empty_art(
            frame,
            cover,
            if art.error.is_some() {
                "Cover unavailable"
            } else {
                "ani / watch"
            },
            hint,
            state.frame,
            state.animate,
        );
    }
    if let Some(anime) = state.selected_anime() {
        let metadata = Rect::new(
            panel_area.x + 2,
            panel_area.bottom().saturating_sub(7),
            panel_area.width.saturating_sub(4),
            6.min(panel_area.height),
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(anime.title.clone(), Style::default().bold()),
                Line::styled(anime.info.clone(), accent()),
                Line::raw(""),
                Line::styled(anime.description.clone(), muted()),
            ])
            .wrap(Wrap { trim: true }),
            metadata,
        );
    }
}

fn empty_art(frame: &mut Frame, area: Rect, title: &str, hint: &str, tick: usize, animate: bool) {
    let lines = if area.height >= 11 && area.width >= 25 {
        let spark = if animate && tick / 8 % 2 == 1 {
            "       .     *       "
        } else {
            "       *     .       "
        };
        vec![
            Line::raw(spark),
            Line::raw("       _..._         "),
            Line::raw("     .'     '.       "),
            Line::raw("    /  .   .  \\      "),
            Line::raw("    |    ~    |      "),
            Line::raw("  ~~'._   _.'~~      "),
            Line::raw("       ~~~           "),
            Line::raw(""),
            Line::styled(title, accent()),
            Line::styled(hint, muted()),
        ]
    } else {
        vec![Line::styled(title, accent()), Line::styled(hint, muted())]
    };
    let y = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    frame.render_widget(
        Paragraph::new(lines).centered().wrap(Wrap { trim: false }),
        Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y)),
    );
}

fn render_help(frame: &mut Frame, state: &State) {
    let area = frame.area();
    let block = panel(" Field guide / Esc to return ".into(), state.ascii, true);
    frame.render_widget(Clear, area);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "ANI / WATCH\n\n/                 Edit search (300 ms debounce)\nEnter             Browse series / play episode\nUp/Down or j/k    Navigate; PgUp/PgDn jump\nHome/End or g/G   First / last item\nCtrl+E            Toggle exact title matching\nD                 Toggle SUB / DUB\nQ                 Cycle quality (best to worst)\nn / p / r         Next / previous / replay\nx                 Stop player / cancel resolving\nR                 Retry current request\nEsc               Back / leave search input\nq or Ctrl+C       Quit and stop owned player\n\nSearch stays live while covers load.\nPlayer controls also work in the player window.\nUse --ascii, --no-animation, or --images off\nfor a quieter interface. Colors use your palette.";
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
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
    use ratatui::{Terminal, backend::TestBackend};
    use ratatui_image::picker::Picker;
    #[test]
    fn responsive_layouts_do_not_overlap_or_panic() {
        for (w, h) in [(140, 45), (88, 24), (60, 40), (40, 18), (32, 12), (20, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let state = State::new(
                "日本語 title".into(),
                crate::model::Mode::Sub,
                "best".into(),
                false,
                false,
                false,
            );
            let art = Artwork::new(Picker::halfblocks(), "/tmp/unused-art".into(), false);
            terminal.draw(|f| render(f, &state, &art)).unwrap();
            let cover = cover_area(Rect::new(0, 0, w, h));
            assert!(cover.right() <= w && cover.bottom() <= h);
        }
    }
    #[test]
    fn input_scroll_uses_display_width() {
        let (text, cursor) = input_window("ab日本語", "ab日本語".len(), 5);
        assert_eq!(text, "本語");
        assert_eq!(cursor, 4);
    }
}
