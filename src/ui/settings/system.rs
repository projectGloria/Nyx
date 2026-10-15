//! Quake mode, single instance, and Explorer's context menu.
//!
//! One of these is not like the others. The Quake and window settings edit the draft and are saved
//! with everything else; "Open Nyx here" writes to the registry and happens **when the button is
//! pressed**, because it is machine state rather than configuration — it survives uninstalling
//! Nyx, and a `config.toml` copied to another machine must not silently register a context menu
//! pointing at an executable that is not there.

use super::{keys as keys_tab, widgets, Capture, State};
use crate::config::keys;
use crate::theme::visuals::to_color32;
use crate::theme::Theme;
use crate::win::{hotkey, registry};

const ALIGNMENTS: [&str; 3] = ["left", "center", "right"];
const MONITORS: [(&str, &str); 2] = [
    ("cursor", "the screen the pointer is on"),
    ("primary", "always the primary screen"),
];

pub fn show(ui: &mut egui::Ui, state: &mut State, theme: &Theme) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Typing sounds").strong());
    egui::ComboBox::from_id_salt("nyx.sounds.pack")
        .selected_text(if state.draft.sounds.pack.is_empty() {
            "Off"
        } else {
            &state.draft.sounds.pack
        })
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(&mut state.draft.sounds.pack, String::new(), "Off")
                .changed();
            for pack in crate::win::sounds::packs() {
                changed |= ui
                    .selectable_value(&mut state.draft.sounds.pack, pack.clone(), pack)
                    .changed();
            }
        });
    let mut volume = state.draft.sounds.volume * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut volume, 0.0..=100.0)
                .text("Volume")
                .suffix("%"),
        )
        .changed()
    {
        state.draft.sounds.volume = volume / 100.0;
        changed = true;
    }
    if !state.draft.sounds.pack.is_empty()
        && !crate::win::sounds::packs().contains(&state.draft.sounds.pack)
    {
        ui.label("Selected sound pack is missing.");
    }
    if let Some(message) = &state.sound_message {
        ui.label(message);
    }
    ui.separator();
    ui.label(egui::RichText::new("Launch from anywhere").strong());
    changed |= ui
        .checkbox(
            &mut state.draft.launcher.enabled,
            "Enable Windows launcher at sign-in",
        )
        .changed();
    ui.horizontal(|ui| {
        ui.label("Launch key");
        changed |= ui
            .text_edit_singleline(&mut state.draft.launcher.hotkey)
            .changed();
    });
    ui.weak("Launches or focuses Nyx, including when it is closed.");
    if let Some(message) = crate::win::launcher::status() {
        ui.label(message);
    }
    ui.separator();

    ui.label(egui::RichText::new("Quake mode").strong());
    ui.label(
        egui::RichText::new(
            "One chord shows and hides Nyx from anywhere, and the window drops down from the top \
             of the screen. Off by default: a global hotkey is a claim on every keystroke on the \
             machine.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    let enabled = state.draft.quake.enabled;
    egui::Grid::new("nyx.settings.quake")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            changed |= widgets::toggle(
                ui,
                "Quake mode",
                &mut state.draft.quake.enabled,
                "a global hotkey shows and hides Nyx",
            );

            ui.label("Hotkey");
            ui.horizontal(|ui| {
                changed |= keys_tab::binding_row(ui, state, Capture::QuakeHotkey, theme);
            });
            ui.end_row();

            // The extra rule a global chord has over an in-app one, said where it is chosen rather
            // than discovered on the next launch.
            if let Some(note) = hotkey_note(&state.draft.quake.hotkey) {
                ui.label("");
                ui.colored_label(
                    to_color32(theme.palette[1]),
                    egui::RichText::new(note).small(),
                );
                ui.end_row();
            }
        });

    ui.add_space(8.0);
    ui.add_enabled_ui(enabled, |ui| {
        egui::Grid::new("nyx.settings.quake.geometry")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                changed |= widgets::toggle(
                    ui,
                    "Drop down",
                    &mut state.draft.quake.dropdown,
                    "snap to the top of the screen; off leaves the window where you put it",
                );

                changed |= percent(ui, "Width", &mut state.draft.quake.width);
                changed |= percent(ui, "Height", &mut state.draft.quake.height);

                ui.label("Align");
                ui.horizontal(|ui| {
                    for name in ALIGNMENTS {
                        if ui
                            .selectable_label(state.draft.quake.align == name, name)
                            .clicked()
                        {
                            state.draft.quake.align = name.to_string();
                            changed = true;
                        }
                    }
                });
                ui.end_row();

                ui.label("Drop on");
                ui.horizontal(|ui| {
                    for (name, hint) in MONITORS {
                        if ui
                            .selectable_label(state.draft.quake.monitor == name, name)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            state.draft.quake.monitor = name.to_string();
                            changed = true;
                        }
                    }
                });
                ui.end_row();

                ui.label("Slide");
                ui.horizontal(|ui| {
                    let mut ms = state.draft.quake.animate_ms as f32;
                    if ui
                        .add(
                            egui::DragValue::new(&mut ms)
                                .range(0.0..=1000.0)
                                .speed(5.0)
                                .suffix(" ms"),
                        )
                        .changed()
                    {
                        state.draft.quake.animate_ms = ms as u64;
                        changed = true;
                    }
                    ui.weak("0 snaps");
                });
                ui.end_row();

                changed |= widgets::toggle(
                    ui,
                    "Hide when it loses focus",
                    &mut state.draft.quake.hide_on_focus_loss,
                    "clicking another window puts Nyx away",
                );
                changed |= widgets::toggle(
                    ui,
                    "Start hidden",
                    &mut state.draft.quake.start_hidden,
                    "for a Nyx launched at sign-in",
                );
            });
    });

    ui.add_space(14.0);
    ui.separator();
    ui.label(egui::RichText::new("Windows").strong());
    ui.add_space(6.0);

    egui::Grid::new("nyx.settings.window")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            changed |= widgets::toggle(
                ui,
                "Single instance",
                &mut state.draft.window.single_instance,
                "a second launch becomes a tab here instead of another window",
            );
        });

    ui.add_space(10.0);
    context_menu(ui, state, theme);

    changed
}

/// The Explorer entry. Not part of the draft — it is written the moment the button is pressed.
fn context_menu(ui: &mut egui::Ui, state: &mut State, theme: &Theme) {
    let exe = registry::executable();
    let installed = registry::installed_for(&exe);

    ui.horizontal(|ui| {
        ui.label("Explorer menu");
        if installed {
            ui.colored_label(
                to_color32(theme.palette[2]),
                "\u{201c}Open Nyx here\u{201d} is installed",
            );
            if ui.button("Remove").clicked() {
                state.menu_message = Some(match registry::remove() {
                    Ok(()) => "Removed.".to_string(),
                    Err(err) => err,
                });
            }
        } else {
            ui.weak("not installed");
            if ui.button("Add \u{201c}Open Nyx here\u{201d}").clicked() {
                state.menu_message = Some(match registry::install("Open Nyx here", &exe) {
                    Ok(()) => "Added. Right-click a folder to use it.".to_string(),
                    Err(err) => err,
                });
            }
        }
    });

    ui.label(
        egui::RichText::new(format!(
            "Written to HKEY_CURRENT_USER only \u{2014} no elevation, and nobody else on this \
             machine is affected. Points at {}.",
            exe.display()
        ))
        .small()
        .weak(),
    );
    ui.label(
        egui::RichText::new(
            "On Windows 11 it appears under \u{201c}Show more options\u{201d}. The short menu only \
             takes entries from a registered package, which is an install shape Nyx does not have.",
        )
        .small()
        .weak(),
    );
    if let Some(message) = &state.menu_message {
        ui.colored_label(
            to_color32(theme.palette[3]),
            egui::RichText::new(message).small(),
        );
    }
}

/// Why this chord will not work as a global hotkey, if it will not.
fn hotkey_note(spec: &str) -> Option<String> {
    if spec.trim().is_empty() {
        return Some("no hotkey set, so there is no way to show a hidden window".into());
    }
    let binding = keys::parse(spec)?; // the row already marks an unparseable one
    if hotkey::Chord::from_binding(binding).is_some() {
        return None;
    }
    Some(format!(
        "\"{spec}\" cannot be a global hotkey \u{2014} it needs at least one of Ctrl, Shift or \
         Alt, and a key Windows has a code for"
    ))
}

/// A fraction of the screen, shown as the percentage a person would say out loud.
fn percent(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    ui.label(label);
    let mut shown = (*value * 100.0).round();
    let changed = ui
        .add(
            egui::DragValue::new(&mut shown)
                .range(10.0..=100.0)
                .speed(1.0)
                .suffix(" %"),
        )
        .changed();
    if changed {
        *value = shown / 100.0;
    }
    ui.end_row();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_alignment_and_monitor_parses() {
        // These strings go straight into config.toml. One that does not parse is a setting the
        // window offers, writes, and then reports as a config error on the next launch.
        for name in ALIGNMENTS {
            assert!(crate::win::quake::Align::parse(name).is_some(), "{name}");
        }
        for (name, _) in MONITORS {
            assert!(crate::win::quake::Monitor::parse(name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_chord_with_no_modifier_is_flagged_where_it_is_chosen() {
        assert!(hotkey_note("Ctrl+Backtick").is_none());
        assert!(hotkey_note("F12").is_some());
        assert!(hotkey_note("").is_some());
    }
}
