//! Command aliases.

use super::State;

pub fn show(ui: &mut egui::Ui, state: &mut State) -> bool {
    let mut changed = false;

    ui.label(
        egui::RichText::new(
            "Only the first word of a line is expanded, so \"echo gs\" stays \"echo gs\". In \
             PowerShell and Clink these become real shell functions; everywhere else Nyx rewrites \
             the line as you press Enter.",
        )
        .small()
        .weak(),
    );
    ui.add_space(8.0);

    let mut remove: Option<String> = None;
    let mut rename: Option<(String, String)> = None;
    let mut retarget: Option<(String, String)> = None;

    egui::Grid::new("nyx.settings.aliases")
        .num_columns(3)
        .spacing([12.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for (name, command) in &state.draft.aliases {
                let mut name_buf = name.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut name_buf)
                            .desired_width(90.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed()
                {
                    rename = Some((name.clone(), name_buf));
                }

                let mut command_buf = command.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut command_buf)
                            .desired_width(340.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed()
                {
                    retarget = Some((name.clone(), command_buf));
                }

                if ui.button("Remove").clicked() {
                    remove = Some(name.clone());
                }
                ui.end_row();
            }
        });

    ui.add_space(6.0);
    if ui.button("Add an alias").clicked() {
        let name = (1..)
            .map(|n| if n == 1 { "alias".to_string() } else { format!("alias{n}") })
            .find(|n| !state.draft.aliases.contains_key(n))
            .unwrap_or_else(|| "alias".into());
        state.draft.aliases.insert(name, String::new());
        changed = true;
    }

    // Renaming a key in a map is remove-then-insert, which reorders it. The map is a BTreeMap so
    // the order is the sorted one either way, and the file is written in that order.
    if let Some((old, new)) = rename {
        if !new.trim().is_empty() && !state.draft.aliases.contains_key(&new) {
            if let Some(command) = state.draft.aliases.remove(&old) {
                state.draft.aliases.insert(new, command);
                changed = true;
            }
        }
    }
    if let Some((name, command)) = retarget {
        state.draft.aliases.insert(name, command);
        changed = true;
    }
    if let Some(name) = remove {
        state.draft.aliases.remove(&name);
        changed = true;
    }

    changed
}
