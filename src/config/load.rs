//! Reading `config.toml`, and the default file written on first launch.
//!
//! The rule throughout: a bad config produces a message, never a dead terminal. A hand-edited
//! file is the primary interface, so a typo has to be survivable.

use std::path::Path;

use crate::config::paths;
use crate::config::schema::Config;
use crate::util::write_atomic;

/// What a load attempt produced.
///
/// Deliberately *not* a `Config` plus a list of complaints. A file that did not parse says
/// nothing about what the user wanted, so in that case there is no config here at all, and the
/// caller is made to say what should happen instead of reaching for a field and being handed
/// `Config::default()` without noticing. Every caller took that easy path when it existed: a hot
/// reload of a half-saved file silently replaced the theme, font, keys, aliases, shortcuts and
/// Quake settings with the defaults, and regenerated the shell integration files from them on the
/// way past.
pub enum Loaded {
    /// The file parsed. `problems` are per-value complaints from `sanitize` — a bad hex colour
    /// costs that colour and nothing else, so this config is still the one to apply.
    Parsed {
        /// Boxed so the failure variant does not carry `Config`'s full footprint in the enum's
        /// size. A load happens once per launch and once per save, so the allocation costs
        /// nothing; the failure case is the one that has to stay cheap to construct.
        config: Box<Config>,
        /// Human-readable complaints, shown to the user rather than logged and forgotten.
        problems: Vec<String>,
    },
    /// The file could not be read or did not parse. There is nothing here to apply.
    Failed { problems: Vec<String> },
}

impl Loaded {
    /// Human-readable complaints, whichever way the load went. Production code destructures the
    /// variant it is handling instead, so this exists for the tests.
    #[cfg(test)]
    pub fn problems(&self) -> &[String] {
        match self {
            Loaded::Parsed { problems, .. } | Loaded::Failed { problems } => problems,
        }
    }

    /// The config to start with, reading a failure as "the defaults".
    ///
    /// Correct for the *first* load only, where nothing is running yet and the defaults are the
    /// only thing there is to fall back to. A hot reload must match on the variant and keep what
    /// it already has — see `config::watch`.
    pub fn or_defaults(self) -> (Config, Vec<String>) {
        match self {
            Loaded::Parsed { config, problems } => (*config, problems),
            Loaded::Failed { problems } => (Config::default(), problems),
        }
    }

    /// The parsed config, for tests over text that is meant to be valid.
    #[cfg(test)]
    pub fn expect_parsed(self) -> Config {
        match self {
            Loaded::Parsed { config, .. } => *config,
            Loaded::Failed { problems } => panic!("expected the text to parse, got {problems:?}"),
        }
    }
}

/// The file written when none exists. Comments included: this is the user interface for
/// everything Nyx can be told to do, and an undocumented file of hex codes is not one.
pub const DEFAULT_CONFIG: &str = r##"# Nyx configuration.
#
# Saved changes apply immediately — there is no need to restart. If a value is wrong, Nyx keeps
# the previous setting and tells you which key was at fault rather than refusing to start.
#
# Every colour is "#rrggbb". Anything you leave out keeps its default.

[theme]
name = "Claude Light"
background = "#faf9f5"
foreground = "#302d29"
surface = "#f0eee6"
accent = "#c96442"
cursor = "#c96442"
cursor_text = "#faf9f5" # the character under a block cursor
selection = "#e8dfd4"
search = "#e9d7a8"
search_text = "#302d29"
search_current = "#c96442"
search_current_text = "#faf9f5"
bold_is_bright = false

[theme.normal]
black = "#302d29"
red = "#a63d40"
green = "#496b42"
yellow = "#86611d"
blue = "#416b96"
magenta = "#805580"
cyan = "#287375"
white = "#777269"

[theme.bright]
black = "#777269"
red = "#a83f32"
green = "#527342"
yellow = "#91651c"
blue = "#4b70a0"
magenta = "#895a8d"
cyan = "#327d7c"
white = "#302d29"

[font]
family      = "Cascadia Mono"
size        = 13.0
line_height = 1.25
# Programming ligatures (-> becoming an arrow, != a crossed equals). Needs a font that has them,
# such as Cascadia Code, JetBrains Mono or Fira Code.
ligatures   = true

[render]
padding          = 6.0
scrollback       = 10000
cursor_style     = "block"  # block, beam (I-Beam), underline
cursor_icon      = ""       # imported .cur mouse pointer; empty uses the system pointer
cursor_blink     = false
cursor_blink_ms  = 530

[window]
width  = 960.0
height = 600.0
# Hand a second launch to the window already open, as a new tab, instead of starting another one.
# Off by default, because "I ran it twice and only got one window" is a surprise. Explorer's
# "Open Nyx here" does not need this — that command line asks for the same thing one launch at a
# time.
single_instance = false

# ---------------------------------------------------------------------------------------------
# Quake mode: one chord shows and hides Nyx from anywhere, and the window drops down from the top
# of the screen.
#
# Off by default and deliberately: a global hotkey is a claim on every keystroke on the machine,
# and nothing should take one without being asked. If the chord is already owned by another
# program, Nyx says so rather than quietly doing nothing.
[sounds]
pack = ""             # Off, or a folder name under sounds beside nyx.exe
volume = 0.5          # 0.0 to 1.0

[launcher]
enabled = false       # Settings can register a per-user Windows sign-in launcher
hotkey = "Ctrl+Shift+Space"

[quake]
enabled            = false
# Must have at least one of Ctrl, Shift or Alt — a bare key would be claimed system-wide.
hotkey             = "Ctrl+Backtick"
# Also snap the window to the top of the screen. false keeps the hotkey and leaves the window
# wherever you put it.
dropdown           = true
# Fractions of the monitor's *work* area, so the taskbar is never covered.
width              = 1.0
height             = 0.45
align              = "center"   # left · center · right
monitor            = "cursor"   # cursor — whichever screen the pointer is on — or primary
# Slide duration in milliseconds. 0 snaps.
animate_ms         = 120
hide_on_focus_loss = true
# Start hidden, for a Nyx launched at sign-in. Ignored if the hotkey could not be claimed, so
# there is no way to end up with a window you cannot reach.
start_hidden       = false

# Tabs and splits. Set a value to "" to leave it unbound so the shell gets the key instead.
# Modifiers are Ctrl, Shift and Alt, in any order: "Ctrl+Shift+D".
#
# Every default here takes a chord the shell does not want. Single-modifier chords are left to
# the program on purpose: Ctrl+P is previous-history, Ctrl+F is forward-char, and a terminal
# that keeps them for itself is one you have to fight. A focus binding with no pane that way
# falls through to the program rather than doing nothing, so Alt+Arrow still reaches a
# full-screen application when there is only one pane.
[keys]
new_tab     = "Ctrl+Shift+T"
close_pane  = "Ctrl+Shift+W"
next_tab    = "Ctrl+Tab"
prev_tab    = "Ctrl+Shift+Tab"
split_right = "Ctrl+Shift+D"
split_down  = "Ctrl+Shift+E"
focus_left  = "Alt+Left"
focus_right = "Alt+Right"
focus_up    = "Alt+Up"
focus_down  = "Alt+Down"
zoom_pane   = "Ctrl+Shift+Z"
palette     = "Ctrl+Shift+P"
find        = "Ctrl+Shift+F"
settings    = "Ctrl+Comma"

# The @shortcut layer, and how much of the keyboard it is allowed to borrow.
#
# `completion` is the menu that appears as you type a sigil. It is the only part of Nyx that takes
# Tab, Escape and the arrows from whatever is running, and Enter on a line that is nothing but a
# shortcut. In a pane with shell integration it stands down on its own whenever a command is
# running. In one without — plain cmd.exe, WSL, anything over ssh — nothing can tell Nyx that a
# program is running, so if you live in full-screen or inline programs that give `@` a meaning of
# their own, turn this off and keep your keys. Expansion on space still works either way.
#
# `hint` is the dim line describing what you have typed. It takes no keys at all.
[expand]
completion = true
hint       = true

# tmux-style leader chords: one reserved chord, then a single key naming a command. Disabled by
# default because Ctrl+A is "beginning of line" in every readline shell and swallowing it is
# surprising unless you asked for it. Set `key` to enable.
#
# Commands: tab.new tab.next tab.prev tab.close · pane.split_right pane.split_down pane.close
#           pane.zoom pane.left pane.right pane.up pane.down · palette · shortcut.<id>
[leader]
key        = ""          # e.g. "Ctrl+A"
timeout_ms = 1200

[leader.chords]
c = "tab.new"
n = "tab.next"
p = "tab.prev"
x = "pane.close"
z = "pane.zoom"
g = "shortcut.tibet"

# ---------------------------------------------------------------------------------------------
# Shortcuts: named directories you can jump to.
#
# Pick one from Saved locations in the footer to open a new tab there, or press Ctrl+P and search
# for it. You can also type
# its sigil on the command line and press space — "cd @nyx " becomes "cd C:\...\Nyx ". The path
# is rewritten for whichever shell the pane is running, so the same shortcut produces
# C:\Users\... in PowerShell, /c/Users/... in Git Bash and /mnt/c/Users/... in WSL.
#
# icon  = "builtin:<name>" where name is one of:
#           folder  terminal  code  home  star  gear  download  disk
#         or "file:icons/my-icon.svg" for imported images (relative to nyx.exe).
# color = tints a builtin icon. Omit it to use the theme's accent.
# All shortcuts appear in Saved locations with their icons.

[[shortcut]]
id    = "home"
label = "Home"
path  = 'C:\Users\Nihil'
icon  = "builtin:home"
order = 10

[[shortcut]]
id    = "tibet"
label = "Free Tibet"
sigil = "@tibet"
path  = 'C:\Users\Nihil\Desktop\Free Tibet'
icon  = "builtin:folder"
order = 20

[[shortcut]]
id    = "nyx"
label = "Nyx"
sigil = "@nyx"
path  = 'C:\Users\Nihil\Desktop\Free Tibet\Nyx'
icon  = "builtin:code"
color = "#9ece6a"
order = 30

# Command aliases. Only the first word of a line is expanded, so "echo gs" stays "echo gs".
[alias]
gs = "git status"
gd = "git diff"
gl = "git log --oneline --graph --decorate -20"

# ---------------------------------------------------------------------------------------------
# Shells.
#
# Nyx finds Windows PowerShell, PowerShell 7, cmd.exe, Git Bash and every installed WSL
# distribution on its own; there is nothing to configure to use them. A [[profile]] entry *layers
# over* what was found, so you only write the fields you want to change — matched by `id`, which
# is one of: powershell · pwsh · cmd · gitbash · wsl-<distro>. Any other id adds a new entry.
#
#   id          matches a detected shell, or names a new one
#   name        what the palette and the + menu call it
#   program     the executable. Omit it to keep the detected path
#   args        omit to keep the detected arguments; [] deliberately means none
#   kind        cmd · powershell · bash · wsl — decides how @shortcut paths are spelled for this
#               shell and which init script it gets. Omit it to infer from the program's name
#   cwd         where a new tab starts
#   integration "auto" (the default) or "none" to turn off shell-side expansion for this shell
#   enabled     false hides a detected shell from the menu
#
# [[profile]]
# id      = "pwsh"
# args    = ["-NoLogo", "-NoProfile"]
#
# [[profile]]
# id      = "dev"
# name    = "Dev shell"
# program = 'C:\Program Files\Git\bin\bash.exe'
# cwd     = 'C:\Users\Nihil\Desktop\Free Tibet'
"##;

/// Read the config, writing the documented default if there is no file yet.
pub fn load() -> Loaded {
    let path = paths::config_file();

    if !path.exists() {
        match write_atomic(&path, DEFAULT_CONFIG.as_bytes()) {
            Ok(()) => log::info!("wrote default config to {}", path.display()),
            Err(err) => log::warn!("could not write default config: {err}"),
        }
    }

    load_from(&path)
}

pub fn load_from(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // No file is not a failure: there is nothing configured, so the defaults are the answer.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Loaded::Parsed { config: Box::new(Config::default()), problems: Vec::new() }
        },
        // Locked, unreadable, a scanner holding it open: we do not know what is in it, so this
        // is a failure like a syntax error and not a request for defaults.
        Err(err) => {
            return Loaded::Failed { problems: vec![format!("could not read config: {err}")] }
        },
    };

    parse(&text)
}

pub fn parse(text: &str) -> Loaded {
    // Notepad and PowerShell's `Set-Content -Encoding utf8` both write a UTF-8 BOM, and a
    // config file is something people edit with whatever is to hand. Left in place it is a
    // stray character before the first table header.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    match toml::from_str::<Config>(text) {
        Ok(mut config) => {
            // Older themes used ANSI black/blue for chrome. Preserve those colours when
            // loading a saved palette that predates the separate UI colour settings.
            if config.theme.background != crate::config::schema::ThemeConfig::default().background {
                if let Ok(doc) = text.parse::<toml_edit::DocumentMut>() {
                    if let Some(theme) = doc.get("theme").and_then(toml_edit::Item::as_table) {
                        if theme.contains_key("normal") && !theme.contains_key("accent") {
                            config.theme.accent = config.theme.normal.blue.clone();
                        }
                        if theme.contains_key("normal") && !theme.contains_key("surface") {
                            config.theme.surface = config.theme.normal.black.clone();
                        }
                    }
                }
            }
            let problems = config.sanitize();
            Loaded::Parsed { config: Box::new(config), problems }
        },
        Err(err) => {
            // No config comes back from here. Everything keeps working on the values already in
            // memory, and it is `Loaded`'s shape rather than this comment that makes the caller
            // decide what to do about it.
            let message = err.message().to_string();
            let where_ = err
                .span()
                .map(|s| format!(" (byte {})", s.start))
                .unwrap_or_default();
            Loaded::Failed {
                problems: vec![format!("config.toml is not valid TOML{where_}: {message}")],
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_file_parses_and_round_trips_to_the_defaults() {
        // If this fails, the documented file and the compiled defaults have drifted apart and
        // a fresh install behaves differently from an upgrade.
        let loaded = parse(DEFAULT_CONFIG);
        assert!(loaded.problems().is_empty(), "{:?}", loaded.problems());
        let config = loaded.expect_parsed();

        let defaults = Config::default();
        assert_eq!(config.theme.background, defaults.theme.background);
        assert_eq!(config.theme.normal.blue, defaults.theme.normal.blue);
        assert_eq!(config.theme.bright.cyan, defaults.theme.bright.cyan);
        assert_eq!(config.font.family, defaults.font.family);
        assert_eq!(config.font.size, defaults.font.size);
        assert_eq!(config.render.scrollback, defaults.render.scrollback);
        assert_eq!(config.window.width, defaults.window.width);

        // Every binding, not a sample of them. The keys table is the part of this file people
        // edit most and the part most likely to drift, because changing a default means changing
        // it in two places — and a documented file that disagrees with the compiled default is a
        // fresh install that behaves differently from an upgrade, silently.
        for (name, label) in crate::config::keys::KeysConfig::ACTIONS {
            assert_eq!(
                config.keys.get(name),
                defaults.keys.get(name),
                "the shipped file and the compiled default disagree about {label}"
            );
        }
    }

    /// The defaults must not take a chord the program is likely to want for itself.
    #[test]
    fn no_default_binding_claims_a_single_modifier_chord_from_the_shell() {
        let defaults = crate::config::keys::KeysConfig::default();
        for (name, label) in crate::config::keys::KeysConfig::ACTIONS {
            let spec = defaults.get(name).unwrap_or_default();
            let Some(binding) = crate::config::keys::parse(spec) else {
                continue; // deliberately unbound
            };
            // Two documented exceptions, and both earn it the same way: the binding hands the
            // key back when it has nothing to do. Alt+Arrow falls through whenever there is no
            // pane that way (`app::keys::directional_focus`) and Ctrl+Tab falls through in a
            // one-tab window (`app::keys::tab_step`) — which between them is most of the time.
            // Any *new* single-modifier default has to earn its place the same way or change.
            let falls_through = matches!(
                binding.key,
                egui::Key::ArrowUp
                    | egui::Key::ArrowDown
                    | egui::Key::ArrowLeft
                    | egui::Key::ArrowRight
                    | egui::Key::Tab
            );
            let modifiers = u8::from(binding.ctrl) + u8::from(binding.shift) + u8::from(binding.alt);
            let m = egui::Modifiers {
                ctrl: binding.ctrl,
                shift: binding.shift,
                alt: binding.alt,
                ..Default::default()
            };
            // A chord that writes no bytes — Ctrl+Comma has no control code and no escape
            // sequence — costs the program nothing to take. Asked of the real encoder rather
            // than of a second list here, so the two cannot drift apart. A modifier is still
            // required: a bare printable key arrives as text and would answer `false` too.
            let program_cannot_use_it =
                modifiers >= 1 && !crate::term::input::means_something_to_the_program(binding.key, m);
            assert!(
                modifiers >= 2 || falls_through || program_cannot_use_it,
                "{label} defaults to {spec:?}, which a shell or full-screen program wants"
            );
        }
    }

    #[test]
    fn a_utf8_bom_does_not_break_the_file() {
        // Notepad and PowerShell both add one; a user who edits their config with either would
        // otherwise be told their whole file is invalid.
        let with_bom = format!("\u{feff}{DEFAULT_CONFIG}");
        let loaded = parse(&with_bom);
        assert!(loaded.problems().is_empty(), "{:?}", loaded.problems());
        assert_eq!(loaded.expect_parsed().theme.background, "#faf9f5");
    }

    #[test]
    fn broken_toml_reports_instead_of_panicking() {
        let loaded = parse("[theme\nbackground = ");
        assert!(!loaded.problems().is_empty());
    }

    #[test]
    fn a_wrong_type_is_reported_not_swallowed() {
        let loaded = parse("[font]\nsize = \"big\"");
        assert!(!loaded.problems().is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_yields_no_config_at_all() {
        // This is the whole fix. The previous shape handed back `Config::default()` with a
        // complaint attached, and the comment on it said the caller would decide whether that
        // meant "defaults" (first load) or "what we had before" (hot reload). No caller decided:
        // the watcher forwarded the defaults and `logic` applied them, so saving a file mid-edit
        // reverted the theme, font, keys, aliases, shortcuts and Quake settings -- and rewrote
        // the generated shell integration files from the defaults on the way past.
        //
        // A `Failed` with no config in it is what stops the next caller making that assumption.
        assert!(matches!(parse("[theme\nbackground = "), Loaded::Failed { .. }));
        assert!(matches!(parse("[font]\nsize = \"big\""), Loaded::Failed { .. }));
    }

    #[test]
    fn a_parse_failure_is_not_confusable_with_a_complaint_about_one_value() {
        // Both produce problems, so "problems is non-empty" cannot be the signal a caller uses
        // to tell them apart. A file that parsed and had one value clamped must still be
        // applied -- that value costs itself and nothing else -- while a file that did not parse
        // must not be applied at all.
        let clamped = parse("[font]\nsize = 0.0");
        assert!(!clamped.problems().is_empty(), "the out-of-range size should be reported");
        assert!(matches!(clamped, Loaded::Parsed { .. }), "a clamped value is not a failure");

        let broken = parse("[theme\nbackground = ");
        assert!(!broken.problems().is_empty());
        assert!(matches!(broken, Loaded::Failed { .. }));
    }

    #[test]
    fn a_missing_file_is_the_defaults_but_an_unreadable_one_is_a_failure() {
        // Nothing configured means the defaults are the answer. A file we could not read is a
        // different thing entirely: we do not know what is in it, so a hot reload has to keep
        // what is already running.
        let missing = load_from(Path::new("no-such-directory-9000/config.toml"));
        assert!(matches!(missing, Loaded::Parsed { .. }));
        assert!(missing.problems().is_empty());

        // A directory stands in for "present but not readable as a file".
        let unreadable = load_from(Path::new("."));
        assert!(matches!(unreadable, Loaded::Failed { .. }));
        assert!(!unreadable.problems().is_empty());
    }
}
