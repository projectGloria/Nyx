//! The GPU side of Nyx. Nothing in here touches a `Term`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use glyphon::{Cache, ColorMode, FontSystem, SwashCache, TextAtlas, TextRenderer, Viewport};
use rustc_hash::FxHashMap;

use crate::render::metrics::CellMetrics;
use crate::render::quads::{QuadBatch, QuadPipeline};
use crate::render::shape::RowCache;
use crate::term::PaneId;

pub mod callback;
pub mod color;
pub mod metrics;
pub mod quads;
pub mod shape;

pub use callback::GridCallback;

/// Where a frame went, in microseconds.
///
/// Shared between the app and the paint callback because `prepare` runs inside egui's renderer
/// and has no other way back. Four `Instant::now` calls a frame is not worth gating behind a
/// flag, and a performance claim that cannot be checked at any moment is a performance claim
/// nobody should believe.
#[derive(Default, Debug)]
pub struct Timings {
    pub snapshot_us: AtomicU32,
    pub shape_us: AtomicU32,
    pub text_prepare_us: AtomicU32,
    pub quads_us: AtomicU32,
    pub rows_reshaped: AtomicU32,
}

/// One complete frame's totals, across every pane in it.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTimings {
    pub snapshot_us: u32,
    pub shape_us: u32,
    pub text_prepare_us: u32,
    pub quads_us: u32,
    pub rows_reshaped: u32,
}

impl Timings {
    /// Add one pane's contribution to the frame in flight.
    ///
    /// **`fetch_add`, not `store`.** Each of these is written once per pane per frame, so storing
    /// left the overlay showing whichever pane happened to be painted last — in a four-way split
    /// it reported a quarter of the frame and called it the frame. The number people read it for
    /// is "where did this frame go", and that is a sum.
    pub fn add(field: &AtomicU32, d: std::time::Duration) {
        let us = d.as_micros().min(u32::MAX as u128) as u32;
        field.fetch_add(us, Ordering::Relaxed);
    }

    pub fn count(field: &AtomicU32, n: u32) {
        field.fetch_add(n, Ordering::Relaxed);
    }

    /// Close the frame: hand back its totals and zero the counters for the next one.
    ///
    /// Called once per frame, from the top of `logic`, which is the only point at which no
    /// pane is contributing. What comes back is therefore the *previous* complete frame — egui
    /// runs `prepare` and `paint` after `update` returns, so a frame's GPU numbers do not exist
    /// yet at the moment its overlay is drawn. That was already true when this was a `store`;
    /// it is only now true of all five fields at once instead of a mixture.
    pub fn take_frame(&self) -> FrameTimings {
        FrameTimings {
            snapshot_us: self.snapshot_us.swap(0, Ordering::Relaxed),
            shape_us: self.shape_us.swap(0, Ordering::Relaxed),
            text_prepare_us: self.text_prepare_us.swap(0, Ordering::Relaxed),
            quads_us: self.quads_us.swap(0, Ordering::Relaxed),
            rows_reshaped: self.rows_reshaped.swap(0, Ordering::Relaxed),
        }
    }
}

/// Per-pane GPU state.
///
/// The `TextRenderer` is per pane and not shared, and that is forced rather than chosen: egui
/// runs every callback's `prepare` before any callback's `paint`, and `TextRenderer::prepare`
/// replaces the renderer's entire vertex set. A shared renderer would leave the last pane
/// prepared as the text every pane draws, each clipped to someone else's rect.
pub struct PaneResources {
    pub text: TextRenderer,
    pub quads: QuadBatch,
    pub rows: RowCache,
}

/// Everything the grid needs on the GPU, parked in egui's `CallbackResources`.
///
/// The atlas, font system and swash cache *are* shared across panes — glyph rasterisations are
/// pane-independent, and sharing them is the whole reason a four-way split costs almost nothing
/// more than one pane.
pub struct GridResources {
    pub font_system: FontSystem,
    pub swash: SwashCache,
    pub cache: Cache,
    pub atlas: TextAtlas,
    pub viewport: Viewport,
    pub panes: FxHashMap<PaneId, PaneResources>,
    pub quad_pipeline: QuadPipeline,

    /// True when the render target applies linear->sRGB on write, so colours must be linear.
    pub srgb: bool,
    pub family: String,
    pub ligatures: bool,
    /// Bumped whenever `family` or `ligatures` changes, so every pane's row cache reshapes.
    pub font_generation: u64,
    pub format: wgpu::TextureFormat,

    /// Last frame on which the atlas was trimmed, so it happens once per frame rather than once
    /// per pane.
    pub trimmed_frame: u64,

    pub timings: Arc<Timings>,
}

impl GridResources {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        family: String,
        timings: Arc<Timings>,
    ) -> Self {
        let srgb = format.is_srgb();

        // Accurate mode makes glyphon convert our sRGB colours to linear in its shader, which is
        // what an sRGB target needs since the GPU converts back on write. Web mode passes the
        // bytes through untouched, which is what a plain UNORM target needs. Choosing the wrong
        // one produces no error at all — just text that looks washed out or muddy and no way to
        // tell why, so it is decided here and nowhere else.
        let color_mode = if srgb { ColorMode::Accurate } else { ColorMode::Web };

        let cache = Cache::new(device);
        let atlas = TextAtlas::with_color_mode(device, queue, &cache, format, color_mode);
        let viewport = Viewport::new(device, &cache);

        let t0 = std::time::Instant::now();
        let font_system = FontSystem::new();
        log::info!(
            "FontSystem::new took {:?}, {} faces",
            t0.elapsed(),
            font_system.db().len()
        );

        Self {
            font_system,
            swash: SwashCache::new(),
            cache,
            atlas,
            viewport,
            panes: FxHashMap::default(),
            quad_pipeline: QuadPipeline::new(device, format),
            srgb,
            family,
            ligatures: true,
            font_generation: 0,
            format,
            trimmed_frame: u64::MAX,
            timings,
        }
    }

    pub fn measure(&mut self, font_size: f32, line_height: f32, padding: f32) -> CellMetrics {
        let family = self.family.clone();
        CellMetrics::measure(&mut self.font_system, &family, font_size, line_height, padding)
    }

    pub fn forget_pane(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// Point the renderer at a different font. Returns whether the requested family exists.
    ///
    /// A family that is not installed falls back to the generic monospace face rather than to
    /// cosmic-text's own choice — cosmic-text picks a *proportional* face, and a terminal whose
    /// glyphs have varying widths is not a grid any more: the cursor drifts from its text and
    /// every underline lands in the wrong place. Falling back to monospace keeps the terminal
    /// usable while the toast explains what happened.
    ///
    /// The atlas is left alone: glyph keys include the font, so stale entries simply go unused
    /// and are reclaimed by the next `trim`. Only the shaped rows have to go.
    pub fn set_font(&mut self, family: &str, ligatures: bool) -> bool {
        let found = self.has_family(family);
        let effective = if found { family.to_string() } else { String::new() };

        if self.family != effective || self.ligatures != ligatures {
            self.family = effective;
            self.ligatures = ligatures;
            self.font_generation = self.font_generation.wrapping_add(1);
        }
        found
    }

    /// Whether a family name resolves to a real installed font.
    ///
    /// cosmic-text falls back silently, so a typo'd family looks like "the font setting does
    /// nothing" rather than like an error. Checking lets us say so.
    pub fn has_family(&self, family: &str) -> bool {
        self.font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(family)))
    }

    /// Every monospace family installed, sorted and de-duplicated. Phase 7's font picker.
    pub fn monospace_families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .font_system
            .db()
            .faces()
            .filter(|face| face.monospaced)
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Recover from `PrepareError::AtlasFull`.
    ///
    /// Recreating the atlas invalidates every rasterised glyph, so every `TextRenderer` and every
    /// shaped row has to go with it. This costs a few milliseconds and happens approximately
    /// never — but the alternative is a rendering stall with no way out of it.
    pub fn rebuild_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let color_mode = if self.srgb { ColorMode::Accurate } else { ColorMode::Web };
        self.cache = Cache::new(device);
        self.atlas =
            TextAtlas::with_color_mode(device, queue, &self.cache, self.format, color_mode);
        self.viewport = Viewport::new(device, &self.cache);
        self.swash = SwashCache::new();
        for pane in self.panes.values_mut() {
            pane.text =
                TextRenderer::new(&mut self.atlas, device, wgpu::MultisampleState::default(), None);
            pane.rows.invalidate();
        }
        log::warn!("glyph atlas was full; rebuilt it");
    }
}

#[cfg(test)]
mod timings_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_frame_reports_every_pane_rather_than_the_last_one() {
        // Four panes, each taking 2 ms to shape. The frame took 8 ms, and the overlay used to
        // say 2 — `store` meant every pane overwrote the one before it, so the busiest possible
        // case was also the one the number understated most.
        let t = Timings::default();
        for _ in 0..4 {
            Timings::add(&t.shape_us, Duration::from_millis(2));
            Timings::count(&t.rows_reshaped, 34);
        }

        let frame = t.take_frame();
        assert_eq!(frame.shape_us, 8_000);
        assert_eq!(frame.rows_reshaped, 136);

        // ...and the next frame starts from nothing. Without the reset the overlay would climb
        // forever, which is a more obvious bug but the same one.
        assert_eq!(t.take_frame(), FrameTimings::default());
    }
}
