//! Theme-coloured, cell-native sakura for the Home canvas.
use crate::palette::Palette;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::sync::OnceLock;

const WIDTH: usize = 360;
const HEIGHT: usize = 200;

pub struct Motion {
    step: u32,
    quiet: u8,
    strength: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            step: 0,
            quiet: 25,
            strength: 0.0,
        }
    }
}

impl Motion {
    pub fn interact(&mut self) {
        self.quiet = 42;
    }

    // Called at the scene's 120 ms interval, only while Home is visible.
    pub fn advance(&mut self) -> bool {
        self.quiet = self.quiet.saturating_sub(1);
        let target = if self.quiet == 0 { 1.0 } else { 0.0 };
        let previous = self.strength;
        self.strength += (target - self.strength).clamp(-0.12, 0.08);
        if self.strength > 0.0 {
            self.step = self.step.wrapping_add(1);
        }
        self.strength > 0.0 || previous > 0.0
    }
}

pub fn fits(area: Rect) -> bool {
    area.width >= 100 && area.height >= 27
}

pub struct Sakura<'a> {
    pub palette: Palette,
    pub protected: Rect,
    pub motion: &'a Motion,
    pub animate: bool,
    pub ascii: bool,
}

impl Widget for Sakura<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !fits(area) {
            return;
        }
        let tree = tree();
        let blossom_color = match (self.palette.focus, self.palette.text) {
            (Color::Rgb(r, g, b), Color::Rgb(tr, tg, tb)) => {
                Color::Rgb(r / 2 + tr / 2, g / 2 + tg / 2, b / 2 + tb / 2)
            }
            _ => self.palette.focus,
        };
        let time = self.motion.step as f32 * 0.12;
        let strength = if self.animate {
            self.motion.strength
        } else {
            0.0
        };
        let breeze = (time * 0.62).sin() * (0.35 + 0.65 * (time * 0.17).sin().powi(2)) * strength;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let clearance = clearance(self.protected, x as f32, y as f32);
                if clearance == 0.0 {
                    continue;
                }
                let mut shade = 0.0_f32;
                let mut blossoms = 0;
                let mut count = 0;
                for dy in 0..4 {
                    for dx in 0..2 {
                        let sy = (((y - area.y) as f32 + dy as f32 / 4.0) / area.height as f32
                            * HEIGHT as f32)
                            .min(HEIGHT as f32 - 1.0);
                        let sway = breeze * 1.5 * (1.0 - sy / 100.0).max(0.0);
                        let sx = ((x - area.x) as f32 + dx as f32 / 2.0) / area.width as f32
                            * WIDTH as f32
                            - sway;
                        if sx < 0.0 || sx >= WIDTH as f32 {
                            continue;
                        }
                        let sample = tree[sy as usize * WIDTH + sx as usize];
                        if sample.shade == 0 {
                            continue;
                        }
                        shade += sample.shade as f32;
                        blossoms += usize::from(sample.blossom);
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let shade = shade / count as f32;
                if shade * clearance < 8.0 {
                    continue;
                }
                let color = if blossoms > count / 2 {
                    blossom_color
                } else {
                    self.palette.secondary
                };
                let cell = &mut buf[(x, y)];
                if self.ascii {
                    cell.set_char(b".,:+*"[(count - 1) * 5 / 8] as char);
                } else if count >= 5 {
                    cell.set_symbol(if noise(x as usize, y as usize, 91) < 0.2 {
                        "•\u{030a}\u{0325}"
                    } else {
                        "∘\u{030a}\u{0325}"
                    });
                } else if count >= 3 {
                    cell.set_symbol("∘\u{030a}");
                } else {
                    cell.set_char('·');
                }
                // Lift faint branches and blossom midtones without flattening highlights.
                let ink = (shade / 255.0 * if blossoms > count / 2 { 1.35 } else { 1.5 })
                    .sqrt()
                    .min(0.95);
                cell.set_style(tint(self.palette, color, ink * clearance));
            }
        }
        if strength == 0.0 {
            return;
        }
        // Drift, sway, tumble and halftone dots adapted from Kyoto Dusk.
        // See assets/kyoto-dusk/LICENSE. Motion uses its 200 x 100 scene coordinates.
        const BAYER: [f32; 16] = [
            0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
        ];
        const COVER: [f32; 4] = [0., 0.3, 0.6, 1.];
        for i in 0..50 {
            let phase = noise(i, 107, 0) * std::f32::consts::TAU;
            let frequency = 0.6 + noise(i, 106, 0) * 1.2;
            let drift = 3.0 + noise(i, 103, 0) * 4.0;
            let fall = 1.6 + noise(i, 104, 0) * 2.2;
            let sway = 1.0 + noise(i, 105, 0) * 2.0;
            let tumble = 2.0 + noise(i, 108, 0) * 4.0;
            let px = (noise(i, 101, 0).powf(2.2) * 140.0
                + drift * time
                + sway * (time * frequency + phase).sin())
            .rem_euclid(240.0)
                - 20.0;
            let py = (noise(i, 102, 0) * 120.0
                + fall * time
                + 0.6 * (time * frequency * 1.7 + phase).sin())
            .rem_euclid(120.0)
                - 10.0;
            if !(0.0..200.0).contains(&px) || !(0.0..100.0).contains(&py) {
                continue;
            }
            let x = area.x + (px / 200.0 * area.width as f32) as u16;
            let y = area.y + (py / 100.0 * area.height as f32) as u16;
            let edge = (px / 5.0)
                .min((200.0 - px) / 5.0)
                .min(py / 3.0)
                .min((100.0 - py) / 3.0)
                .clamp(0.0, 1.0);
            let fade = strength * edge * clearance(self.protected, x as f32, y as f32);
            if fade < 0.12 {
                continue;
            }
            let face = 0.55 + 0.45 * (time * tumble + phase).sin().abs();
            let level = (0.3 + 0.7 * face.powf(1.1) * 1.02).min(1.0);
            let local_x = usize::from(x - area.x);
            let local_y = usize::from(y - area.y);
            let dither = (BAYER[(local_y & 3) * 4 + (local_x & 3)] / 16.0 - 0.47) * 0.4
                + (noise(local_y * usize::from(area.width) + local_x, 77, 0) - 0.5) * 0.5;
            let step = (level * 3.0 + dither).round().clamp(1.0, 3.0) as usize;
            let glyph = if self.ascii {
                [' ', '.', 'o', 'O'][step]
            } else {
                [' ', '·', '•', '●'][step]
            };
            let ink = 0.3 + 0.7 * ((level + 0.06) / COVER[step]).min(1.0);
            buf[(x, y)]
                .set_char(glyph)
                .set_style(tint(self.palette, blossom_color, ink * fade));
        }
    }
}

fn clearance(rect: Rect, x: f32, y: f32) -> f32 {
    let dx = (rect.left() as f32 - x)
        .max(x - (rect.right() - 1) as f32)
        .max(0.0);
    let dy = (rect.top() as f32 - y)
        .max(y - (rect.bottom() - 1) as f32)
        .max(0.0);
    // A soft margin in physical cell proportions avoids a rectangular cutout.
    ((dx * dx + dy * dy * 4.0).sqrt() / 5.0).clamp(0.0, 1.0)
}

fn tint(palette: Palette, color: Color, amount: f32) -> Style {
    match (palette.canvas, color) {
        (Color::Rgb(r, g, b), Color::Rgb(cr, cg, cb)) => {
            let mix = |base: u8, ink: u8| (base as f32 + (ink as f32 - base as f32) * amount) as u8;
            Style::default().fg(Color::Rgb(mix(r, cr), mix(g, cg), mix(b, cb)))
        }
        _ => Style::default().fg(color).dim(),
    }
}

#[derive(Clone, Copy, Default)]
struct Sample {
    shade: u8,
    blossom: bool,
}

fn noise(x: usize, y: usize, seed: usize) -> f32 {
    let mut n = (x as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((y as u32).wrapping_mul(668_265_263))
        .wrapping_add((seed as u32).wrapping_mul(127_412_617));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    (n ^ (n >> 16)) as f32 / u32::MAX as f32
}

fn tree() -> &'static [Sample] {
    static TREE: OnceLock<Vec<Sample>> = OnceLock::new();
    TREE.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!("../assets/sakura/reference.png"))
            .expect("bundled approved sakura reference")
            .to_rgb8();
        let mut map = vec![Sample::default(); WIDTH * HEIGHT];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let u = x as f32 / WIDTH as f32;
                let v = y as f32 / HEIGHT as f32;
                // The source is the approved composition. Read only its tree;
                // controls, background and static petals are never used as artwork.
                let tree_region = u < 0.17
                    || (u < 0.26 && v < 0.35)
                    || (u < 0.29 && v < 0.21)
                    || (u < 0.72 && v < 0.19);
                if !tree_region {
                    continue;
                }
                let sx = x as u32 * image.width() / WIDTH as u32;
                let sy = y as u32 * image.height() / HEIGHT as u32;
                let ex = ((x + 1) as u32 * image.width() / WIDTH as u32).max(sx + 1);
                let ey = ((y + 1) as u32 * image.height() / HEIGHT as u32).max(sy + 1);
                let mut peak = 0;
                for py in sy..ey {
                    for px in sx..ex {
                        peak = peak.max(image.get_pixel(px, py)[1]);
                    }
                }
                // Extract ink coverage, not the source's green hue. Every emitted
                // cell is recoloured using the live terminal palette.
                if peak >= 64 {
                    let shade = ((peak as f32 - 45.0) / 150.0 * 255.0).clamp(0.0, 255.0) as u8;
                    map[y * WIDTH + x] = Sample {
                        shade,
                        blossom: v < 0.34 && peak > 95,
                    };
                }
            }
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(
        area: Rect,
        protected: Rect,
        motion: &Motion,
        animate: bool,
        ascii: bool,
        palette: Palette,
    ) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, area.right() + 2, area.bottom() + 2));
        for cell in &mut buf.content {
            cell.set_char('@');
        }
        Sakura {
            palette,
            protected,
            motion,
            animate,
            ascii,
        }
        .render(area, &mut buf);
        buf
    }

    #[test]
    fn artwork_never_touches_controls_or_cells_outside_its_canvas() {
        let area = Rect::new(3, 2, 133, 32);
        let protected = Rect::new(37, 7, 68, 17);
        let mut motion = Motion::default();
        for _ in 0..130 {
            motion.advance();
        }
        for ascii in [false, true] {
            let buf = draw(
                area,
                protected,
                &motion,
                true,
                ascii,
                Palette::terminal(true),
            );
            let mut painted = 0;
            for pos in buf.area.positions() {
                let cell = &buf[pos];
                if protected.contains(pos) || !area.contains(pos) {
                    assert_eq!(cell.symbol(), "@");
                }
                if cell.symbol() != "@" {
                    painted += 1;
                }
                if ascii {
                    assert!(cell.symbol().is_ascii());
                }
                assert_eq!(cell.bg, Color::Reset);
                assert_eq!(cell.fg, Color::Reset);
            }
            assert!(
                painted > 100,
                "tree must remain visible in the available margins"
            );
        }
    }

    #[test]
    fn compact_canvases_are_left_clear_and_reduced_motion_stays_still() {
        let motion = Motion::default();
        for (width, height) in [(40, 18), (80, 24), (99, 40), (160, 26)] {
            let buf = draw(
                Rect::new(0, 0, width, height),
                Rect::new(1, 1, 1, 1),
                &motion,
                true,
                false,
                Palette::terminal(false),
            );
            assert!(buf.content.iter().all(|cell| cell.symbol() == "@"));
        }
        let area = Rect::new(0, 0, 133, 32);
        let protected = Rect::new(32, 6, 68, 17);
        let still = draw(
            area,
            protected,
            &motion,
            false,
            false,
            Palette::terminal(false),
        );
        let mut later = Motion::default();
        for _ in 0..180 {
            later.advance();
        }
        assert_eq!(
            still,
            draw(
                area,
                protected,
                &later,
                false,
                false,
                Palette::terminal(false)
            )
        );
        assert_ne!(
            still,
            draw(
                area,
                protected,
                &later,
                true,
                false,
                Palette::terminal(false)
            )
        );
    }

    #[test]
    fn typing_settles_motion_then_idle_resumes_without_restarting() {
        let mut motion = Motion::default();
        for _ in 0..80 {
            motion.advance();
        }
        let first = motion.step;
        motion.interact();
        for _ in 0..15 {
            motion.advance();
        }
        assert_eq!(motion.strength, 0.0);
        assert!(!motion.advance(), "settled scene must not request redraws");
        let settled = motion.step;
        assert!(settled >= first);
        for _ in 0..50 {
            motion.advance();
        }
        assert!(motion.step > settled && motion.strength > 0.0);
    }

    #[test]
    fn tree_recolours_without_changing_its_silhouette() {
        let area = Rect::new(0, 0, 133, 32);
        let protected = Rect::new(32, 6, 68, 17);
        let palette = |source| Palette::themed(&crate::theme::Theme::parse(source).unwrap(), false);
        let dark = palette("background = '#172b23'\nforeground = '#e1eee7'\naccent = '#8fbaa7'");
        let light = palette("background = '#f6f1e8'\nforeground = '#24282b'\naccent = '#68507e'");
        let motion = Motion::default();
        let a = draw(area, protected, &motion, false, false, dark);
        let b = draw(area, protected, &motion, false, false, light);
        assert!(
            a.content
                .iter()
                .zip(&b.content)
                .all(|(a, b)| a.symbol() == b.symbol())
        );
        assert_ne!(a, b);
    }
}
