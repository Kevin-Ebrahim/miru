use ratatui::style::{Color, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub canvas: Color,
    pub surface: Color,
    pub text: Color,
    pub focus: Color,
    pub blossom: Color,
    pub secondary: Color,
    pub border: Color,
    pub success: Color,
    pub pending: Color,
    pub error: Color,
    pub selection: Style,
}

impl Palette {
    pub fn terminal(colorless: bool) -> Self {
        Self {
            canvas: Color::Reset,
            surface: Color::Reset,
            text: Color::Reset,
            focus: if colorless { Color::Reset } else { Color::Blue },
            blossom: if colorless {
                Color::Reset
            } else {
                Color::Magenta
            },
            secondary: Color::Reset,
            border: Color::Reset,
            success: if colorless {
                Color::Reset
            } else {
                Color::Green
            },
            pending: if colorless {
                Color::Reset
            } else {
                Color::Yellow
            },
            error: if colorless { Color::Reset } else { Color::Red },
            selection: Style::default().reversed(),
        }
    }

    pub fn themed(theme: &crate::theme::Theme, colorless: bool) -> Self {
        if colorless {
            return Self::terminal(true);
        }
        let background = theme.background;
        let mut surface = theme
            .surface
            .unwrap_or_else(|| mix(background, theme.foreground, 6));
        let endpoint = if contrast((255, 255, 255), background) > contrast((0, 0, 0), background) {
            (255, 255, 255)
        } else {
            (0, 0, 0)
        };
        if contrast(endpoint, surface) < 4.5 {
            surface = background;
        }
        let backgrounds = [background, surface];
        let muted = theme
            .muted
            .unwrap_or_else(|| mix(background, theme.foreground, 65));
        let selected_bg = mix(background, theme.accent, 25);
        Self {
            canvas: rgb(background),
            surface: rgb(surface),
            text: readable(theme.foreground, &backgrounds, 4.5),
            focus: readable(theme.accent, &backgrounds, 4.5),
            blossom: readable(theme.blossom, &backgrounds, 4.5),
            secondary: readable(muted, &backgrounds, 4.5),
            border: readable(
                theme
                    .muted
                    .unwrap_or_else(|| mix(background, theme.foreground, 40)),
                &backgrounds,
                3.0,
            ),
            success: readable(theme.success, &backgrounds, 4.5),
            pending: readable(theme.pending, &backgrounds, 4.5),
            error: readable(theme.error, &backgrounds, 4.5),
            selection: Style::default()
                .fg(readable(theme.foreground, &[selected_bg], 4.5))
                .bg(rgb(selected_bg)),
        }
    }
}

fn mix(base: crate::theme::Rgb, tint: crate::theme::Rgb, percent: u16) -> crate::theme::Rgb {
    let blend =
        |a: u8, b: u8| ((u16::from(a) * (100 - percent) + u16::from(b) * percent) / 100) as u8;
    (
        blend(base.0, tint.0),
        blend(base.1, tint.1),
        blend(base.2, tint.2),
    )
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let linear = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// Retain each hue where possible, even on a terminal background between light and dark.
fn readable(mut color: (u8, u8, u8), backgrounds: &[(u8, u8, u8)], minimum: f64) -> Color {
    let weakest = |color| {
        backgrounds
            .iter()
            .map(|&bg| contrast(color, bg))
            .fold(f64::INFINITY, f64::min)
    };
    let endpoint = if weakest((255, 255, 255)) > weakest((0, 0, 0)) {
        255
    } else {
        0
    };
    while weakest(color) < minimum {
        let step = |c: u8| {
            if endpoint == 255 {
                c.saturating_add(1)
            } else {
                c.saturating_sub(1)
            }
        };
        color = (step(color.0), step(color.1), step(color.2));
    }
    rgb(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(c: Color) -> (u8, u8, u8) {
        let Color::Rgb(r, g, b) = c else {
            panic!("Expected measured RGB color")
        };
        (r, g, b)
    }

    #[test]
    fn semantic_pairs_remain_readable_on_dark_light_and_mid_tone_backgrounds() {
        for background in [
            (46, 52, 64),
            (15, 18, 21),
            (246, 242, 232),
            (255, 255, 255),
            (128, 128, 128),
        ]
        .into_iter()
        .chain((0..=255).map(|v| (v, v, v)))
        {
            let p = Palette::themed(
                &crate::theme::Theme {
                    background,
                    foreground: (230, 230, 230),
                    accent: (180, 120, 200),
                    blossom: (210, 130, 155),
                    surface: None,
                    muted: None,
                    success: (60, 180, 90),
                    pending: (200, 160, 20),
                    error: (200, 60, 60),
                },
                false,
            );
            for bg in [background, channels(p.surface)] {
                for c in [
                    p.text,
                    p.focus,
                    p.blossom,
                    p.secondary,
                    p.success,
                    p.pending,
                    p.error,
                ] {
                    assert!(contrast(channels(c), bg) >= 4.5);
                }
                assert!(contrast(channels(p.border), bg) >= 3.0);
            }
            assert!(
                contrast(
                    channels(p.selection.fg.unwrap()),
                    channels(p.selection.bg.unwrap())
                ) >= 4.5
            );
        }
    }
}
