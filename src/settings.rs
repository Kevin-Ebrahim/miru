use crate::{
    config::{Config, QUALITIES},
    input::Input,
    model::Mode,
    player::{Backend, Player},
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::path::PathBuf;

fn fit_value(value: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    if value.width() <= width {
        return value.into();
    }
    let mut result = String::new();
    for grapheme in value.graphemes(true) {
        if result.width() + grapheme.width() + 1 > width {
            break;
        }
        result.push_str(grapheme);
    }
    if width > 0 {
        result.push('~');
    }
    result
}

#[derive(Clone, Default)]
pub struct Overrides {
    pub download: Option<PathBuf>,
    pub audio: Option<Mode>,
    pub quality: Option<String>,
    pub player: Option<String>,
    pub args: Option<Vec<String>>,
    pub backend: Option<Backend>,
}

pub struct Settings {
    pub saved: Config,
    pub overrides: Overrides,
    pub path: PathBuf,
    pub open: bool,
    pub(crate) selected: usize,
    arguments: bool,
    argument: usize,
    error: bool,
    edit: Option<Input>,
    message: String,
}
impl Settings {
    pub fn new(saved: Config, overrides: Overrides, path: PathBuf) -> Self {
        Self {
            saved,
            overrides,
            path,
            open: false,
            selected: 0,
            arguments: false,
            argument: 0,
            error: false,
            edit: None,
            message: String::new(),
        }
    }
    pub fn player(&self) -> Player {
        Player::new(
            self.overrides
                .player
                .clone()
                .or(self.saved.player.clone())
                .unwrap_or_else(|| "mpv".into()),
            self.overrides
                .args
                .clone()
                .unwrap_or_else(|| self.saved.player_args.clone()),
            self.overrides.backend.or(self.saved.player_backend),
        )
    }
    pub fn download(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.overrides.download {
            anyhow::ensure!(
                !path.as_os_str().is_empty(),
                "--download-dir cannot be empty"
            );
            return Ok(path.clone());
        }
        match self.saved.saved_download_dir()? {
            Some(path) => Ok(path),
            None => crate::config::automatic_download_dir(),
        }
    }
    fn value(&self, index: usize) -> String {
        match index {
            0 => self
                .saved
                .download_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Automatic".into()),
            1 => self.saved.audio.unwrap_or_default().as_str().to_uppercase(),
            2 => self.saved.quality.clone().unwrap_or_else(|| "best".into()),
            3 => self.saved.player.clone().unwrap_or_else(|| "mpv".into()),
            4 => format!("{} arguments", self.saved.player_args.len()),
            _ => match self.saved.player_backend {
                None => "Automatic",
                Some(Backend::Mpv) => "mpv",
                Some(Backend::Vlc) => "VLC",
            }
            .into(),
        }
    }
    pub fn paste(&mut self, text: &str) {
        if let Some(edit) = &mut self.edit {
            let mut value = edit.text.clone();
            value.insert_str(edit.cursor, &text.replace(['\n', '\r'], ""));
            edit.set(value);
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if self.edit.is_none() {
            let step = match key.code {
                KeyCode::PageDown => Some(true),
                KeyCode::PageUp => Some(false),
                _ => None,
            };
            if let Some(down) = step {
                let (selected, max) = if self.arguments {
                    (&mut self.argument, self.saved.player_args.len())
                } else {
                    (&mut self.selected, 5)
                };
                *selected = if down {
                    selected.saturating_add(3).min(max)
                } else {
                    selected.saturating_sub(3)
                };
                self.message.clear();
                return false;
            }
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k')
        ) && self.edit.is_none()
        {
            self.message.clear();
        }
        if let Some(edit) = &mut self.edit {
            match key.code {
                KeyCode::Esc => {
                    self.edit = None;
                }
                KeyCode::Enter => {
                    let text = edit.text.clone();
                    let mut next = self.saved.clone();
                    match self.selected {
                        0 => next.download_dir = Some(text.into()),
                        3 => next.player = Some(text),
                        4 => {
                            if self.argument < next.player_args.len() {
                                next.player_args[self.argument] = text;
                            } else {
                                next.player_args.push(text);
                            }
                        }
                        _ => {}
                    }
                    return self.save(next);
                }
                _ => {
                    edit.edit(key);
                }
            }
            return false;
        }
        if self.arguments {
            match key.code {
                KeyCode::Esc => self.arguments = false,
                KeyCode::Up => self.argument = self.argument.saturating_sub(1),
                KeyCode::Down => {
                    self.argument = (self.argument + 1).min(self.saved.player_args.len())
                }
                KeyCode::Enter | KeyCode::Left | KeyCode::Right => {
                    let mut input = Input::default();
                    input.set(
                        self.saved
                            .player_args
                            .get(self.argument)
                            .cloned()
                            .unwrap_or_default(),
                    );
                    self.edit = Some(input);
                }
                KeyCode::Delete if self.argument < self.saved.player_args.len() => {
                    let mut next = self.saved.clone();
                    next.player_args.remove(self.argument);
                    return self.save(next);
                }
                _ => {}
            }
            return false;
        }
        let mut next = self.saved.clone();
        match key.code {
            KeyCode::Esc | KeyCode::F(4) => self.open = false,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(5),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter | KeyCode::Left | KeyCode::Right => match self.selected {
                4 => {
                    self.arguments = true;
                    self.argument = 0;
                }
                0 | 3 => {
                    let mut input = Input::default();
                    input.set(if self.selected == 0 && self.saved.download_dir.is_none() {
                        String::new()
                    } else {
                        self.value(self.selected)
                    });
                    self.edit = Some(input);
                }
                1 => {
                    let mut mode = next.audio.unwrap_or_default();
                    mode.toggle();
                    next.audio = Some(mode);
                    return self.save(next);
                }
                2 => {
                    let i = QUALITIES
                        .iter()
                        .position(|q| Some(*q) == next.quality.as_deref())
                        .unwrap_or(0);
                    next.quality = Some(
                        QUALITIES[(i + if key.code == KeyCode::Left {
                            QUALITIES.len() - 1
                        } else {
                            1
                        }) % QUALITIES.len()]
                        .into(),
                    );
                    return self.save(next);
                }
                _ => {
                    next.player_backend = match (next.player_backend, key.code == KeyCode::Left) {
                        (None, true) => Some(Backend::Vlc),
                        (Some(Backend::Vlc), true) => Some(Backend::Mpv),
                        (Some(Backend::Mpv), true) => None,
                        (None, false) => Some(Backend::Mpv),
                        (Some(Backend::Mpv), false) => Some(Backend::Vlc),
                        _ => None,
                    };
                    return self.save(next);
                }
            },
            KeyCode::Char('r' | 'R') => {
                match self.selected {
                    0 => next.download_dir = None,
                    1 => next.audio = Some(Mode::Sub),
                    2 => next.quality = Some("best".into()),
                    3 => next.player = None,
                    4 => next.player_args.clear(),
                    5 => next.player_backend = None,
                    _ => return false,
                };
                return self.save(next);
            }
            _ => {}
        }
        false
    }
    fn save(&mut self, next: Config) -> bool {
        use crate::config::Setting;
        let edited = match self.selected {
            0 => Setting::DownloadFolder,
            1 => Setting::Audio,
            2 => Setting::Quality,
            3 => Setting::Player,
            4 => Setting::PlayerArguments,
            _ => Setting::PlayerBackend,
        };
        match next.save(&self.path, &self.saved, edited) {
            Ok(saved) => {
                self.error = false;
                self.saved = saved;
                self.edit = None;
                self.message =
                    "Saved. Applies to the next queue; CLI overrides still apply.".into();
                true
            }
            Err(error) => {
                self.error = true;
                self.message = format!("{error:#}");
                false
            }
        }
    }
    pub fn render(&self, frame: &mut Frame, state: &crate::state::State) {
        let palette = if state.colorless {
            crate::palette::Palette::terminal(true)
        } else {
            state.palette
        };
        let area = frame.area();
        let width = area
            .width
            .saturating_sub(if area.width >= 40 { 4 } else { 0 })
            .min(78);
        let height = area
            .height
            .saturating_sub(u16::from(area.height >= 24) * 2)
            .min(28);
        let modal = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let base = Style::default().fg(palette.text).bg(palette.canvas);
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(base), area);
        let block = crate::ui::panel(
            if self.arguments {
                " Player arguments "
            } else {
                " Settings "
            }
            .into(),
            state,
            true,
        )
        .style(base);
        let inner = block.inner(modal);
        frame.render_widget(block, modal);
        let [header, body, detail, editor, footer] = Layout::vertical([
            Constraint::Length(u16::from(inner.height >= 12) * 2),
            Constraint::Min(1),
            Constraint::Length(if inner.height >= 16 { 4 } else { 2 }),
            Constraint::Length(if self.edit.is_some() { 3 } else { 0 }),
            Constraint::Length(if inner.width < 48 { 2 } else { 1 }),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new("Preferences for your next queue")
                .style(Style::default().fg(palette.secondary)),
            header,
        );
        let names = [
            "Download folder",
            "Default audio",
            "Video quality",
            "Player executable",
            "Player arguments",
            "Compatibility",
        ];
        let mut items = Vec::new();
        let mut selected = 0;
        if self.arguments {
            for (i, value) in self
                .saved
                .player_args
                .iter()
                .map(String::as_str)
                .chain(std::iter::once("+ Add argument"))
                .enumerate()
            {
                items.push(ListItem::new(value.to_owned()));
                if i == self.argument {
                    selected = i;
                }
            }
        } else {
            for (i, name) in names.iter().enumerate().take(6) {
                if matches!(i, 0 | 1 | 3) {
                    if i != 0 {
                        items.push(ListItem::new(""));
                    }
                    items.push(ListItem::new(Line::styled(
                        match i {
                            0 => "DOWNLOADS",
                            1 => "PLAYBACK",
                            _ => "ADVANCED",
                        },
                        Style::default().fg(palette.focus).bold(),
                    )));
                }
                if i == self.selected {
                    selected = items.len();
                }
                let label = *name;
                let value = match i {
                    0 | 3 | 4 => self.value(i),
                    _ => format!(
                        "< {} >",
                        if i == 2 && self.value(i) == "best" {
                            "Best".into()
                        } else {
                            self.value(i)
                        }
                    ),
                };
                if body.width >= 48 {
                    let available = usize::from(body.width.saturating_sub(4));
                    let value = fit_value(&value, available.saturating_sub(label.len() + 2));
                    let gap = available.saturating_sub(
                        label.len() + unicode_width::UnicodeWidthStr::width(value.as_str()),
                    );
                    items.push(ListItem::new(Line::from(vec![
                        Span::raw(format!("{label}{}", " ".repeat(gap))),
                        Span::styled(value, Style::default().bold()),
                    ])));
                } else {
                    items.push(ListItem::new(vec![
                        Line::from(label),
                        Line::styled(format!("  {value}"), Style::default().fg(palette.secondary)),
                    ]));
                }
            }
        }
        let content_height: usize = items.iter().map(ListItem::height).sum();
        let mut selection = ListState::default();
        selection.select(Some(selected));
        frame.render_stateful_widget(
            List::new(items)
                .style(base)
                .highlight_style(palette.selection)
                .highlight_symbol("> "),
            body,
            &mut selection,
        );
        if content_height > usize::from(body.height) && body.width >= 10 {
            let indicator = format!(
                " {}/{} ",
                if self.arguments {
                    self.argument + 1
                } else {
                    self.selected + 1
                },
                if self.arguments {
                    self.saved.player_args.len() + 1
                } else {
                    6
                }
            );
            frame.render_widget(
                Paragraph::new(indicator.clone()).style(Style::default().fg(palette.secondary)),
                Rect::new(
                    modal.right().saturating_sub(indicator.len() as u16 + 2),
                    modal.y,
                    indicator.len() as u16,
                    1,
                ),
            );
        }
        let description = if !self.message.is_empty() {
            self.message.clone()
        } else if self.arguments {
            "Each entry is one argument. Spaces are kept; no shell expansion.".into()
        } else {
            match self.selected {
                0 => format!(
                    "Destination: {}\nEnter edits the folder path. R restores Automatic.",
                    self.download()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|e| e.to_string())
                ),
                1 => "Preferred language for new queues. Existing queues keep their audio.".into(),
                2 => "Preferred quality; the closest available stream is used.".into(),
                3 => "Enter edits the executable name or path. Defaults to mpv.".into(),
                4 => "Add, edit or remove individual player arguments.".into(),
                _ => "Automatic infers mpv or VLC from the executable name.".into(),
            }
        };
        let overridden = match self.selected {
            0 => self.overrides.download.is_some(),
            1 => self.overrides.audio.is_some(),
            2 => self.overrides.quality.is_some(),
            3 => self.overrides.player.is_some(),
            4 => self.overrides.args.is_some(),
            _ => self.overrides.backend.is_some(),
        };
        let description = if overridden {
            format!("CLI override active for this run. Saved default shown above.\n{description}")
        } else {
            description
        };
        frame.render_widget(
            Paragraph::new(description)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(if self.message.is_empty() {
                    palette.secondary
                } else if self.error {
                    palette.error
                } else {
                    palette.success
                })),
            detail,
        );
        if let Some(edit) = &self.edit {
            let input = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.focus))
                .title(" Edit value ");
            let input_area = input.inner(editor);
            frame.render_widget(input, editor);
            let cursor = unicode_width::UnicodeWidthStr::width(&edit.text[..edit.cursor]);
            let offset = cursor.saturating_sub(usize::from(input_area.width.saturating_sub(1)));
            frame.render_widget(
                Paragraph::new(edit.text.as_str())
                    .scroll((0, offset.min(u16::MAX as usize) as u16)),
                input_area,
            );
            if input_area.width > 0 && input_area.height > 0 {
                frame.set_cursor_position((input_area.x + (cursor - offset) as u16, input_area.y));
            }
        }
        let hint = if self.edit.is_some() {
            "Enter Save   Esc Cancel"
        } else if self.arguments {
            "Enter Edit/Add   Del Remove   Esc Back"
        } else if matches!(self.selected, 0 | 3) {
            "Enter Edit   R Reset   Esc Back"
        } else if self.selected == 4 {
            "Enter Manage   R Reset   Esc Back"
        } else if state.ascii {
            "Left/Right Change   R Reset   Esc Back"
        } else {
            "←/→ Change   R Reset   Esc Back"
        };
        frame.render_widget(
            Paragraph::new(hint)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(palette.focus)),
            footer,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn explicit_reset_overrides_concurrent_folder_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut settings = Settings::new(Config::default(), Overrides::default(), path.clone());
        std::fs::write(
            &path,
            serde_json::json!({"download_dir": dir.path().join("other"), "quality": "720p"})
                .to_string(),
        )
        .unwrap();
        assert!(settings.key(key(KeyCode::Char('r'))));
        let saved = Config::load(&path).unwrap();
        assert!(saved.download_dir.is_none());
        assert!(settings.saved.download_dir.is_none());
        assert_eq!(saved.quality.as_deref(), Some("720p"));
    }

    #[test]
    fn arrows_cycle_both_directions_and_open_editors() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::new(
            Config::default(),
            Overrides::default(),
            dir.path().join("config.json"),
        );
        settings.selected = 2;
        assert!(settings.key(key(KeyCode::Left)));
        assert_eq!(settings.saved.quality.as_deref(), Some("worst"));
        assert!(settings.key(key(KeyCode::Right)));
        assert_eq!(settings.saved.quality.as_deref(), Some("best"));
        settings.selected = 5;
        assert!(settings.key(key(KeyCode::Left)));
        assert_eq!(settings.saved.player_backend, Some(Backend::Vlc));
        assert!(settings.key(key(KeyCode::Right)));
        assert_eq!(settings.saved.player_backend, None);
        settings.selected = 0;
        settings.key(key(KeyCode::Right));
        assert!(settings.edit.is_some());
    }

    #[test]
    fn saves_defaults_without_persisting_overrides_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut settings = Settings::new(
            Config::default(),
            Overrides {
                quality: Some("360p".into()),
                download: Some(dir.path().join("override")),
                ..Default::default()
            },
            path.clone(),
        );
        settings.selected = 1;
        assert!(settings.key(key(KeyCode::Enter)));
        let saved = Config::load(&path).unwrap();
        assert_eq!(saved.audio, Some(Mode::Dub));
        assert_eq!(saved.quality, None);
        assert_eq!(saved.download_dir, None);
        assert!(settings.key(key(KeyCode::Char('r'))));
        assert_eq!(Config::load(&path).unwrap().audio, Some(Mode::Sub));
    }
    #[test]
    fn invalid_input_retains_saved_value_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut settings = Settings::new(Config::default(), Overrides::default(), path.clone());
        settings.key(key(KeyCode::Enter));
        settings.paste("relative/folder");
        assert!(!settings.key(key(KeyCode::Enter)));
        assert!(settings.edit.is_some());
        assert!(settings.saved.download_dir.is_none());
        assert!(!path.exists());
        settings.key(key(KeyCode::Esc));
        assert!(settings.edit.is_none());
    }
    #[test]
    fn all_settings_visible_and_page_renders_in_narrow_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::new(
            Config::default(),
            Overrides::default(),
            dir.path().join("config.json"),
        );
        for _ in 0..3 {
            settings.key(key(KeyCode::Down));
        }
        settings.key(key(KeyCode::Down));
        settings.key(key(KeyCode::Down));
        for (width, height) in [(30, 12), (30, 25), (50, 25), (100, 25)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    settings.render(
                        frame,
                        &crate::state::State::new(
                            String::new(),
                            Mode::Sub,
                            "best".into(),
                            false,
                            false,
                            false,
                        ),
                    )
                })
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(text.contains("Settings"));
            assert!(text.contains("Compatibility"));
        }
    }
}
