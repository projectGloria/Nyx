//! The bridge into egui's render pass.
//!
//! `prepare` runs on the UI thread after `update()` has returned; `paint` runs inside egui's own
//! render pass, sharing its encoder and surface texture. Neither may touch a `Term` — everything
//! either needs is already in the `Arc<GridSnapshot>` this struct carries.

use std::sync::Arc;

use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use glyphon::{Resolution, TextArea, TextBounds, TextRenderer};

use crate::render::metrics::CellMetrics;
use crate::render::shape::RowCache;
use crate::render::{quads, GridResources, PaneResources, Timings};
use crate::term::snapshot::GridSnapshot;
use crate::term::PaneId;
use crate::theme::Theme;

pub struct GridCallback {
    pub repaint: egui::Context,
    pub pane: PaneId,
    pub snapshot: Arc<GridSnapshot>,
    pub metrics: CellMetrics,
    pub theme: Arc<Theme>,
    /// Pane rect in physical pixels, relative to the framebuffer origin.
    pub rect_px: [f32; 4],
    pub focused: bool,
    pub frame: u64,
}

impl CallbackTrait for GridCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<GridResources>() else {
            return Vec::new();
        };

        // Trim once per frame rather than once per pane. Doing it at the top of the first
        // prepare of frame N reclaims glyphs that went unused in frame N-1, which is the same
        // result one frame later — and `paint` only gets `&CallbackResources`, so it cannot be
        // done after rendering.
        if res.trimmed_frame != self.frame {
            res.atlas.trim();
            res.trimmed_frame = self.frame;
        }

        let [screen_w, screen_h] = screen.size_in_pixels;
        res.viewport.update(queue, Resolution { width: screen_w, height: screen_h });
        res.quad_pipeline.set_screen(queue, screen_w as f32, screen_h as f32);

        // Destructured so the borrows of the shared caches and the per-pane state are disjoint.
        let GridResources {
            font_system,
            swash,
            atlas,
            viewport,
            panes,
            family,
            ligatures,
            font_generation,
            srgb,
            timings,
            ..
        } = res;
        let timings = timings.clone();
        let ligatures = *ligatures;
        let font_generation = *font_generation;

        let pane_res = panes.entry(self.pane).or_insert_with(|| PaneResources {
            text: TextRenderer::new(atlas, device, wgpu::MultisampleState::default(), None),
            quads: Default::default(),
            rows: RowCache::new(&self.metrics),
        });

        let PaneResources { text, quads: batch, rows } = pane_res;
        rows.retune(&self.metrics, font_generation);
        let t_shape = std::time::Instant::now();
        let reshaped = rows.sync(font_system, family, ligatures, &self.snapshot);
        Timings::add(&timings.shape_us, t_shape.elapsed());
        Timings::count(&timings.rows_reshaped, reshaped);

        let [x, y, w, h] = self.rect_px;
        let left = x + self.metrics.padding;
        let top = y + self.metrics.padding;
        let bounds = TextBounds {
            left: x as i32,
            top: y as i32,
            right: (x + w) as i32,
            bottom: (y + h) as i32,
        };
        let default_color = self.theme.fg.to_glyphon();

        let areas = rows.positioned_buffers().map(|(row, col, cols, buffer)| TextArea {
            buffer,
            left: left + col as f32 * self.metrics.cell_w,
            top: top + row as f32 * self.metrics.cell_h,
            scale: 1.0,
            bounds: TextBounds {
                left: bounds.left.max((left + col as f32 * self.metrics.cell_w).floor() as i32),
                right: cols.map_or(bounds.right, |cols|
                    bounds.right.min((left + (col + cols) as f32 * self.metrics.cell_w).ceil() as i32)),
                ..bounds
            },
            default_color,
            custom_glyphs: &[],
        });

        let t_prepare = std::time::Instant::now();
        let prepared =
            text.prepare(device, queue, font_system, atlas, viewport, areas, swash);
        Timings::add(&timings.text_prepare_us, t_prepare.elapsed());

        let t_quads = std::time::Instant::now();
        let (instances, under_count) = quads::build(
            &self.snapshot,
            &self.metrics,
            &self.theme,
            (x, y),
            *srgb,
            self.focused,
        );
        batch.upload(device, queue, &instances, under_count);
        Timings::add(&timings.quads_us, t_quads.elapsed());

        if let Err(glyphon::PrepareError::AtlasFull) = prepared {
            res.rebuild_atlas(device, queue);
            // Recovery invalidates every pane, including ones prepared earlier this frame.
            self.repaint.request_repaint();
        }

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(res) = resources.get::<GridResources>() else { return };
        let Some(pane) = res.panes.get(&self.pane) else { return };

        // egui sets the viewport to this callback's rect as a courtesy and documents that a
        // callback may override it. glyphon and our quad shader are both fed screen-absolute
        // pixel coordinates, so leaving egui's viewport in place would compose two coordinate
        // spaces and offset every pane but the top-left one by its own origin — which reads as
        // "splits are broken" rather than as a coordinate bug.
        let [screen_w, screen_h] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, screen_w as f32, screen_h as f32, 0.0, 1.0);

        // Clamped into the framebuffer: wgpu validates that a scissor lies inside the render
        // target and kills the whole submission if it does not, and egui's clip rect can be
        // fractionally outside it after rounding at a non-integer scale factor.
        let clip = info.clip_rect_in_pixels();
        let x = clip.left_px.max(0) as u32;
        let y = clip.top_px.max(0) as u32;
        let w = (clip.width_px.max(0) as u32).min(screen_w.saturating_sub(x));
        let h = (clip.height_px.max(0) as u32).min(screen_h.saturating_sub(y));
        if w == 0 || h == 0 {
            return;
        }
        pass.set_scissor_rect(x, y, w, h);

        if let Some(buffer) = pane.quads.buffer() {
            res.quad_pipeline.bind(pass);
            pass.set_vertex_buffer(0, buffer.slice(..));
            if !pane.quads.under.is_empty() {
                pass.draw(0..4, pane.quads.under.clone());
            }
        }

        let _ = pane.text.render(&res.atlas, &res.viewport, pass);

        if let Some(buffer) = pane.quads.buffer() {
            if !pane.quads.over.is_empty() {
                res.quad_pipeline.bind(pass);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..4, pane.quads.over.clone());
            }
        }
    }
}
