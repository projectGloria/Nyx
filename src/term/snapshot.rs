//! The seam between the PTY side and the GPU side.
//!
//! A `GridSnapshot` is plain owned data with no lifetimes and no reference to a `Term`. It is
//! built under the lock in one pass and handed to the renderer inside an `Arc`, so the lock is
//! long gone by the time `prepare`/`paint` run on egui's renderer.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;

use crate::render::color::{self, Resolved};
use crate::term::cursor::CursorSettle;
use crate::term::proxy::EventProxy;
use crate::term::search::{Highlights, Hit};
use crate::theme::{Rgba, Theme};

/// A maximal span of adjacent cells that resolved to identical attributes.
///
/// `cols` is the number of terminal columns covered, which is *not* the number of chars: a
/// double-width CJK glyph is one char over two columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellRun {
    pub col: u16,
    pub cols: u16,
    /// Byte range into the owning `RowSnapshot::text`.
    pub start: u32,
    pub end: u32,
    pub attrs: Resolved,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RowSnapshot {
    /// The row's characters in column order, one per cell, spaces for blanks.
    pub text: String,
    pub runs: Vec<CellRun>,
    pub cells: Vec<CellSpan>,
}

/// The actual grid allocation, including wide characters and their combining marks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellSpan {
    pub col: u16,
    pub cols: u16,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CursorSnapshot {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
    /// True when the block was already painted as this cell's background, so the quad pass must
    /// not draw it a second time.
    pub drawn_inline: bool,
}

#[derive(Clone, Debug)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub rows_data: Vec<RowSnapshot>,
    pub cursor: Option<CursorSnapshot>,
    pub default_bg: Rgba,
    /// Carried here rather than read back under the lock on every keystroke: input encoding
    /// needs `APP_CURSOR`, and taking the `Term` lock to answer that would put a mutex on the
    /// path between a keypress and the PTY.
    pub mode: TermMode,
    /// How far back the view is scrolled, in lines. Zero means the prompt is visible.
    pub display_offset: usize,
    pub selection_active: bool,
}

impl GridSnapshot {
    pub fn empty(theme: &Theme) -> Self {
        Self {
            cols: 0,
            rows: 0,
            rows_data: Vec::new(),
            cursor: None,
            default_bg: theme.bg,
            mode: TermMode::empty(),
            display_offset: 0,
            selection_active: false,
        }
    }
}

/// Copy the visible grid out of a locked `Term`.
///
/// Target hold time is well under a millisecond: this walks `cols * rows` cells and allocates
/// per row, and nothing here touches the GPU, the filesystem or a channel.
/// `cursor_on` is the blink phase, and is deliberately separate from `focused`: a blinked-off
/// cursor must be absent, whereas an unfocused one is drawn as a hollow outline. Collapsing the
/// two makes a blinking cursor flash between solid and hollow instead of on and off.
///
/// `search` is baked in here rather than painted over the grid afterwards for the same reason the
/// block cursor is: a quad on top would hide the character it is highlighting, and going through
/// the resolver keeps run-merging working — a row of hits stays one run instead of becoming one
/// run per cell.
///
/// `settle` decides *where* the cursor is drawn: ConPTY delivers a repaint and the cursor's real
/// position in two pieces, with the cursor parked somewhere else in between — see
/// [`crate::term::cursor`]. It applies to drawing only. A selection keeps the terminal's own
/// cursor point, because there it is what decides which half of a wide glyph a cell is.
#[cfg(test)]
pub fn capture(
    term: &mut Term<EventProxy>,
    theme: &Theme,
    focused: bool,
    cursor_on: bool,
    search: &Highlights,
    settle: &mut CursorSettle,
) -> GridSnapshot {
    capture_with_style(term, theme, focused, cursor_on, search, settle, crate::config::schema::CursorStyle::Block)
}

pub fn capture_with_style(
    term: &mut Term<EventProxy>, theme: &Theme, focused: bool, cursor_on: bool,
    search: &Highlights, settle: &mut CursorSettle, style: crate::config::schema::CursorStyle,
) -> GridSnapshot {
    // Read the dimensions before `renderable_content` borrows the terminal.
    let cols = term.columns();
    let rows = term.screen_lines();
    let selection_active = term.selection.as_ref().is_some_and(|selection| !selection.is_empty());

    let snapshot = {
        let content = term.renderable_content();
        let display_offset = content.display_offset as i32;
        let colors = content.colors;
        let selection = content.selection;
        let cursor_point = content.cursor.point;
        let cursor_shape = if content.cursor.shape == CursorShape::Hidden {
            CursorShape::Hidden
        } else { style.shape() };
        let draw_point = settle.resolve(cursor_point, cols, rows, std::time::Instant::now());

        let mut rows_data = vec![RowSnapshot::default(); rows];

        let cursor_row = draw_point.line.0 + display_offset;
        let cursor_visible = cursor_on
            && cursor_shape != CursorShape::Hidden
            && cursor_row >= 0
            && (cursor_row as usize) < rows;
        // A focused block cursor is painted as the cell's own background rather than as a quad
        // on top, so the glyph underneath stays readable. Doing it here means it goes through
        // the same resolver as everything else instead of being a second colour path.
        let inline_block = focused && cursor_visible && cursor_shape == CursorShape::Block;

        for indexed in content.display_iter {
            // `display_iter` yields lines from -(display_offset) upward, so this maps the
            // visible region onto 0..rows.
            let row = (indexed.point.line.0 + display_offset) as usize;
            if row >= rows {
                continue;
            }
            let cell = indexed.cell;
            let col = indexed.point.column.0;

            // The trailing half of a wide glyph carries no character of its own; the leading
            // spacer is padding inserted when a wide glyph would straddle the right edge.
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }

            let mut attrs = color::resolve(cell.fg, cell.bg, cell.flags, theme, colors);

            // Search first, selection second: a selection is a thing the user is doing right now
            // and has to win, otherwise dragging across a highlighted match shows no selection at
            // all and the copy looks like it did not happen.
            match search.at(row, col) {
                Some(Hit::Other) => {
                    attrs.fg = theme.search_fg;
                    attrs.bg = theme.search_bg;
                },
                Some(Hit::Current) => {
                    attrs.fg = theme.search_current_fg;
                    attrs.bg = theme.search_current_bg;
                },
                None => {},
            }

            if let Some(range) = selection {
                if range.contains_cell(&indexed, cursor_point, cursor_shape) {
                    attrs.bg = theme.selection_bg;
                    if let Some(fg) = theme.selection_fg {
                        attrs.fg = fg;
                    }
                }
            }

            if inline_block && indexed.point == draw_point {
                attrs.fg = theme.cursor_fg;
                attrs.bg = theme.cursor_bg;
            }

            let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2u16 } else { 1u16 };

            let row_data = &mut rows_data[row];
            let start = row_data.text.len() as u32;
            row_data.text.push(cell.c);
            // Zero-width marks (combining accents) belong to the cell they follow.
            for z in cell.zerowidth().into_iter().flatten() {
                row_data.text.push(*z);
            }
            let end = row_data.text.len() as u32;
            row_data.cells.push(CellSpan { col: col as u16, cols: width, start, end });

            match row_data.runs.last_mut() {
                Some(last)
                    if last.attrs == attrs && last.col as usize + last.cols as usize == col =>
                {
                    last.cols += width;
                    last.end = end;
                },
                _ => row_data.runs.push(CellRun {
                    col: col as u16,
                    cols: width,
                    start,
                    end,
                    attrs,
                }),
            }
        }

        let cursor = cursor_visible.then_some(CursorSnapshot {
            col: draw_point.column.0,
            row: cursor_row as usize,
            shape: cursor_shape,
            drawn_inline: inline_block,
        });

        GridSnapshot {
            cols,
            rows,
            rows_data,
            cursor,
            default_bg: theme.bg,
            mode: content.mode,
            display_offset: content.display_offset,
            selection_active,
        }
    };

    // Damage is reset even though Phase 1 reshapes by content comparison: left unreset it
    // accumulates inside `Term` and the first consumer of `damage()` would see the history of
    // every frame since launch rather than this one's.
    term.reset_damage();

    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::ThemeConfig;
    use crate::term::testing::headless_term;
    use crate::term::GridSize;

    #[test]
    fn configured_shapes_keep_blink_and_hidden_cursors_working() {
        use crate::config::schema::CursorStyle;
        use alacritty_terminal::vte::ansi::Processor;
        for style in [CursorStyle::Block, CursorStyle::Beam, CursorStyle::Underline] {
            let mut term = headless_term(GridSize::new(20, 4));
            let theme = theme();
            let hits = Highlights::default();
            let mut settle = CursorSettle::default();
            let on = capture_with_style(&mut term, &theme, true, true, &hits, &mut settle, style);
            let cursor = on.cursor.unwrap();
            assert_eq!(cursor.shape, style.shape());
            assert_eq!(cursor.drawn_inline, style == CursorStyle::Block);
            assert!(capture_with_style(&mut term, &theme, true, false, &hits, &mut settle, style).cursor.is_none());
            let mut parser: Processor = Processor::new();
            parser.advance(&mut term, b"\x1b[?25l");
            assert!(capture_with_style(&mut term, &theme, true, true, &hits, &mut settle, style).cursor.is_none());
        }
    }

    fn theme() -> Theme {
        Theme::from_config(&ThemeConfig::default()).0
    }

    #[test]
    fn a_blinked_off_cursor_is_absent_but_the_terminal_still_wants_one() {
        // These are two different questions and the blink controller has to ask the second one.
        // `snapshot.cursor` folds in the blink phase, so reading it as "does the terminal want a
        // cursor" made the controller see its own blinked-off frame as the program having hidden
        // the cursor — and turn it straight back on, collapsing the off phase to a single frame.
        // `mode` carries `SHOW_CURSOR`, which is the question actually being asked.
        let mut term = headless_term(GridSize::new(20, 4));
        let theme = theme();
        let hits = Highlights::default();

        let on = capture(&mut term, &theme, true, true, &hits, &mut CursorSettle::default());
        assert!(on.cursor.is_some(), "a cursor that is on should be drawn");
        assert!(on.mode.contains(TermMode::SHOW_CURSOR));

        let off = capture(&mut term, &theme, true, false, &hits, &mut CursorSettle::default());
        assert!(off.cursor.is_none(), "a blinked-off cursor must be absent, not hollow");
        assert!(
            off.mode.contains(TermMode::SHOW_CURSOR),
            "the terminal still wants a cursor; only the blink phase says otherwise"
        );
    }

    #[test]
    fn an_unfocused_cursor_is_present_but_not_drawn_inline() {
        // The counterpart: unfocused is a hollow outline, which is a cursor that exists. Folding
        // focus and blink into one flag makes a blinking cursor alternate solid and hollow.
        let mut term = headless_term(GridSize::new(20, 4));
        let theme = theme();
        let hits = Highlights::default();

        let unfocused = capture(&mut term, &theme, false, true, &hits, &mut CursorSettle::default());
        let cursor = unfocused.cursor.expect("an unfocused cursor is still a cursor");
        assert!(!cursor.drawn_inline, "only a focused block cursor is painted as the background");
    }
}
