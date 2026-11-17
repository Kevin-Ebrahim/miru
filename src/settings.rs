use crate::{
    config::{Config, QUALITIES},
    input::Input,
    model::Mode,
    player::{Backend, Player},
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::path::PathBuf;

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
    selected: usize,
    advanced: bool,
    arguments: bool,
    argument: usize,
    scroll: u16,
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
            advanced: false,
            arguments: false,
            argument: 0,
            scroll: 0,
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
        match key.code {
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(3);
                return false;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(3);
                return false;
            }
            _ => {}
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
                KeyCode::Enter => {
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
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(if self.advanced { 5 } else { 3 })
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter if self.selected == 3 && !self.advanced => self.advanced = true,
            KeyCode::Enter => match self.selected {
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
                    next.quality = Some(QUALITIES[(i + 1) % QUALITIES.len()].into());
                    return self.save(next);
                }
                _ => {
                    next.player_backend = match next.player_backend {
                        None => Some(Backend::Mpv),
                        Some(Backend::Mpv) => Some(Backend::Vlc),
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
                    3 if self.advanced => next.player = None,
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
        match next.save(&self.path) {
            Ok(()) => {
                self.saved = next;
                self.edit = None;
                self.message =
                    "Saved. Applies to the next queue; CLI overrides still apply.".into();
                true
            }
            Err(error) => {
                self.message = format!("{error:#}");
                false
            }
        }
    }
    pub fn render(&self, frame: &mut Frame) {
        let names = [
            "Downloads / Folder",
            "Playback / Default audio",
            "Playback / Video quality",
            "Advanced / Player executable",
            "Advanced / Player arguments",
            "Advanced / Compatibility",
        ];
        let mut lines = vec![
            Line::from("Settings"),
            Line::from("Enter Change  R Reset  Esc Back  PgUp/PgDn Scroll"),
            Line::from(""),
        ];
        for (i, name) in names
            .iter()
            .enumerate()
            .take(if self.advanced { 6 } else { 4 })
        {
            lines.push(Line::from(format!(
                "{} {}",
                if self.selected == i { ">" } else { " " },
                if i == 3 && !self.advanced {
                    "Advanced…".into()
                } else {
                    format!("{name}: {}", self.value(i))
                }
            )));
        }
        if self.arguments {
            lines.clear();
            lines.push(Line::from("Player arguments"));
            lines.push(Line::from("Enter Edit/Add  Delete Remove  Esc Back"));
            for (i, argument) in self
                .saved
                .player_args
                .iter()
                .map(String::as_str)
                .chain(std::iter::once("Add argument…"))
                .enumerate()
            {
                lines.push(Line::from(format!(
                    "{} {argument}",
                    if i == self.argument { ">" } else { " " }
                )));
            }
        }
        if let Some(edit) = &self.edit {
            lines.push(Line::from(format!(
                "Edit: {}|{}",
                &edit.text[..edit.cursor],
                &edit.text[edit.cursor..]
            )));
            lines.push(Line::from("Enter Save  Esc Cancel"));
        }
        lines.push(Line::from(format!(
            "Resolved destination: {}",
            self.download()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| e.to_string())
        )));
        let o = &self.overrides;
        let active: Vec<_> = [
            (o.download.is_some(), "folder"),
            (o.audio.is_some(), "audio"),
            (o.quality.is_some(), "quality"),
            (o.player.is_some(), "player"),
            (o.args.is_some(), "arguments"),
            (o.backend.is_some(), "compatibility"),
        ]
        .into_iter()
        .filter_map(|(yes, name)| yes.then_some(name))
        .collect();
        if !active.is_empty() {
            lines.push(Line::from(format!(
                "CLI overrides this run: {}",
                active.join(", ")
            )));
        }
        lines.push(Line::from(self.message.clone()));
        let rows: usize = lines
            .iter()
            .map(|line| {
                line.width()
                    .max(1)
                    .div_ceil(usize::from(frame.area().width.max(1)))
            })
            .sum();
        let scroll = self
            .scroll
            .min(rows.saturating_sub(1).min(u16::MAX as usize) as u16);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            frame.area(),
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
    fn advanced_is_hidden_and_page_renders_in_narrow_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::new(
            Config::default(),
            Overrides::default(),
            dir.path().join("config.json"),
        );
        for _ in 0..3 {
            settings.key(key(KeyCode::Down));
        }
        assert!(!settings.advanced);
        settings.key(key(KeyCode::Enter));
        assert!(settings.advanced);
        for width in [30, 50, 100] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 25)).unwrap();
            terminal.draw(|frame| settings.render(frame)).unwrap();
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
