//! The settings window.
//!
//! It edits a **draft** `Config`, not the running one. The draft is applied live on every change —
//! that is what makes the theme editor a theme editor rather than a form — and written to disk on
//! a debounce, because a colour-picker drag produces a change per frame and each one is a file
//! write plus a watcher round trip.
//!
//! Nothing here writes to disk or touches a `Term`. `show` reports that the draft moved and the
//! app decides what that costs.

use crate::config::schema::Config;
use crate::term::detect::Detected;
use crate::term::profile::Profile;
use crate::theme::Theme;

mod aliases;
mod font;
mod keys;
mod shells;
mod shortcuts;
mod system;
mod theme_tab;
mod widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Theme,
    Text,
    Shortcuts,
    Keys,
    Shells,
    Aliases,
    System,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::Theme, "Theme"),
    (Tab::Text, "Text & window"),
    (Tab::Shortcuts, "Shortcuts"),
    (Tab::Keys, "Keys"),
    (Tab::Shells, "Shells"),
    (Tab::Aliases, "Aliases"),
    (Tab::System, "Quake & Windows"),
];

/// Which binding is waiting for a keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// A `[keys]` action, by its config key.
    Action(String),
    /// The leader chord itself.
    Leader,
    /// The global show/hide chord. Recorded the same way as the rest, but it has one extra
    /// requirement the others do not — see `win::hotkey::Chord`.
    QuakeHotkey,
}

pub struct State {
    pub open: bool,
    pub tab: Tab,
    /// The config being edited. Applied live; written to disk on a debounce.
    pub draft: Config,
    /// Complaints from the last live apply. Shown in the window rather than as toasts: a
    /// half-typed hex colour is invalid for three keystrokes and would otherwise fill the screen
    /// with warnings about a value the user is still typing.
    pub problems: Vec<String>,
    /// Installed monospace families. Snapshotted when the window opens, because enumerating them
    /// needs the render state and the list cannot change while Nyx is running.
    pub families: Vec<String>,
    /// Armed by a "set" button; the app feeds the next keystroke here instead of to the shell.
    pub capture: Option<Capture>,
    /// Why the last recorded keystroke was refused.
    pub capture_error: Option<String>,
    /// Textures for `icon = "exe:..."` previews. Its own rather than the app's, so closing and
    /// reopening the window re-reads an executable the user has just replaced.
    pub icons: crate::ui::icons::Cache,
    /// What the last Explorer-menu button press did. Kept separately from `problems`, which is
    /// replaced wholesale by every live config apply — a result the user asked for by pressing a
    /// button should not vanish on the next keystroke somewhere else in the window.
    pub menu_message: Option<String>,
    pub cursor_message: Option<String>,
    pub icon_message: Option<String>,
    pub sound_message: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Theme,
            draft: Config::default(),
            problems: Vec::new(),
            families: Vec::new(),
            capture: None,
            capture_error: None,
            icons: Default::default(),
            menu_message: None,
            cursor_message: None,
            icon_message: None,
            sound_message: None,
        }
    }
}

impl State {
    /// Open the window on a fresh copy of the running config.
    ///
    /// `families` is filled in on the first frame rather than here: enumerating installed fonts
    /// needs the render state, which the keyboard path does not have.
    pub fn open(&mut self, config: &Config) {
        self.open = true;
        self.draft = config.clone();
        self.families.clear();
        self.capture = None;
        self.capture_error = None;
        self.icons.clear();
        self.menu_message = None;
        self.cursor_message = None;
        self.icon_message = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.capture = None;
    }

    /// Adopt a config that changed underneath us — someone edited the file in another editor.
    ///
    /// Keeping the draft would mean the next keystroke in the window silently reverted their
    /// edit, so the window shows the truth instead.
    pub fn adopt(&mut self, config: &Config) {
        self.draft = config.clone();
    }

    /// Store a recorded keystroke, or say why it cannot be stored.
    ///
    /// Returns true if the draft changed.
    pub fn record(&mut self, binding: crate::config::keys::Binding) -> bool {
        let Some(slot) = self.capture.take() else {
            return false;
        };
        if !binding.is_writable() {
            // egui knows keys the config grammar does not. Writing one would produce a spec that
            // fails to parse on the next launch — a binding that quietly disappears.
            self.capture_error = Some(format!(
                "\"{}\" cannot be written to config.toml",
                binding.to_spec()
            ));
            return false;
        }
        self.capture_error = None;
        let spec = binding.to_spec();
        match slot {
            Capture::Leader => self.draft.leader.key = spec,
            Capture::QuakeHotkey => {
                // A chord with no modifier parses and writes back perfectly well, and would then
                // be refused by `RegisterHotKey` at the next launch — so the extra rule is
                // enforced where the keystroke is taken, not where it is used.
                if crate::win::hotkey::Chord::from_binding(binding).is_none() {
                    self.capture_error = Some(format!(
                        "\"{spec}\" cannot be a global hotkey — it needs at least one of Ctrl, \
                         Shift or Alt"
                    ));
                    return false;
                }
                self.draft.quake.hotkey = spec;
            }
            Capture::Action(name) => {
                if let Some(slot) = self.draft.keys.get_mut(&name) {
                    *slot = spec;
                }
            }
        }
        true
    }
}

/// Draw the window. Returns true when the draft changed and has to be applied.
pub fn show(
    ctx: &egui::Context,
    state: &mut State,
    theme: &Theme,
    profiles: &[Profile],
    detected: &Detected,
) -> bool {
    let mut changed = false;
    let mut open = state.open;

    egui::Window::new("Nyx settings")
        .open(&mut open)
        .default_size([760.0, 560.0])
        .min_width(560.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, label) in TABS {
                    if ui.selectable_label(state.tab == tab, label).clicked() {
                        state.tab = tab;
                        // A half-armed recorder that survived a tab change would eat the next
                        // keystroke somewhere the user can no longer see what it is for.
                        state.capture = None;
                    }
                }
            });
            ui.separator();

            // The problem list is pinned to the bottom so it does not jump around as the list
            // above it scrolls.
            let problems = std::mem::take(&mut state.problems);
            let reserved = if problems.is_empty() {
                30.0
            } else {
                30.0 + problems.len() as f32 * 16.0
            };

            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - reserved).max(120.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    changed |= match state.tab {
                        Tab::Theme => theme_tab::show(ui, state, theme),
                        Tab::Text => font::show(ui, state),
                        Tab::Shortcuts => shortcuts::show(ui, state, theme),
                        Tab::Keys => keys::show(ui, state, theme),
                        Tab::Shells => shells::show(ui, state, profiles, detected, theme),
                        Tab::Aliases => aliases::show(ui, state),
                        Tab::System => system::show(ui, state, theme),
                    };
                });

            ui.separator();
            for problem in &problems {
                ui.colored_label(
                    crate::theme::visuals::to_color32(theme.palette[3]),
                    egui::RichText::new(problem).small(),
                );
            }
            state.problems = problems;

            ui.horizontal(|ui| {
                ui.weak(format!(
                    "saved to {}",
                    crate::config::paths::config_file().display()
                ));
            });
        });

    if !open {
        state.close();
    }
    changed
}
