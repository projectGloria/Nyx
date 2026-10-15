//! Nyx-owned window chrome and reserved terminal/footer rectangles.

use crate::theme::{visuals::to_color32, Theme};

pub const TITLE_HEIGHT: f32 = 34.0;
const FOOTER_HEIGHT: f32 = 30.0;
const EDGE: f32 = 4.0;

pub struct Regions {
    pub title: egui::Rect,
    pub terminal: egui::Rect,
    pub tabs: egui::Rect,
    pub footer: egui::Rect,
}

pub fn regions(full: egui::Rect) -> Regions {
    let content = full.shrink(EDGE);
    let title_end = (content.min.y + TITLE_HEIGHT).min(content.max.y);
    let footer_start = (content.max.y - FOOTER_HEIGHT).max(title_end);
    let slice = |top, bottom| egui::Rect::from_min_max(
        egui::pos2(content.min.x, top), egui::pos2(content.max.x, bottom),
    );
    Regions {
        title: slice(content.min.y, title_end),
        terminal: slice(title_end, footer_start),
        tabs: egui::Rect::from_min_max(content.min,
            egui::pos2((content.max.x - 3.0 * 44.0 - 80.0).max(content.min.x), title_end)),
        footer: slice(footer_start, content.max.y),
    }
}

pub fn title_bar(ui: &mut egui::Ui, rect: egui::Rect, theme: &Theme) -> egui::Response {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ui.painter().rect_filled(rect, 0.0, to_color32(theme.surface));
    let controls_left = (rect.max.x - 3.0 * 44.0).max(rect.min.x);
    let drag_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(controls_left, rect.max.y));
    let drag = ui.interact(drag_rect, egui::Id::new("nyx.window.drag"), egui::Sense::click_and_drag());
    let controls_visible = ctx.pointer_hover_pos().is_some_and(|pos| rect.contains(pos));
    for (index, (label, hint, command)) in [
        ("−", "Minimize", egui::ViewportCommand::Minimized(true)),
        (if maximized { "❐" } else { "□" }, if maximized { "Restore" } else { "Maximize" }, egui::ViewportCommand::Maximized(!maximized)),
        ("×", "Close", egui::ViewportCommand::Close),
    ].into_iter().enumerate() {
        let button = egui::Rect::from_min_max(
            egui::pos2(controls_left + index as f32 * 44.0, rect.min.y),
            egui::pos2((controls_left + (index + 1) as f32 * 44.0).min(rect.max.x), rect.max.y),
        );
        let response = ui.interact(button, egui::Id::new(("nyx.window.control", index)), egui::Sense::click()).on_hover_text(hint);
        if response.hovered() {
            ui.painter().rect_filled(button, 0.0, to_color32(if index == 2 { theme.palette[1] } else { theme.selection_bg }));
        }
        if controls_visible {
            ui.painter().text(button.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(18.0), to_color32(theme.fg));
        }
        if response.clicked() {
            ctx.send_viewport_cmd(command);
        }
    }
    if !maximized {
        resize_edges(ui);
    }
    drag
}

/// Resolve the background interaction after the tabs have supplied their occupied rectangles.
pub fn drag_action(ui: &egui::Ui, drag: &egui::Response, buttons: &[egui::Rect]) {
    let origin = ui.input(|input| input.pointer.press_origin());
    if origin.is_some_and(|pos| buttons.iter().any(|rect| rect.contains(pos))) { return; }
    if drag.double_clicked() {
        let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        // Starting native drag on press consumes the click/release in Windows' move loop,
        // preventing egui from ever recognizing a double click. Wait for actual movement.
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

fn resize_edges(ui: &mut egui::Ui) {
    use egui::ResizeDirection as D;
    let full = ui.max_rect();
    let Some(pos) = ui.ctx().pointer_hover_pos().filter(|p| full.contains(*p)) else { return };
    let left = pos.x < full.min.x + EDGE;
    let right = pos.x >= full.max.x - EDGE;
    let top = pos.y < full.min.y + EDGE;
    let bottom = pos.y >= full.max.y - EDGE;
    let (direction, cursor) = match (left, right, top, bottom) {
        (true, _, true, _) => (D::NorthWest, egui::CursorIcon::ResizeNwSe),
        (_, true, _, true) => (D::SouthEast, egui::CursorIcon::ResizeNwSe),
        (_, true, true, _) => (D::NorthEast, egui::CursorIcon::ResizeNeSw),
        (true, _, _, true) => (D::SouthWest, egui::CursorIcon::ResizeNeSw),
        (true, _, _, _) => (D::West, egui::CursorIcon::ResizeHorizontal),
        (_, true, _, _) => (D::East, egui::CursorIcon::ResizeHorizontal),
        (_, _, true, _) => (D::North, egui::CursorIcon::ResizeVertical),
        (_, _, _, true) => (D::South, egui::CursorIcon::ResizeVertical),
        _ => return,
    };
    ui.ctx().set_cursor_icon(cursor);
    if ui.input(|i| i.pointer.primary_pressed()) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_reserves_top_tabs_and_footer_without_a_sidebar() {
        let full = egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(960.0, 600.0));
        let r = regions(full);
        assert_eq!(r.title.min.y, r.tabs.min.y);
        assert_eq!(r.tabs.max.y, r.terminal.min.y);
        assert_eq!(r.title.max.y, r.terminal.min.y);
        assert_eq!(r.terminal.max.y, r.footer.min.y);
        assert_eq!(r.footer.max.y, full.max.y - EDGE);
        assert_eq!(r.terminal.width(), full.width() - 2.0 * EDGE);
    }

    #[test]
    fn chrome_does_not_invert_rectangles_at_minimum_window_size() {
        let r = regions(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 160.0)));
        for rect in [r.title, r.terminal, r.tabs, r.footer] {
            assert!(rect.width() > 0.0 && rect.height() > 0.0);
        }
    }

    fn draw_title(ui: &mut egui::Ui) {
        let drag = title_bar(ui, regions(ui.max_rect()).title, &Theme::default());
        drag_action(ui, &drag, &[]);
    }

    fn event_pass(ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<egui::ViewportCommand> {
        if ctx.cumulative_pass_nr() == 0 {
            let mut first = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            }, draw_title);
            first.textures_delta.clear();
        }
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events, ..Default::default()
        }, draw_title);
        let commands = output.viewport_output.get_mut(&egui::ViewportId::ROOT)
            .map(|viewport| std::mem::take(&mut viewport.commands)).unwrap_or_default();
        output.textures_delta.clear();
        commands
    }

    fn pointer_pass(ctx: &egui::Context, pos: egui::Pos2, pressed: bool) -> Vec<egui::ViewportCommand> {
        event_pass(ctx, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        }])
    }
    #[test]
    fn custom_controls_request_native_window_actions_without_dragging() {
        for (index, x) in [686.0, 730.0, 774.0].into_iter().enumerate() {
            let ctx = egui::Context::default();
            let pos = egui::pos2(x, 21.0);
            let press = pointer_pass(&ctx, pos, true);
            assert!(!press.iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag)));
            let release = pointer_pass(&ctx, pos, false);
            assert!(release.iter().any(|c| match index {
                0 => matches!(c, egui::ViewportCommand::Minimized(true)),
                1 => matches!(c, egui::ViewportCommand::Maximized(true)),
                _ => matches!(c, egui::ViewportCommand::Close),
            }), "window control {index} did not issue its command");
        }
    }

    #[test]
    fn borderless_title_and_corner_can_start_move_and_resize() {
        for x in [150.0, 620.0] {
            let ctx = egui::Context::default();
            assert!(!pointer_pass(&ctx, egui::pos2(x, 21.0), true).iter()
                .any(|c| matches!(c, egui::ViewportCommand::StartDrag)));
            assert!(event_pass(&ctx, vec![egui::Event::PointerMoved(egui::pos2(x + 12.0, 21.0))]).iter()
                .any(|c| matches!(c, egui::ViewportCommand::StartDrag)));
        }
        let ctx = egui::Context::default();
        assert!(pointer_pass(&ctx, egui::pos2(1.0, 1.0), true).iter()
            .any(|c| matches!(c, egui::ViewportCommand::BeginResize(egui::ResizeDirection::NorthWest))));
    }

    #[test]
    fn double_clicking_unused_bar_space_maximizes_without_starting_drag() {
        for x in [150.0, 620.0] {
            let ctx = egui::Context::default();
            let pos = egui::pos2(x, 21.0);
            for pressed in [true, false, true] {
                assert!(!pointer_pass(&ctx, pos, pressed).iter()
                    .any(|command| matches!(command, egui::ViewportCommand::StartDrag)));
            }
            assert!(pointer_pass(&ctx, pos, false).iter()
                .any(|command| matches!(command, egui::ViewportCommand::Maximized(true))));
        }
    }

    #[test]
    fn window_control_labels_are_only_painted_while_hovering_the_bar() {
        let ctx = egui::Context::default();
        for (hover, expected) in [(false, 0), (true, 3)] {
            let events = if hover { vec![egui::Event::PointerMoved(egui::pos2(150.0, 21.0))] } else { vec![] };
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events, ..Default::default()
            }, draw_title);
            let count = output.shapes.iter().filter(|shape| matches!(shape.shape, egui::Shape::Text(_))).count();
            output.textures_delta.clear();
            assert_eq!(count, expected);
        }
    }

    #[test]
    fn tab_clicks_in_the_window_bar_do_not_start_window_dragging() {
        let ctx = egui::Context::default();
        let pos = egui::pos2(30.0, 21.0);
        let mut clicked = false;
        for pressed in [None, Some(true), Some(false)] {
            let events = pressed.map(|pressed| vec![egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary,
                    pressed, modifiers: egui::Modifiers::NONE }]).unwrap_or_default();
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events, ..Default::default()
            }, |ui| {
                let regions = regions(ui.max_rect());
                let drag = title_bar(ui, regions.title, &Theme::default());
                let tab = egui::Rect::from_min_size(regions.tabs.min, egui::vec2(58.0, 26.0));
                clicked = ui.interact(tab, egui::Id::new("tab.test"), egui::Sense::click()).clicked();
                drag_action(ui, &drag, &[tab]);
            });
            let dragged = output.viewport_output[&egui::ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, egui::ViewportCommand::StartDrag));
            output.textures_delta.clear();
            assert!(!dragged);
        }
        assert!(clicked);
    }

    #[test]
    fn moving_the_pointer_after_pressing_a_tab_does_not_drag_the_window() {
        let ctx = egui::Context::default();
        let press = egui::pos2(30.0, 21.0);
        let moved = egui::pos2(45.0, 21.0);
        for events in [vec![], vec![egui::Event::PointerMoved(press), egui::Event::PointerButton {
            pos: press, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE,
        }], vec![egui::Event::PointerMoved(moved)]] {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events, ..Default::default()
            }, |ui| {
                let regions = regions(ui.max_rect());
                let drag = title_bar(ui, regions.title, &Theme::default());
                let tab = egui::Rect::from_min_size(regions.tabs.min, egui::vec2(58.0, 26.0));
                ui.interact(tab, egui::Id::new("tab.test"), egui::Sense::click());
                drag_action(ui, &drag, &[tab]);
            });
            let dragged = output.viewport_output[&egui::ViewportId::ROOT].commands.iter()
                .any(|command| matches!(command, egui::ViewportCommand::StartDrag));
            output.textures_delta.clear();
            assert!(!dragged);
        }
    }
}
