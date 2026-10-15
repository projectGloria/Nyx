//! Hover-only copy controls. Button rectangles are shared with raw terminal pointer routing.

use crate::theme::{visuals::to_color32, Theme};

/// Cover exactly the block's own rows. Blocks are contiguous -- an output ends on
/// the row before the next prompt -- so any padding outside the rows is drawn over
/// the neighbouring block's text.
pub fn bounds(pane: egui::Rect, first_row: i32, end_row: i32, cell_height: f32, padding: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(pane.min.x, pane.min.y + padding + first_row as f32 * cell_height),
        egui::pos2(pane.max.x, pane.min.y + padding + end_row as f32 * cell_height),
    ).intersect(pane)
}

pub fn buttons(rect: egui::Rect) -> Vec<egui::Rect> {
    let width = 26.0_f32.min((rect.width() - 4.0).max(0.0));
    let inset = 4.0_f32.min(rect.height() / 4.0);
    let height = 18.0_f32.min((rect.height() - inset * 2.0).max(0.0));
    let bottom = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - width - 4.0, rect.max.y - height - inset), egui::vec2(width, height));
    vec![bottom]
}

pub fn show(ui: &mut egui::Ui, rect: egui::Rect, id: egui::Id, theme: &Theme, fade: f32) -> bool {
    let mut highlight = to_color32(theme.selection_bg).to_array();
    highlight[3] = (36.0 * fade) as u8;
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 5.0, egui::Color32::from_rgba_unmultiplied(
        highlight[0], highlight[1], highlight[2], highlight[3]));
    let mut clicked = false;
    for (index, button) in buttons(rect).into_iter().enumerate() {
        let response = ui.interact(button, id.with(index), egui::Sense::click()).on_hover_text("Copy this block");
        let bg = to_color32(theme.selection_bg).gamma_multiply(if response.hovered() { 0.22 * fade } else { 0.0 });
        painter.rect_filled(button, 4.0, bg);
        let back = egui::Rect::from_center_size(button.center() - egui::vec2(2.0, 2.0), egui::vec2(9.0, 10.0));
        let front = back.translate(egui::vec2(3.0, 3.0));
        let stroke = egui::Stroke::new(1.2, to_color32(theme.fg).gamma_multiply(if response.hovered() { fade } else { 0.38 * fade }));
        painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
        painter.rect_filled(front, 1.0, bg);
        painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
        if response.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
        clicked |= response.clicked();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_covers_its_own_rows_and_stays_inside_the_pane() {
        let pane = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0));
        let rect = bounds(pane, 2, 6, 20.0, 6.0);
        assert_eq!(rect.min.y, pane.min.y + 6.0 + 2.0 * 20.0);
        assert_eq!(rect.max.y, pane.min.y + 6.0 + 6.0 * 20.0);
        assert!(pane.contains_rect(bounds(pane, -3, 20, 20.0, 6.0)));
    }

    #[test]
    fn adjacent_blocks_abut_rather_than_overlap() {
        let pane = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0));
        let output = bounds(pane, 2, 6, 20.0, 6.0);
        let next_prompt = bounds(pane, 6, 7, 20.0, 6.0);
        assert_eq!(output.max.y, next_prompt.min.y);
    }

    #[test]
    fn inputs_and_outputs_have_one_icon_control_at_the_bottom_right() {
        let input = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 20.0));
        assert_eq!(buttons(input).len(), 1);
        let output = egui::Rect::from_min_size(input.min, egui::vec2(400.0, 100.0));
        let buttons = buttons(output);
        assert_eq!(buttons.len(), 1);
        assert!(buttons.iter().all(|button| output.contains_rect(*button)));
        assert_eq!(buttons[0].max.y, output.max.y - 4.0);
        assert_eq!(buttons[0].max.x, output.max.x - 4.0);
    }

    #[test]
    fn copy_icon_responds_to_pointer_clicks() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 100.0));
        for button in buttons(rect) {
            let ctx = egui::Context::default();
            let mut clicked = false;
            for pressed in [None, Some(true), Some(false)] {
                let events = pressed.map(|pressed| vec![egui::Event::PointerMoved(button.center()),
                    egui::Event::PointerButton { pos: button.center(), button: egui::PointerButton::Primary,
                        pressed, modifiers: egui::Modifiers::NONE }]).unwrap_or_default();
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    events, ..Default::default()
                }, |ui| { clicked = show(ui, rect, egui::Id::new("copy.test"), &Theme::default(), 1.0); });
                output.textures_delta.clear();
                if pressed != Some(false) { assert!(!clicked); }
            }
            assert!(clicked);
        }
    }
}
