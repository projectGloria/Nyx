//! Keybindings and leader chords.
//!
//! Bindings are *recorded*, not typed: you press "Set" and then the chord itself. That is the only
//! way to be sure the string in the file is the one your keyboard actually produces — a German
//! layout's `@` is AltGr+Q, and asking someone to spell that is asking them to guess.

use super::{Capture, State};
use crate::config::keys::{self, KeysConfig};
use crate::expand::chords::Command;
use crate::theme::visuals::to_color32;
use crate::theme::Theme;

pub fn show(ui: &mut egui::Ui, state: &mut State, theme: &Theme) -> bool {
    let mut changed = false;

    ui.label(
        egui::RichText::new(
            "A binding takes the key away from the shell. Clear one to hand it back.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    egui::Grid::new("nyx.settings.keys")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for (name, label) in KeysConfig::ACTIONS {
                ui.label(*label);
                ui.horizontal(|ui| {
                    changed |= binding_row(ui, state, Capture::Action((*name).to_string()), theme);
                });
                ui.end_row();
            }
        });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("Leader chords").strong());
    ui.label(
        egui::RichText::new(
            "One reserved chord, then a single key naming a command \u{2014} tmux style. Off by \
             default: Ctrl+A is \"beginning of line\" in every readline shell, and swallowing it \
             is surprising unless you asked for it.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    egui::Grid::new("nyx.settings.leader")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label("Leader");
            ui.horizontal(|ui| {
                changed |= binding_row(ui, state, Capture::Leader, theme);
            });
            ui.end_row();

            ui.label("Timeout");
            let mut ms = state.draft.leader.timeout_ms as f32;
            if ui
                .add(egui::DragValue::new(&mut ms).range(100.0..=10_000.0).speed(10.0).suffix(" ms"))
                .changed()
            {
                state.draft.leader.timeout_ms = ms as u64;
                changed = true;
            }
            ui.end_row();
        });

    if let Some(error) = &state.capture_error {
        ui.colored_label(to_color32(theme.palette[1]), error);
    }

    ui.add_space(8.0);
    changed |= chords(ui, state);
    changed
}

/// One "Ctrl+Shift+T  [Set] [Clear]" row, for any recordable binding.
pub(super) fn binding_row(
    ui: &mut egui::Ui,
    state: &mut State,
    slot: Capture,
    theme: &Theme,
) -> bool {
    let current = match &slot {
        Capture::Leader => state.draft.leader.key.clone(),
        Capture::QuakeHotkey => state.draft.quake.hotkey.clone(),
        Capture::Action(name) => state.draft.keys.get(name).unwrap_or_default().to_string(),
    };
    let arming = state.capture.as_ref() == Some(&slot);

    let (text, color) = if arming {
        ("press a key\u{2026}".to_string(), theme.palette[3])
    } else if current.trim().is_empty() {
        ("unbound".to_string(), theme.palette[8])
    } else if keys::parse(&current).is_none() {
        (format!("{current}  (not a key combination)"), theme.palette[1])
    } else {
        (current.clone(), theme.fg)
    };

    ui.add_sized(
        [200.0, 20.0],
        egui::Label::new(
            egui::RichText::new(text).monospace().color(to_color32(color)),
        )
        .selectable(false),
    );

    let mut changed = false;
    if ui.button(if arming { "Cancel" } else { "Set" }).clicked() {
        state.capture = if arming { None } else { Some(slot.clone()) };
        state.capture_error = None;
    }
    if ui.add_enabled(!current.trim().is_empty(), egui::Button::new("Clear")).clicked() {
        match &slot {
            Capture::Leader => state.draft.leader.key.clear(),
            Capture::QuakeHotkey => state.draft.quake.hotkey.clear(),
            Capture::Action(name) => {
                if let Some(v) = state.draft.keys.get_mut(name) {
                    v.clear();
                }
            },
        }
        state.capture = None;
        changed = true;
    }
    changed
}

fn chords(ui: &mut egui::Ui, state: &mut State) -> bool {
    let mut changed = false;
    let enabled = !state.draft.leader.key.trim().is_empty();

    // Shortcut chords are offered by id, so `shortcut.tibet` is a menu item rather than something
    // you have to know the syntax for.
    let mut names: Vec<String> = Command::NAMES.iter().map(|s| (*s).to_string()).collect();
    for shortcut in &state.draft.shortcuts {
        if !shortcut.id.trim().is_empty() {
            names.push(format!("shortcut.{}", shortcut.id));
        }
    }

    let mut remove: Option<String> = None;
    let mut rename: Option<(String, String)> = None;
    let mut retarget: Option<(String, String)> = None;

    ui.add_enabled_ui(enabled, |ui| {
        egui::Grid::new("nyx.settings.chords")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                for (key, command) in &state.draft.leader.chords {
                    let mut key_buf = key.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut key_buf)
                                .desired_width(36.0)
                                .char_limit(1)
                                .font(egui::TextStyle::Monospace),
                        )
                        .changed()
                    {
                        rename = Some((key.clone(), key_buf));
                    }

                    egui::ComboBox::from_id_salt(("nyx.settings.chord", key))
                        .width(220.0)
                        .selected_text(command.clone())
                        .show_ui(ui, |ui| {
                            for name in &names {
                                if ui.selectable_label(command == name, name).clicked() {
                                    retarget = Some((key.clone(), name.clone()));
                                }
                            }
                        });

                    if ui.button("Remove").clicked() {
                        remove = Some(key.clone());
                    }
                    ui.end_row();
                }
            });

        if ui.button("Add a chord").clicked() {
            let key = ('a'..='z')
                .map(|c| c.to_string())
                .find(|c| !state.draft.leader.chords.contains_key(c));
            if let Some(key) = key {
                state.draft.leader.chords.insert(key, "tab.new".into());
                changed = true;
            }
        }
    });

    if !enabled {
        ui.label(
            egui::RichText::new("Set a leader above to enable chords.").small().weak(),
        );
    }

    if let Some((old, new)) = rename {
        // A chord key is one character: it is matched against what the keystroke produced, and
        // there is only ever one of those.
        let mut chars = new.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if let Some(command) = state.draft.leader.chords.remove(&old) {
                state.draft.leader.chords.insert(c.to_string(), command);
                changed = true;
            }
        }
    }
    if let Some((key, command)) = retarget {
        state.draft.leader.chords.insert(key, command);
        changed = true;
    }
    if let Some(key) = remove {
        state.draft.leader.chords.remove(&key);
        changed = true;
    }

    changed
}
