//! The shortcut and icon manager.
//!
//! Each entry is a collapsing block rather than a row in a grid: a shortcut has eight fields, and
//! eight columns across is unreadable at any window width anyone actually uses.

use super::widgets;
use super::State;
use crate::config::schema::ShortcutConfig;
use crate::theme::visuals::to_color32;
use crate::theme::{Rgba, Theme};
use crate::ui::icons::{self, Builtin, Icon};

pub fn show(ui: &mut egui::Ui, state: &mut State, theme: &Theme) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut swap: Option<(usize, usize)> = None;
    let count = state.draft.shortcuts.len();

    // Split before the loop: the list is being iterated mutably, so the icon cache cannot be
    // reached through `state` from inside it.
    let State {
        draft,
        icons: icons_cache,
        icon_message,
        ..
    } = state;
    for (i, shortcut) in draft.shortcuts.iter_mut().enumerate() {
        let title = if shortcut.label.trim().is_empty() {
            shortcut.id.clone()
        } else {
            shortcut.label.clone()
        };

        let response = egui::CollapsingHeader::new(if title.trim().is_empty() {
            format!("shortcut {}", i + 1)
        } else {
            title
        })
        .id_salt(("nyx.settings.shortcut", i))
        .default_open(count <= 3)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                preview(ui, shortcut, theme, icons_cache);
                ui.vertical(|ui| {
                    changed |= fields(ui, i, shortcut, theme, icon_message);
                });
            });

            ui.horizontal(|ui| {
                // Words rather than arrow glyphs: egui's default proportional face has no
                // U+2191/U+2193 and draws them as tofu boxes.
                if ui
                    .add_enabled(i > 0, egui::Button::new("Move up"))
                    .clicked()
                {
                    swap = Some((i, i - 1));
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("Move down"))
                    .clicked()
                {
                    swap = Some((i, i + 1));
                }
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        });
        response
            .header_response
            .on_hover_text(shortcut.path.clone());
    }

    ui.add_space(8.0);
    if ui.button("Add a shortcut").clicked() {
        let id = unique_id(&state.draft.shortcuts);
        state.draft.shortcuts.push(ShortcutConfig {
            sigil: format!("@{id}"),
            label: id.clone(),
            id,
            icon: "builtin:folder".into(),
            path: dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..Default::default()
        });
        changed = true;
    }

    if let Some((a, b)) = swap {
        // Order is what the footer menu and the palette sort by, so reordering has to move the numbers
        // rather than only the list — otherwise the change is invisible everywhere but here.
        state.draft.shortcuts.swap(a, b);
        for (i, s) in state.draft.shortcuts.iter_mut().enumerate() {
            s.order = (i as i32 + 1) * 10;
        }
        changed = true;
    }
    if let Some(i) = remove {
        state.draft.shortcuts.remove(i);
        changed = true;
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "The sigil is what you type on the command line: \"cd @tibet\" then space. It is \
             rewritten for whichever shell the pane is running.",
        )
        .small()
        .weak(),
    );

    ui.add_space(12.0);
    egui::Grid::new("nyx.settings.shortcut.behaviour")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            widgets::heading(ui, "At the prompt");
            changed |= widgets::toggle(
                ui,
                "Completion menu",
                &mut state.draft.expand.completion,
                "offer matching shortcuts as you type",
            );
            changed |= widgets::toggle(
                ui,
                "Hint",
                &mut state.draft.expand.hint,
                "describe the line under the cursor",
            );
        });
    ui.label(
        egui::RichText::new(
            "The menu is the only part of Nyx that takes Tab, Escape, Enter and the arrows from \
             whatever is running. Where a shell reports what it is doing, it stands down by \
             itself while a command runs; in a plain cmd.exe, WSL or ssh pane nothing can tell \
             it, so turn it off if you work in programs that give \"@\" a meaning of their own. \
             Expansion on space is unaffected.",
        )
        .small()
        .weak(),
    );

    changed
}

fn fields(
    ui: &mut egui::Ui,
    index: usize,
    shortcut: &mut ShortcutConfig,
    theme: &Theme,
    message: &mut Option<String>,
) -> bool {
    let mut changed = false;

    egui::Grid::new(("nyx.settings.shortcut.grid", index))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            changed |= widgets::text(ui, "Label", &mut shortcut.label, 240.0);
            changed |= widgets::text(ui, "Id", &mut shortcut.id, 160.0);
            // Empty is not "no sigil", it is "@<id>" — worth saying, since a blank field
            // otherwise reads as a feature that is switched off.
            let default_sigil = format!("@{}", shortcut.id);
            changed |=
                widgets::text_hinted(ui, "Sigil", &mut shortcut.sigil, 160.0, &default_sigil);
            changed |= widgets::text(ui, "Path", &mut shortcut.path, 380.0);

            ui.label("Icon");
            ui.horizontal(|ui| {
                let current = Icon::parse(&shortcut.icon);
                let label = match &current {
                    Icon::Builtin(b) => b.name().to_string(),
                    Icon::File(_) => "file…".to_string(),
                    Icon::Exe(_) => "a program…".to_string(),
                };
                egui::ComboBox::from_id_salt(("nyx.settings.icon", index))
                    .width(140.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for builtin in Builtin::ALL {
                            if ui
                                .selectable_label(current == Icon::Builtin(builtin), builtin.name())
                                .clicked()
                            {
                                shortcut.icon = format!("builtin:{}", builtin.name());
                                changed = true;
                            }
                        }
                    });
                if ui.button("Upload from PC").clicked() {
                    match crate::win::assets::choose()
                        .and_then(|path| path.map(|p| crate::win::assets::import(&p)).transpose())
                    {
                        Ok(Some(spec)) => {
                            shortcut.icon = spec;
                            changed = true;
                            *message = None;
                        }
                        Ok(None) => {}
                        Err(err) => *message = Some(format!("Could not import icon: {err}")),
                    }
                }
                // The raw spec, so `file:C:\icons\thing.png` is reachable without a file dialog.
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut shortcut.icon)
                            .desired_width(240.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
            ui.end_row();

            changed |= widgets::optional_hex(
                ui,
                "Icon colour",
                &mut shortcut.color,
                "#7aa2f7",
                "the theme's accent",
                theme,
            );

            ui.label("Order");
            let mut order = shortcut.order as f32;
            if ui
                .add(egui::DragValue::new(&mut order).speed(1.0))
                .changed()
            {
                shortcut.order = order as i32;
                changed = true;
            }
            ui.end_row();
        });

    if let Some(message) = message {
        ui.colored_label(to_color32(theme.palette[1]), message.as_str());
    }
    changed
}

/// The icon as the footer menu will actually draw it, at the size it draws it.
fn preview(ui: &mut egui::Ui, shortcut: &ShortcutConfig, theme: &Theme, cache: &mut icons::Cache) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
    let color = shortcut
        .color
        .as_deref()
        .and_then(Rgba::parse_hex)
        .unwrap_or(theme.accent);
    match Icon::parse(&shortcut.icon) {
        Icon::Builtin(builtin) => {
            icons::draw_builtin(ui.painter(), rect, builtin, to_color32(color));
        }
        // The real icon, not a placeholder: this one Nyx loads itself, so the preview can show
        // exactly what the menu will.
        Icon::Exe(path) => match cache.exe(ui.ctx(), &path) {
            Some(texture) => {
                ui.painter().image(
                    texture.id(),
                    icons::fitted_rect(rect, texture),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                icons::draw_builtin(
                    ui.painter(),
                    rect,
                    icons::Builtin::Folder,
                    to_color32(color),
                );
                ui.painter().text(
                    rect.center_bottom(),
                    egui::Align2::CENTER_BOTTOM,
                    "no icon",
                    egui::FontId::monospace(9.0),
                    to_color32(theme.palette[1]),
                );
            }
        },
        // Also the real icon. This used to be a "file" placeholder because nothing could load one
        // — now that `icons::Cache` does, the preview shows what the menu will draw, which is the
        // whole point of a preview.
        Icon::File(path) => match cache.file(ui.ctx(), &path) {
            Some(texture) => {
                ui.painter().image(
                    texture.id(),
                    icons::fitted_rect(rect, texture),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                icons::draw_builtin(
                    ui.painter(),
                    rect,
                    icons::Builtin::Folder,
                    to_color32(color),
                );
                ui.painter().text(
                    rect.center_bottom(),
                    egui::Align2::CENTER_BOTTOM,
                    "no image",
                    egui::FontId::monospace(9.0),
                    to_color32(theme.palette[1]),
                );
            }
        },
    }
}

/// `shortcut`, `shortcut2`, … — never a duplicate, because two shortcuts with the same id make
/// `shortcut.<id>` chords and the palette ambiguous.
fn unique_id(existing: &[ShortcutConfig]) -> String {
    let taken = |id: &str| existing.iter().any(|s| s.id == id);
    if !taken("shortcut") {
        return "shortcut".into();
    }
    (2..)
        .map(|n| format!("shortcut{n}"))
        .find(|id| !taken(id))
        .unwrap_or_else(|| "shortcut".into())
}
