//! Built-in palettes; selecting one copies all colours into the editable config.
use crate::config::schema::{AnsiGroup, ThemeConfig};

#[derive(Clone, Copy)]
pub enum Preset {
    ClaudeLight,
    ClaudeDark,
    TokyoNight,
    Nord,
}

impl Preset {
    pub const ALL: [Self; 4] = [
        Self::ClaudeLight,
        Self::ClaudeDark,
        Self::TokyoNight,
        Self::Nord,
    ];

    pub fn description(self) -> &'static str {
        match self {
            Self::ClaudeLight => "Warm cream, charcoal text and terracotta accents. Default.",
            Self::ClaudeDark => "Warm charcoal surfaces with soft cream text and terracotta.",
            Self::TokyoNight => "The original Nyx palette: midnight blue and vivid colours.",
            Self::Nord => "Cool slate surfaces with frost blue accents.",
        }
    }

    pub fn config(self) -> ThemeConfig {
        let mut t = ThemeConfig::tokyo_night();
        let (name, bg, fg, surface, accent, selection, normal, bright) = match self {
            Self::TokyoNight => return t,
            Self::ClaudeLight => (
                "Claude Light",
                "#faf9f5",
                "#302d29",
                "#f0eee6",
                "#c96442",
                "#e8dfd4",
                [
                    "#302d29", "#a63d40", "#496b42", "#86611d", "#416b96", "#805580", "#287375",
                    "#777269",
                ],
                [
                    "#777269", "#a83f32", "#527342", "#91651c", "#4b70a0", "#895a8d", "#327d7c",
                    "#302d29",
                ],
            ),
            Self::ClaudeDark => (
                "Claude Dark",
                "#262624",
                "#eee9df",
                "#1f1f1d",
                "#da7756",
                "#49443e",
                [
                    "#1f1f1d", "#de827b", "#a3b88c", "#d8b777", "#91b4d3", "#b9a0c5", "#8cbcb5",
                    "#c8c2b8",
                ],
                [
                    "#969084", "#ed9690", "#b6c99e", "#e8c98b", "#a9c9e4", "#cbb2d7", "#a2cec7",
                    "#eee9df",
                ],
            ),
            Self::Nord => (
                "Nord",
                "#2e3440",
                "#eceff4",
                "#272d38",
                "#88c0d0",
                "#434c5e",
                [
                    "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                    "#e5e9f0",
                ],
                [
                    "#7b88a1", "#d57780", "#b5d09e", "#f5dba0", "#91b1d1", "#c6a0bf", "#8fbcbb",
                    "#eceff4",
                ],
            ),
        };
        t.name = name.into();
        t.background = bg.into();
        t.foreground = fg.into();
        t.accent = accent.into();
        t.surface = surface.into();
        t.cursor = accent.into();
        t.cursor_text = bg.into();
        t.selection = selection.into();
        t.selection_text = None;
        t.search = if matches!(self, Self::ClaudeLight) {
            "#e9d7a8"
        } else {
            "#65583b"
        }
        .into();
        t.search_text = fg.into();
        t.search_current = accent.into();
        t.search_current_text = bg.into();
        t.bold_is_bright = false;
        t.normal = group(normal);
        t.bright = group(bright);
        t
    }
}

fn group(c: [&str; 8]) -> AnsiGroup {
    AnsiGroup {
        black: c[0].into(),
        red: c[1].into(),
        green: c[2].into(),
        yellow: c[3].into(),
        blue: c[4].into(),
        magenta: c[5].into(),
        cyan: c[6].into(),
        white: c[7].into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_survive_settings_save_and_resolve_without_errors() {
        for preset in Preset::ALL {
            let theme = preset.config();
            let config = crate::config::schema::Config {
                theme: theme.clone(),
                ..crate::config::schema::Config::default()
            };
            let saved =
                crate::config::write::render(crate::config::load::DEFAULT_CONFIG, &config).unwrap();
            let loaded = crate::config::load::parse(&saved).expect_parsed();
            assert_eq!(
                toml::to_string(&loaded.theme).unwrap(),
                toml::to_string(&theme).unwrap()
            );
            let (resolved, problems) = crate::theme::Theme::from_config(&loaded.theme);
            assert!(problems.is_empty(), "{problems:?}");
            let visuals = crate::theme::visuals::visuals(&resolved);
            assert_eq!(visuals.dark_mode, !matches!(preset, Preset::ClaudeLight));
            assert_ne!(resolved.bg, resolved.fg);
        }
    }

    #[test]
    fn generated_config_matches_default_theme() {
        let generated =
            crate::config::load::parse(crate::config::load::DEFAULT_CONFIG).expect_parsed();
        assert_eq!(
            toml::to_string(&generated.theme).unwrap(),
            toml::to_string(&ThemeConfig::default()).unwrap()
        );
    }

    #[test]
    fn legacy_palette_keeps_its_chrome_colours() {
        let config = crate::config::load::parse(
            "[theme]\nbackground = \"#1a1b26\"\n[theme.normal]\nblack = \"#15161e\"\nblue = \"#7aa2f7\"\n"
        ).expect_parsed();
        assert_eq!(config.theme.surface, "#15161e");
        assert_eq!(config.theme.accent, "#7aa2f7");
    }
}
