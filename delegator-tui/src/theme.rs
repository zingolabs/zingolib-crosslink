//! Colour themes.
//!
//! A theme maps the interface's semantic roles (muted text, positive amounts, the Orchard pool,
//! and so on) to colours. Views ask for roles, never for colours, so every theme covers every
//! screen. Palettes are taken from each theme's official source:
//!
//! - Catppuccin: `catppuccin/palette` `palette.json`
//! - Gruvbox: `morhetz/gruvbox`, as shipped in the `jdinhlife.gruvbox` editor theme
//! - Nord: `arcticicestudio/nord`, with the brightened comment grey from `nord-vim`
//! - Vitesse: `antfu/vscode-theme-vitesse`, translucent colours pre-blended over the background
//! - Vercel: the Geist design system tokens from `vercel.com/geist/colors`

use ratatui::style::{Color, Modifier, Style};

use crate::app::Pool;

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit colour.
    TrueColor,
    /// The xterm 256-colour palette. Theme colours are mapped to the nearest entry.
    Ansi256,
}

impl ColorDepth {
    /// Truecolor when the terminal advertises it, otherwise the 256-colour palette.
    ///
    /// `COLORTERM` is the de facto signal. It is often not forwarded over SSH, which costs
    /// some accuracy but never shows wrong colours.
    pub fn detect() -> Self {
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let term = std::env::var("TERM").unwrap_or_default();
        let truecolor = matches!(colorterm.as_str(), "truecolor" | "24bit")
            || term.ends_with("-direct")
            || std::env::var_os("WT_SESSION").is_some();
        if truecolor {
            ColorDepth::TrueColor
        } else {
            ColorDepth::Ansi256
        }
    }

    /// Parses the `--color-depth` flag. `None` means detect.
    pub fn from_flag(value: &str) -> Option<Self> {
        match value {
            "24bit" | "truecolor" => Some(ColorDepth::TrueColor),
            "256" => Some(ColorDepth::Ansi256),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

const fn rgb(hex: u32) -> Rgb {
    Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// One colour per semantic role. Transparent-pool text uses `fg`: the only pool without
/// privacy gets no colour of its own.
#[derive(Clone, Copy, Debug)]
struct Palette {
    /// Screen background.
    bg: Rgb,
    /// Background of the highlighted row.
    surface: Rgb,
    /// Body text.
    fg: Rgb,
    /// Labels, hints, and secondary text.
    muted: Rgb,
    /// Separator lines and diagram wires.
    border: Rgb,
    /// Titles, key names, and the transaction box.
    accent: Rgb,
    /// Incoming value, success.
    positive: Rgb,
    /// Outgoing value, failure.
    negative: Rgb,
    /// Pending state and notices.
    warning: Rgb,
    /// Transaction ids and addresses.
    info: Rgb,
    orchard: Rgb,
    sapling: Rgb,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Palette(Palette),
    /// The terminal's own sixteen colours, on its own background.
    Terminal,
    /// No colour at all. Used when `NO_COLOR` is set.
    Mono,
}

pub struct ThemeDef {
    pub id: &'static str,
    pub name: &'static str,
    kind: Kind,
}

const fn catppuccin(
    base: u32,
    select: u32,
    text: u32,
    subtext0: u32,
    overlay0: u32,
    mauve: u32,
    [green, red, yellow, blue, pink, _teal, peach]: [u32; 7],
) -> Kind {
    Kind::Palette(Palette {
        bg: rgb(base),
        surface: rgb(select),
        fg: rgb(text),
        muted: rgb(subtext0),
        border: rgb(overlay0),
        accent: rgb(mauve),
        positive: rgb(green),
        negative: rgb(red),
        warning: rgb(yellow),
        info: rgb(blue),
        orchard: rgb(pink),
        sapling: rgb(peach),
    })
}

/// Every theme, in the order the picker lists them.
pub const THEMES: &[ThemeDef] = &[
    ThemeDef {
        id: "nord",
        name: "Nord",
        kind: Kind::Palette(Palette {
            bg: rgb(0x2e3440),
            surface: rgb(0x434c5e),
            fg: rgb(0xd8dee9),
            muted: rgb(0x616e88),
            border: rgb(0x4c566a),
            accent: rgb(0x88c0d0),
            positive: rgb(0xa3be8c),
            negative: rgb(0xbf616a),
            warning: rgb(0xebcb8b),
            info: rgb(0x81a1c1),
            orchard: rgb(0xb48ead),
            sapling: rgb(0xd08770),
        }),
    },
    ThemeDef {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        kind: catppuccin(
            0xeff1f5,
            0xd2d4dc,
            0x4c4f69,
            0x6c6f85,
            0x9ca0b0,
            0x8839ef,
            [
                0x40a02b, 0xd20f39, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xfe640b,
            ],
        ),
    },
    ThemeDef {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappé",
        kind: catppuccin(
            0x303446,
            0x494e63,
            0xc6d0f5,
            0xa5adce,
            0x737994,
            0xca9ee6,
            [
                0xa6d189, 0xe78284, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xef9f76,
            ],
        ),
    },
    ThemeDef {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        kind: catppuccin(
            0x24273a,
            0x404459,
            0xcad3f5,
            0xa5adcb,
            0x6e738d,
            0xc6a0f6,
            [
                0xa6da95, 0xed8796, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xf5a97f,
            ],
        ),
    },
    ThemeDef {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        kind: catppuccin(
            0x1e1e2e,
            0x3b3d4f,
            0xcdd6f4,
            0xa6adc8,
            0x6c7086,
            0xcba6f7,
            [
                0xa6e3a1, 0xf38ba8, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xfab387,
            ],
        ),
    },
    ThemeDef {
        id: "vitesse-dark",
        name: "Vitesse Dark",
        kind: Kind::Palette(Palette {
            bg: rgb(0x121212),
            surface: rgb(0x353535),
            fg: rgb(0xcecabe),
            muted: rgb(0x858480),
            border: rgb(0x52514f),
            accent: rgb(0x4d9375),
            positive: rgb(0x80a665),
            negative: rgb(0xcb7676),
            warning: rgb(0xe6cc77),
            info: rgb(0x6394bf),
            orchard: rgb(0xd9739f),
            sapling: rgb(0xbd976a),
        }),
    },
    ThemeDef {
        id: "vitesse-light",
        name: "Vitesse Light",
        kind: Kind::Palette(Palette {
            bg: rgb(0xffffff),
            surface: rgb(0xdcdcdc),
            fg: rgb(0x393a34),
            muted: rgb(0x8f908c),
            border: rgb(0xc1c1bf),
            accent: rgb(0x1c6b48),
            positive: rgb(0x59873a),
            negative: rgb(0xab5959),
            warning: rgb(0xbda437),
            info: rgb(0x296aa3),
            orchard: rgb(0xa13865),
            sapling: rgb(0xb07d48),
        }),
    },
    ThemeDef {
        id: "vitesse-black",
        name: "Vitesse Black",
        kind: Kind::Palette(Palette {
            bg: rgb(0x000000),
            surface: rgb(0x252525),
            fg: rgb(0xafaca2),
            muted: rgb(0x7d7c78),
            border: rgb(0x464543),
            accent: rgb(0x4d9375),
            positive: rgb(0x80a665),
            negative: rgb(0xcb7676),
            warning: rgb(0xe6cc77),
            info: rgb(0x6394bf),
            orchard: rgb(0xd9739f),
            sapling: rgb(0xbd976a),
        }),
    },
    ThemeDef {
        id: "vercel-dark",
        name: "Vercel Dark",
        kind: Kind::Palette(Palette {
            bg: rgb(0x000000),
            surface: rgb(0x292929),
            fg: rgb(0xededed),
            muted: rgb(0xa0a0a0),
            border: rgb(0x454545),
            accent: rgb(0x50a8ff),
            positive: rgb(0x00ca52),
            negative: rgb(0xff5e63),
            warning: rgb(0xff9900),
            info: rgb(0x50a8ff),
            orchard: rgb(0xc472fb),
            sapling: rgb(0xff518d),
        }),
    },
    ThemeDef {
        id: "vercel-light",
        name: "Vercel Light",
        kind: Kind::Palette(Palette {
            bg: rgb(0xffffff),
            surface: rgb(0xebebeb),
            fg: rgb(0x171717),
            muted: rgb(0x4d4d4d),
            border: rgb(0xc9c9c9),
            accent: rgb(0x0064e2),
            positive: rgb(0x107d32),
            negative: rgb(0xd60020),
            warning: rgb(0xa64f00),
            info: rgb(0x0064e2),
            orchard: rgb(0x7c00c9),
            sapling: rgb(0xc41562),
        }),
    },
    ThemeDef {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        kind: Kind::Palette(Palette {
            bg: rgb(0x282828),
            surface: rgb(0x504945),
            fg: rgb(0xebdbb2),
            muted: rgb(0xa89984),
            border: rgb(0x665c54),
            accent: rgb(0xfe8019),
            positive: rgb(0xb8bb26),
            negative: rgb(0xfb4934),
            warning: rgb(0xfabd2f),
            info: rgb(0x83a598),
            orchard: rgb(0xd3869b),
            sapling: rgb(0xd65d0e),
        }),
    },
    ThemeDef {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        kind: Kind::Palette(Palette {
            bg: rgb(0xfbf1c7),
            surface: rgb(0xd5c4a1),
            fg: rgb(0x3c3836),
            muted: rgb(0x7c6f64),
            border: rgb(0xbdae93),
            accent: rgb(0xaf3a03),
            positive: rgb(0x79740e),
            negative: rgb(0x9d0006),
            warning: rgb(0xb57614),
            info: rgb(0x076678),
            orchard: rgb(0x8f3f71),
            sapling: rgb(0xd65d0e),
        }),
    },
    ThemeDef {
        id: "terminal",
        name: "Terminal colours",
        kind: Kind::Terminal,
    },
    ThemeDef {
        id: "mono",
        name: "Monochrome",
        kind: Kind::Mono,
    },
];

/// Used when nothing else picks a theme.
pub const DEFAULT_THEME: &str = "gruvbox-dark";
/// Used when `NO_COLOR` is set and nothing more explicit picks a theme.
pub const NO_COLOR_THEME: &str = "mono";

pub fn index_of(id: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.id == id)
}

pub fn ids() -> impl Iterator<Item = &'static str> {
    THEMES.iter().map(|t| t.id)
}

/// Picks the starting theme. An explicit flag wins, then the saved preference, then
/// `NO_COLOR`, then the default. This follows no-color.org: user configuration and
/// command-line arguments override the environment variable. Unknown ids are skipped.
pub fn choose(flag: Option<&str>, saved: Option<&str>, no_color: bool) -> usize {
    flag.and_then(index_of)
        .or_else(|| saved.and_then(index_of))
        .or_else(|| no_color.then(|| index_of(NO_COLOR_THEME)).flatten())
        .or_else(|| index_of(DEFAULT_THEME))
        .unwrap_or(0)
}

/// Resolved styles, ready to draw with.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Background and body text for the whole screen.
    pub base: Style,
    pub text: Style,
    pub muted: Style,
    pub border: Style,
    pub accent: Style,
    /// Accent, bold. Screen titles and section headings.
    pub title: Style,
    pub selection: Style,
    pub positive: Style,
    pub negative: Style,
    pub warning: Style,
    pub info: Style,
    /// The block cursor in text fields.
    pub cursor: Style,
    pools: [Style; 3],
    /// Dark modules in `fg`, light modules and the quiet zone in `bg`. Derived from the theme's
    /// own tones, so the code matches the screen while still scanning. `None` leaves QR codes
    /// in the terminal's default colours.
    pub qr: Option<Style>,
}

impl Theme {
    pub fn new(index: usize, depth: ColorDepth) -> Self {
        let def = THEMES.get(index).unwrap_or(&THEMES[0]);
        match def.kind {
            Kind::Palette(p) => Self::from_palette(p, depth),
            Kind::Terminal => Self::terminal(),
            Kind::Mono => Self::mono(),
        }
    }

    pub fn pool(&self, pool: Pool) -> Style {
        self.pools[match pool {
            Pool::Orchard => 0,
            Pool::Sapling => 1,
            Pool::Transparent => 2,
        }]
    }

    fn from_palette(p: Palette, depth: ColorDepth) -> Self {
        let c = |rgb| color(rgb, depth);
        let fg = |rgb| Style::new().fg(c(rgb));
        Self {
            base: Style::new().bg(c(p.bg)).fg(c(p.fg)),
            text: fg(p.fg),
            muted: fg(p.muted),
            border: fg(p.border),
            accent: fg(p.accent),
            title: fg(p.accent).add_modifier(Modifier::BOLD),
            // body text on the surface colour: every palette is designed for that pairing,
            // while muted text on it can all but vanish (Nord)
            selection: Style::new()
                .bg(c(p.surface))
                .fg(c(p.fg))
                .add_modifier(Modifier::BOLD),
            positive: fg(p.positive),
            negative: fg(p.negative),
            warning: fg(p.warning),
            info: fg(p.info),
            cursor: Style::new().bg(c(p.accent)).fg(c(p.bg)),
            pools: [fg(p.orchard), fg(p.sapling), fg(p.fg)],
            qr: Some(qr_style(&p, depth)),
        }
    }

    fn terminal() -> Self {
        let fg = |color| Style::new().fg(color);
        let dim = Style::new().add_modifier(Modifier::DIM);
        Self {
            base: Style::new().bg(Color::Reset).fg(Color::Reset),
            text: Style::new(),
            muted: dim,
            border: dim,
            accent: fg(Color::Cyan),
            title: fg(Color::Cyan).add_modifier(Modifier::BOLD),
            selection: Style::new().add_modifier(Modifier::REVERSED),
            positive: fg(Color::Green),
            negative: fg(Color::Red),
            warning: fg(Color::Yellow),
            info: fg(Color::Blue),
            cursor: Style::new().add_modifier(Modifier::REVERSED),
            pools: [fg(Color::Magenta), fg(Color::LightRed), Style::new()],
            // the terminal's own black and bright white, whatever its palette makes of them
            qr: Some(Style::new().fg(Color::Black).bg(Color::White)),
        }
    }

    fn mono() -> Self {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let dim = Style::new().add_modifier(Modifier::DIM);
        Self {
            base: Style::new().bg(Color::Reset).fg(Color::Reset),
            text: Style::new(),
            muted: dim,
            border: dim,
            accent: bold,
            title: bold,
            selection: Style::new().add_modifier(Modifier::REVERSED),
            positive: Style::new(),
            negative: bold,
            warning: Style::new(),
            info: Style::new(),
            cursor: Style::new().add_modifier(Modifier::REVERSED),
            pools: [Style::new(); 3],
            qr: None,
        }
    }
}

/// Minimum contrast between a QR code's dark and light modules, as a WCAG ratio. Well above the
/// 7:1 accessibility bar, so a phone camera pointed at a screen still reads it.
pub const QR_CONTRAST: f64 = 12.0;

/// QR colours in the theme's own tones. The darker of text and background inks the modules,
/// the lighter one is the paper, and both are pushed towards black and white in small steps
/// until they reach [`QR_CONTRAST`]. Contrast is measured on the colours the terminal will
/// actually show, so 256-colour rounding cannot undo it. Dark modules stay darker than light
/// ones, the polarity every scanner expects.
fn qr_style(p: &Palette, depth: ColorDepth) -> Style {
    const BLACK: Rgb = Rgb(0, 0, 0);
    const WHITE: Rgb = Rgb(255, 255, 255);
    let shown = |rgb: Rgb| match depth {
        ColorDepth::TrueColor => rgb,
        ColorDepth::Ansi256 => ansi256_rgb(ansi256(rgb)),
    };
    let (mut ink, mut paper) = if luminance(p.bg) < luminance(p.fg) {
        (p.bg, p.fg)
    } else {
        (p.fg, p.bg)
    };
    // 8% per step; 60 steps reach black and white from anywhere, which is 21:1
    for _ in 0..60 {
        if contrast(shown(ink), shown(paper)) >= QR_CONTRAST {
            break;
        }
        ink = mix(ink, BLACK, 0.08);
        paper = mix(paper, WHITE, 0.08);
    }
    Style::new().fg(color(ink, depth)).bg(color(paper, depth))
}

/// WCAG relative luminance, 0 for black to 1 for white.
pub fn luminance(Rgb(r, g, b): Rgb) -> f64 {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG contrast ratio, from 1 for identical colours to 21 for black on white.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Moves `from` towards `to` by the fraction `t`.
fn mix(from: Rgb, to: Rgb, t: f64) -> Rgb {
    let step = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    Rgb(step(from.0, to.0), step(from.1, to.1), step(from.2, to.2))
}

/// The colour an xterm 256-colour index stands for. Indices below 16 depend on the terminal's
/// palette, so they are not used for theme colours and map to their conventional values only
/// roughly.
pub fn ansi256_rgb(index: u8) -> Rgb {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match index {
        16..=231 => {
            let i = usize::from(index - 16);
            Rgb(LEVELS[i / 36], LEVELS[(i / 6) % 6], LEVELS[i % 6])
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);
            Rgb(level, level, level)
        }
        0 => Rgb(0, 0, 0),
        15 => Rgb(255, 255, 255),
        _ => Rgb(128, 128, 128),
    }
}

/// The colours the picker shows for a theme: its background and seven role colours. `None`
/// for the monochrome theme.
pub fn swatch(index: usize, depth: ColorDepth) -> Option<(Color, Vec<Color>)> {
    match THEMES.get(index)?.kind {
        Kind::Palette(p) => Some((
            color(p.bg, depth),
            [
                p.accent, p.positive, p.negative, p.warning, p.info, p.orchard, p.sapling,
            ]
            .into_iter()
            .map(|rgb| color(rgb, depth))
            .collect(),
        )),
        Kind::Terminal => Some((
            Color::Reset,
            vec![
                Color::Cyan,
                Color::Green,
                Color::Red,
                Color::Yellow,
                Color::Blue,
                Color::Magenta,
                Color::Cyan,
                Color::LightRed,
            ],
        )),
        Kind::Mono => None,
    }
}

fn color(rgb: Rgb, depth: ColorDepth) -> Color {
    match depth {
        ColorDepth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        ColorDepth::Ansi256 => Color::Indexed(ansi256(rgb)),
    }
}

/// Nearest entry in the xterm 256-colour palette: the closer of the 6x6x6 cube (16 to 231)
/// and the 24-step grey ramp (232 to 255), by squared RGB distance.
pub fn ansi256(Rgb(r, g, b): Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| {
        (0..LEVELS.len())
            .min_by_key(|&i| (i32::from(v) - i32::from(LEVELS[i])).abs())
            .unwrap_or(0)
    };
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let step = ((average.saturating_sub(8) + 5) / 10).min(23);
    let level = (8 + 10 * step) as u8;
    let grey_index = 232 + step as usize;

    let index = if dist(cube) <= dist((level, level, level)) {
        cube_index
    } else {
        grey_index
    };
    index as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names_are_unique() {
        for (i, a) in THEMES.iter().enumerate() {
            for b in &THEMES[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.name, b.name);
            }
        }
        assert!(index_of(DEFAULT_THEME).is_some());
        assert!(index_of(NO_COLOR_THEME).is_some());
    }

    #[test]
    fn every_requested_family_is_present() {
        for id in [
            "nord",
            "catppuccin-latte",
            "catppuccin-frappe",
            "catppuccin-macchiato",
            "catppuccin-mocha",
            "vitesse-dark",
            "vitesse-light",
            "vitesse-black",
            "vercel-dark",
            "vercel-light",
            "gruvbox-dark",
            "gruvbox-light",
        ] {
            assert!(index_of(id).is_some(), "{id} missing");
        }
    }

    #[test]
    fn precedence_is_flag_then_saved_then_no_color_then_default() {
        let at = |id| index_of(id).unwrap();
        assert_eq!(choose(Some("nord"), Some("vitesse-dark"), true), at("nord"));
        assert_eq!(choose(None, Some("vitesse-dark"), true), at("vitesse-dark"));
        assert_eq!(choose(None, None, true), at(NO_COLOR_THEME));
        assert_eq!(choose(None, None, false), at(DEFAULT_THEME));
        // an unknown saved id falls through rather than failing
        assert_eq!(choose(None, Some("solarized"), false), at(DEFAULT_THEME));
        assert_eq!(choose(Some("bogus"), Some("nord"), false), at("nord"));
    }

    #[test]
    fn ansi256_hits_known_entries() {
        assert_eq!(ansi256(Rgb(0, 0, 0)), 16);
        assert_eq!(ansi256(Rgb(255, 255, 255)), 231);
        assert_eq!(ansi256(Rgb(255, 0, 0)), 196);
        assert_eq!(ansi256(Rgb(0, 0, 255)), 21);
        assert_eq!(ansi256(Rgb(128, 128, 128)), 244, "mid grey uses the ramp");
        assert_eq!(ansi256(Rgb(0x28, 0x28, 0x28)), 235, "gruvbox background");
    }

    #[test]
    fn palettes_resolve_at_both_depths() {
        for (i, def) in THEMES.iter().enumerate() {
            let truecolor = Theme::new(i, ColorDepth::TrueColor);
            let indexed = Theme::new(i, ColorDepth::Ansi256);
            if let Kind::Palette(_) = def.kind {
                assert!(
                    matches!(truecolor.base.bg, Some(Color::Rgb(..))),
                    "{}",
                    def.id
                );
                assert!(
                    matches!(indexed.base.bg, Some(Color::Indexed(_))),
                    "{} must not emit 24-bit colour at 256 colours",
                    def.id
                );
            }
        }
    }

    fn rgb_of(color: Option<Color>) -> Rgb {
        match color {
            Some(Color::Rgb(r, g, b)) => Rgb(r, g, b),
            Some(Color::Indexed(i)) => ansi256_rgb(i),
            other => panic!("expected a concrete colour, got {other:?}"),
        }
    }

    #[test]
    fn contrast_matches_wcag_reference_points() {
        assert!((contrast(Rgb(0, 0, 0), Rgb(255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast(Rgb(119, 119, 119), Rgb(255, 255, 255)) - 4.48).abs() < 0.01);
        assert!((contrast(Rgb(9, 9, 9), Rgb(9, 9, 9)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ansi256_rgb_inverts_ansi256() {
        for index in 16..=255u8 {
            assert_eq!(ansi256(ansi256_rgb(index)), index, "index {index}");
        }
    }

    #[test]
    fn qr_colours_follow_each_theme_and_always_scan() {
        let mut papers = Vec::new();
        for (i, def) in THEMES.iter().enumerate() {
            let Kind::Palette(p) = def.kind else { continue };
            for depth in [ColorDepth::TrueColor, ColorDepth::Ansi256] {
                let qr = Theme::new(i, depth)
                    .qr
                    .expect("palette themes colour their QR codes");
                let (ink, paper) = (rgb_of(qr.fg), rgb_of(qr.bg));
                assert!(
                    contrast(ink, paper) >= QR_CONTRAST,
                    "{} at {depth:?}: {:.1}:1",
                    def.id,
                    contrast(ink, paper)
                );
                assert!(
                    luminance(ink) < luminance(paper),
                    "{}: dark modules must be darker than light ones",
                    def.id
                );
                if depth == ColorDepth::TrueColor {
                    papers.push((def.id, paper));
                    // the tones come from the theme: a theme already past the threshold is
                    // left exactly as it is
                    if contrast(p.bg, p.fg) >= QR_CONTRAST {
                        let native = [p.bg, p.fg];
                        assert!(
                            native.contains(&ink) && native.contains(&paper),
                            "{}",
                            def.id
                        );
                    }
                }
            }
        }
        let paper_of = |id| papers.iter().find(|(t, _)| *t == id).unwrap().1;
        assert_ne!(
            paper_of("gruvbox-dark"),
            paper_of("nord"),
            "each theme tints its own"
        );
        assert_ne!(
            paper_of("gruvbox-dark"),
            Rgb(255, 255, 255),
            "not plain white"
        );
    }

    #[test]
    fn the_terminal_theme_uses_the_terminals_black_and_white() {
        let qr = Theme::new(index_of("terminal").unwrap(), ColorDepth::TrueColor)
            .qr
            .unwrap();
        assert_eq!((qr.fg, qr.bg), (Some(Color::Black), Some(Color::White)));
    }

    #[test]
    fn monochrome_uses_no_colour() {
        let t = Theme::new(index_of("mono").unwrap(), ColorDepth::TrueColor);
        for style in [
            t.text,
            t.muted,
            t.border,
            t.accent,
            t.title,
            t.selection,
            t.positive,
            t.negative,
            t.warning,
            t.info,
            t.cursor,
        ] {
            assert!(style.fg.is_none() && style.bg.is_none(), "{style:?}");
        }
        assert!(t.qr.is_none());
        assert!(swatch(index_of("mono").unwrap(), ColorDepth::TrueColor).is_none());
    }
}
