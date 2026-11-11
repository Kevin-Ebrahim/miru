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
const DOTS: [u32; 8] = [1, 8, 2, 16, 4, 32, 64, 128];

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
                let mut mask = 0;
                let mut shade = 0.0_f32;
                let mut blossoms = 0;
                let mut count = 0;
                for dy in 0..4 {
                    for dx in 0..2 {
                        let sy = ((y - area.y) as f32 + dy as f32 / 4.0) / area.height as f32
                            * HEIGHT as f32;
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
                        mask |= DOTS[dy * 2 + dx];
                        shade += sample.shade as f32;
                        blossoms += usize::from(sample.blossom);
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let shade = shade / count as f32 * clearance;
                if shade < 8.0 {
                    continue;
                }
                let glyph = if self.ascii {
                    b".,:+*"[(count - 1) * 5 / 8] as char
                } else {
                    char::from_u32(0x2800 + mask).unwrap()
                };
                let color = if blossoms > count / 2 {
                    self.palette.focus
                } else {
                    self.palette.secondary
                };
                buf[(x, y)]
                    .set_char(glyph)
                    .set_style(tint(self.palette, color, shade / 255.0));
            }
        }
        if strength == 0.0 {
            return;
        }
        // Staggered lifetimes keep a few petals in flight between stronger gusts.
        for i in 0..28 {
            let seed = i as f32;
            let life = (time / (23.0 + seed % 5.0 * 2.0) + seed * 0.618_034).fract();
            let upper = i < 9;
            let px = 0.08 + seed % 4.0 * 0.07 + life * 1.02;
            let py = if upper {
                0.025 + (seed % 5.0) * 0.025 + 0.025 * (life * 7.0 + seed).sin()
            } else {
                0.12 + (seed % 3.0) * 0.035 + life * 0.9 + 0.035 * (life * 9.0 + seed).sin()
            };
            if px >= 1.0 || !(0.0..1.0).contains(&py) {
                continue;
            }
            let x = area.x + (px * area.width as f32) as u16;
            let y = area.y + (py * area.height as f32) as u16;
            let fade = (life * 8.0).min(1.0)
                * ((1.0 - life) * 8.0).min(1.0)
                * clearance(self.protected, x as f32, y as f32)
                * strength;
            if fade < 0.12 {
                continue;
            }
            let turn = ((time * 0.7 + seed) as usize) % 4;
            let glyph = if self.ascii {
                [',', '\'', '`', '.'][turn]
            } else {
                ['⠢', '⡔', '⢄', '⠔'][turn]
            };
            buf[(x, y)].set_char(glyph).set_style(tint(
                self.palette,
                self.palette.focus,
                0.76 * fade,
            ));
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
        let mut map = vec![Sample::default(); WIDTH * HEIGHT];
        // Art-directed boughs: one rooted trunk, an open fan, and long lateral limbs.
        let branches = [
            ([(13., 204.), (20., 163.), (13., 125.), (34., 82.)], 8., 4.),
            ([(34., 82.), (42., 59.), (31., 36.), (17., 14.)], 4., 0.6),
            ([(34., 82.), (52., 55.), (71., 26.), (102., 14.)], 4., 0.5),
            ([(29., 99.), (53., 84.), (74., 69.), (123., 64.)], 3., 0.5),
            ([(45., 65.), (87., 43.), (120., 39.), (177., 31.)], 2.8, 0.4),
            ([(70., 43.), (101., 14.), (141., 14.), (173., 11.)], 2., 0.3),
            (
                [(104., 41.), (143., 33.), (195., 35.), (237., 30.)],
                1.5,
                0.3,
            ),
            (
                [(27., 115.), (44., 108.), (61., 101.), (81., 91.)],
                2.6,
                0.3,
            ),
            ([(34., 82.), (22., 69.), (13., 65.), (-4., 63.)], 2.5, 0.3),
            ([(34., 53.), (21., 39.), (6., 38.), (-8., 31.)], 2., 0.3),
            ([(64., 48.), (49., 34.), (53., 23.), (64., 17.)], 1.8, 0.3),
            (
                [(98., 42.), (116., 49.), (145., 49.), (157., 45.)],
                1.1,
                0.2,
            ),
        ];
        for (points, start, end) in branches {
            for step in 0..=200 {
                let t = step as f32 / 200.0;
                let s = 1.0 - t;
                let weights = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
                let (mut x, mut y) = (0.0, 0.0);
                for (point, weight) in points.iter().zip(weights) {
                    x += point.0 * weight;
                    y += point.1 * weight;
                }
                dab(
                    &mut map,
                    x,
                    y,
                    start * s + end * t,
                    (start * s + end * t) * 0.85,
                    false,
                    0,
                );
            }
        }
        let clusters = [
            (6., 13., 23., 8.),
            (35., 24., 24., 9.),
            (69., 16., 24., 8.),
            (101., 8., 27., 8.),
            (139., 9., 27., 7.),
            (171., 13., 24., 7.),
            (205., 24., 27., 7.),
            (234., 31., 23., 6.),
            (1., 39., 23., 10.),
            (32., 49., 25., 9.),
            (72., 40., 28., 8.),
            (111., 31., 25., 7.),
            (145., 43., 24., 6.),
            (7., 65., 22., 10.),
            (52., 69., 21., 7.),
            (85., 61., 23., 7.),
            (115., 65., 23., 6.),
            (73., 87., 23., 7.),
        ];
        for (seed, (cx, cy, rx, ry)) in clusters.into_iter().enumerate() {
            for lobe in 0..13 {
                let angle = noise(lobe, seed, 1) * std::f32::consts::TAU;
                let radius = noise(lobe, seed, 2).sqrt();
                let x = cx + angle.cos() * rx * radius;
                let y = cy + angle.sin() * ry * radius;
                dab(
                    &mut map,
                    x,
                    y,
                    rx * (0.25 + noise(lobe, seed, 3) * 0.2),
                    ry * 0.55,
                    true,
                    seed + lobe,
                );
            }
        }
        map
    })
}

fn dab(map: &mut [Sample], cx: f32, cy: f32, rx: f32, ry: f32, blossom: bool, seed: usize) {
    let left = (cx - rx).max(0.0) as usize;
    let right = ((cx + rx).ceil() as usize).min(WIDTH - 1);
    let top = (cy - ry).max(0.0) as usize;
    let bottom = ((cy + ry).ceil() as usize).min(HEIGHT - 1);
    for y in top..=bottom {
        for x in left..=right {
            let distance = ((x as f32 - cx) / rx).powi(2) + ((y as f32 - cy) / ry).powi(2);
            if distance > 1.0 {
                continue;
            }
            let grain = noise(x, y, seed);
            let (density, ink) = if blossom {
                (
                    0.75 - distance * 0.3,
                    65.0 + 105.0 * grain + 30.0 * (1.0 - distance),
                )
            } else {
                (0.83, 30.0 + 60.0 * noise(x, y, 43))
            };
            if grain > density {
                continue;
            }
            let cell = &mut map[y * WIDTH + x];
            if ink as u8 > cell.shade {
                *cell = Sample {
                    shade: ink as u8,
                    blossom,
                };
            }
        }
    }
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
