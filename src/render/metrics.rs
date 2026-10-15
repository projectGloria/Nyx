//! Cell geometry, derived once per font change and then treated as the single source of truth.
//!
//! Everything downstream — how many columns fit, where a glyph's baseline sits, where the PTY
//! thinks the window ends — is computed from this struct. Two independent notions of "how wide
//! is a cell" is how a terminal ends up with a cursor that drifts from its text.

use glyphon::cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
    /// Baseline offset from the top of the cell.
    pub baseline: f32,
    pub font_size: f32,
    pub padding: f32,
    pub underline_y: f32,
    pub underline_h: f32,
}

impl CellMetrics {
    pub fn measure(
        font_system: &mut FontSystem,
        family: &str,
        font_size: f32,
        line_height: f32,
        padding: f32,
    ) -> Self {
        let cell_h = (font_size * line_height).ceil().max(1.0);
        let metrics = Metrics::new(font_size, cell_h);
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(None, None);

        let attrs = Attrs::new().family(family_of(family));
        // A wide-ish ASCII glyph. In a monospace face every advance is identical, so the choice
        // only matters if the configured family turns out not to be monospace.
        buffer.set_text("MMMMMMMMMM", &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);

        let mut cell_w = font_size * 0.6;
        let mut baseline = font_size;
        if let Some(run) = buffer.layout_runs().next() {
            if !run.glyphs.is_empty() {
                // Average over several glyphs so a single hinted advance cannot skew the grid.
                let total: f32 = run.glyphs.iter().map(|g| g.w).sum();
                let measured = total / run.glyphs.len() as f32;
                if measured > 0.0 {
                    cell_w = measured;
                }
            }
            baseline = run.line_y;
        }

        Self {
            cell_w,
            cell_h,
            baseline,
            font_size,
            padding,
            underline_y: (baseline + font_size * 0.12).min(cell_h - 1.0),
            underline_h: (font_size * 0.07).max(1.0),
        }
    }

    /// How many columns and rows fit in a pixel rect.
    ///
    /// Clamped to alacritty's minimums: it panics on a grid narrower than two columns, and a
    /// pane dragged to nothing is otherwise an easy way to reach that.
    pub fn grid_size(&self, width_px: f32, height_px: f32) -> (usize, usize) {
        let usable_w = (width_px - 2.0 * self.padding).max(0.0);
        let usable_h = (height_px - 2.0 * self.padding).max(0.0);
        let cols = (usable_w / self.cell_w).floor() as usize;
        let rows = (usable_h / self.cell_h).floor() as usize;
        (cols.max(2), rows.max(1))
    }
}

pub fn family_of(name: &str) -> Family<'_> {
    if name.is_empty() || name.eq_ignore_ascii_case("monospace") {
        Family::Monospace
    } else {
        Family::Name(name)
    }
}
