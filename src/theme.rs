use std::{collections::HashMap, path::PathBuf};
use tokio::io::AsyncReadExt;

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ThemeMode {
    Auto,
    Terminal,
}

pub struct Theme {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
    pub blossom: Rgb,
    pub surface: Option<Rgb>,
    pub muted: Option<Rgb>,
    pub success: Rgb,
    pub pending: Rgb,
    pub error: Rgb,
}

impl Theme {
    // Omarchy exports flat, quoted hexadecimal color assignments.
    pub fn parse(source: &str) -> Option<Self> {
        let mut colors = HashMap::new();
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                break;
            }

            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            let value = value.trim();
            let Some(quote @ ('\'' | '"')) = value.chars().next() else {
                continue;
            };
            let Some((hex, tail)) = value[1..].split_once(quote) else {
                continue;
            };
            if !tail.trim().is_empty() && !tail.trim().starts_with('#') {
                continue;
            }

            let Some(hex) = hex.strip_prefix('#') else {
                continue;
            };
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }

            let value = u32::from_str_radix(hex, 16).ok()?;
            colors.insert(
                key.trim(),
                ((value >> 16) as u8, (value >> 8) as u8, value as u8),
            );
        }

        let accent = *colors.get("accent")?;
        Some(Self {
            background: *colors.get("background")?,
            foreground: *colors.get("foreground")?,
            accent,
            blossom: colors.get("magenta").copied().unwrap_or(accent),
            surface: colors.get("lighter_background").copied(),
            muted: colors.get("muted").copied(),
            success: colors.get("green").copied().unwrap_or(accent),
            pending: colors.get("yellow").copied().unwrap_or(accent),
            error: colors.get("red").copied().unwrap_or(accent),
        })
    }
}

pub struct ThemeSource {
    paths: Vec<PathBuf>,
}

impl ThemeSource {
    pub fn new(mode: ThemeMode, colorless: bool) -> Self {
        let paths = if colorless
            || matches!(mode, ThemeMode::Terminal)
            || std::env::var_os("SSH_CONNECTION").is_some()
        {
            vec![]
        } else {
            vec![
                crate::directory(
                    "XDG_STATE_HOME",
                    ".local/state",
                    "omarchy/current/theme/colors.toml",
                ),
                crate::directory(
                    "XDG_CONFIG_HOME",
                    ".config",
                    "omarchy/current/theme/colors.toml",
                ),
            ]
        };

        Self { paths }
    }

    pub async fn load(&self, colorless: bool) -> crate::palette::Palette {
        for path in &self.paths {
            let Ok(file) = tokio::fs::File::open(path).await else {
                continue;
            };

            let mut source = String::new();
            if file.take(32_769).read_to_string(&mut source).await.is_ok()
                && source.len() <= 32_768
                && let Some(theme) = Theme::parse(&source)
            {
                return crate::palette::Palette::themed(&theme, colorless);
            }
        }

        crate::palette::Palette::terminal(colorless)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS: &str = "background = \"#101010\"\nforeground = '#eeeeee'\naccent = \"#eeaaff\" # comment\ngreen = \"#11ee88\"\n";

    #[test]
    fn reads_exported_colors_without_using_nested_or_invalid_values() {
        let theme = Theme::parse(&format!("{COLORS}[other]\naccent = '#000000'\n")).unwrap();
        assert_eq!(theme.accent, (238, 170, 255));
        assert_eq!(theme.success, (17, 238, 136));
        assert!(Theme::parse(&COLORS.replace("#101010", "#10101z")).is_none());
        assert!(Theme::parse("accent = '#ff00ff'").is_none());
    }

    #[tokio::test]
    async fn reloads_replaced_theme_and_falls_back_when_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("colors.toml");
        let source = ThemeSource {
            paths: vec![path.clone()],
        };
        let terminal = crate::palette::Palette::terminal(false);
        assert_eq!(source.load(false).await, terminal);
        std::fs::write(&path, COLORS).unwrap();
        let first = source.load(false).await;
        assert_ne!(first, terminal);
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, COLORS.replace("#eeaaff", "#aaffee")).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_ne!(source.load(false).await.focus, first.focus);
        assert_eq!(
            source.load(true).await,
            crate::palette::Palette::terminal(true)
        );
        std::fs::write(&path, "invalid").unwrap();
        assert_eq!(source.load(false).await, terminal);
    }
}
