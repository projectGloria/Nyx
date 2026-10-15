//! Copyable command/input and output ranges in the retained terminal grid.
//!
//! Prompt signatures come from shell integration (with conventional prompt detection for
//! profiles without integration). Ranges are resolved against the current grid, so scrolling,
//! reflow and history eviction cannot leave stale line numbers pointing at another command.

use alacritty_terminal::{grid::Dimensions, index::{Column, Line, Point},
    term::{cell::Flags, Term, TermMode}};
use super::proxy::EventProxy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Input, Output }

/// How many distinct prompt signatures to remember.
///
/// Every cmd.exe prompt carries the working directory and most PowerShell ones do too, so the
/// table gains an entry per directory visited; a prompt carrying a clock or a git branch gains one
/// per command. Unbounded that is a table which grows for the life of the process — and it is not
/// only memory, because `prompt_columns` scans all of it once per history line. The oldest go
/// first: an evicted signature costs a block in old scrollback its exact bounds, which is the
/// paragraph fallback, not its copy button.
const MAX_PROMPTS: usize = 64;
// Hover work runs under the terminal lock. Hide controls when exact bounds cost an
// unbounded history walk; copying a silently truncated command would be misleading.
const MAX_SCAN_DISTANCE: i32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub start: Point,
    pub end: Point,
}

#[derive(Default)]
pub struct Blocks {
    prompts: Vec<Vec<char>>,
    pub pending_prompt: bool,
    cached: std::cell::RefCell<Option<(Point, usize, usize, Block)>>,
}

impl Blocks {
    pub fn new() -> Self { Self { pending_prompt: true, ..Self::default() } }

    pub fn invalidate(&self) { *self.cached.borrow_mut() = None; }

    pub fn observe_prompt(&mut self, term: &Term<EventProxy>, integrated: bool) {
        if term.mode().contains(TermMode::ALT_SCREEN) || (integrated && !self.pending_prompt && !self.prompts.is_empty()) {
            return;
        }
        let cursor = term.grid().cursor.point;
        let cols = cursor.column.0.min(term.columns());
        if cols == 0 { return; }
        let row = &term.grid()[cursor.line];
        // A conventional prompt terminator excludes input already echoed during a
        // delayed repaint. Custom integrated prompts use the shell's input-start cursor.
        let marker = (0..cols).find(|&col| row[Column(col)].c == '>')
            .or_else(|| (0..cols).find(|&col| matches!(row[Column(col)].c, '$' | '#')));
        let end = if let Some(col) = marker {
            let mut end = col + 1;
            while end < cols && row[Column(end)].c == ' ' { end += 1; }
            end
        } else if integrated && self.pending_prompt { cols } else { return };
        let signature: Vec<char> = (0..end).map(|col| row[Column(col)].c).collect();
        if !integrated && !self.pending_prompt && end < cols && !self.prompts.contains(&signature) { return; }
        if !self.prompts.contains(&signature) {
            self.prompts.push(signature);
            if self.prompts.len() > MAX_PROMPTS { self.prompts.remove(0); }
            self.invalidate();
        }
        self.pending_prompt = false;
    }

    fn prompt_columns(&self, term: &Term<EventProxy>, line: i32) -> Option<usize> {
        let row = &term.grid()[Line(line)];
        self.prompts.iter().filter(|signature| signature.len() <= term.columns())
            .filter(|signature| signature.iter().enumerate().all(|(col, c)| row[Column(col)].c == *c))
            .map(Vec::len).max()
    }

    fn wrapped(term: &Term<EventProxy>, line: i32) -> bool {
        term.grid()[Line(line)][Column(term.columns() - 1)].flags.contains(Flags::WRAPLINE)
    }

    fn blank(term: &Term<EventProxy>, line: i32) -> bool {
        (0..term.columns()).all(|col| term.grid()[Line(line)][Column(col)].c == ' ')
    }

    pub fn at(&self, term: &Term<EventProxy>, line: i32) -> Option<Block> {
        if term.mode().intersects(TermMode::ALT_SCREEN | TermMode::MOUSE_MODE) || term.columns() == 0 { return None; }
        let first = -(term.history_size() as i32);
        let cursor = term.grid().cursor.point;
        let last = if cursor.column.0 == 0 { cursor.line.0 - 1 } else { cursor.line.0 };
        if line < first || line > last { return None; }
        if let Some((point, history, columns, block)) = *self.cached.borrow() {
            if point == cursor && history == term.history_size() && columns == term.columns()
                && line >= block.start.line.0 && line <= block.end.line.0 { return Some(block); }
        }
        let cache = |block: Block| {
            *self.cached.borrow_mut() = Some((cursor, term.history_size(), term.columns(), block));
            Some(block)
        };
        let lower = first.max(line.saturating_sub(MAX_SCAN_DISTANCE));
        let upper = last.min(line.saturating_add(MAX_SCAN_DISTANCE));
        let prompt = (lower..=line).rev().find_map(|row| self.prompt_columns(term, row).map(|cols| (row, cols)));
        if let Some((start, prompt_cols)) = prompt {
            let mut input_end = start;
            while input_end < upper && Self::wrapped(term, input_end) { input_end += 1; }
            if input_end < last && Self::wrapped(term, input_end) { return None; }
            if line <= input_end {
                let input_start = if prompt_cols == term.columns() {
                    Point::new(Line(start + 1), Column(0))
                } else { Point::new(Line(start), Column(prompt_cols)) };
                if input_start.line.0 > input_end { return None; }
                return cache(Block { kind: Kind::Input, start: input_start,
                    end: Point::new(Line(input_end), Column(term.columns() - 1)) });
            }
            let next_prompt = ((input_end + 1)..=upper).find(|&row| self.prompt_columns(term, row).is_some());
            if next_prompt.is_none() && upper < last { return None; }
            let end = next_prompt.map(|row| row - 1).unwrap_or(last);
            return if line <= end { cache(Block { kind: Kind::Output,
                start: Point::new(Line(input_end + 1), Column(0)),
                end: Point::new(Line(end), Column(term.columns() - 1)) }) } else { None };
        }
        // Startup messages and unintegrated output are still copyable as paragraphs.
        if Self::blank(term, line) { return None; }
        let mut start = line;
        let mut end = line;
        while start > lower && !Self::blank(term, start - 1) { start -= 1; }
        while end < upper && !Self::blank(term, end + 1) && self.prompt_columns(term, end + 1).is_none() { end += 1; }
        if (start > first && !Self::blank(term, start - 1))
            || (end < last && !Self::blank(term, end + 1) && self.prompt_columns(term, end + 1).is_none()) { return None; }
        cache(Block { kind: Kind::Output, start: Point::new(Line(start), Column(0)),
            end: Point::new(Line(end), Column(term.columns() - 1)) })
    }

    pub fn text(term: &Term<EventProxy>, block: Block) -> String {
        term.bounds_to_string(block.start, block.end).trim_end_matches(['\r', '\n']).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::{testing::headless_term, GridSize};
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn oversized_hover_ranges_are_hidden_instead_of_scanning_or_copying_partial_output() {
        let mut term = headless_term(GridSize::new(20, 3));
        let blocks = Blocks::new();
        feed(&mut term, &"data\r\n".repeat(1200));
        assert!(blocks.at(&term, -600).is_none());
        feed(&mut term, "\r\nshort\r\n\r\n");
        let block = blocks.at(&term, 0).unwrap();
        assert_eq!(Blocks::text(&term, block), "short");
    }

    fn feed(term: &mut Term<EventProxy>, bytes: &str) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, bytes.as_bytes());
    }

    #[test]
    fn commands_and_outputs_copy_separately_without_the_next_prompt() {
        let mut term = headless_term(GridSize::new(40, 12));
        let mut blocks = Blocks::new();
        feed(&mut term, "C:\\work>");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "dir\r\nalpha\r\n\r\nbeta\r\nC:\\work>echo ok\r\nok\r\nC:\\work>");
        let input = blocks.at(&term, 0).unwrap();
        assert_eq!(input.kind, Kind::Input);
        assert_eq!(Blocks::text(&term, input), "dir");
        let output = blocks.at(&term, 2).unwrap();
        assert_eq!(output.kind, Kind::Output);
        assert_eq!(Blocks::text(&term, output), "alpha\n\nbeta");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 4).unwrap()), "echo ok");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 5).unwrap()), "ok");
        assert!(blocks.at(&term, 8).is_none());
    }

    #[test]
    fn wrapped_commands_and_output_keep_logical_lines() {
        let mut term = headless_term(GridSize::new(12, 12));
        let mut blocks = Blocks::new();
        feed(&mut term, "$ ");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "echo abcdefghijklmnop\r\nabcdefghijklmnopqrst\r\n$ ");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 1).unwrap()), "echo abcdefghijklmnop");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 3).unwrap()), "abcdefghijklmnopqrst");
    }

    #[test]
    fn retained_history_is_resolved_after_scrolling() {
        let mut term = headless_term(GridSize::new(20, 3));
        let mut blocks = Blocks::new();
        feed(&mut term, "$ ");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "dir\r\na\r\nb\r\nc\r\n$ ");
        let block = blocks.at(&term, -1).unwrap();
        assert_eq!(Blocks::text(&term, block), "a\nb\nc");
        term.scroll_display(alacritty_terminal::grid::Scroll::Top);
        assert_eq!(Blocks::text(&term, blocks.at(&term, -1).unwrap()), "a\nb\nc");
    }

    #[test]
    fn evicted_commands_cannot_leave_stale_ranges() {
        let mut term = headless_term(GridSize::new(20, 3));
        term.set_options(alacritty_terminal::term::Config { scrolling_history: 2, ..Default::default() });
        let mut blocks = Blocks::new();
        feed(&mut term, "$ ");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "dir\r\na\r\nb\r\nc\r\nd\r\ne\r\nf\r\n$ ");
        assert!(blocks.at(&term, -3).is_none());
        assert_eq!(Blocks::text(&term, blocks.at(&term, -2).unwrap()), "c\nd\ne\nf");
    }

    #[test]
    fn unicode_and_combining_characters_are_copied_from_original_cells() {
        let mut term = headless_term(GridSize::new(40, 8));
        let mut blocks = Blocks::new();
        feed(&mut term, "PS C:\\世界> ");
        blocks.observe_prompt(&term, true);
        feed(&mut term, "dir\r\ncafe\u{301} 世界\r\nPS C:\\世界> ");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 0).unwrap()), "dir");
        assert_eq!(Blocks::text(&term, blocks.at(&term, 1).unwrap()), "cafe\u{301} 世界");
    }

    #[test]
    fn unintegrated_folder_changes_and_input_redirection_keep_the_prompt_separate() {
        let mut term = headless_term(GridSize::new(40, 8));
        let mut blocks = Blocks::new();
        feed(&mut term, "C:\\old>");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "cd ..\r\nC:\\new>");
        blocks.observe_prompt(&term, false);
        feed(&mut term, "dir > listing.txt");
        blocks.observe_prompt(&term, false);
        assert_eq!(Blocks::text(&term, blocks.at(&term, 1).unwrap()), "dir > listing.txt");
    }

    #[test]
    fn the_prompt_table_does_not_grow_without_bound() {
        // A prompt that carries the working directory produces a new signature per directory, and
        // one carrying a clock produces one per command. Left unbounded this is both a leak and a
        // table `prompt_columns` rescans for every line of a million-line scrollback.
        let mut term = headless_term(GridSize::new(40, 8));
        let mut blocks = Blocks::new();
        for i in 0..MAX_PROMPTS * 3 {
            feed(&mut term, &format!(r"C:\dir{i}>"));
            blocks.pending_prompt = true;
            blocks.observe_prompt(&term, false);
            feed(&mut term, "cd ..\r\n");
        }
        assert!(blocks.prompts.len() <= MAX_PROMPTS, "{} signatures kept", blocks.prompts.len());
        // The newest is what a live prompt is matched against, so it must be the one that stayed.
        assert!(blocks.prompts.iter().any(|p| p.iter().collect::<String>()
            .contains(&format!("dir{}>", MAX_PROMPTS * 3 - 1))));
    }

    #[test]
    fn alternate_screen_and_mouse_reporting_do_not_get_copy_controls() {
        let mut term = headless_term(GridSize::new(20, 3));
        feed(&mut term, "output\r\n\x1b[?1049h");
        assert!(Blocks::new().at(&term, 0).is_none());
        feed(&mut term, "\x1b[?1049l\x1b[?1000h");
        assert!(Blocks::new().at(&term, 0).is_none());
    }
}
