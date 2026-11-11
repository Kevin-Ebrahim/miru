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
const BRAILLE: [u32; 8] = [1, 8, 2, 16, 4, 32, 64, 128];
const BAYER: [f32; 16] = [
    0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
];

pub struct Motion {
    step: u32,
}

impl Default for Motion {
    fn default() -> Self {
        // Start mid-breeze, with petals already distributed across the canvas.
        Self { step: 200 }
    }
}

impl Motion {
    // Called at the scene's 120 ms interval, only while Home is visible.
    pub fn advance(&mut self) -> bool {
        self.step = self.step.wrapping_add(1);
        true
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
        let blossom_color = self.palette.blossom;
        let time = self.motion.step as f32 * 0.12;
        let strength = if self.animate { 1.0 } else { 0.0 };
        let (gust, wind_travel) = wind(time);
        let breeze = ((time * 0.62).sin() * 0.28 + gust * 1.2) * strength;
        let atmosphere = matches!(
            (self.palette.canvas, self.palette.focus),
            (Color::Rgb(..), Color::Rgb(..))
        );
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let clearance = clearance(self.protected, x as f32, y as f32);
                if clearance == 0.0 {
                    continue;
                }
                let scene_x = usize::from(x - area.x) * WIDTH / usize::from(area.width);
                let scene_y = usize::from(y - area.y) * HEIGHT / usize::from(area.height);
                if atmosphere
                    && let Some(background) = tint(
                        self.palette,
                        self.palette.focus,
                        f32::from(tree[scene_y * WIDTH + scene_x].haze) / 255.0 * clearance,
                    )
                    .fg
                {
                    buf[(x, y)].set_bg(background);
                }
                let mut shade = 0.0_f32;
                let mut blossoms = 0;
                let mut count = 0;
                let mut mask = 0;
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
                        mask |= BRAILLE[dy * 2 + dx];
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
                    // A little foreground light on the upper surfaces; shadows retain
                    // the theme's richer blossom hue, on light backgrounds too.
                    match (blossom_color, self.palette.text) {
                        (Color::Rgb(r, g, b), Color::Rgb(tr, tg, tb)) => {
                            let light = ((shade / 255.0 - 0.55) / 0.45).clamp(0.0, 1.0) * 0.2;
                            let mix =
                                |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * light) as u8;
                            Color::Rgb(mix(r, tr), mix(g, tg), mix(b, tb))
                        }
                        _ => blossom_color,
                    }
                } else {
                    self.palette.secondary
                };
                let cell = &mut buf[(x, y)];
                if self.ascii {
                    cell.set_char(b".,:+*"[(count - 1) * 5 / 8] as char);
                } else if blossoms <= count / 2 {
                    cell.set_char(char::from_u32(0x2800 + mask).unwrap());
                } else if count >= 5 {
                    // Halftone coverage follows the lighting, instead of repeating one stamp.
                    let tone = shade / 255.0
                        + (BAYER[(scene_y & 3) * 4 + (scene_x & 3)] / 16.0 - 0.47) * 0.16;
                    cell.set_symbol(if tone > 0.72 {
                        "•\u{0307}\u{0323}"
                    } else if tone > 0.48 {
                        "•\u{0323}"
                    } else {
                        "∘\u{030a}\u{0325}"
                    });
                } else if count >= 3 {
                    cell.set_symbol("∘\u{030a}");
                } else {
                    cell.set_char('·');
                }
                // Lift faint branches and blossom midtones without flattening highlights.
                let ink = if blossoms > count / 2 {
                    (shade / 255.0).powf(0.8).clamp(0.34, 0.95)
                } else {
                    (shade / 255.0 * 1.5).sqrt().min(0.95)
                };
                cell.set_style(tint(self.palette, color, ink * clearance));
            }
        }
        if strength == 0.0 {
            return;
        }
        // Drift, sway, tumble and halftone dots adapted from Kyoto Dusk.
        // See assets/kyoto-dusk/LICENSE. Motion uses its 200 x 100 scene coordinates.
        const COVER: [f32; 4] = [0., 0.3, 0.6, 1.];
        for i in 0..50 {
            // Draw distant petals first, with smaller dots and slower motion.
            let (depth, opacity, min_step, max_step) = match i {
                0..18 => (0.48, 0.45, 1, 2),
                18..36 => (0.75, 0.72, 1, 2),
                _ => (1.0, 1.0, 2, 3),
            };
            let phase = noise(i, 107, 0) * std::f32::consts::TAU;
            let frequency = 0.6 + noise(i, 106, 0) * 1.2;
            let drift = 3.0 + noise(i, 103, 0) * 4.0;
            let fall = 1.6 + noise(i, 104, 0) * 2.2;
            let sway = 1.0 + noise(i, 105, 0) * 2.0;
            let tumble = 2.0 + noise(i, 108, 0) * 4.0;
            let px = (noise(i, 101, 0).powf(2.2) * 140.0
                + depth * (drift * time + wind_travel)
                + depth * sway * (time * frequency + phase).sin())
            .rem_euclid(240.0)
                - 20.0;
            let py = (noise(i, 102, 0) * 120.0
                + depth * fall * time
                + 0.6 * (time * frequency * 1.7 + phase).sin())
            .rem_euclid(120.0)
                - 10.0;
            if !(0.0..200.0).contains(&px) || !(0.0..100.0).contains(&py) {
                continue;
            }
            let x = area.x + (px / 200.0 * area.width as f32) as u16;
            let y = area.y + (py / 100.0 * area.height as f32) as u16;
            if i < 18 {
                let tx = (px / 200.0 * WIDTH as f32) as usize;
                let ty = (py / 100.0 * HEIGHT as f32) as usize;
                if tree[ty * WIDTH + tx].shade > 0 {
                    continue;
                }
            }
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
            let step = (level * 3.0 * depth + dither)
                .round()
                .clamp(min_step as f32, max_step as f32) as usize;
            let glyph = if self.ascii {
                [' ', '.', 'o', 'O'][step]
            } else {
                [' ', '·', '•', '●'][step]
            };
            let ink = 0.3 + 0.7 * ((level + 0.06) / COVER[step]).min(1.0);
            buf[(x, y)].set_char(glyph).set_style(tint(
                self.palette,
                blossom_color,
                ink * fade * opacity,
            ));
        }
    }
}

// Five-second gusts every eighteen seconds. Integrate the raised cosine so
// petals accelerate together without jumping when a gust begins or ends.
fn wind(time: f32) -> (f32, f32) {
    let time = time + 10.0;
    let phase = time.rem_euclid(18.0).min(5.0);
    let angle = std::f32::consts::TAU * phase / 5.0;
    let gust = (1.0 - angle.cos()) * 0.5;
    let travel = 3.0
        * ((time / 18.0).floor() * 2.5 + phase * 0.5 - 2.5 / std::f32::consts::TAU * angle.sin());
    (gust, travel)
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
    haze: u8,
}

fn noise(x: usize, y: usize, seed: usize) -> f32 {
    let mut n = (x as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((y as u32).wrapping_mul(668_265_263))
        .wrapping_add((seed as u32).wrapping_mul(127_412_617));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    (n ^ (n >> 16)) as f32 / u32::MAX as f32
}

// Smooth, layered noise from Kyoto Dusk's clump and lighting treatment.
fn cloud_noise(x: f32, y: f32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    for _ in 0..3 {
        let (x, y) = (x * frequency, y * frequency);
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x.fract(), y.fract());
        let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let a = noise(ix, iy, 0);
        let b = noise(ix + 1, iy, 0);
        let c = noise(ix, iy + 1, 0);
        let d = noise(ix + 1, iy + 1, 0);
        sum += amplitude * (a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v);
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / 0.875
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
                let veil = cloud_noise(u * 3.0 + 7.0, v * 4.0 + 2.0);
                let glow = (-((u - 0.10).powi(2) * 22.0 + (v - 0.48).powi(2) * 8.0)).exp();
                let haze = ((0.012 * veil + 0.035 * glow) * 255.0) as u8;
                map[y * WIDTH + x].haze = haze;
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
                    let blossom = v < 0.34 && peak > 95;
                    let clump = cloud_noise(u * 22.0 + 2.0, v * 27.0 + 5.0);
                    let light = if blossom {
                        0.65 + 0.85 * clump + 0.12 * glow - 0.16 * (v / 0.34)
                    } else {
                        0.95 + 0.18 * glow
                    };
                    let shade =
                        ((peak as f32 - 45.0) / 150.0 * light * 255.0).clamp(0.0, 255.0) as u8;
                    map[y * WIDTH + x] = Sample {
                        shade,
                        blossom,
                        haze,
                    };
                }
            }
        }
        // Shade each cluster from above: exposed top edges catch light, while
        // undersides and dense interior pockets keep the richer blossom colour.
        let source = map.clone();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let sample = &mut map[y * WIDTH + x];
                if !sample.blossom {
                    continue;
                }
                let mut above = 0.0;
                let mut below = 0.0;
                for dy in 1..=5 {
                    for dx in -4..=4 {
                        let sx = (x as isize + dx).clamp(0, WIDTH as isize - 1) as usize;
                        above += f32::from(
                            y >= dy && source[(y.saturating_sub(dy)) * WIDTH + sx].blossom,
                        );
                        below += f32::from(
                            y + dy < HEIGHT
                                && source[(y + dy).min(HEIGHT - 1) * WIDTH + sx].blossom,
                        );
                    }
                }
                let edge = (below - above) / 45.0;
                let pocket = (above + below) / 90.0;
                let light = 0.95 + 0.55 * edge - 0.22 * pocket;
                sample.shade = (sample.shade as f32 * light).clamp(0.0, 255.0) as u8;
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
        let themed = Palette::themed(
            &crate::theme::Theme::parse(
                "background = '#172b23'\nforeground = '#e1eee7'\naccent = '#8fbaa7'",
            )
            .unwrap(),
            false,
        );
        for (ascii, palette) in [
            (false, Palette::terminal(true)),
            (true, Palette::terminal(true)),
            (false, themed),
            (true, themed),
        ] {
            let buf = draw(area, protected, &motion, true, ascii, palette);
            let mut painted = 0;
            for pos in buf.area.positions() {
                let cell = &buf[pos];
                if protected.contains(pos) || !area.contains(pos) {
                    assert_eq!(cell.symbol(), "@");
                    assert_eq!(
                        cell.bg,
                        Color::Reset,
                        "atmosphere must stay outside controls"
                    );
                    assert_eq!(cell.fg, Color::Reset);
                }
                if cell.symbol() != "@" {
                    painted += 1;
                }
                if ascii {
                    assert!(cell.symbol().is_ascii());
                }
                if palette.canvas == Color::Reset {
                    assert_eq!(cell.bg, Color::Reset);
                    assert_eq!(cell.fg, Color::Reset);
                }
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
    fn petals_are_visible_on_the_first_frame_and_keep_moving() {
        let area = Rect::new(0, 0, 133, 32);
        let protected = Rect::new(32, 6, 68, 17);
        let mut motion = Motion::default();
        let first = draw(
            area,
            protected,
            &motion,
            true,
            false,
            Palette::terminal(false),
        );
        assert!(first.content.iter().any(|cell| cell.symbol() == "●"));
        assert!(
            area.positions()
                .any(|p| p.x > area.width * 3 / 4 && matches!(first[p].symbol(), "●" | "•"))
        );
        for _ in 0..10 {
            motion.advance();
        }
        let later = draw(
            area,
            protected,
            &motion,
            true,
            false,
            Palette::terminal(false),
        );
        assert_ne!(first, later);
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
