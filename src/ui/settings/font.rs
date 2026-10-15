//! Font, rendering and window size.

use super::widgets;
use super::State;

pub fn show(ui: &mut egui::Ui, state: &mut State) -> bool {
    let mut changed = false;
    let families = state.families.clone();
    let draft = &mut state.draft;

    egui::Grid::new("nyx.settings.font")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label("Family");
            ui.horizontal(|ui| {
                // A dropdown of what is installed *and* a free text field. The list is only
                // monospace faces, and someone who wants a proportional font for a reason we did
                // not think of should not be stopped by a combo box.
                egui::ComboBox::from_id_salt("nyx.settings.family")
                    .width(240.0)
                    .selected_text(draft.font.family.clone())
                    .show_ui(ui, |ui| {
                        for family in &families {
                            if ui
                                .selectable_label(&draft.font.family == family, family)
                                .clicked()
                            {
                                draft.font.family = family.clone();
                                changed = true;
                            }
                        }
                    });
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut draft.font.family)
                            .desired_width(180.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
            ui.end_row();

            changed |= widgets::number(ui, "Size", &mut draft.font.size, 4.0..=200.0, 0.25, " pt");
            changed |= widgets::number(
                ui,
                "Line height",
                &mut draft.font.line_height,
                0.5..=4.0,
                0.01,
                "\u{00d7}",
            );
            changed |= widgets::toggle(
                ui,
                "Ligatures",
                &mut draft.font.ligatures,
                "needs a font that has them — Cascadia Code, JetBrains Mono, Fira Code",
            );

            widgets::heading(ui, "Rendering");
            changed |= widgets::number(
                ui,
                "Padding",
                &mut draft.render.padding,
                0.0..=200.0,
                0.5,
                " px",
            );
            changed |= widgets::integer(
                ui,
                "Scrollback",
                &mut draft.render.scrollback,
                100..=1_000_000,
                50.0,
            );
            ui.label("Cursor style");
            use crate::config::schema::CursorStyle;
            egui::ComboBox::from_id_salt("nyx.settings.cursor_style")
                .selected_text(match draft.render.cursor_style {
                    CursorStyle::Block => "Block", CursorStyle::Beam => "I-Beam (Bar)", CursorStyle::Underline => "Underline",
                }).show_ui(ui, |ui| {
                    for (style, label) in [(CursorStyle::Block, "Block"), (CursorStyle::Beam, "I-Beam (Bar)"), (CursorStyle::Underline, "Underline")] {
                        changed |= ui.selectable_value(&mut draft.render.cursor_style, style, label).changed();
                    }
                });
            ui.end_row();
            ui.label("Mouse cursor icon");
            ui.horizontal(|ui| {
                if ui.button("Choose .cur file...").clicked() {
                    match crate::win::cursor::choose().and_then(|source| source.map(|source| crate::win::cursor::import(&source)).transpose()) {
                        Ok(Some(path)) => {
                            draft.render.cursor_icon = path.to_string_lossy().into_owned();
                            state.cursor_message = Some(format!("Saved to {}", path.display()));
                            changed = true;
                        },
                        Ok(None) => {},
                        Err(err) => state.cursor_message = Some(format!("Could not import cursor: {err}")),
                    }
                }
                if !draft.render.cursor_icon.is_empty() {
                    ui.label(std::path::Path::new(&draft.render.cursor_icon).file_name().unwrap_or_default().to_string_lossy())
                        .on_hover_text(&draft.render.cursor_icon);
                    if ui.button("Use system cursor").clicked() {
                        draft.render.cursor_icon.clear();
                        state.cursor_message = None;
                        changed = true;
                    }
                }
            });
            ui.end_row();
            if let Some(message) = &state.cursor_message {
                ui.label(""); ui.label(egui::RichText::new(message).small()); ui.end_row();
            }
            changed |= widgets::toggle(ui, "Blink the cursor", &mut draft.render.cursor_blink, "");

            ui.label("Blink period");
            let mut ms = draft.render.cursor_blink_ms as f32;
            if ui
                .add(
                    egui::DragValue::new(&mut ms)
                        .range(100.0..=5000.0)
                        .speed(5.0)
                        .suffix(" ms"),
                )
                .changed()
            {
                draft.render.cursor_blink_ms = ms as u64;
                changed = true;
            }
            ui.end_row();

            widgets::heading(ui, "Window");
            changed |= widgets::number(
                ui,
                "Width",
                &mut draft.window.width,
                200.0..=20_000.0,
                1.0,
                " px",
            );
            changed |= widgets::number(
                ui,
                "Height",
                &mut draft.window.height,
                100.0..=20_000.0,
                1.0,
                " px",
            );

        });

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Scrollback applies to shells started from now on: a terminal fixes its history size \
             when it is created, so changing it cannot reach the tabs already open.",
        )
        .small()
        .weak(),
    );
    ui.label(
        egui::RichText::new(
            "Width and height are the size Nyx opens at, not the size of the window right now.",
        )
        .small()
        .weak(),
    );

    changed
}
