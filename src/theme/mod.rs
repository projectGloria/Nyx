//! Every colour and dimension Nyx draws, resolved from config exactly once.
//!
//! Nothing outside this module may contain a colour literal. The live theme works by rebuilding
//! this struct and handing it to the renderer; a hardcoded hex elsewhere does not fail, it just
//! quietly ignores the user's theme, and finding out which widget did that later is expensive.

use alacritty_terminal::vte::ansi::Rgb;

use crate::config::schema::ThemeConfig;

pub mod presets;
pub mod visuals;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn from_rgb(c: Rgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a: 255,
        }
    }

    /// Parse `#rrggbb`, with the `#` optional. Returns `None` rather than panicking: this reads
    /// user-authored config, and a typo must produce a message naming the key, not a terminal
    /// that will not launch.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    /// The SGR 2 "faint" attribute. Alacritty and xterm both implement this as a straight
    /// multiply rather than by selecting a dim palette entry.
    pub fn dim(self) -> Self {
        Self {
            r: (self.r as u16 * 2 / 3) as u8,
            g: (self.g as u16 * 2 / 3) as u8,
            b: (self.b as u16 * 2 / 3) as u8,
            a: self.a,
        }
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(self.r, self.g, self.b, self.a)
    }

    /// Colour for the GPU, in whichever space the render target expects.
    ///
    /// An sRGB target applies linear->sRGB on write, so the shader has to emit linear; a plain
    /// UNORM target applies nothing, so the shader has to emit sRGB bytes as-is. Getting this
    /// backwards is not a crash — it is "the text looks washed out" with no error anywhere,
    /// which is why the choice is made in exactly one place and passed down.
    pub fn to_target(self, srgb_target: bool) -> [f32; 4] {
        let f = |c: u8| {
            let c = c as f32 / 255.0;
            if srgb_target {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            } else {
                c
            }
        };
        [f(self.r), f(self.g), f(self.b), self.a as f32 / 255.0]
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub accent: Rgba,
    pub surface: Rgba,
    pub bg: Rgba,
    pub fg: Rgba,
    pub cursor_bg: Rgba,
    pub cursor_fg: Rgba,
    pub selection_bg: Rgba,
    /// `None` keeps each cell's own foreground inside a selection, which is what a terminal
    /// that shows coloured `ls` output through a selection needs.
    pub selection_fg: Option<Rgba>,

    /// Search hits. Unlike a selection these always force both colours: a match you cannot read
    /// is not a match you have found, and shell output is already every colour there is.
    pub search_bg: Rgba,
    pub search_fg: Rgba,
    pub search_current_bg: Rgba,
    pub search_current_fg: Rgba,

    /// 0..16 named, 16..232 colour cube, 232..256 greyscale ramp.
    pub palette: [Rgba; 256],

    /// Bold text promotes ANSI 0..8 to 8..16.
    pub bold_is_bright: bool,
}

/// The six levels of the xterm 6x6x6 colour cube.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

impl Theme {
    /// Build a theme from config, reporting every colour it could not parse.
    ///
    /// One bad value falls back to the default for that key alone — the other 263 colours in the
    /// table are unaffected, which is what makes editing the file by hand tolerable.
    pub fn from_config(cfg: &ThemeConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let defaults = ThemeConfig::default();

        // A free function rather than a closure: a closure capturing `problems` would hold the
        // borrow for the whole body, and `selection_text` below needs to report into it too.
        fn pick(problems: &mut Vec<String>, name: &str, value: &str, fallback: &str) -> Rgba {
            match Rgba::parse_hex(value) {
                Some(c) => c,
                None => {
                    problems.push(format!("theme.{name}: \"{value}\" is not a #rrggbb colour"));
                    Rgba::parse_hex(fallback).unwrap_or(Rgba::rgb(0, 0, 0))
                }
            }
        }

        let mut palette = [Rgba::rgb(0, 0, 0); 256];
        for (i, (value, fallback)) in cfg
            .normal
            .as_array()
            .iter()
            .chain(cfg.bright.as_array().iter())
            .zip(
                defaults
                    .normal
                    .as_array()
                    .iter()
                    .chain(defaults.bright.as_array().iter()),
            )
            .enumerate()
        {
            palette[i] = pick(&mut problems, &ansi_name(i), value, fallback);
        }

        // 16..256 are defined by the xterm standard, not by the user: programs index into them
        // expecting exact values, and a "themed" colour cube makes 256-colour output wrong.
        for (i, slot) in palette.iter_mut().enumerate().skip(16).take(216) {
            let n = i - 16;
            *slot = Rgba::rgb(CUBE[n / 36], CUBE[(n / 6) % 6], CUBE[n % 6]);
        }
        for (i, slot) in palette.iter_mut().enumerate().skip(232) {
            let v = 8 + (i - 232) as u8 * 10;
            *slot = Rgba::rgb(v, v, v);
        }

        let selection_fg = cfg.selection_text.as_deref().and_then(|v| {
            let parsed = Rgba::parse_hex(v);
            if parsed.is_none() {
                problems.push(format!(
                    "theme.selection_text: \"{v}\" is not a #rrggbb colour"
                ));
            }
            parsed
        });

        let accent = if cfg.accent.is_empty() {
            palette[4]
        } else {
            pick(&mut problems, "accent", &cfg.accent, &cfg.normal.blue)
        };
        let surface = if cfg.surface.is_empty() {
            palette[0]
        } else {
            pick(&mut problems, "surface", &cfg.surface, &cfg.background)
        };
        let theme = Self {
            accent,
            surface,
            bg: pick(
                &mut problems,
                "background",
                &cfg.background,
                &defaults.background,
            ),
            fg: pick(
                &mut problems,
                "foreground",
                &cfg.foreground,
                &defaults.foreground,
            ),
            cursor_bg: pick(&mut problems, "cursor", &cfg.cursor, &defaults.cursor),
            cursor_fg: pick(
                &mut problems,
                "cursor_text",
                &cfg.cursor_text,
                &defaults.cursor_text,
            ),
            selection_bg: pick(
                &mut problems,
                "selection",
                &cfg.selection,
                &defaults.selection,
            ),
            selection_fg,
            search_bg: pick(&mut problems, "search", &cfg.search, &defaults.search),
            search_fg: pick(
                &mut problems,
                "search_text",
                &cfg.search_text,
                &defaults.search_text,
            ),
            search_current_bg: pick(
                &mut problems,
                "search_current",
                &cfg.search_current,
                &defaults.search_current,
            ),
            search_current_fg: pick(
                &mut problems,
                "search_current_text",
                &cfg.search_current_text,
                &defaults.search_current_text,
            ),
            palette,
            bold_is_bright: cfg.bold_is_bright,
        };

        (theme, problems)
    }
}

fn ansi_name(i: usize) -> String {
    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let group = if i < 8 { "normal" } else { "bright" };
    format!("{group}.{}", NAMES[i % 8])
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_config(&ThemeConfig::default()).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_greyscale_match_xterm() {
        let t = Theme::default();
        assert_eq!(t.palette[16], Rgba::rgb(0, 0, 0));
        assert_eq!(t.palette[231], Rgba::rgb(255, 255, 255));
        assert_eq!(t.palette[232], Rgba::rgb(8, 8, 8));
        assert_eq!(t.palette[255], Rgba::rgb(238, 238, 238));
        // 16 + 36*3 + 6*1 + 2 = 132 -> (175, 95, 135)
        assert_eq!(t.palette[132], Rgba::rgb(175, 95, 135));
    }

    #[test]
    fn hex_parse_rejects_junk() {
        assert_eq!(
            Rgba::parse_hex("#7aa2f7"),
            Some(Rgba::rgb(0x7a, 0xa2, 0xf7))
        );
        assert_eq!(Rgba::parse_hex("7aa2f7"), Some(Rgba::rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(Rgba::parse_hex("#7aa2f"), None);
        assert_eq!(Rgba::parse_hex("#zzzzzz"), None);
        // from_str_radix accepts a leading '+'; the length check alone would let it through.
        assert_eq!(Rgba::parse_hex("+12345"), None);
    }

    #[test]
    fn linear_conversion_only_happens_for_srgb_targets() {
        let c = Rgba::rgb(128, 128, 128);
        let raw = c.to_target(false);
        let lin = c.to_target(true);
        assert!((raw[0] - 128.0 / 255.0).abs() < 1e-6);
        assert!(lin[0] < 0.25 && lin[0] > 0.18);
    }

    #[test]
    fn one_bad_colour_does_not_take_the_rest_of_the_theme_with_it() {
        let mut cfg = ThemeConfig::tokyo_night();
        cfg.normal.red = "not a colour".into();
        let (theme, problems) = Theme::from_config(&cfg);

        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("normal.red"), "{}", problems[0]);
        // The bad one fell back...
        assert_eq!(
            theme.palette[1],
            Rgba::parse_hex(&ThemeConfig::default().normal.red).unwrap()
        );
        // ...and everything else still came from the config.
        assert_eq!(theme.palette[2], Rgba::parse_hex("#9ece6a").unwrap());
        assert_eq!(theme.bg, Rgba::parse_hex("#1a1b26").unwrap());
    }

    #[test]
    fn user_colours_reach_the_palette() {
        use crate::config::schema::AnsiGroup;
        let cfg = ThemeConfig {
            background: "#000000".into(),
            normal: AnsiGroup {
                blue: "#0000ff".into(),
                ..ThemeConfig::default().normal
            },
            bright: AnsiGroup {
                blue: "#8080ff".into(),
                ..ThemeConfig::default().bright
            },
            ..ThemeConfig::default()
        };
        let (theme, problems) = Theme::from_config(&cfg);
        assert!(problems.is_empty());
        assert_eq!(theme.bg, Rgba::rgb(0, 0, 0));
        assert_eq!(theme.palette[4], Rgba::rgb(0, 0, 255));
        assert_eq!(theme.palette[12], Rgba::rgb(0x80, 0x80, 0xff));
    }

    #[test]
    fn the_256_colour_cube_ignores_the_theme() {
        // Programs index 16..256 expecting exact xterm values; theming them makes 256-colour
        // output wrong in a way that looks like a rendering bug.
        let mut cfg = ThemeConfig::default();
        cfg.normal.black = "#ff00ff".into();
        let (theme, _) = Theme::from_config(&cfg);
        assert_eq!(theme.palette[0], Rgba::rgb(0xff, 0x00, 0xff));
        assert_eq!(theme.palette[16], Rgba::rgb(0, 0, 0));
    }
}
