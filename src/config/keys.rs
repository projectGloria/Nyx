//! Parsing keybindings out of config strings like `"Ctrl+Shift+T"`.

use serde::{Deserialize, Serialize};

/// One bound chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: egui::Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Binding {
    /// Whether an egui key event is this binding.
    ///
    /// Modifiers must match exactly. A binding on Ctrl+Shift+D that also fired for
    /// Ctrl+Alt+Shift+D would steal a chord the shell might want.
    pub fn matches(&self, key: egui::Key, m: egui::Modifiers) -> bool {
        self.key == key && m.ctrl == self.ctrl && m.shift == self.shift && m.alt == self.alt
    }

    /// What this binding is spelled as in `config.toml`.
    ///
    /// The settings GUI records a keystroke and has to write it back as text, so this is the
    /// inverse of [`parse`] and there is a round-trip test to keep it that way. Modifier order is
    /// fixed (Ctrl, Shift, Alt) so the same chord always produces the same string and a recorded
    /// binding does not show up as a spurious diff in the user's file.
    pub fn to_spec(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// Whether this chord can be written to `config.toml` and read back.
    ///
    /// egui knows keys that the config grammar does not — `Copy`, `Pipe`, `F24`. Recording one
    /// would produce a spec that fails to parse on the next launch, so the settings GUI checks
    /// here and refuses the keystroke instead of writing a binding that silently disappears.
    pub fn is_writable(self) -> bool {
        !is_modifier(self.key) && parse(&self.to_spec()).is_some()
    }

    /// Build one from what egui reported, ignoring the modifiers it cannot express in a spec.
    pub fn from_event(key: egui::Key, m: egui::Modifiers) -> Self {
        Self { key, ctrl: m.ctrl, shift: m.shift, alt: m.alt }
    }
}

/// Whether this key *is* a modifier rather than a key a modifier applies to.
///
/// egui reports Ctrl, Shift and Alt twice: as flags on every event, and as key presses of their
/// own. They arrive *first*, so anything that takes "the next key pressed" at face value records
/// `Ctrl+ControlLeft` — which is what the settings recorder did until this existed — and an armed
/// leader chord is cancelled by the Shift someone pressed on the way to typing a capital letter.
pub fn is_modifier(key: egui::Key) -> bool {
    use egui::Key;
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// Parse `"Ctrl+Shift+T"`. Returns `None` for anything unrecognised so the caller can report it
/// by name rather than silently binding nothing.
pub fn parse(spec: &str) -> Option<Binding> {
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut key = None;

    for part in spec.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            // The table below is the friendly spelling people type by hand ("pgdn", "esc", "-").
            // egui's own names are the fallback so that every key `Binding::to_spec` can produce
            // reads back — the settings GUI records a keystroke and writes it here, and a name it
            // could write but not parse would be a binding that vanished on the next launch.
            other => key = Some(key_from_name(other).or_else(|| egui::Key::from_name(part))?),
        }
    }

    key.map(|key| Binding { key, ctrl, shift, alt })
}

fn key_from_name(name: &str) -> Option<egui::Key> {
    use egui::Key;
    Some(match name {
        "a" => Key::A, "b" => Key::B, "c" => Key::C, "d" => Key::D, "e" => Key::E,
        "f" => Key::F, "g" => Key::G, "h" => Key::H, "i" => Key::I, "j" => Key::J,
        "k" => Key::K, "l" => Key::L, "m" => Key::M, "n" => Key::N, "o" => Key::O,
        "p" => Key::P, "q" => Key::Q, "r" => Key::R, "s" => Key::S, "t" => Key::T,
        "u" => Key::U, "v" => Key::V, "w" => Key::W, "x" => Key::X, "y" => Key::Y,
        "z" => Key::Z,
        "0" => Key::Num0, "1" => Key::Num1, "2" => Key::Num2, "3" => Key::Num3,
        "4" => Key::Num4, "5" => Key::Num5, "6" => Key::Num6, "7" => Key::Num7,
        "8" => Key::Num8, "9" => Key::Num9,
        "f1" => Key::F1, "f2" => Key::F2, "f3" => Key::F3, "f4" => Key::F4,
        "f5" => Key::F5, "f6" => Key::F6, "f7" => Key::F7, "f8" => Key::F8,
        "f9" => Key::F9, "f10" => Key::F10, "f11" => Key::F11, "f12" => Key::F12,
        "left" | "arrowleft" => Key::ArrowLeft,
        "right" | "arrowright" => Key::ArrowRight,
        "up" | "arrowup" => Key::ArrowUp,
        "down" | "arrowdown" => Key::ArrowDown,
        "tab" => Key::Tab,
        "enter" | "return" => Key::Enter,
        "space" => Key::Space,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "minus" | "-" => Key::Minus,
        "plus" | "+" => Key::Plus,
        "equals" | "=" => Key::Equals,
        "comma" | "," => Key::Comma,
        "period" | "." => Key::Period,
        "slash" | "/" => Key::Slash,
        "backslash" | "\\" => Key::Backslash,
        "backtick" | "`" => Key::Backtick,
        "openbracket" | "[" => Key::OpenBracket,
        "closebracket" | "]" => Key::CloseBracket,
        _ => return None,
    })
}

/// Every action a key can be bound to.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KeysConfig {
    pub new_tab: String,
    pub close_pane: String,
    pub next_tab: String,
    pub prev_tab: String,
    pub split_right: String,
    pub split_down: String,
    pub focus_left: String,
    pub focus_right: String,
    pub focus_up: String,
    pub focus_down: String,
    pub zoom_pane: String,
    pub palette: String,
    pub find: String,
    pub settings: String,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            new_tab: "Ctrl+Shift+T".into(),
            close_pane: "Ctrl+Shift+W".into(),
            next_tab: "Ctrl+Tab".into(),
            prev_tab: "Ctrl+Shift+Tab".into(),
            split_right: "Ctrl+Shift+D".into(),
            split_down: "Ctrl+Shift+E".into(),
            focus_left: "Alt+Left".into(),
            focus_right: "Alt+Right".into(),
            focus_up: "Alt+Up".into(),
            focus_down: "Alt+Down".into(),
            zoom_pane: "Ctrl+Shift+Z".into(),
            // Ctrl+Shift+P and Ctrl+Shift+F, not the single-modifier forms.
            //
            // `Ctrl+P` is `previous-history` and `Ctrl+F` is `forward-char` in every readline
            // shell, in PSReadLine, and in most full-screen programs — and unlike a focus
            // binding these two have no "nothing happened" case to fall through on: they open a
            // modal that then owns the whole keyboard, so the next thing typed goes into Nyx's
            // search box instead of the program. This is the same reasoning `[leader]` already
            // states for staying disabled by default ("Ctrl+A is beginning of line in every
            // readline shell"); it just had not been applied here. Windows Terminal uses the
            // Shift forms for the same reason.
            palette: "Ctrl+Shift+P".into(),
            find: "Ctrl+Shift+F".into(),
            settings: "Ctrl+Comma".into(),
        }
    }
}

impl KeysConfig {
    /// Every bindable action as (config key, label), in the order the settings GUI lists them.
    ///
    /// A table rather than a match arm per widget, so adding an action means adding one row here
    /// and one field above — and the test below fails if you do only one of the two.
    pub const ACTIONS: &'static [(&'static str, &'static str)] = &[
        ("new_tab", "New tab"),
        ("close_pane", "Close pane"),
        ("next_tab", "Next tab"),
        ("prev_tab", "Previous tab"),
        ("split_right", "Split right"),
        ("split_down", "Split down"),
        ("focus_left", "Focus left"),
        ("focus_right", "Focus right"),
        ("focus_up", "Focus up"),
        ("focus_down", "Focus down"),
        ("zoom_pane", "Zoom pane"),
        ("palette", "Command palette"),
        ("find", "Find in scrollback"),
        ("settings", "Settings"),
    ];

    pub fn get(&self, name: &str) -> Option<&str> {
        Some(match name {
            "new_tab" => &self.new_tab,
            "close_pane" => &self.close_pane,
            "next_tab" => &self.next_tab,
            "prev_tab" => &self.prev_tab,
            "split_right" => &self.split_right,
            "split_down" => &self.split_down,
            "focus_left" => &self.focus_left,
            "focus_right" => &self.focus_right,
            "focus_up" => &self.focus_up,
            "focus_down" => &self.focus_down,
            "zoom_pane" => &self.zoom_pane,
            "palette" => &self.palette,
            "find" => &self.find,
            "settings" => &self.settings,
            _ => return None,
        })
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut String> {
        Some(match name {
            "new_tab" => &mut self.new_tab,
            "close_pane" => &mut self.close_pane,
            "next_tab" => &mut self.next_tab,
            "prev_tab" => &mut self.prev_tab,
            "split_right" => &mut self.split_right,
            "split_down" => &mut self.split_down,
            "focus_left" => &mut self.focus_left,
            "focus_right" => &mut self.focus_right,
            "focus_up" => &mut self.focus_up,
            "focus_down" => &mut self.focus_down,
            "zoom_pane" => &mut self.zoom_pane,
            "palette" => &mut self.palette,
            "find" => &mut self.find,
            "settings" => &mut self.settings,
            _ => return None,
        })
    }
}

/// The parsed form, with every unparseable entry reported.
#[derive(Debug, Clone)]
pub struct Keys {
    pub new_tab: Option<Binding>,
    pub close_pane: Option<Binding>,
    pub next_tab: Option<Binding>,
    pub prev_tab: Option<Binding>,
    pub split_right: Option<Binding>,
    pub split_down: Option<Binding>,
    pub focus_left: Option<Binding>,
    pub focus_right: Option<Binding>,
    pub focus_up: Option<Binding>,
    pub focus_down: Option<Binding>,
    pub zoom_pane: Option<Binding>,
    pub palette: Option<Binding>,
    pub find: Option<Binding>,
    pub settings: Option<Binding>,
}

impl Keys {
    pub fn from_config(cfg: &KeysConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut one = |name: &str, spec: &str| -> Option<Binding> {
            if spec.trim().is_empty() {
                return None; // deliberately unbound
            }
            let parsed = parse(spec);
            if parsed.is_none() {
                problems.push(format!("keys.{name}: \"{spec}\" is not a key combination"));
            }
            parsed
        };

        let keys = Self {
            new_tab: one("new_tab", &cfg.new_tab),
            close_pane: one("close_pane", &cfg.close_pane),
            next_tab: one("next_tab", &cfg.next_tab),
            prev_tab: one("prev_tab", &cfg.prev_tab),
            split_right: one("split_right", &cfg.split_right),
            split_down: one("split_down", &cfg.split_down),
            focus_left: one("focus_left", &cfg.focus_left),
            focus_right: one("focus_right", &cfg.focus_right),
            focus_up: one("focus_up", &cfg.focus_up),
            focus_down: one("focus_down", &cfg.focus_down),
            zoom_pane: one("zoom_pane", &cfg.zoom_pane),
            palette: one("palette", &cfg.palette),
            find: one("find", &cfg.find),
            settings: one("settings", &cfg.settings),
        };
        (keys, problems)
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self::from_config(&KeysConfig::default()).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_in_any_order_and_case() {
        let a = parse("Ctrl+Shift+T").unwrap();
        let b = parse("shift+CTRL+t").unwrap();
        assert_eq!(a, b);
        assert!(a.ctrl && a.shift && !a.alt);
        assert_eq!(a.key, egui::Key::T);
    }

    #[test]
    fn parses_a_bare_key() {
        let b = parse("F11").unwrap();
        assert_eq!(b.key, egui::Key::F11);
        assert!(!b.ctrl && !b.shift && !b.alt);
    }

    #[test]
    fn rejects_nonsense_rather_than_binding_nothing() {
        assert!(parse("Ctrl+Nonsense").is_none());
        assert!(parse("Ctrl+Shift").is_none()); // modifiers only, no key
        assert!(parse("").is_none());
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let b = parse("Ctrl+Shift+D").unwrap();
        let exact = egui::Modifiers { ctrl: true, shift: true, ..Default::default() };
        let extra = egui::Modifiers { ctrl: true, shift: true, alt: true, ..Default::default() };
        assert!(b.matches(egui::Key::D, exact));
        // Otherwise this would steal Ctrl+Alt+Shift+D from the shell.
        assert!(!b.matches(egui::Key::D, extra));
        assert!(!b.matches(egui::Key::E, exact));
    }

    #[test]
    fn the_defaults_all_parse() {
        let (keys, problems) = Keys::from_config(&KeysConfig::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(keys.new_tab.is_some());
        assert!(keys.focus_left.is_some());
        assert!(keys.zoom_pane.is_some());
    }

    #[test]
    fn an_empty_binding_is_unbound_not_an_error() {
        let cfg = KeysConfig { new_tab: "".into(), ..Default::default() };
        let (keys, problems) = Keys::from_config(&cfg);
        assert!(keys.new_tab.is_none());
        assert!(problems.is_empty());
    }

    #[test]
    fn the_action_table_and_the_struct_agree() {
        // An action listed but not reachable is a settings row that edits nothing; a field with
        // no row is a binding the GUI cannot see. Both are silent, so they are asserted here.
        let mut cfg = KeysConfig::default();
        for (name, label) in KeysConfig::ACTIONS {
            assert!(cfg.get(name).is_some(), "{name} is listed but not readable");
            assert!(cfg.get_mut(name).is_some(), "{name} is listed but not writable");
            assert!(!label.is_empty());
        }
        let (keys, _) = Keys::from_config(&cfg);
        // Every default binding is one of the listed actions, so nothing is bound invisibly.
        let bound = [
            keys.new_tab, keys.close_pane, keys.next_tab, keys.prev_tab, keys.split_right,
            keys.split_down, keys.focus_left, keys.focus_right, keys.focus_up, keys.focus_down,
            keys.zoom_pane, keys.palette, keys.find, keys.settings,
        ];
        assert_eq!(bound.len(), KeysConfig::ACTIONS.len());
    }

    #[test]
    fn every_binding_the_recorder_can_produce_reads_back_as_itself() {
        // The settings GUI records a keystroke, writes `to_spec()` into config.toml, and the next
        // launch parses it. A key that survives one direction but not the other is a binding that
        // silently disappears, so the two functions are tested against each other rather than
        // against a list of strings someone maintained by hand.
        use egui::Key;
        let keys = [
            Key::A, Key::Z, Key::Num0, Key::Num9, Key::F1, Key::F12,
            Key::ArrowLeft, Key::ArrowDown, Key::Tab, Key::Enter, Key::Space, Key::Escape,
            Key::Backspace, Key::Delete, Key::Insert, Key::Home, Key::End,
            Key::PageUp, Key::PageDown, Key::Minus, Key::Plus, Key::Equals, Key::Comma,
            Key::Period, Key::Slash, Key::Backslash, Key::Backtick,
            Key::OpenBracket, Key::CloseBracket,
            // Keys the hand-written table does not know, reached through egui's own names.
            Key::Semicolon, Key::Quote, Key::Pipe, Key::Colon,
        ];
        for key in keys {
            for (ctrl, shift, alt) in
                [(false, false, false), (true, false, false), (true, true, true)]
            {
                let binding = Binding { key, ctrl, shift, alt };
                let spec = binding.to_spec();
                assert_eq!(parse(&spec), Some(binding), "{spec} did not round-trip");
                assert!(binding.is_writable(), "{spec} is not writable");
            }
        }
    }

    #[test]
    fn a_modifier_on_its_own_is_never_a_writable_binding() {
        // The recorder sees ControlLeft before it sees the key the user is actually pressing.
        let ctrl = Binding { key: egui::Key::ControlLeft, ctrl: true, shift: false, alt: false };
        assert!(!ctrl.is_writable());
        assert!(is_modifier(egui::Key::ShiftRight));
        assert!(!is_modifier(egui::Key::F));
        // The chord it was on its way to is perfectly writable.
        let real = Binding { key: egui::Key::F, ctrl: true, shift: true, alt: false };
        assert!(real.is_writable());
        assert_eq!(real.to_spec(), "Ctrl+Shift+F");
    }

    #[test]
    fn to_spec_writes_modifiers_in_a_fixed_order() {
        // Otherwise recording the same chord twice produces two different strings and the user's
        // file shows a diff for a setting they did not change.
        let b = parse("Alt+shift+CTRL+t").unwrap();
        assert_eq!(b.to_spec(), "Ctrl+Shift+Alt+T");
    }

    #[test]
    fn a_bad_binding_reports_its_key_name() {
        let cfg = KeysConfig { split_down: "Ctrl+Banana".into(), ..Default::default() };
        let (_, problems) = Keys::from_config(&cfg);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("split_down"), "{}", problems[0]);
    }
}
