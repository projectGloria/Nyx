//! One `cosmic_text::Buffer` per visible row, cached per pane and reshaped only when the row's
//! content actually changed.
//!
//! Shaping is the expensive half of drawing a terminal. A screen where only the prompt line
//! changed must reshape one row, not sixty — otherwise a blinking cursor costs a full-screen
//! shaping pass sixty times a second.

use glyphon::cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, Style, Weight};

use crate::render::metrics::{family_of, CellMetrics};
use crate::term::snapshot::{CellSpan, GridSnapshot, RowSnapshot};

struct PositionedBuffer {
    col: u16,
    cols: u16,
    buffer: Buffer,
}

struct CachedRow {
    /// What this buffer was built from. The comparison against the incoming row is what
    /// replaces damage tracking: it can never be wrong, only occasionally redundant.
    source: RowSnapshot,
    buffer: Buffer,
    pieces: Vec<PositionedBuffer>,
    /// False until the row has been shaped at least once.
    valid: bool,
}

pub struct RowCache {
    rows: Vec<CachedRow>,
    metrics: Metrics,
    cell_w: f32,
    /// Bumped by the app whenever the font family or ligature setting changes. Those do not
    /// necessarily change the cell size, so comparing metrics alone would leave every row shaped
    /// in the old font until something else happened to invalidate it.
    generation: u64,
}

impl RowCache {
    pub fn new(metrics: &CellMetrics) -> Self {
        Self {
            rows: Vec::new(),
            metrics: Metrics::new(metrics.font_size, metrics.cell_h),
            cell_w: metrics.cell_w,
            generation: u64::MAX,
        }
    }

    /// Drop every shaped row. Needed on a font change and on atlas recovery, where the cached
    /// buffers refer to glyphs that no longer exist.
    pub fn invalidate(&mut self) {
        self.rows.clear();
    }

    pub fn retune(&mut self, metrics: &CellMetrics, generation: u64) {
        let next = Metrics::new(metrics.font_size, metrics.cell_h);
        if next != self.metrics || metrics.cell_w != self.cell_w || generation != self.generation {
            self.metrics = next;
            self.cell_w = metrics.cell_w;
            self.generation = generation;
            self.invalidate();
        }
    }

    /// Bring the cache in line with a snapshot, reshaping only what changed.
    ///
    /// Returns how many rows actually had to be reshaped, which is the number that decides
    /// whether this terminal is fast.
    pub fn sync(
        &mut self,
        font_system: &mut FontSystem,
        family: &str,
        ligatures: bool,
        snapshot: &GridSnapshot,
    ) -> u32 {
        if self.rows.len() != snapshot.rows_data.len() {
            self.rows.clear();
            self.rows.reserve(snapshot.rows_data.len());
            for _ in 0..snapshot.rows_data.len() {
                let mut buffer = Buffer::new(font_system, self.metrics);
                // No wrapping and no width limit: a terminal row is exactly as long as it is,
                // and letting cosmic-text wrap would silently shift the whole grid down a line.
                buffer.set_size(None, None);
                // Validate native advances against the snapshot before using the row path.
                self.rows.push(CachedRow {
                    source: RowSnapshot::default(),
                    buffer,
                    pieces: Vec::new(),
                    valid: false,
                });
            }
        }

        let mut reshaped = 0;
        for (cached, incoming) in self.rows.iter_mut().zip(snapshot.rows_data.iter()) {
            if cached.valid && &cached.source == incoming {
                continue;
            }
            reshaped += 1;
            cached.source = incoming.clone();
            cached.valid = true;

            // Destructured so the span strings borrow `source` while `buffer` is borrowed
            // mutably. `Attrs` borrows its family name, which is why `family` is a parameter
            // rather than a field: a field would borrow `self` and collide with `rows`.
            let CachedRow { source, buffer, pieces, .. } = cached;
            pieces.clear();
            let default_attrs = Attrs::new().family(family_of(family));
            let spans = source.runs.iter().map(|run| {
                let text = &source.text[run.start as usize..run.end as usize];
                let attrs = Attrs::new()
                    .family(family_of(family))
                    .weight(if run.attrs.bold { Weight::BOLD } else { Weight::NORMAL })
                    .style(if run.attrs.italic { Style::Italic } else { Style::Normal })
                    .color(run.attrs.fg.to_glyphon());
                (text, attrs)
            });

            // Advanced runs the full shaper, which is what turns `->` into one arrow cluster.
            // Basic skips it: faster, and the only way to get literal glyphs from a font that
            // has ligatures you do not want.
            let shaping = if ligatures { Shaping::Advanced } else { Shaping::Basic };
            buffer.set_rich_text(spans, &default_attrs, shaping, None);
            // cosmic-text 0.19 defers shaping until something asks for layout. glyphon takes an
            // immutable `&Buffer` and calls the non-mut `layout_runs`, which cannot resolve a
            // dirty buffer — so the shaping has to be forced here or the row renders empty.
            buffer.shape_until_scroll(font_system, false);
            if !on_grid(buffer, &source.cells, self.cell_w) {
                // Keep ASCII ligatures together where their advances match. Wide/fallback
                // characters and proportional faces get explicit terminal-cell origins.
                for run in &source.runs {
                    let cells: Vec<_> = source.cells.iter().copied()
                        .filter(|cell| cell.start >= run.start && cell.end <= run.end).collect();
                    let attrs = Attrs::new().family(family_of(family))
                        .weight(if run.attrs.bold { Weight::BOLD } else { Weight::NORMAL })
                        .style(if run.attrs.italic { Style::Italic } else { Style::Normal })
                        .color(run.attrs.fg.to_glyphon());
                    let mut first = 0;
                    while first < cells.len() {
                        let mut end = first + 1;
                        if source.text[cells[first].start as usize..cells[first].end as usize].is_ascii() {
                            while end < cells.len()
                                && source.text[cells[end].start as usize..cells[end].end as usize].is_ascii() {
                                end += 1;
                            }
                        }
                        let slice = &cells[first..end];
                        let text = &source.text[slice[0].start as usize..slice.last().unwrap().end as usize];
                        let mut part = Buffer::new(font_system, self.metrics);
                        part.set_size(None, None);
                        part.set_text(text, &attrs, shaping, None);
                        part.shape_until_scroll(font_system, false);
                        let relative: Vec<_> = slice.iter().map(|cell| CellSpan {
                            col: cell.col - slice[0].col, cols: cell.cols,
                            start: cell.start - slice[0].start, end: cell.end - slice[0].start,
                        }).collect();
                        if slice.len() == 1 || on_grid(&part, &relative, self.cell_w) {
                            let last = slice.last().unwrap();
                            pieces.push(PositionedBuffer { col: slice[0].col,
                                cols: last.col + last.cols - slice[0].col, buffer: part });
                        } else {
                            for cell in slice {
                                let mut part = Buffer::new(font_system, self.metrics);
                                part.set_size(None, None);
                                part.set_text(&source.text[cell.start as usize..cell.end as usize], &attrs, shaping, None);
                                part.shape_until_scroll(font_system, false);
                                pieces.push(PositionedBuffer { col: cell.col, cols: cell.cols, buffer: part });
                            }
                        }
                        first = end;
                    }
                }
            }
        }
        reshaped
    }

    #[cfg(test)]
    pub fn buffers(&self) -> impl Iterator<Item = &Buffer> {
        self.rows.iter().map(|r| &r.buffer)
    }

    pub fn positioned_buffers(&self) -> impl Iterator<Item = (usize, u16, Option<u16>, &Buffer)> {
        self.rows.iter().enumerate().flat_map(|(row, cached)| {
            let regular = cached.pieces.is_empty().then_some((row, 0, None, &cached.buffer));
            regular.into_iter().chain(cached.pieces.iter()
                .map(move |piece| (row, piece.col, Some(piece.cols), &piece.buffer)))
        })
    }
}

fn on_grid(buffer: &Buffer, cells: &[CellSpan], cell_w: f32) -> bool {
    buffer.layout_runs().flat_map(|run| run.glyphs).all(|glyph| {
        if glyph.w.abs() < 0.001 { return true; }
        let first = cells.partition_point(|cell| cell.end as usize <= glyph.start);
        let end = cells.partition_point(|cell| (cell.start as usize) < glyph.end);
        let Some(start) = cells.get(first) else { return false };
        let Some(last) = end.checked_sub(1).and_then(|index| cells.get(index)) else { return false };
        (glyph.x - start.col as f32 * cell_w).abs() < 0.05
            && (glyph.x + glyph.w - (last.col + last.cols) as f32 * cell_w).abs() < 0.05
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::ThemeConfig;
    use crate::render::color::Resolved;
    use crate::term::snapshot::CellRun;
    use crate::theme::Theme;

    #[test]
    fn wide_combining_and_proportional_glyphs_do_not_move_the_following_cell() {
        use crate::term::{testing::headless_term, GridSize};
        use alacritty_terminal::vte::ansi::Processor;
        let theme = Theme::from_config(&ThemeConfig::default()).0;
        let mut fonts = FontSystem::new();
        let mut term = headless_term(GridSize::new(12, 1));
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, "Ae\u{301}界\x1b[1;3mB".as_bytes());
        let snapshot = crate::term::snapshot::capture(&mut term, &theme, false, false,
            &crate::term::search::Highlights::default(), &mut crate::term::cursor::CursorSettle::default());
        for family in ["Cascadia Mono", "Arial"] {
            let metrics = CellMetrics::measure(&mut fonts, family, 16.0, 1.2, 4.0);
            for ligatures in [false, true] {
                let mut cache = RowCache::new(&metrics);
                assert_eq!(cache.sync(&mut fonts, family, ligatures, &snapshot), 1);
                let mut found = false;
                for (_, col, _, buffer) in cache.positioned_buffers() {
                    for run in buffer.layout_runs() {
                        for glyph in run.glyphs {
                            if &run.text[glyph.start..glyph.end] == "B" {
                                let x = col as f32 * metrics.cell_w + glyph.x;
                                assert!((x - 4.0 * metrics.cell_w).abs() < 0.05, "{family}: {x}");
                                found = true;
                            }
                        }
                    }
                }
                assert!(found);
                assert_eq!(cache.sync(&mut fonts, family, ligatures, &snapshot), 0,
                    "an unchanged fallback row must remain cached");
            }
        }
    }

    #[test]
    fn prompt_advances_match_the_cursor_grid() {
        let mut fonts = FontSystem::new();
        let theme = Theme::from_config(&ThemeConfig::default()).0;
        let prompt = "PS C:\\Users\\Nihil> asd";
        for size in [13.0, 16.25, 19.5, 26.0] {
            let metrics = CellMetrics::measure(&mut fonts, "Cascadia Mono", size, 1.2, 4.0);
            for ligatures in [false, true] {
                let mut snapshot = GridSnapshot::empty(&theme);
                snapshot.rows_data.push(RowSnapshot {
                    text: prompt.into(),
                    cells: prompt.char_indices().enumerate().map(|(col, (start, c))| CellSpan {
                        col: col as u16, cols: 1, start: start as u32, end: (start + c.len_utf8()) as u32,
                    }).collect(),
                    runs: vec![CellRun {
                        col: 0, cols: prompt.len() as u16, start: 0, end: prompt.len() as u32,
                        attrs: Resolved {
                            fg: theme.fg, bg: theme.bg, bold: false, italic: false,
                            underline: false, strikeout: false,
                        },
                    }],
                });
                let mut cache = RowCache::new(&metrics);
                cache.sync(&mut fonts, "Cascadia Mono", ligatures, &snapshot);
                let run = cache.buffers().next().unwrap().layout_runs().next().unwrap();
                let end = run.glyphs.last().unwrap();
                let expected = prompt.len() as f32 * metrics.cell_w;
                assert!((end.x + end.w - expected).abs() < 0.05,
                    "size={size}, ligatures={ligatures}: text={}, cursor={expected}", end.x + end.w);
            }
        }
    }
}
