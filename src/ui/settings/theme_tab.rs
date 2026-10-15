//! The colour editor.
//!
//! Named `theme_tab` rather than `theme` so that `crate::theme` is still reachable by its own name
//! from inside this module.

use super::widgets;
use super::State;
use crate::config::schema::AnsiGroup;
use crate::theme::Theme;

pub fn show(ui: &mut egui::Ui, state: &mut State, theme: &Theme) -> bool {
    let mut changed = false;
    ui.heading("Theme");
    ui.weak("Choose a preset. Changes apply immediately and save automatically.");
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for preset in crate::theme::presets::Preset::ALL {
            let config = preset.config();
            let selected =
                toml::to_string(&state.draft.theme).ok() == toml::to_string(&config).ok();
            if ui
                .selectable_label(selected, &config.name)
                .on_hover_text(preset.description())
                .clicked()
            {
                state.draft.theme = config;
                changed = true;
            }
        }
    });
    ui.add_space(10.0);
    let (preview, _) = Theme::from_config(&state.draft.theme);
    egui::Frame::new()
        .fill(crate::theme::visuals::to_color32(preview.bg))
        .stroke(egui::Stroke::new(
            1.0,
            crate::theme::visuals::to_color32(preview.accent),
        ))
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.colored_label(
                crate::theme::visuals::to_color32(preview.fg),
                egui::RichText::new(&state.draft.theme.name).strong(),
            );
            ui.colored_label(
                crate::theme::visuals::to_color32(preview.accent),
                egui::RichText::new("PS C:\\Projects> ").monospace(),
            );
            ui.colored_label(
                crate::theme::visuals::to_color32(preview.fg),
                "Your workspace, in your colours.",
            );
            ui.horizontal(|ui| {
                for c in &preview.palette[..8] {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 8.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 3.0, crate::theme::visuals::to_color32(*c));
                }
            });
        });
    ui.add_space(12.0);
    ui.label(egui::RichText::new("Custom colours").strong());
    let t = &mut state.draft.theme;

    egui::Grid::new("nyx.settings.theme")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            changed |= widgets::text(ui, "Name", &mut t.name, 220.0);
            changed |= widgets::hex(ui, "Accent", &mut t.accent, theme);
            changed |= widgets::hex(ui, "UI surfaces", &mut t.surface, theme);
            changed |= widgets::hex(ui, "Background", &mut t.background, theme);
            changed |= widgets::hex(ui, "Foreground", &mut t.foreground, theme);
            changed |= widgets::hex(ui, "Cursor", &mut t.cursor, theme);
            changed |= widgets::hex(ui, "Under the cursor", &mut t.cursor_text, theme);
            changed |= widgets::hex(ui, "Selection", &mut t.selection, theme);
            changed |= widgets::optional_hex(
                ui,
                "Selected text",
                &mut t.selection_text,
                "#ffffff",
                "each cell keeps its own colour",
                theme,
            );

            widgets::heading(ui, "Find");
            changed |= widgets::hex(ui, "Matches", &mut t.search, theme);
            changed |= widgets::hex(ui, "Match text", &mut t.search_text, theme);
            changed |= widgets::hex(ui, "Current match", &mut t.search_current, theme);
            changed |= widgets::hex(ui, "Current match text", &mut t.search_current_text, theme);

            widgets::heading(ui, "ANSI");
            changed |= widgets::toggle(
                ui,
                "Bold is bright",
                &mut t.bold_is_bright,
                "bold text also uses the bright palette",
            );
        });

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Colours 16-255 are the xterm cube and greyscale ramp. They are deliberately not \
             themeable: programs index into them expecting exact values.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);

    // The two palettes side by side, because the thing you are usually checking is whether a
    // bright colour is distinguishable from its normal twin.
    ui.horizontal_top(|ui| {
        for (name, group) in [
            ("normal", &mut state.draft.theme.normal),
            ("bright", &mut state.draft.theme.bright),
        ] {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(name).strong());
                egui::Grid::new(format!("nyx.settings.ansi.{name}"))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        changed |= ansi_group(ui, group, theme);
                    });
            });
            ui.add_space(24.0);
        }
    });

    changed
}

fn ansi_group(ui: &mut egui::Ui, group: &mut AnsiGroup, theme: &Theme) -> bool {
    let mut changed = false;
    changed |= widgets::hex(ui, "black", &mut group.black, theme);
    changed |= widgets::hex(ui, "red", &mut group.red, theme);
    changed |= widgets::hex(ui, "green", &mut group.green, theme);
    changed |= widgets::hex(ui, "yellow", &mut group.yellow, theme);
    changed |= widgets::hex(ui, "blue", &mut group.blue, theme);
    changed |= widgets::hex(ui, "magenta", &mut group.magenta, theme);
    changed |= widgets::hex(ui, "cyan", &mut group.cyan, theme);
    changed |= widgets::hex(ui, "white", &mut group.white, theme);
    changed
}
