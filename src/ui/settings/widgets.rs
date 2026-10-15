//! The handful of rows every settings tab is built out of.
//!
//! Each returns whether it changed the value it was given, and the caller ORs those together into
//! one "the draft moved" flag. That is what drives both the live apply and the debounced save, so
//! a row that forgets to report a change is a setting that silently does not stick.

use crate::theme::visuals::to_color32;
use crate::theme::{Rgba, Theme};

/// A colour, as both a picker and the hex the config file will contain.
///
/// Both, not either: the picker is how you choose one and the text is how you paste one from a
/// palette someone published. The text field is also the only way to see *why* a colour is not
/// applying, which is why an unparseable value is marked rather than silently corrected.
pub fn hex(ui: &mut egui::Ui, label: &str, value: &mut String, theme: &Theme) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        let parsed = Rgba::parse_hex(value);
        let mut rgb = parsed.map(|c| [c.r, c.g, c.b]).unwrap_or([0, 0, 0]);
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            changed = true;
        }

        let mut text = value.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(78.0)
                .font(egui::TextStyle::Monospace),
        );
        if response.changed() {
            *value = text;
            changed = true;
        }

        if parsed.is_none() {
            ui.colored_label(to_color32(theme.palette[1]), "not #rrggbb");
        }
    });
    ui.end_row();
    changed
}

/// A colour that may be absent, with a checkbox for "have one at all".
///
/// `absent` says what *not* having one means, which differs per setting: a selection without a
/// foreground keeps each cell's own colour, an icon without one takes the theme's accent. A shared
/// "none" would be true and useless.
pub fn optional_hex(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<String>,
    fallback: &str,
    absent: &str,
    theme: &Theme,
) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, "").changed() {
            *value = enabled.then(|| fallback.to_string());
            changed = true;
        }
        if let Some(text) = value.as_mut() {
            let parsed = Rgba::parse_hex(text);
            let mut rgb = parsed.map(|c| [c.r, c.g, c.b]).unwrap_or([0, 0, 0]);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                *text = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                changed = true;
            }
            let mut buf = text.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut buf)
                        .desired_width(78.0)
                        .font(egui::TextStyle::Monospace),
                )
                .changed()
            {
                *text = buf;
                changed = true;
            }
            if parsed.is_none() {
                ui.colored_label(to_color32(theme.palette[1]), "not #rrggbb");
            }
        } else {
            ui.weak(absent);
        }
    });
    ui.end_row();
    changed
}

pub fn text(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) -> bool {
    text_hinted(ui, label, value, width, "")
}

/// A text row whose empty state means something, shown greyed inside the field.
pub fn text_hinted(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    width: f32,
    hint: &str,
) -> bool {
    ui.label(label);
    let changed = ui
        .add(
            egui::TextEdit::singleline(value)
                .desired_width(width)
                .hint_text(hint)
                .font(egui::TextStyle::Monospace),
        )
        .changed();
    ui.end_row();
    changed
}

pub fn number(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    speed: f64,
    suffix: &str,
) -> bool {
    ui.label(label);
    let changed = ui
        .add(egui::DragValue::new(value).range(range).speed(speed).suffix(suffix))
        .changed();
    ui.end_row();
    changed
}

pub fn integer(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    speed: f64,
) -> bool {
    ui.label(label);
    let changed =
        ui.add(egui::DragValue::new(value).range(range).speed(speed)).changed();
    ui.end_row();
    changed
}

pub fn toggle(ui: &mut egui::Ui, label: &str, value: &mut bool, hint: &str) -> bool {
    ui.label(label);
    let changed = ui.checkbox(value, hint).changed();
    ui.end_row();
    changed
}

/// A heading between groups of rows inside one `Grid`.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).strong());
    ui.end_row();
}
