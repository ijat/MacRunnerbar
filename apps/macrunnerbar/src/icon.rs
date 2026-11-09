//! The menu-bar icon: a pure CPU raster of the state, with the running count drawn into the image.
//!
//! LuneUI's own tray raster has three states and no count (its docs: "Not mocked: badge counts"), but
//! `Tray::new` takes any RGBA image, so the app draws its own. The image is 18 px square (36 at 2x),
//! non-template with a white or black glyph picked from the bar's appearance, and transparent
//! elsewhere so it works on a light or a dark bar.
//!
//! - idle: a rounded outline with a `>` prompt inside
//! - running: a filled rounded square with the number of running jobs knocked out of it
//! - error: the outline with a `!` and a red dot at the corner
//! - paused: the outline with a pause symbol

use macrunnerbar_core::IconState;

/// The icon's side at 1x, in pixels; macOS draws the bitmap at 18 pt.
pub(crate) const SIDE: u32 = 18;

/// The glyph colour: white on a dark bar, black on a light one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// A white glyph, for a dark bar.
    Light,
    /// A black glyph, for a light bar.
    Dark,
}

impl Tone {
    fn rgb(self) -> [u8; 3] {
        match self {
            Self::Light => [0xFF, 0xFF, 0xFF],
            Self::Dark => [0x00, 0x00, 0x00],
        }
    }
}

/// The error dot's colour (the toolkit's count-badge red).
const RED: [u8; 3] = [0xE7, 0x48, 0x56];

/// An RGBA image, straight alpha, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// A coverage canvas: every shape is anti-aliased from its signed distance.
struct Canvas {
    w: u32,
    h: u32,
    /// Pixels per design unit: 1 at 1x, 2 at 2x.
    k: f32,
    px: Vec<[u8; 4]>,
}

impl Canvas {
    fn new(scale: u32) -> Self {
        let side = SIDE * scale;
        Self {
            w: side,
            h: side,
            k: scale as f32,
            px: vec![[0; 4]; (side * side) as usize],
        }
    }

    /// Composes `color` at `coverage` over the pixel (straight-alpha "over").
    fn blend(&mut self, x: u32, y: u32, color: [u8; 3], coverage: f32) {
        let c = coverage.clamp(0.0, 1.0);
        if c <= 0.0 {
            return;
        }
        let at = (y * self.w + x) as usize;
        let dst = self.px[at];
        let da = f32::from(dst[3]) / 255.0;
        let out_a = c + da * (1.0 - c);
        let mix = |s: u8, d: u8| -> u8 {
            let v = (f32::from(s) * c + f32::from(d) * da * (1.0 - c)) / out_a;
            v.round().clamp(0.0, 255.0) as u8
        };
        self.px[at] = [
            mix(color[0], dst[0]),
            mix(color[1], dst[1]),
            mix(color[2], dst[2]),
            (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
        ];
    }

    /// Removes coverage `c` of what is drawn (a knock-out).
    fn erase(&mut self, x: u32, y: u32, coverage: f32) {
        let c = coverage.clamp(0.0, 1.0);
        let at = (y * self.w + x) as usize;
        let a = f32::from(self.px[at][3]) * (1.0 - c);
        self.px[at][3] = a.round().clamp(0.0, 255.0) as u8;
    }

    /// Visits every pixel with its centre in design units.
    fn each(&self, mut f: impl FnMut(u32, u32, f32, f32)) {
        for y in 0..self.h {
            for x in 0..self.w {
                f(x, y, (x as f32 + 0.5) / self.k, (y as f32 + 0.5) / self.k);
            }
        }
    }

    /// Distance, in pixels, outside (positive) or inside (negative) a rounded rectangle.
    fn rrect_sd(px: f32, py: f32, r: [f32; 4], radius: f32) -> f32 {
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        let (hx, hy) = (r[2] / 2.0 - radius, r[3] / 2.0 - radius);
        let (dx, dy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
        let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
        outside + dx.max(dy).min(0.0) - radius
    }

    fn fill_rrect(&mut self, r: [f32; 4], radius: f32, color: [u8; 3]) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            let cov = 0.5 - Self::rrect_sd(px, py, r, radius) * k;
            if cov > 0.0 {
                hits.push((x, y, cov));
            }
        });
        for (x, y, c) in hits {
            self.blend(x, y, color, c);
        }
    }

    fn stroke_rrect(&mut self, r: [f32; 4], radius: f32, width: f32, color: [u8; 3]) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            // Distance to the outline: |sd| - half the width.
            let d = Self::rrect_sd(px, py, r, radius).abs() - width / 2.0;
            let cov = 0.5 - d * k;
            if cov > 0.0 {
                hits.push((x, y, cov));
            }
        });
        for (x, y, c) in hits {
            self.blend(x, y, color, c);
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: [u8; 3]) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            let cov = 0.5 - (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius) * k;
            if cov > 0.0 {
                hits.push((x, y, cov));
            }
        });
        for (x, y, c) in hits {
            self.blend(x, y, color, c);
        }
    }

    /// A line segment with round ends, `width` design units thick.
    fn stroke_line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 3]) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            let (abx, aby) = (b.0 - a.0, b.1 - a.1);
            let len2 = abx * abx + aby * aby;
            let t = if len2 == 0.0 {
                0.0
            } else {
                (((px - a.0) * abx + (py - a.1) * aby) / len2).clamp(0.0, 1.0)
            };
            let d = ((px - (a.0 + t * abx)).powi(2) + (py - (a.1 + t * aby)).powi(2)).sqrt()
                - width / 2.0;
            let cov = 0.5 - d * k;
            if cov > 0.0 {
                hits.push((x, y, cov));
            }
        });
        for (x, y, c) in hits {
            self.blend(x, y, color, c);
        }
    }

    /// Knocks a rectangle out of what is drawn, hard-edged on the design grid.
    fn erase_rect(&mut self, r: [f32; 4]) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            if px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3] {
                hits.push((x, y));
            }
        });
        let _ = k;
        for (x, y) in hits {
            self.erase(x, y, 1.0);
        }
    }

    /// Cuts a disc out of what is drawn (anti-aliased).
    fn clear_circle(&mut self, cx: f32, cy: f32, radius: f32) {
        let k = self.k;
        let mut hits = Vec::new();
        self.each(|x, y, px, py| {
            let cov = 0.5 - (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius) * k;
            if cov > 0.0 {
                hits.push((x, y, cov));
            }
        });
        for (x, y, c) in hits {
            self.erase(x, y, c);
        }
    }

    fn finish(self) -> IconImage {
        IconImage {
            width: self.w,
            height: self.h,
            rgba: self.px.into_iter().flatten().collect(),
        }
    }
}

/// 3 x 5 digit bitmaps, one row per `u8` (bits 2..0, left to right).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
const PLUS: [u8; 5] = [0b000, 0b010, 0b111, 0b010, 0b000];
const BANG: [u8; 5] = [0b010, 0b010, 0b010, 0b000, 0b010];

/// What the count shows: the number, or `99+`.
fn count_glyphs(n: usize) -> Vec<[u8; 5]> {
    if n > 99 {
        return vec![DIGITS[9], DIGITS[9], PLUS];
    }
    n.to_string()
        .bytes()
        .map(|b| DIGITS[usize::from(b - b'0')])
        .collect()
}

/// Knocks `glyphs` (3 x 5 bitmaps, 1 design unit per dot, 1 unit between them) out of the canvas,
/// centred on (`cx`, `cy`).
fn knock_out(c: &mut Canvas, glyphs: &[[u8; 5]], cx: f32, cy: f32) {
    let width = glyphs.len() as f32 * 4.0 - 1.0;
    let (x0, y0) = ((cx - width / 2.0).round(), (cy - 2.5).round());
    for (i, g) in glyphs.iter().enumerate() {
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    c.erase_rect([x0 + i as f32 * 4.0 + col as f32, y0 + row as f32, 1.0, 1.0]);
                }
            }
        }
    }
}

/// Draws `glyphs` as coloured dots (not knocked out): the `!` inside the error outline.
fn plot(c: &mut Canvas, glyph: [u8; 5], cx: f32, cy: f32, color: [u8; 3]) {
    let (x0, y0) = ((cx - 1.5).round(), (cy - 2.5).round());
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..3 {
            if bits & (0b100 >> col) != 0 {
                c.fill_rrect([x0 + col as f32, y0 + row as f32, 1.0, 1.0], 0.0, color);
            }
        }
    }
}

/// The icon for `state` at `scale` (1 or 2) in `tone`; `count` is the number of running jobs and
/// only matters while running. The image is exactly `18 * scale` pixels square.
pub(crate) fn render(state: IconState, count: usize, tone: Tone, scale: u32) -> IconImage {
    let mut c = Canvas::new(scale.clamp(1, 2));
    let ink = tone.rgb();
    // The body: 14 x 14 design units, centred, leaving room for the dot at the corner.
    let body = [2.0, 2.0, 14.0, 14.0];
    match state {
        IconState::Running => {
            c.fill_rrect(body, 3.0, ink);
            knock_out(&mut c, &count_glyphs(count.max(1)), 9.0, 9.0);
        }
        IconState::Idle => {
            c.stroke_rrect(body, 3.0, 1.5, ink);
            // A `>` prompt.
            c.stroke_line((6.5, 6.5), (10.0, 9.0), 1.5, ink);
            c.stroke_line((10.0, 9.0), (6.5, 11.5), 1.5, ink);
        }
        IconState::Paused => {
            c.stroke_rrect(body, 3.0, 1.5, ink);
            c.fill_rrect([6.25, 6.0, 1.75, 6.0], 0.5, ink);
            c.fill_rrect([10.0, 6.0, 1.75, 6.0], 0.5, ink);
        }
        IconState::Error => {
            c.stroke_rrect(body, 3.0, 1.5, ink);
            plot(&mut c, BANG, 9.0, 9.0, ink);
            // A red dot at the top right, with a clear ring cut around it so it reads on any bar.
            c.clear_circle(14.5, 3.5, 3.0);
            c.fill_circle(14.5, 3.5, 2.25, RED);
        }
    }
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [IconState; 4] = [
        IconState::Idle,
        IconState::Running,
        IconState::Error,
        IconState::Paused,
    ];

    #[test]
    fn the_image_is_exactly_w_h_4_bytes_at_both_scales() {
        for scale in [1, 2] {
            for state in STATES {
                let img = render(state, 3, Tone::Light, scale);
                assert_eq!((img.width, img.height), (18 * scale, 18 * scale));
                assert_eq!(
                    img.rgba.len(),
                    (img.width * img.height * 4) as usize,
                    "{state:?}"
                );
            }
        }
    }

    #[test]
    fn every_state_has_a_different_image_and_none_is_empty() {
        for scale in [1, 2] {
            let imgs: Vec<IconImage> = STATES
                .iter()
                .map(|s| render(*s, 3, Tone::Light, scale))
                .collect();
            for (i, a) in imgs.iter().enumerate() {
                assert!(a.rgba.chunks(4).any(|p| p[3] > 0), "state {i} drew nothing");
                for b in &imgs[i + 1..] {
                    assert_ne!(
                        a.rgba, b.rgba,
                        "two states drew the same image at scale {scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_running_count_is_drawn_into_the_image() {
        let one = render(IconState::Running, 1, Tone::Light, 2);
        let two = render(IconState::Running, 2, Tone::Light, 2);
        let ten = render(IconState::Running, 10, Tone::Light, 2);
        let many = render(IconState::Running, 250, Tone::Light, 2);
        assert_ne!(one.rgba, two.rgba);
        assert_ne!(two.rgba, ten.rgba);
        assert_ne!(ten.rgba, many.rgba);
        // Counts above 99 all draw `99+`.
        assert_eq!(
            many.rgba,
            render(IconState::Running, 101, Tone::Light, 2).rgba
        );
        // The digit is knocked out: transparent pixels inside the filled body.
        let transparent_inside = (14..22u32)
            .flat_map(|y| (12..24u32).map(move |x| (x, y)))
            .filter(|(x, y)| two.rgba[((y * 36 + x) * 4 + 3) as usize] == 0)
            .count();
        assert!(transparent_inside > 0, "no digit pixels were knocked out");
        // Idle, paused and error ignore the count.
        for s in [IconState::Idle, IconState::Paused, IconState::Error] {
            assert_eq!(render(s, 0, Tone::Light, 1), render(s, 9, Tone::Light, 1));
        }
    }

    #[test]
    fn running_with_zero_still_shows_a_digit_rather_than_an_empty_square() {
        assert_eq!(
            render(IconState::Running, 0, Tone::Dark, 1),
            render(IconState::Running, 1, Tone::Dark, 1)
        );
    }

    #[test]
    fn the_glyph_follows_the_bar_tone() {
        for state in [IconState::Idle, IconState::Running, IconState::Paused] {
            let light = render(state, 2, Tone::Light, 2);
            let dark = render(state, 2, Tone::Dark, 2);
            assert_ne!(light.rgba, dark.rgba);
            // Opaque ink pixels are white on a dark bar and black on a light one.
            assert!(
                light
                    .rgba
                    .chunks(4)
                    .filter(|p| p[3] == 255)
                    .all(|p| p[..3] == [255, 255, 255])
            );
            assert!(
                dark.rgba
                    .chunks(4)
                    .filter(|p| p[3] == 255)
                    .all(|p| p[..3] == [0, 0, 0])
            );
        }
    }

    #[test]
    fn the_error_icon_carries_a_red_dot() {
        let img = render(IconState::Error, 0, Tone::Light, 2);
        let reds = img
            .rgba
            .chunks(4)
            .filter(|p| p[3] > 200 && p[0] > 200 && p[1] < 120)
            .count();
        assert!(reds > 8, "expected a red dot, found {reds} red pixels");
        for s in [IconState::Idle, IconState::Running, IconState::Paused] {
            let other = render(s, 2, Tone::Light, 2);
            assert_eq!(
                other
                    .rgba
                    .chunks(4)
                    .filter(|p| p[0] > 200 && p[1] < 120 && p[3] > 200)
                    .count(),
                0
            );
        }
    }

    #[test]
    fn rendering_is_deterministic_and_stays_inside_the_bitmap() {
        for state in STATES {
            assert_eq!(
                render(state, 7, Tone::Light, 2),
                render(state, 7, Tone::Light, 2)
            );
        }
        // The outermost pixel ring stays transparent: the shapes sit inside the 14 unit body.
        let img = render(IconState::Running, 12, Tone::Light, 2);
        for i in 0..36u32 {
            for (x, y) in [(i, 0), (i, 35), (0, i), (35, i)] {
                assert_eq!(
                    img.rgba[((y * 36 + x) * 4 + 3) as usize],
                    0,
                    "edge pixel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn the_2x_image_is_the_1x_drawing_at_twice_the_size() {
        // The same shape: opaque coverage scales with the area (within the edge's anti-aliasing).
        let a1 = render(IconState::Idle, 0, Tone::Light, 1)
            .rgba
            .chunks(4)
            .map(|p| f32::from(p[3]))
            .sum::<f32>();
        let a2 = render(IconState::Idle, 0, Tone::Light, 2)
            .rgba
            .chunks(4)
            .map(|p| f32::from(p[3]))
            .sum::<f32>();
        let ratio = a2 / a1;
        assert!((3.4..4.6).contains(&ratio), "area ratio {ratio}");
    }
}
