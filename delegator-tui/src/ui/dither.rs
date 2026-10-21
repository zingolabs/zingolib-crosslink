//! An ordered-dither gradient for empty panes: three orbiting blobs summed into a value field,
//! thresholded against an 8x8 Bayer matrix into shade glyphs. A port of lwtui's `dither.rs`
//! (lightwallet-tools), kept line for line: the field, the colours, the thresholds, the fade,
//! and the clock. Value drives glyph density, position drives hue: a cell near blob two reads
//! rust, the dot coverage there says how bright.
//!
//! No allocation in the render path. Blob centers are computed once per call, never per cell,
//! and the field is sampled once per cell.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::app::State;
use crate::theme::{ColorDepth, Dither, Rgb, Theme};

/// Shade ramp, empty to full. `floor(v*4 + bayer)` indexes it, so inter-cell dithering carries
/// the levels between the five glyphs.
const SHADES: [char; 5] = [' ', '░', '▒', '▓', '█'];

/// The gradient body, already dimmed 40% toward the ground at the palette level (the single
/// brightness knob, held at its default). Used directly, as lwtui does.
const TAN_FAINT: Rgb = Rgb(0x41, 0x36, 0x25);
const SAND: Rgb = Rgb(0x8b, 0x6e, 0x52);
const RUST: Rgb = Rgb(0x7b, 0x43, 0x31);
const GRAD_OLIVE: Rgb = Rgb(0x67, 0x66, 0x38);
const GREIGE: Rgb = Rgb(0x77, 0x6b, 0x58);
const TEAL_DEEP: Rgb = Rgb(0x39, 0x55, 0x4e);

/// Standard 8x8 ordered-dither threshold matrix, values 0..63.
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

/// Braille dot bit per subpixel (column 0..1, row 0..3), over the U+2800 base.
const DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Whole-field strength toward the ground. The gradient is a faint material under content,
/// not a foreground: this reads as present but not competing.
const OPACITY: f32 = 0.45;
/// Braille dots cover far less of the cell than shade blocks, so the same alpha reads fainter.
/// Lift it so the two charsets land at a similar weight.
const BRAILLE_OPACITY: f32 = 0.65;

/// The field advances half a unit per second of animation, quantized to a 90 ms step so the
/// frame rate caps near 11 fps and frozen frames diff to nothing.
const STEP: f32 = 0.09;

/// Which glyph set the field dithers into.
#[derive(Clone, Copy)]
pub enum Charset {
    /// Shade blocks `░▒▓█`: near-universal, cell-aligned, two colors per cell.
    Blocks,
    /// Braille `⠿`: finer 2×4 dot density, but the glyphs are a font gamble.
    Braille,
}

/// A color in float space, so blends accumulate without rounding per step.
#[derive(Clone, Copy)]
struct Rgbf {
    r: f32,
    g: f32,
    b: f32,
}

/// A point in the normalized field, both axes `0..=1`.
#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

/// The three blob centers at field time `t`, in normalized coords.
struct Blobs {
    b1: Point,
    b2: Point,
    b3: Point,
}

/// The raw falloff weight of each blob at one point.
#[derive(Clone, Copy)]
struct Weights {
    f1: f32,
    f2: f32,
    f3: f32,
}

/// The field at one point: its clamped value and the weights behind it.
struct Sample {
    value: f32,
    weights: Weights,
}

/// Blob centers orbit on independent cosine/sine paths. Computed once per frame.
fn blobs(t: f32) -> Blobs {
    Blobs {
        b1: Point {
            x: 0.50 + 0.30 * (0.40 * t).cos(),
            y: 0.42 + 0.26 * (0.31 * t).sin(),
        },
        b2: Point {
            x: 0.50 + 0.34 * (2.1 - 0.23 * t).cos(),
            y: 0.55 + 0.30 * (0.19 * t + 1.0).sin(),
        },
        b3: Point {
            x: 0.50 + 0.26 * (0.17 * t + 4.2).cos(),
            y: 0.50 + 0.34 * (3.3 - 0.27 * t).sin(),
        },
    }
}

/// Rational stand-in for `exp(-9·d²)`: no transcendental per blob per cell.
fn falloff(dx: f32, dy: f32) -> f32 {
    let q = 1.0 + 4.5 * (dx * dx + dy * dy);
    1.0 / (q * q)
}

fn mix(a: Rgbf, b: Rgb, t: f32) -> Rgbf {
    mixf(a, cf(b), t)
}

fn cf(c: Rgb) -> Rgbf {
    Rgbf {
        r: f32::from(c.0),
        g: f32::from(c.1),
        b: f32::from(c.2),
    }
}

fn mixf(a: Rgbf, b: Rgbf, t: f32) -> Rgbf {
    Rgbf {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
    }
}

/// The value field and the three raw falloff weights at one point.
fn field(x: f32, y: f32, t: f32, b: &Blobs) -> Sample {
    let f1 = 1.00 * falloff(x - b.b1.x, y - b.b1.y);
    let f2 = 0.85 * falloff(x - b.b2.x, y - b.b2.y);
    let f3 = 0.70 * falloff(x - b.b3.x, y - b.b3.y);
    let mut v = f1 + f2 + f3;
    v *= 0.72 + 0.28 * (6.0 * x - 4.0 * y + 1.3 * t).sin();
    Sample {
        value: (v - 0.06).clamp(0.0, 1.0),
        weights: Weights { f1, f2, f3 },
    }
}

/// The cell hue: blob colors weighted by the cubed falloffs, over a tan-faint floor, washed
/// greige at the top and teal at the bottom. Cubing sharpens toward the nearest blob so
/// sand/rust/olive stay saturated instead of averaging to one brown, while still blending
/// smoothly across overlaps.
fn hue_of(y: f32, Weights { f1, f2, f3 }: Weights) -> Rgbf {
    let w1 = f1 * f1 * f1;
    let w2 = f2 * f2 * f2;
    let w3 = f3 * f3 * f3;
    let wsum = w1 + w2 + w3;
    let hue = if wsum > 1e-9 {
        let inv = 1.0 / wsum;
        let channel =
            |s: u8, r: u8, o: u8| (f32::from(s) * w1 + f32::from(r) * w2 + f32::from(o) * w3) * inv;
        Rgbf {
            r: channel(SAND.0, RUST.0, GRAD_OLIVE.0),
            g: channel(SAND.1, RUST.1, GRAD_OLIVE.1),
            b: channel(SAND.2, RUST.2, GRAD_OLIVE.2),
        }
    } else {
        cf(TAN_FAINT)
    };
    let hue = mix(hue, GREIGE, 0.28 * (1.0 - y));
    mix(hue, TEAL_DEEP, 0.30 * y)
}

/// Paint the dither field over `area` at field time `t`, one glyph per cell. The cell
/// background is always the app ground, so the field blends into the surrounding panes; the fg
/// hue is faded toward the ground by the charset's opacity. With `blank_only`, cells already
/// carrying content are left untouched, so the field fills empty space without painting over
/// text.
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    t: f32,
    charset: Charset,
    blank_only: bool,
    ground: Rgb,
) {
    let opacity = match charset {
        Charset::Blocks => OPACITY,
        Charset::Braille => BRAILLE_OPACITY,
    };
    paint(buf, area, t, charset, blank_only, opacity, ground);
}

fn paint(
    buf: &mut Buffer,
    area: Rect,
    t: f32,
    charset: Charset,
    blank_only: bool,
    opacity: f32,
    ground: Rgb,
) {
    let (w, h) = (area.width, area.height);
    if w == 0 || h == 0 {
        return;
    }
    let b = blobs(t);
    let (wf, hf) = (f32::from(w), f32::from(h));
    let bg = to_color(cf(ground));

    for cy in 0..h {
        let y = (f32::from(cy) + 0.5) / hf;
        // Anchor the field to the bottom edge: full strength at the last row, fading to the
        // ground at the top, so the band dissolves up toward the content instead of floating
        // with a faded top edge.
        let alpha = opacity * y;
        for cx in 0..w {
            let x = (f32::from(cx) + 0.5) / wf;
            let Sample { value: v, weights } = field(x, y, t, &b);
            let hue = hue_of(y, weights);

            let (fg, glyph) = match charset {
                // Shade glyph, dithered by Bayer, in the faded hue on the ground.
                Charset::Blocks => {
                    let bayer =
                        (f32::from(BAYER[(cy % 8) as usize][(cx % 8) as usize]) + 0.5) / 64.0;
                    let level = (v * 4.0 + bayer).floor().clamp(0.0, 4.0) as usize;
                    (mixf(cf(ground), hue, alpha), SHADES[level])
                }
                // Braille: 2×4 dots, each on when its subpixel value clears the Bayer
                // threshold. Finer density, one fg color on the ground.
                Charset::Braille => {
                    let mut bits = 0u8;
                    for sy in 0..4 {
                        for sx in 0..2 {
                            let sxx = (f32::from(cx) + (sx as f32 + 0.5) / 2.0) / wf;
                            let syy = (f32::from(cy) + (sy as f32 + 0.5) / 4.0) / hf;
                            let sv = field(sxx, syy, t, &b).value;
                            let thr = (f32::from(
                                BAYER[(cy as usize * 4 + sy) % 8][(cx as usize * 2 + sx) % 8],
                            ) + 0.5)
                                / 64.0;
                            if sv > thr {
                                bits |= DOTS[sy][sx];
                            }
                        }
                    }
                    let glyph = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
                    (mixf(cf(ground), hue, alpha), glyph)
                }
            };

            if let Some(cell) = buf.cell_mut((area.x + cx, area.y + cy)) {
                if blank_only && cell.symbol() != " " {
                    continue;
                }
                cell.set_bg(bg);
                cell.set_fg(to_color(fg));
                cell.set_char(glyph);
            }
        }
    }
}

fn to_color(c: Rgbf) -> Color {
    let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Color::Rgb(byte(c.r), byte(c.g), byte(c.b))
}

/// The field clock: seconds of animation, quantized to the step, at half a unit per second.
pub fn time(secs: f32) -> f32 {
    // the epsilon keeps 1.8 / 0.09 from flooring to nineteen steps
    ((secs / STEP) + 1e-4).floor() * STEP * 0.5
}

/// Paints the field over the empty tail of `area`: from just below the last row that carries
/// content to the bottom, the way lwtui's `gradient_band` does. The field is sampled over this
/// whole height, so the bright lobes sit near the middle and fade up toward the content. Needs
/// 24-bit colour and a palette theme; otherwise nothing.
pub fn band(frame: &mut Frame, area: Rect, state: &State, t: &Theme) {
    let charset = match state.dither {
        Dither::Off => return,
        Dither::Blocks => Charset::Blocks,
        Dither::Braille => Charset::Braille,
    };
    if state.color_depth != ColorDepth::TrueColor {
        return;
    }
    let Some((_, ground)) = t.rgb else {
        return;
    };
    let buf = frame.buffer_mut();
    // The last row in the area that carries content. Skip the first column: a pane's left
    // border runs the full height and would otherwise mark every row as filled.
    let mut last = None;
    for y in area.top()..area.bottom() {
        let filled = (area.left().saturating_add(1)..area.right())
            .any(|x| buf.cell((x, y)).is_some_and(|c| c.symbol() != " "));
        if filled {
            last = Some(y);
        }
    }
    let bottom = area.bottom().saturating_sub(1);
    let top = last.map_or(area.top(), |y| y + 1);
    if bottom <= top {
        return;
    }
    let region = Rect {
        x: area.x,
        y: top,
        width: area.width,
        height: bottom + 1 - top,
    };
    render(
        buf,
        region,
        time(state.elapsed_secs()),
        charset,
        true,
        ground,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUND: Rgb = Rgb(0x18, 0x18, 0x18);

    #[test]
    fn every_block_cell_gets_a_shade_glyph_and_an_rgb_pair() {
        let area = Rect::new(2, 1, 20, 8);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 12));
        render(&mut buf, area, 3.5, Charset::Blocks, false, GROUND);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = buf.cell((x, y)).unwrap();
                let ch = cell.symbol().chars().next().unwrap();
                assert!(SHADES.contains(&ch), "glyph {ch:?} is off the ramp");
                assert!(matches!(cell.bg, Color::Rgb(..)));
                assert!(matches!(cell.fg, Color::Rgb(..)));
            }
        }
    }

    #[test]
    fn braille_cells_stay_in_the_braille_block() {
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, 3.5, Charset::Braille, false, GROUND);
        for y in 0..area.height {
            for x in 0..area.width {
                let ch = buf.cell((x, y)).unwrap().symbol().chars().next().unwrap();
                assert!(
                    ('\u{2800}'..='\u{28ff}').contains(&ch),
                    "glyph {ch:?} is not braille"
                );
            }
        }
    }

    #[test]
    fn a_zero_area_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        render(
            &mut buf,
            Rect::new(0, 0, 0, 0),
            1.0,
            Charset::Blocks,
            false,
            GROUND,
        );
    }

    #[test]
    fn the_field_is_bounded_and_stable() {
        // The clamp keeps v in range and the same t reproduces the same frame, so a frozen
        // clock diffs to nothing.
        let mut a = Buffer::empty(Rect::new(0, 0, 24, 10));
        let mut b = Buffer::empty(Rect::new(0, 0, 24, 10));
        let area = Rect::new(0, 0, 24, 10);
        render(&mut a, area, 7.0, Charset::Braille, false, GROUND);
        render(&mut b, area, 7.0, Charset::Braille, false, GROUND);
        assert_eq!(a, b);
    }

    #[test]
    fn the_clock_steps_every_ninety_milliseconds_at_half_speed() {
        assert_eq!(time(0.0), 0.0);
        assert_eq!(time(0.04), time(0.08));
        assert!(time(0.10) > time(0.08));
        assert!((time(1.8) - 0.9).abs() < 1e-6);
    }
}
