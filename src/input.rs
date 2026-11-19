use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Help,
    Audio,
    Quality,
    Provider,
    Alternate,
    Settings,
    Refresh,
    RetryQueue,
}

impl Shortcut {
    pub fn from_key(key: KeyEvent, typing: bool) -> Option<Self> {
        use KeyCode::Char;
        match (key.code, key.modifiers) {
            (Char('a'), KeyModifiers::ALT) => Some(Self::Audio),
            (Char('q'), KeyModifiers::ALT) => Some(Self::Quality),
            (Char('p'), KeyModifiers::ALT) => Some(Self::Provider),
            (Char('r'), KeyModifiers::ALT) => Some(Self::RetryQueue),
            _ if typing => None,
            (Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT) => Some(Self::Help),
            (Char('a'), KeyModifiers::NONE) => Some(Self::Alternate),
            (Char(','), KeyModifiers::NONE | KeyModifiers::SHIFT) => Some(Self::Settings),
            (Char('R'), KeyModifiers::NONE | KeyModifiers::SHIFT)
            | (Char('r'), KeyModifiers::SHIFT) => Some(Self::Refresh),
            _ => None,
        }
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

    pub fn edit(&mut self, key: KeyEvent) -> bool {
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
                _ => return false,
            }
        } else if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_keys_do_not_invoke_miru_shortcuts() {
        for number in 1..=6 {
            let key = KeyEvent::new(KeyCode::F(number), KeyModifiers::NONE);
            for typing in [false, true] {
                assert!(Shortcut::from_key(key, typing).is_none());
            }
        }
    }
}
