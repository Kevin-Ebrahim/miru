//! Native playback of ascii.rest's MIT-licensed Kyoto Dusk cell animation.
//! See assets/kyoto-dusk/LICENSE and scripts/generate-dusk.mjs.
use flate2::read::ZlibDecoder;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::{io::Read, sync::OnceLock};

pub const FRAME_INTERVAL_MS: u64 = 120;
const BACKGROUND: Color = Color::Rgb(11, 10, 22);
const DOT_BITS: [u32; 8] = [1, 8, 2, 16, 4, 32, 64, 128];
// Braille dots have a fixed size: express the source's small/medium/large dots
// as light intensity, keeping the sky subdued and the blossom highlights bright.
const COVERAGE: [f32; 4] = [0.0, 0.1, 0.4, 1.0];

pub struct Dusk {
    pub tick: usize,
    pub animate: bool,
    pub ascii: bool,
    pub monochrome: bool,
}

impl Widget for Dusk {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 24 || area.height < 6 {
            return;
        }
        // The source has square cells and a 2:1 aspect ratio. Terminal cells are
        // approximately twice as tall as they are wide; fit without cropping.
        let height = area.height.min(area.width / 4).min(30);
        let width = height * 4;
        let area = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let scene = frames();
        let frame = scene.frame(if self.animate { self.tick } else { 0 });
        for y in 0..height {
            for x in 0..width {
                // Eight source samples per character preserve the silhouette and
                // blossom clusters even when the panel is much smaller than 200x100.
                let mut points = [[0.0; 3]; 8];
                for dy in 0..4 {
                    for dx in 0..2 {
                        let px = usize::from(x) * 2 + dx;
                        let py = usize::from(y) * 4 + dy;
                        let sx = px * scene.width / (usize::from(width) * 2);
                        let sy = py * scene.height / (usize::from(height) * 4);
                        let ex = ((px + 1) * scene.width).div_ceil(usize::from(width) * 2);
                        let ey = ((py + 1) * scene.height).div_ceil(usize::from(height) * 4);
                        // Preserve narrow roof rims and moon highlights when reducing
                        // the scene, instead of dropping them between sample centres.
                        let mut brightest = 0.0;
                        for yy in sy..ey {
                            for xx in sx..ex {
                                let cell = frame[yy * scene.width + xx];
                                let rgb = scene.palette[usize::from(cell & 63)];
                                let coverage = COVERAGE[usize::from(cell >> 6)];
                                // Scale the colour together so bright pink stays pink.
                                let peak = f32::from(*rgb.iter().max().unwrap()).max(1.0);
                                let scale = (coverage / 0.6).min(255.0 / peak);
                                let point = rgb.map(|c| f32::from(c) * scale);
                                let brightness = point.iter().copied().fold(0.0_f32, f32::max);
                                if brightness > brightest {
                                    brightest = brightness;
                                    points[dy * 2 + dx] = point;
                                }
                            }
                        }
                    }
                }
                let peak = points.iter().flatten().copied().fold(0.0_f32, f32::max);
                let mut rgb = [0.0; 3];
                let mut count = 0;
                let mut mask = 0;
                for (i, point) in points.iter().enumerate() {
                    if point.iter().copied().fold(0.0_f32, f32::max) > (peak * 0.38).max(6.0) {
                        mask |= DOT_BITS[i];
                        count += 1;
                        for c in 0..3 {
                            rgb[c] += point[c];
                        }
                    }
                }
                let rgb = rgb.map(|c| (c / count.max(1) as f32) as u8);
                let glyph = if self.ascii || self.monochrome {
                    let brightness = (0.3 * f32::from(rgb[0])
                        + 0.5 * f32::from(rgb[1])
                        + 0.2 * f32::from(rgb[2]))
                        * count as f32
                        / 8.0;
                    b" .:-=+*#%@"[(brightness / 256.0 * 10.0) as usize] as char
                } else if mask == 0 {
                    ' '
                } else {
                    char::from_u32(0x2800 + mask).expect("braille mask is eight bits")
                };
                if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                    cell.reset();
                    let style = if self.monochrome {
                        Style::default()
                    } else {
                        Style::default()
                            .fg(Color::Rgb(rgb[0], rgb[1], rgb[2]))
                            .bg(BACKGROUND)
                    };
                    cell.set_char(glyph).set_style(style);
                }
            }
        }
    }
}

struct Frames {
    width: usize,
    height: usize,
    count: usize,
    palette: Vec<[u8; 3]>,
    data: Vec<u8>,
    offset: usize,
}

impl Frames {
    fn load() -> Self {
        let mut data = Vec::new();
        ZlibDecoder::new(include_bytes!("../assets/kyoto-dusk/frames.zlib").as_slice())
            .read_to_end(&mut data)
            .expect("bundled Kyoto Dusk frames must decompress");
        assert_eq!(&data[..4], b"AWKD", "invalid scene header");
        let field = |i| usize::from(u16::from_le_bytes([data[i], data[i + 1]]));
        let (width, height, count) = (field(4), field(6), field(8));
        assert_eq!(field(10) as u64, FRAME_INTERVAL_MS);
        let offset = 13 + usize::from(data[12]) * 3;
        assert!(width > 0 && height > 0 && count > 0 && data[12] <= 64);
        assert_eq!(data.len(), offset + width * height * count);
        let palette: Vec<_> = data[13..offset]
            .chunks_exact(3)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(
            data[offset..]
                .iter()
                .all(|v| usize::from(v & 63) < palette.len())
        );
        Self {
            width,
            height,
            count,
            palette,
            data,
            offset,
        }
    }

    fn frame(&self, tick: usize) -> &[u8] {
        let cells = self.width * self.height;
        let start = self.offset + (tick % self.count) * cells;
        &self.data[start..start + cells]
    }
}

fn frames() -> &'static Frames {
    static FRAMES: OnceLock<Frames> = OnceLock::new();
    FRAMES.get_or_init(Frames::load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_animation_has_an_exact_loop_boundary() {
        let scene = frames();
        assert_eq!((scene.width, scene.height), (200, 100));
        assert_eq!(scene.frame(0), scene.frame(scene.count - 1));
        assert_eq!(scene.frame(0), scene.frame(scene.count));
        assert_ne!(scene.frame(0), scene.frame(80));
    }

    #[test]
    fn scene_stays_inside_its_panel_in_both_character_modes() {
        for ascii in [false, true] {
            for (width, height) in [(0, 0), (23, 5), (24, 6), (38, 13), (52, 18), (140, 40)] {
                let panel = Rect::new(3, 2, width, height);
                let bounds = Rect::new(0, 0, width + 6, height + 4);
                let mut buf = Buffer::empty(bounds);
                for cell in &mut buf.content {
                    cell.set_char('@');
                }
                Dusk {
                    tick: usize::MAX,
                    animate: true,
                    ascii,
                    monochrome: false,
                }
                .render(panel, &mut buf);
                for position in bounds.positions() {
                    let cell = &buf[position];
                    if !panel.contains(position) {
                        assert_eq!(cell.symbol(), "@", "scene escaped {panel:?}");
                    }
                    if ascii {
                        assert!(cell.symbol().is_ascii());
                    }
                }
            }
        }
    }

    #[test]
    fn reduced_motion_is_still_but_animation_advances() {
        let area = Rect::new(0, 0, 80, 24);
        let render = |tick, animate| {
            let mut buf = Buffer::empty(area);
            Dusk {
                tick,
                animate,
                ascii: false,
                monochrome: false,
            }
            .render(area, &mut buf);
            buf
        };
        assert_eq!(render(0, false), render(137, false));
        assert_eq!(render(0, false), render(0, true));
        assert_ne!(render(0, true), render(137, true));
    }

    #[test]
    fn monochrome_does_not_depend_on_colour_or_unicode() {
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        Dusk {
            tick: 0,
            animate: false,
            ascii: false,
            monochrome: true,
        }
        .render(area, &mut buf);
        assert!(buf.content.iter().any(|c| c.symbol() != " "));
        for cell in buf.content {
            assert!(cell.symbol().is_ascii());
            assert_eq!(cell.fg, Color::Reset);
            assert_eq!(cell.bg, Color::Reset);
        }
    }
}
