//! Connect retained terminal blocks to hover controls and the system clipboard.

use super::*;
use crate::term::blocks::{Block, Blocks, Kind};
use crate::ui::copyblock;

pub(super) struct Hover {
    pane: PaneId,
    block: Block,
    pub rect: egui::Rect,
    text: Option<String>,
}

impl NyxApp {
    pub(super) fn copy_block_hover(&mut self, ui: &egui::Ui) -> Option<Hover> {
        if self.settings.open || self.palette.open || egui::Popup::is_any_open(ui.ctx()) || self.mouse.selecting.is_some() {
            return None;
        }
        let pos = ui.ctx().pointer_hover_pos()?;
        let pane = self.solved.pane_at(pos)?;
        let pane_rect = self.solved.rect_of(pane)?;
        if self.search.open && findbar::rect(window::regions(ui.max_rect()).terminal).contains(pos) { return None; }
        let state = self.panes.get_mut(&pane)?;
        let term = state.pane.term.try_lock_unfair()?;
        if state.pane.is_dirty() { state.copy_blocks.invalidate(); }
        if !state.integration.child_running {
            state.copy_blocks.observe_prompt(&term, state.integration.integration == handshake::Integration::Active);
        }
        let offset = term.grid().display_offset() as i32;
        let cell_height = self.metrics.cell_h / self.scale;
        let padding = self.metrics.padding / self.scale;
        let line = ((pos.y - pane_rect.min.y - padding) / cell_height).floor() as i32 - offset;
        // The highlight covers exactly the block's rows, so the row under the pointer
        // is the block -- no strip of one block reaches into the row of the next.
        let block = state.copy_blocks.at(&term, line.max(-offset))?;
        let rect = copyblock::bounds(pane_rect, block.start.line.0 + offset,
            block.end.line.0 + offset + 1, cell_height, padding);
        if rect.height() <= 0.0 { return None; }
        let releasing = ui.input(|i| i.pointer.primary_released()) && copyblock::buttons(rect).iter().any(|button| button.contains(pos));
        let text = if block.kind == Kind::Input || releasing { Some(Blocks::text(&term, block)) } else { None };
        if text.as_ref().is_some_and(|text| text.trim().is_empty()) { return None; }
        Some(Hover { pane, block, rect, text })
    }

    pub(super) fn draw_copy_block(&mut self, ui: &mut egui::Ui, hover: Option<Hover>) {
        let fade = ui.ctx().animate_bool_with_time(egui::Id::new("nyx.copy.highlight"), hover.is_some(), 0.14);
        let Some(hover) = hover else { return };
        let id = egui::Id::new(("nyx.copy.block", hover.pane, hover.block.start.line.0,
            hover.block.kind == Kind::Input));
        if copyblock::show(ui, hover.rect, id, &self.theme, fade) {
            if let Some(text) = hover.text {
                self.copy_text(ui.ctx(), text);
            }
        }
    }

    pub(super) fn copy_text(&mut self, ctx: &egui::Context, text: String) {
        if let Some(clipboard) = self.clipboard.as_mut() {
            if let Err(err) = clipboard.set_text(text) {
                self.toast(format!("Could not copy: {err}"));
                return;
            }
        } else { ctx.copy_text(text); }
        self.copied_at = Some(Instant::now());
        ctx.request_repaint();
    }

    pub(super) fn draw_copy_feedback(&mut self, ui: &egui::Ui) {
        let Some(started) = self.copied_at else { return };
        let elapsed = started.elapsed().as_secs_f32();
        if elapsed >= 0.95 {
            self.copied_at = None;
            return;
        }
        let enter = (elapsed / 0.12).clamp(0.0, 1.0);
        let enter = 1.0 - (1.0 - enter).powi(3);
        let opacity = enter * ((0.95 - elapsed) / 0.25).clamp(0.0, 1.0);
        let color = |color: crate::theme::Rgba| egui::Color32::from_rgba_unmultiplied(
            color.r, color.g, color.b, (color.a as f32 * opacity) as u8);
        let rect = egui::Rect::from_center_size(ui.max_rect().center() + egui::vec2(0.0, 8.0 * (1.0 - enter)), egui::vec2(126.0, 42.0));
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("nyx.copied")));
        painter.rect_filled(rect, 10.0, color(self.theme.surface));
        painter.rect_stroke(rect, 10.0, egui::Stroke::new(1.0, color(self.theme.accent)), egui::StrokeKind::Inside);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Copied!", egui::FontId::proportional(16.0), color(self.theme.fg));
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}
