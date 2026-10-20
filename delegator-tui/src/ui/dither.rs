//! The dither field: three soft blobs drifting over the empty part of a pane, drawn as
//! Bayer-dithered shade blocks or braille dots in the warm gradient tones. A port of lwtui's
//! `dither.rs`; the field maths is in its DESIGN.md.
//!
//! It fills only blank cells below the last row that has content, fades toward that row, and
//! runs on the tick clock. It needs 24-bit colour and a palette theme; otherwise nothing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::app::State;
use crate::theme::{ColorDepth, Dither, Rgb, Theme, lerp};

const SAND: Rgb = Rgb(0x8b, 0x6e, 0x52);
const RUST: Rgb = Rgb(0x7b, 0x43, 0x31);
const OLIVE: Rgb = Rgb(0x67, 0x66, 0x38);
const TAN_FAINT: Rgb = Rgb(0x41, 0x36, 0x25);
const GREIGE: Rgb = Rgb(0x77, 0x6b, 0x58);
const TEAL_DEEP: Rgb = Rgb(0x39, 0x55, 0x4e);

const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];
const SHADES: [char; 5] = [' ', '░', '▒', '▓', '█'];
/// How much of the hue shows: blocks cover the cell, braille dots cover less.
const OPACITY_BLOCKS: f32 = 0.45;
const OPACITY_BRAILLE: f32 = 0.65;
/// The field advances half a unit per second, in steps so frozen frames diff to nothing.
const SPEED: f32 = 0.5;
const STEP_SECS: f32 = 0.09;

/// Paints the field over the blank tail of `area`.
pub fn band(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    if state.dither == Dither::Off || state.color_depth != ColorDepth::TrueColor {
        return;
    }
    let Some((_, bg)) = t.rgb else {
        return;
    };
    let buffer = frame.buffer_mut();
    // the band starts under the last row with anything on it
    let last_content = (area.y..area.y + area.height)
        .rev()
        .find(|&y| (area.x + 1..area.x + area.width).any(|x| buffer[(x, y)].symbol() != " "));
    // one clear row between the content and the water
    let start = last_content.map_or(area.y, |y| y + 2);
    let end = area.y + area.height;
    if start >= end {
        return;
    }
    let t_clock = (state.elapsed_secs() * SPEED / STEP_SECS).floor() * STEP_SECS;
    let field = Field::at(t_clock);
    let opacity = match state.dither {
        Dither::Braille => OPACITY_BRAILLE,
        _ => OPACITY_BLOCKS,
    };
    for y in start..end {
        let fade = f32::from(y - start + 1) / f32::from(end - start);
        for x in area.x + 1..area.x + area.width {
            let cell = &buffer[(x, y)];
            if cell.symbol() != " " {
                continue;
            }
            let nx = f32::from(x - area.x) / f32::from(area.width);
            let ny = f32::from(y - area.y) / f32::from(area.height);
            let glyph = match state.dither {
                Dither::Braille => braille(&field, x, y, nx, ny, area, fade),
                _ => {
                    let v = field.value(nx, ny) * fade;
                    let threshold =
                        (f32::from(BAYER[usize::from(y % 8)][usize::from(x % 8)]) + 0.5) / 64.0;
                    SHADES[((v * 4.0 + threshold).floor() as usize).min(4)]
                }
            };
            if glyph == ' ' || glyph == '⠀' {
                continue;
            }
            let hue = field.hue(nx, ny);
            let fg = lerp(bg, hue, opacity);
            buffer[(x, y)].set_char(glyph).set_style(
                Style::new()
                    .fg(Color::Rgb(fg.0, fg.1, fg.2))
                    .bg(Color::Rgb(bg.0, bg.1, bg.2)),
            );
        }
    }
}

/// A 2×4 dot cell: each dot is on when its own sample clears the Bayer threshold.
fn braille(field: &Field, x: u16, y: u16, nx: f32, ny: f32, area: Rect, fade: f32) -> char {
    const BITS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    let dx = 1.0 / f32::from(area.width);
    let dy = 1.0 / f32::from(area.height);
    let mut code = 0x2800u32;
    for (row, bits) in BITS.iter().enumerate() {
        for (col, bit) in bits.iter().enumerate() {
            let sx = nx + dx * (col as f32 + 0.5) / 2.0;
            let sy = ny + dy * (row as f32 + 0.5) / 4.0;
            let v = field.value(sx, sy) * fade;
            let bx = usize::from(x % 4) * 2 + col;
            let by = usize::from(y % 2) * 4 + row;
            let threshold = (f32::from(BAYER[by % 8][bx % 8]) + 0.5) / 64.0;
            if v > threshold {
                code |= bit;
            }
        }
    }
    char::from_u32(code).unwrap_or(' ')
}

/// The blob centres for one frame, computed once per frame.
struct Field {
    blobs: [(f32, f32, f32, Rgb); 3],
    t: f32,
}

impl Field {
    fn at(t: f32) -> Self {
        Self {
            blobs: [
                (
                    0.50 + 0.30 * (0.40 * t).cos(),
                    0.42 + 0.26 * (0.31 * t).sin(),
                    1.00,
                    SAND,
                ),
                (
                    0.50 + 0.34 * (2.1 - 0.23 * t).cos(),
                    0.55 + 0.30 * (0.19 * t + 1.0).sin(),
                    0.85,
                    RUST,
                ),
                (
                    0.50 + 0.26 * (0.17 * t + 4.2).cos(),
                    0.50 + 0.34 * (3.3 - 0.27 * t).sin(),
                    0.70,
                    OLIVE,
                ),
            ],
            t,
        }
    }

    /// A rational stand-in for `exp(-9 d²)`.
    fn falloff(d2: f32) -> f32 {
        let a = 1.0 + 4.5 * d2;
        1.0 / (a * a)
    }

    /// Glyph density at a point, `0..=1`.
    fn value(&self, x: f32, y: f32) -> f32 {
        let mut v = 0.0;
        for (bx, by, w, _) in &self.blobs {
            let dx = x - bx;
            let dy = y - by;
            v += w * Self::falloff(dx * dx + dy * dy);
        }
        v *= 0.72 + 0.28 * (6.0 * x - 4.0 * y + 1.3 * self.t).sin();
        (v - 0.06).clamp(0.0, 1.0)
    }

    /// The hue at a point: the blob colours weighted by their cubed falloffs, over a faint
    /// floor, washed toward greige at the top and teal at the bottom.
    fn hue(&self, x: f32, y: f32) -> Rgb {
        let mut sum = 0.0;
        let mut acc = [0.0f32; 3];
        for (bx, by, _, color) in &self.blobs {
            let dx = x - bx;
            let dy = y - by;
            let f = Self::falloff(dx * dx + dy * dy);
            let w = f * f * f;
            sum += w;
            acc[0] += w * f32::from(color.0);
            acc[1] += w * f32::from(color.1);
            acc[2] += w * f32::from(color.2);
        }
        let hue = if sum < 1e-4 {
            TAN_FAINT
        } else {
            Rgb(
                (acc[0] / sum).round() as u8,
                (acc[1] / sum).round() as u8,
                (acc[2] / sum).round() as u8,
            )
        };
        let wash = if y < 0.5 { GREIGE } else { TEAL_DEEP };
        lerp(hue, wash, 0.25)
    }
}
