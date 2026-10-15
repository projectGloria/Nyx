//! tmux-style leader chords: a prefix key, then a single key that names a command.
//!
//! The point of a leader is that Nyx needs exactly one reserved chord instead of a dozen. While
//! it is armed the next keystroke is swallowed whether or not it matches — otherwise a mistyped
//! chord would leak a character into the shell, which is worse than doing nothing.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::config::keys::{self, Binding};

/// A command a chord can name. Parsed from config so an unknown name is reported rather than
/// silently ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    NewTab,
    NextTab,
    PrevTab,
    CloseTab,
    SplitRight,
    SplitDown,
    ClosePane,
    ZoomPane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    Palette,
    /// `shortcut.<id>` — jump to a path.
    Shortcut(String),
}

impl Command {
    /// Every fixed command name, for the settings GUI's dropdown. `shortcut.<id>` is not here
    /// because the list of shortcuts is not fixed; the GUI appends those from the config.
    pub const NAMES: &'static [&'static str] = &[
        "tab.new",
        "tab.next",
        "tab.prev",
        "tab.close",
        "pane.split_right",
        "pane.split_down",
        "pane.close",
        "pane.zoom",
        "pane.left",
        "pane.right",
        "pane.up",
        "pane.down",
        "palette",
    ];

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if let Some(id) = name.strip_prefix("shortcut.") {
            if id.is_empty() {
                return None;
            }
            return Some(Command::Shortcut(id.to_string()));
        }
        Some(match name {
            "tab.new" => Command::NewTab,
            "tab.next" => Command::NextTab,
            "tab.prev" => Command::PrevTab,
            "tab.close" => Command::CloseTab,
            "pane.split_right" => Command::SplitRight,
            "pane.split_down" => Command::SplitDown,
            "pane.close" => Command::ClosePane,
            "pane.zoom" => Command::ZoomPane,
            "pane.left" => Command::FocusLeft,
            "pane.right" => Command::FocusRight,
            "pane.up" => Command::FocusUp,
            "pane.down" => Command::FocusDown,
            "palette" => Command::Palette,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Chords {
    pub leader: Option<Binding>,
    pub timeout: Duration,
    /// Keyed by the literal character typed after the leader.
    map: BTreeMap<char, Command>,
}

/// What a keystroke did to the chord state machine.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not chord-related; hand it to the shell.
    Ignored,
    /// The leader was pressed; the next key belongs to us.
    Armed,
    /// Swallowed, and it named this command.
    Run(Command),
    /// Swallowed and named nothing. Deliberately not passed through: leaking a stray character
    /// into the shell is worse than a chord that did nothing.
    Cancelled,
}

impl Chords {
    pub fn from_config(cfg: &crate::config::schema::LeaderConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();

        let leader = if cfg.key.trim().is_empty() {
            None
        } else {
            let parsed = keys::parse(&cfg.key);
            if parsed.is_none() {
                problems.push(format!("leader.key: \"{}\" is not a key combination", cfg.key));
            }
            parsed
        };

        let mut map = BTreeMap::new();
        for (key, command) in &cfg.chords {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                problems.push(format!("leader.chords: \"{key}\" must be a single character"));
                continue;
            };
            match Command::parse(command) {
                Some(cmd) => {
                    map.insert(c, cmd);
                },
                None => problems.push(format!(
                    "leader.chords.{key}: \"{command}\" is not a known command"
                )),
            }
        }

        (Chords { leader, timeout: Duration::from_millis(cfg.timeout_ms.max(100)), map }, problems)
    }

    pub fn is_empty(&self) -> bool {
        self.leader.is_none() || self.map.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct ChordState {
    armed: Option<Instant>,
}

impl ChordState {
    /// Feed a key press. `text` is the character it would have produced, if any.
    pub fn press(
        &mut self,
        chords: &Chords,
        key: egui::Key,
        modifiers: egui::Modifiers,
        text: Option<char>,
    ) -> Outcome {
        // Ctrl, Shift and Alt arrive as key presses of their own, and they arrive *before* the
        // key they modify. Treating one as the chord's second key means the Shift someone pressed
        // on their way to a capital letter cancels the chord they had just armed.
        if keys::is_modifier(key) {
            return Outcome::Ignored;
        }

        // An armed leader that was never followed up expires, so the terminal does not sit
        // swallowing a keystroke minutes later.
        if let Some(at) = self.armed {
            if at.elapsed() > chords.timeout {
                self.armed = None;
            }
        }

        if self.armed.take().is_some() {
            // Modifiers are ignored on the second key: tmux's `prefix c` works whether or not
            // shift happens to be held.
            return match text.and_then(|c| chords.map.get(&c)) {
                Some(cmd) => Outcome::Run(cmd.clone()),
                None => Outcome::Cancelled,
            };
        }

        if chords.leader.is_some_and(|b| b.matches(key, modifiers)) {
            self.armed = Some(Instant::now());
            return Outcome::Armed;
        }

        Outcome::Ignored
    }

    /// Whether a leader has been pressed and its second key is still awaited.
    ///
    /// `#[cfg(test)]` because that is the truth: nothing in the app asks. The chord state is
    /// read by `Chords::feed` alone, and there is no on-screen indicator that a leader is
    /// pending. Left here rather than inlined into the tests because "is the chord armed" is
    /// what those tests are about, and spelling it `self.armed.is_some()` four times makes them
    /// read like assertions about a field instead.
    #[cfg(test)]
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::LeaderConfig;

    fn chords() -> Chords {
        // The default leader is empty (chords off), so a fixture has to opt in.
        let mut cfg = LeaderConfig { key: "Ctrl+A".into(), ..Default::default() };
        cfg.chords.insert("c".into(), "tab.new".into());
        cfg.chords.insert("z".into(), "pane.zoom".into());
        cfg.chords.insert("g".into(), "shortcut.tibet".into());
        Chords::from_config(&cfg).0
    }

    const NONE: egui::Modifiers = egui::Modifiers::NONE;

    #[test]
    fn the_leader_arms_and_the_next_key_runs() {
        let c = chords();
        let mut state = ChordState::default();
        assert_eq!(state.press(&c, egui::Key::A, egui::Modifiers::CTRL, None), Outcome::Armed);
        assert!(state.is_armed());
        assert_eq!(
            state.press(&c, egui::Key::C, NONE, Some('c')),
            Outcome::Run(Command::NewTab)
        );
        assert!(!state.is_armed());
    }

    #[test]
    fn an_unbound_second_key_is_swallowed_rather_than_leaked() {
        // Letting it through would type a stray character into the shell.
        let c = chords();
        let mut state = ChordState::default();
        state.press(&c, egui::Key::A, egui::Modifiers::CTRL, None);
        assert_eq!(state.press(&c, egui::Key::Q, NONE, Some('q')), Outcome::Cancelled);
        assert!(!state.is_armed());
    }

    #[test]
    fn a_modifier_press_does_not_cancel_an_armed_chord() {
        // Reaching for a capital letter presses Shift first, and egui reports that as a key.
        let c = chords();
        let mut state = ChordState::default();
        state.press(&c, egui::Key::A, egui::Modifiers::CTRL, None);
        assert_eq!(
            state.press(&c, egui::Key::ShiftLeft, egui::Modifiers::SHIFT, None),
            Outcome::Ignored
        );
        assert!(state.is_armed(), "the chord should still be waiting");
        assert_eq!(
            state.press(&c, egui::Key::C, egui::Modifiers::SHIFT, Some('c')),
            Outcome::Run(Command::NewTab)
        );
    }

    #[test]
    fn an_ordinary_key_passes_straight_through() {
        let c = chords();
        let mut state = ChordState::default();
        assert_eq!(state.press(&c, egui::Key::X, NONE, Some('x')), Outcome::Ignored);
    }

    #[test]
    fn shortcut_chords_carry_their_id() {
        let c = chords();
        let mut state = ChordState::default();
        state.press(&c, egui::Key::A, egui::Modifiers::CTRL, None);
        assert_eq!(
            state.press(&c, egui::Key::G, NONE, Some('g')),
            Outcome::Run(Command::Shortcut("tibet".into()))
        );
    }

    #[test]
    fn every_offered_command_name_parses() {
        // The settings GUI writes these strings straight into config.toml; one that does not
        // parse is a chord the user set that does nothing and reports itself as a config error.
        for name in Command::NAMES {
            assert!(Command::parse(name).is_some(), "{name} is offered but not parsed");
        }
    }

    #[test]
    fn an_unknown_command_name_is_reported() {
        let mut cfg = LeaderConfig { key: "Ctrl+A".into(), ..Default::default() };
        cfg.chords.insert("q".into(), "not.a.command".into());
        let (chords, problems) = Chords::from_config(&cfg);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("not.a.command"));
        assert!(chords.is_empty());
    }

    #[test]
    fn a_multi_character_chord_key_is_refused() {
        let mut cfg = LeaderConfig { key: "Ctrl+A".into(), ..Default::default() };
        cfg.chords.insert("ab".into(), "tab.new".into());
        let (_, problems) = Chords::from_config(&cfg);
        assert!(problems[0].contains("single character"));
    }

    #[test]
    fn an_empty_leader_disables_chords_without_complaint() {
        let cfg = LeaderConfig { key: String::new(), ..Default::default() };
        let (chords, problems) = Chords::from_config(&cfg);
        assert!(problems.is_empty());
        assert!(chords.is_empty());
    }
}
