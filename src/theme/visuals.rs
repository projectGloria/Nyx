//! The only place egui receives a colour.
//!
//! Chrome is minimal in Phase 1 (a status line and the perf overlay), but the seam exists from
//! the start: when the tab strip, footer and settings window arrive they inherit the theme
//! for free, and a hardcoded `Color32` in a widget stays findable as the anomaly it is.

use super::{Rgba, Theme};

pub fn to_color32(c: Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

pub fn visuals(theme: &Theme) -> egui::Visuals {
    let light =
        0.2126 * theme.bg.r as f32 + 0.7152 * theme.bg.g as f32 + 0.0722 * theme.bg.b as f32
            > 128.0;
    let mut v = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };

    let bg = to_color32(theme.bg);
    let fg = to_color32(theme.fg);
    let accent = to_color32(theme.accent);

    v.panel_fill = bg;
    v.window_fill = to_color32(theme.surface);
    v.extreme_bg_color = bg;
    v.faint_bg_color = to_color32(theme.selection_bg);
    v.selection.bg_fill = to_color32(theme.selection_bg);
    v.selection.stroke = egui::Stroke::new(1.0, accent);
    v.override_text_color = Some(fg);

    let border = fg.gamma_multiply(0.2);
    v.window_stroke = egui::Stroke::new(1.0, border);
    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.bg_fill = to_color32(theme.surface);
        widget.weak_bg_fill = to_color32(theme.surface);
        widget.bg_stroke = egui::Stroke::new(1.0, border);
        widget.fg_stroke = egui::Stroke::new(1.0, fg);
    }
    v.widgets.hovered.bg_fill = to_color32(theme.selection_bg);
    v.widgets.hovered.weak_bg_fill = to_color32(theme.selection_bg);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
    v.widgets.active.bg_fill = to_color32(theme.selection_bg);
    v.widgets.active.weak_bg_fill = to_color32(theme.selection_bg);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v
}
