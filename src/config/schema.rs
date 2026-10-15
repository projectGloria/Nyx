//! The shape of `config.toml`. This module is the source of truth for every setting.
//!
//! Colours are `String` here rather than a parsed type on purpose: `theme::Theme::from_config`
//! converts them and reports each bad one by name. Parsing them in serde would make one typo'd
//! hex value reject the whole table and silently revert every other colour in it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeConfig,
    pub font: FontConfig,
    pub render: RenderConfig,
    pub window: WindowConfig,
    pub quake: QuakeConfig,
    pub sounds: SoundsConfig,
    pub launcher: LauncherConfig,
    pub keys: crate::config::keys::KeysConfig,
    pub leader: LeaderConfig,
    pub expand: ExpandConfig,
    /// `[[shortcut]]` entries, in file order.
    #[serde(rename = "shortcut")]
    pub shortcuts: Vec<ShortcutConfig>,
    /// `[[profile]]` entries. These layer over what `term::detect` found rather than replacing
    /// it — see [`ProfileConfig`].
    #[serde(rename = "profile")]
    pub profiles: Vec<ProfileConfig>,
    #[serde(rename = "alias")]
    pub aliases: AliasMap,
}

/// A shell to offer in the palette and the `+` menu.
///
/// Every field is an *override*: an entry whose `id` matches a detected shell changes only the
/// fields it names. That is what lets `id = "cmd"` plus `enabled = false` hide the Command Prompt
/// without having to restate its path, and what stops a Windows update moving `powershell.exe`
/// from breaking a profile that only wanted to add an argument.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProfileConfig {
    pub id: String,
    pub name: String,
    pub program: String,
    /// `None` (the key absent) inherits the detected arguments; `[]` deliberately means none.
    pub args: Option<Vec<String>>,
    /// `cmd` · `powershell` · `bash` · `wsl`. Empty infers from the program's file name, which
    /// decides both how a path shortcut is spelled and which init script is injected.
    pub kind: String,
    pub cwd: String,
    /// `auto` or `none`. `auto` injects shell integration only when the kind is known to have a
    /// script — an unrecognised program is never handed a `-NoExit -Command` it cannot parse.
    pub integration: String,
    pub enabled: bool,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            program: String::new(),
            args: None,
            kind: String::new(),
            cwd: String::new(),
            integration: "auto".into(),
            enabled: true,
        }
    }
}

/// Aliases are an open-ended table, so a map rather than a struct. `BTreeMap` keeps the order
/// stable, which matters because the settings GUI in Phase 7 writes this back.
pub type AliasMap = std::collections::BTreeMap<String, String>;

/// tmux-style leader chords. One reserved chord instead of a dozen bindings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LeaderConfig {
    /// Empty disables chords entirely.
    pub key: String,
    pub timeout_ms: u64,
    /// Single character -> command name.
    pub chords: std::collections::BTreeMap<String, String>,
}

impl Default for LeaderConfig {
    fn default() -> Self {
        // No chords by default: a leader that swallows a keystroke is surprising unless it was
        // asked for, and Ctrl+A is "beginning of line" in every readline shell.
        Self {
            key: String::new(),
            timeout_ms: 1200,
            chords: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub id: String,
    pub label: String,
    /// What to type on the command line. Defaults to `@<id>`.
    pub sigil: String,
    pub path: String,
    /// `builtin:<name>`, or `file:<absolute path to .png/.bmp>`.
    pub icon: String,
    pub color: Option<String>,
    pub order: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub accent: String,
    pub surface: String,
    pub name: String,
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub cursor_text: String,
    pub selection: String,
    /// Omitted keeps each cell's own foreground inside a selection, which is what lets coloured
    /// `ls` output stay readable when highlighted.
    pub selection_text: Option<String>,
    /// Every match Ctrl+F found on screen.
    pub search: String,
    pub search_text: String,
    /// The one match Enter moved to. Deliberately a different hue from `search` rather than a
    /// lighter shade of it: on a busy screen "which one am I on" has to be answerable at a glance.
    pub search_current: String,
    pub search_current_text: String,
    pub bold_is_bright: bool,
    pub normal: AnsiGroup,
    pub bright: AnsiGroup,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnsiGroup {
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub ligatures: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Block,
    #[serde(alias = "bar", alias = "i_beam")]
    Beam,
    Underline,
}

impl CursorStyle {
    pub fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Beam => "beam",
            Self::Underline => "underline",
        }
    }
    pub fn shape(self) -> alacritty_terminal::vte::ansi::CursorShape {
        use alacritty_terminal::vte::ansi::CursorShape;
        match self {
            Self::Block => CursorShape::Block,
            Self::Beam => CursorShape::Beam,
            Self::Underline => CursorShape::Underline,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RenderConfig {
    /// Text caret shape. Hidden cursors requested by terminal programs stay hidden.
    pub cursor_style: CursorStyle,
    /// Path to an imported Windows mouse cursor, separate from the text caret.
    pub cursor_icon: String,
    pub padding: f32,
    pub scrollback: usize,
    pub cursor_blink: bool,
    pub cursor_blink_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: f32,
    pub height: f32,
    /// Hand a second launch to the instance already running, as a new tab.
    ///
    /// Off by default, because "I ran it twice and only got one window" is a surprise. Explorer's
    /// "Open Nyx here" does not need it — that command line carries `--attach`, which asks for
    /// the same behaviour one launch at a time.
    pub single_instance: bool,
}

/// The drop-down terminal: a hotkey that shows and hides Nyx from anywhere.
///
/// Off by default, and deliberately. A global hotkey is a claim on every keystroke on the
/// machine — nothing should take one without being asked.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct QuakeConfig {
    pub enabled: bool,
    /// Must have a modifier: a bare key would be claimed system-wide.
    pub hotkey: String,
    /// Also snap the window to the top of the screen. `false` keeps the hotkey but leaves the
    /// window wherever the user put it.
    pub dropdown: bool,
    /// Fractions of the monitor's work area.
    pub width: f32,
    pub height: f32,
    /// `left` · `center` · `right`
    pub align: String,
    /// `cursor` — whichever screen the pointer is on — or `primary`.
    pub monitor: String,
    /// Slide duration. `0` snaps.
    pub animate_ms: u64,
    pub hide_on_focus_loss: bool,
    /// Start hidden, for a Nyx launched at sign-in.
    pub start_hidden: bool,
}

impl Default for QuakeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            // The console key on every game that ever had one, and the reason this is called
            // Quake mode.
            hotkey: "Ctrl+Backtick".into(),
            dropdown: true,
            width: 1.0,
            height: 0.45,
            align: "center".into(),
            monitor: "cursor".into(),
            animate_ms: 120,
            hide_on_focus_loss: true,
            start_hidden: false,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        crate::theme::presets::Preset::ClaudeLight.config()
    }
}

impl ThemeConfig {
    pub fn tokyo_night() -> Self {
        Self {
            name: "Tokyo Night".into(),
            accent: "#7aa2f7".into(),
            surface: "#15161e".into(),
            background: "#1a1b26".into(),
            foreground: "#c0caf5".into(),
            cursor: "#c0caf5".into(),
            cursor_text: "#1a1b26".into(),
            selection: "#283457".into(),
            selection_text: None,
            search: "#3d59a1".into(),
            search_text: "#c0caf5".into(),
            search_current: "#e0af68".into(),
            search_current_text: "#1a1b26".into(),
            bold_is_bright: true,
            normal: AnsiGroup {
                black: "#15161e".into(),
                red: "#f7768e".into(),
                green: "#9ece6a".into(),
                yellow: "#e0af68".into(),
                blue: "#7aa2f7".into(),
                magenta: "#bb9af7".into(),
                cyan: "#7dcfff".into(),
                white: "#a9b1d6".into(),
            },
            bright: AnsiGroup {
                black: "#414868".into(),
                red: "#ff7a93".into(),
                green: "#b9f27c".into(),
                yellow: "#ff9e64".into(),
                blue: "#7da6ff".into(),
                magenta: "#bb9af7".into(),
                cyan: "#0db9d7".into(),
                white: "#c0caf5".into(),
            },
        }
    }
}

impl Default for AnsiGroup {
    fn default() -> Self {
        ThemeConfig::default().normal
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Cascadia Mono".into(),
            size: 13.0,
            line_height: 1.25,
            ligatures: true,
        }
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            cursor_style: CursorStyle::Block,
            cursor_icon: String::new(),
            padding: 6.0,
            scrollback: 10_000,
            cursor_blink: false,
            cursor_blink_ms: 530,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 960.0,
            height: 600.0,
            single_instance: false,
        }
    }
}

impl AnsiGroup {
    pub fn as_array(&self) -> [&str; 8] {
        [
            &self.black,
            &self.red,
            &self.green,
            &self.yellow,
            &self.blue,
            &self.magenta,
            &self.cyan,
            &self.white,
        ]
    }
}

impl Config {
    /// Clamp values that would make the app unusable or crash something downstream.
    ///
    /// A font size of zero panics cosmic-text's `Metrics` ("line height cannot be 0"), and a
    /// negative padding puts the first cell off the left edge of the window. A config file is
    /// hand-edited, so both are one keystroke away.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut clamp = |name: &str, value: &mut f32, lo: f32, hi: f32| {
            if !value.is_finite() || *value < lo || *value > hi {
                problems.push(format!("{name} must be between {lo} and {hi}"));
                *value = value.clamp(lo, hi);
                if !value.is_finite() {
                    *value = lo;
                }
            }
        };

        clamp("sounds.volume", &mut self.sounds.volume, 0.0, 1.0);
        clamp("font.size", &mut self.font.size, 4.0, 200.0);
        clamp("font.line_height", &mut self.font.line_height, 0.5, 4.0);
        clamp("render.padding", &mut self.render.padding, 0.0, 200.0);
        clamp("window.width", &mut self.window.width, 200.0, 20_000.0);
        clamp("window.height", &mut self.window.height, 100.0, 20_000.0);
        // A tenth of the screen is still a usable terminal; anything smaller is a window with no
        // rows in it, which on screen is indistinguishable from a crash.
        clamp("quake.width", &mut self.quake.width, 0.1, 1.0);
        clamp("quake.height", &mut self.quake.height, 0.1, 1.0);

        if self.render.scrollback > 1_000_000 {
            problems.push("render.scrollback capped at 1000000".into());
            self.render.scrollback = 1_000_000;
        }
        if self.render.cursor_blink_ms < 100 {
            problems.push("render.cursor_blink_ms raised to 100".into());
            self.render.cursor_blink_ms = 100;
        }
        // Anything longer stops being a slide and starts being a wait.
        if self.quake.animate_ms > 1_000 {
            problems.push("quake.animate_ms capped at 1000".into());
            self.quake.animate_ms = 1_000;
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rescues_values_that_would_panic_downstream() {
        let mut c = Config::default();
        c.font.size = 0.0;
        c.font.line_height = 0.0;
        c.render.padding = -50.0;
        let problems = c.sanitize();

        // cosmic_text::Metrics asserts line_height != 0.0, so these would be a panic on the
        // first frame rather than a bad-looking terminal.
        assert!(c.font.size >= 4.0);
        assert!(c.font.line_height >= 0.5);
        assert!(c.render.padding >= 0.0);
        assert_eq!(problems.len(), 3);
    }

    #[test]
    fn sanitize_handles_nan() {
        let mut c = Config::default();
        c.font.size = f32::NAN;
        c.sanitize();
        assert!(c.font.size.is_finite());
    }

    #[test]
    fn a_partial_table_keeps_defaults_for_everything_it_omits() {
        // This is the property that makes hand-editing safe: you write the two keys you care
        // about and the other forty keep working.
        let toml = r#"
            [font]
            size = 18.0
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.font.size, 18.0);
        assert_eq!(c.font.family, "Cascadia Mono");
        assert_eq!(c.theme.background, "#faf9f5");
        assert_eq!(c.render.scrollback, 10_000);
    }

    #[test]
    fn an_unknown_key_is_ignored_rather_than_fatal() {
        // A key from a newer Nyx, or a typo, must not stop the terminal from launching.
        let toml = r#"
            [font]
            size = 14.0
            colour_of_the_bikeshed = "blue"
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.font.size, 14.0);
    }
}


/// Whether the sigil layer may act on what is being typed, and how far.
///
/// Split along the one axis that matters: whether it **takes keys from the program**. The hint is
/// a dim line under the cursor and claims nothing; the menu claims Tab, Escape and the arrows,
/// and a whole-line `@shortcut` claims Enter.
///
/// That distinction is load-bearing in a pane with no shell integration — a plain cmd.exe, WSL,
/// or anything over ssh — because there is no signal there that a child process is running.
/// `Shadow` goes on modelling keystrokes as if they were a shell command line, so typing `@` into
/// a full-screen or inline program that has its own meaning for it (an agent CLI's file
/// reference, most obviously) opens Nyx's menu over the program's, and Enter then answers the
/// wrong one. The project's position on detecting that case is that it cannot be done; this is
/// the switch for people who would rather have their keys than the feature.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ExpandConfig {
    pub completion: bool,
    pub hint: bool,
}
impl Default for ExpandConfig {
    fn default() -> Self {
        Self {
            completion: true,
            hint: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SoundsConfig {
    pub pack: String,
    pub volume: f32,
}
impl Default for SoundsConfig {
    fn default() -> Self {
        Self {
            pack: String::new(),
            volume: 0.5,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub enabled: bool,
    pub hotkey: String,
}
impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: "Ctrl+Shift+Space".into(),
        }
    }
}
