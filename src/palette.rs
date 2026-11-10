use ratatui::style::{Color, Style};

#[derive(Clone, Copy)]
pub struct Palette {
    pub surface: Color,
    pub text: Color,
    pub focus: Color,
    pub secondary: Color,
    pub border: Color,
    pub success: Color,
    pub pending: Color,
    pub error: Color,
    pub selection: Style,
}

impl Palette {
    pub fn terminal(background: Option<(u8, u8, u8)>, colorless: bool) -> Self {
        if colorless {
            return Self {
                surface: Color::Reset,
                text: Color::Reset,
                focus: Color::Reset,
                secondary: Color::Reset,
                border: Color::Reset,
                success: Color::Reset,
                pending: Color::Reset,
                error: Color::Reset,
                selection: Style::default().reversed(),
            };
        }
        let Some(background) = background else {
            return Self {
                surface: Color::Reset,
                text: Color::Reset,
                focus: Color::Cyan,
                secondary: Color::Reset,
                border: Color::Reset,
                success: Color::Green,
                pending: Color::Yellow,
                error: Color::LightRed,
                selection: Style::default().reversed(),
            };
        };
        let dark = luminance(background) < 0.35;
        let (focus, secondary, border, success, pending, error, selected_fg, selected_bg) = if dark
        {
            (
                (136, 192, 208),
                (174, 187, 204),
                (88, 100, 116),
                (163, 211, 156),
                (235, 203, 139),
                (255, 154, 162),
                (227, 248, 255),
                (36, 73, 91),
            )
        } else {
            (
                (0, 102, 117),
                (71, 85, 105),
                (130, 139, 152),
                (35, 105, 54),
                (125, 80, 0),
                (170, 35, 52),
                (18, 55, 67),
                (214, 238, 242),
            )
        };
        let blend = |base: u8, tint: u8| ((u16::from(base) * 92 + u16::from(tint) * 8) / 100) as u8;
        let mut surface = (
            blend(background.0, focus.0),
            blend(background.1, focus.1),
            blend(background.2, focus.2),
        );
        let foreground = if contrast((255, 255, 255), background) > contrast((0, 0, 0), background)
        {
            (255, 255, 255)
        } else {
            (0, 0, 0)
        };
        if contrast(foreground, surface) < 4.5 {
            surface = background;
        }
        let backgrounds = [background, surface];
        Self {
            surface: rgb(surface),
            text: readable(
                if dark { (224, 228, 234) } else { (30, 38, 48) },
                &backgrounds,
                4.5,
            ),
            focus: readable(focus, &backgrounds, 4.5),
            secondary: readable(secondary, &backgrounds, 4.5),
            border: readable(border, &backgrounds, 3.0),
            success: readable(success, &backgrounds, 4.5),
            pending: readable(pending, &backgrounds, 4.5),
            error: readable(error, &backgrounds, 4.5),
            selection: Style::default().fg(rgb(selected_fg)).bg(rgb(selected_bg)),
        }
    }
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
            let p = Palette::terminal(Some(background), false);
            for bg in [background, channels(p.surface)] {
                for c in [p.text, p.focus, p.secondary, p.success, p.pending, p.error] {
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
