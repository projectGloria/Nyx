//! Scrollback search.
//!
//! Two separate jobs live here and they are easy to confuse.
//!
//! *Navigation* (`advance`) walks the **whole buffer** with `Term::search_next` and parks the
//! viewport on one match. *Highlighting* (`highlights`) only ever looks at the rows **currently on
//! screen**, because the highlight is baked into the snapshot and the snapshot is the visible grid.
//! Running the whole-buffer search to colour 34 rows would put a DFA pass over ten thousand lines
//! into every frame.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

use crate::term::proxy::EventProxy;
use crate::term::PaneId;

/// What a cell's highlight means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// One of the matches on screen.
    Other,
    /// The match `Enter` last moved to.
    Current,
}

/// Which visible cells are part of a match.
///
/// A byte per cell rather than a list of spans, because the lookup happens once per cell inside
/// `snapshot::capture`: a query like `.` produces a match in every cell, and scanning a span list
/// per cell would be quadratic in the width of the grid for the one query most likely to be typed
/// by accident.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Highlights {
    cols: usize,
    rows: usize,
    cells: Vec<u8>,
    /// How many matches are on screen. Not the number in the buffer — see the module docs.
    pub visible: usize,
}

const NONE: u8 = 0;
const OTHER: u8 = 1;
const CURRENT: u8 = 2;

impl Highlights {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows, cells: vec![NONE; cols * rows], visible: 0 }
    }

    pub fn at(&self, row: usize, col: usize) -> Option<Hit> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        match self.cells[row * self.cols + col] {
            OTHER => Some(Hit::Other),
            CURRENT => Some(Hit::Current),
            _ => None,
        }
    }

    /// Paint one match onto the visible grid.
    ///
    /// A match can start on one row and end on another — a wrapped line, or a regex crossing a
    /// newline — so this fills the tail of the first row, every row between, and the head of the
    /// last. `display_offset` converts buffer lines to visible rows the same way `capture` does.
    pub fn mark(&mut self, m: &Match, display_offset: usize, current: bool) {
        let start = *m.start();
        let end = *m.end();
        let mut painted = false;

        for line in start.line.0..=end.line.0 {
            let row = line + display_offset as i32;
            if row < 0 || row as usize >= self.rows {
                continue;
            }
            let row = row as usize;

            let first = if line == start.line.0 { start.column.0 } else { 0 };
            let last = if line == end.line.0 { end.column.0 } else { self.cols.saturating_sub(1) };

            for col in first..=last.min(self.cols.saturating_sub(1)) {
                let slot = &mut self.cells[row * self.cols + col];
                // Current wins: an overlapping paint must not demote the match you are on.
                if current || *slot == NONE {
                    *slot = if current { CURRENT } else { OTHER };
                }
                painted = true;
            }
        }

        if painted {
            self.visible += 1;
        }
    }
}

/// The find bar's state: what is being searched for, and where we are in it.
#[derive(Default)]
pub struct Search {
    pub open: bool,
    pub query: String,
    /// True treats the query as a regular expression.
    ///
    /// Off by default, and that is a deliberate reversal of what a "regex search" feature sounds
    /// like it should do. A terminal's scrollback is made of paths, JSON and stack traces — and
    /// `C:\Users`, `(unclosed`, `a{2,1}` are all *invalid patterns*, so a regex default means the
    /// most ordinary search anyone types in a terminal fails with a message about hexadecimal
    /// digits. Regex is a keystroke away, shown in the bar, and unabridged when you want it.
    pub regex_mode: bool,
    /// Why the query did not compile, shown in the bar rather than logged.
    pub error: Option<String>,
    /// The match the viewport is parked on.
    pub current: Option<Match>,
    /// Which pane the current match belongs to. A `Point` means nothing in another pane's grid.
    pub owner: Option<PaneId>,
    compiled: Option<RegexSearch>,
    /// The pattern actually handed to the regex engine: the query, escaped unless in regex mode.
    ///
    /// Kept because a scan compiles its own copy on the thread that will use it. Lending this
    /// one out instead would take the regex the UI needs every frame for `highlights` away from
    /// it for as long as a walk lasts.
    pattern: String,
    /// The scan currently walking the buffer, and at most one request waiting behind it.
    ///
    /// Only the latest waiting request is kept. Typing three more characters while a scan of a
    /// million lines is running should cost one more scan, not three, and holding Enter down —
    /// which repeats — must not be able to start a thread per repeat.
    running: Option<u64>,
    running_pane: Option<PaneId>,
    running_wanted: bool,
    pending_pane: Option<PaneId>,
    /// The pattern the running scan was started for.
    ///
    /// `set_query` cannot stop a walk that is already in flight, and the debounce means the
    /// replacement request waits 100 ms behind it — so the old scan's result can arrive after the
    /// query changed. Without this record, `land` would apply it and park the view on a match of
    /// the query the user has already replaced.
    running_pattern: String,
    pending: Option<(u64, Direction)>,
    /// Monotonic and never reused: this is what tells a result for the query you are still
    /// typing from one you have already moved past.
    seq: u64,
}

/// What the UI should do when a scan's result comes back.
#[derive(Default, PartialEq, Eq, Debug)]
pub struct Landed {
    /// Move the view to it. False when it was superseded while it was being computed.
    pub apply: bool,
    /// Start this scan now.
    pub next: Option<(u64, Direction)>,
}

impl Search {
    pub fn open(&mut self) {
        self.open = true;
        self.error = None;
    }

    /// Close the bar and forget everything. The caller must mark panes dirty so the highlight
    /// actually leaves the screen.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.compiled = None;
        self.pattern.clear();
        self.current = None;
        self.owner = None;
        self.error = None;
        // Closing the bar cannot release the worker's lock. Keep its identity until it lands.
        self.running_wanted = false;
        self.pending = None;
        self.pending_pane = None;
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.recompile();
    }

    pub fn toggle_regex(&mut self) {
        self.regex_mode = !self.regex_mode;
        self.recompile();
    }

    fn recompile(&mut self) {
        // A new query invalidates where we were; otherwise `next` would step from a point that
        // belongs to a match that no longer exists.
        self.current = None;

        if self.query.is_empty() {
            self.compiled = None;
            self.pattern.clear();
            self.error = None;
            return;
        }

        let pattern =
            if self.regex_mode { self.query.clone() } else { escape_literal(&self.query) };

        match RegexSearch::new(&pattern) {
            Ok(regex) => {
                self.compiled = Some(regex);
                self.pattern = pattern;
                self.error = None;
            },
            Err(err) => {
                self.compiled = None;
                self.pattern.clear();
                self.error = Some(describe(&*err));
            },
        }
    }

    pub fn is_active(&self) -> bool {
        self.open && self.compiled.is_some()
    }

    /// The pattern a scan should compile, or `None` when there is nothing to search for.
    pub fn pattern(&self) -> Option<&str> {
        (!self.pattern.is_empty()).then_some(self.pattern.as_str())
    }

    /// True while a scan is walking a buffer, holding that pane's lock for the whole walk.
    pub fn scanning(&self) -> bool {
        self.running.is_some()
    }

    pub fn scanning_pane(&self) -> Option<PaneId> { self.running_pane }

    /// Ask for a scan, and say whether one should be started now.
    ///
    /// `None` means one is already walking. The request is remembered and started when that one
    /// lands, so there is never more than one thread in the buffer.
    #[cfg(test)]
    pub fn request(&mut self, direction: Direction) -> Option<u64> {
        self.request_in(self.owner.unwrap_or(1), direction)
    }

    pub fn request_in(&mut self, pane: PaneId, direction: Direction) -> Option<u64> {
        self.seq += 1;
        let seq = self.seq;
        if self.running.is_some() {
            self.pending = Some((seq, direction));
            self.pending_pane = Some(pane);
            return None;
        }
        self.running = Some(seq);
        self.running_pane = Some(pane);
        self.running_wanted = true;
        // Recorded when the walk starts, not when the result lands: this is the pattern the
        // answer that comes back will be an answer *to*.
        self.running_pattern.clone_from(&self.pattern);
        Some(seq)
    }

    /// A scan finished. Says whether its answer is still the one being waited for.
    pub fn land(&mut self, seq: u64) -> Landed {
        if self.running != Some(seq) {
            // From a scan nobody is waiting for any more: the bar was closed, or the pane it was
            // searching went away. Applying it would scroll a view for a query that is gone.
            return Landed::default();
        }
        self.running = None;
        self.running_pane = None;
        match self.pending.take() {
            // Superseded while it ran. Its answer is to a question that has since been replaced,
            // and moving the view to it would park on a match of the *previous* query -- which
            // is precisely the jump a debounced find bar exists to avoid.
            Some(next) => {
                self.running = Some(next.0);
                self.running_pane = self.pending_pane.take();
                self.running_wanted = true;
                // The queued request starts with whatever the pattern is when it is handed to
                // `spawn_scan`, so that is what this new walk is now running.
                self.running_pattern.clone_from(&self.pattern);
                Landed { apply: false, next: Some(next) }
            },
            // A query typed while the walk ran has already reset `current` in `recompile`; the
            // only thing left that could apply the stale answer is this. The debounce means the
            // replacement request is still 100 ms away, so without the pattern check the view
            // would jump to a match of the query the user has moved past.
            None if !self.open || !self.running_wanted || self.running_pattern != self.pattern => Landed::default(),
            None => Landed { apply: true, next: None },
        }
    }

    /// Stop waiting, because the scan that was asked for could not be started.
    ///
    /// Without this a request that failed to launch leaves `running` set for ever, and every
    /// later search queues behind a scan that is not running -- a find bar that stops finding.
    pub fn abandon(&mut self) {
        self.running = None;
        self.running_pane = None;
        self.pending_pane = None;
        self.running_pattern.clear();
        self.pending = None;
    }
}

/// One line a person can act on, out of the regex builder's error chain.
///
/// `RegexSearch::new` fails with `error building NFA`, which says nothing at all; the reason is at
/// the bottom of the source chain, and the last line of *that* is the summary — "unclosed group",
/// or "invalid hexadecimal digit", which is what `C:\Users` produces.
fn describe(err: &dyn std::error::Error) -> String {
    let mut deepest = err;
    while let Some(source) = deepest.source() {
        deepest = source;
    }
    let text = deepest.to_string();
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("invalid pattern")
        .trim()
        .trim_start_matches("error: ")
        .to_string()
}

/// Escape every regex metacharacter, so a literal search for `C:\Users (2)` finds itself.
fn escape_literal(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for c in query.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Where the next scan starts, and which side of a match counts as past it.
///
/// Everything `advance` used to do *before* the walk. It stays on the UI thread, under a lock
/// held for microseconds, because both halves of it -- the display offset and stepping off the
/// current match -- are about the grid the origin is about to be used against.
pub fn origin_for(
    term: &Term<EventProxy>,
    search: &mut Search,
    pane: PaneId,
    direction: Direction,
) -> (Point, Side) {
    // A `Point` is an index into one grid. Carrying one across a focus change would scroll the
    // new pane to a line number that meant something in the old one.
    if search.owner != Some(pane) {
        search.current = None;
        search.owner = Some(pane);
    }

    let display_offset = term.grid().display_offset();

    // Step one cell *past* the current match, not onto its far edge. `next_match_right` accepts
    // a match whose start is at or after the origin column -- so an origin sitting on the match's
    // own end re-accepts that match whenever its start is not strictly before its end, which is
    // every match one cell wide. Searching for a single letter then never left the first hit.
    // `Direction::Left` has the mirror of it, against the match's start.
    //
    // With no current match, start from the corner of the viewport you are reading from.
    let (origin, side) = match (&search.current, direction) {
        (Some(m), Direction::Right) => (m.end().add(term, Boundary::Grid, 1), Side::Left),
        (Some(m), Direction::Left) => (m.start().sub(term, Boundary::Grid, 1), Side::Right),
        (None, Direction::Right) => {
            (Point::new(Line(-(display_offset as i32)), Column(0)), Side::Left)
        },
        (None, Direction::Left) => {
            let bottom = term.screen_lines() as i32 - 1 - display_offset as i32;
            (Point::new(Line(bottom), Column(term.columns().saturating_sub(1))), Side::Right)
        },
    };

    // The stored match predates any resize since, so the point it carries can be outside the grid
    // it is about to index -- see `clamp_to_grid`.
    (clamp_to_grid(term, origin), side)
}

/// Walk the buffer for the next match. **This is the part that takes hundreds of milliseconds.**
///
/// It takes the lock itself and holds it for the whole walk, which is why it must not be on the
/// UI thread: 214-217 ms over 100k rows, against a configurable scrollback ceiling of a million.
///
/// There is no cheaper shape to reach for. `max_lines: None` is what makes "next" wrap around to
/// the first match, and searching in bounded pieces is not the same thing done in instalments:
/// `regex_search` stops dead at the end of the range it is handed, so every boundary silently
/// loses the matches that straddle it, and nothing bounds how many lines a match may cover.
///
/// The pattern is compiled here rather than handed over, so the `RegexSearch` the UI thread uses
/// to highlight the visible rows every frame is never borrowed away from it.
pub fn scan(
    term: &FairMutex<Term<EventProxy>>,
    pattern: &str,
    origin: Point,
    direction: Direction,
    side: Side,
) -> Option<Match> {
    let mut regex = match RegexSearch::new(pattern) {
        Ok(regex) => regex,
        Err(err) => {
            // Not a user error: the UI compiled this same pattern before asking for the scan.
            log::warn!("search pattern did not compile on the scan thread: {err}");
            return None;
        },
    };

    // `search_next` takes `&self`. This needs exclusive access only because `FairMutex` has
    // nothing weaker to offer.
    let term = term.lock();
    // The origin was clamped on the UI thread under its own short lock, but that lock is long
    // gone by the time this one arrives — a resize in between is exactly the gap the stored
    // `search.current` panic came through. Re-clamped against the grid actually locked, which is
    // the one that is about to be indexed.
    let origin = clamp_to_grid(&*term, origin);
    term.search_next(&mut regex, origin, direction, side, None)
}

/// Park the viewport on what a scan found. False when the pattern matches nothing at all, which
/// is what turns the bar red.
///
/// The grid can have been resized, scrolled or written to while the scan ran, which makes this
/// the one place where a `Point` crosses from one moment into another. `scroll_to_point` only
/// does arithmetic and `scroll_display` clamps, so this is about landing somewhere sensible
/// rather than about a panic -- but the point is pulled back inside the grid for the same reason
/// `origin_for` does it.
pub fn apply(term: &mut Term<EventProxy>, search: &mut Search, found: Option<Match>) -> bool {
    match found {
        Some(m) => {
            let point = clamp_to_grid(&*term, *m.start());
            term.scroll_to_point(point);
            search.current = Some(m);
            true
        },
        None => {
            search.current = None;
            false
        },
    }
}

/// Pull a point from an older grid into the current one.
///
/// `Search::owner` already records that a `Point` means nothing outside the grid it came from.
/// A resize is the second way that happens and it was not covered: `search.current` survives
/// `Term::resize`, so its column can be past the end of a row and its line off either end of the
/// history. `Term::search_next` opens with `expand_wide`, which indexes the row directly — so an
/// unclamped origin is an index-out-of-bounds panic on the UI thread rather than a search that
/// quietly finds nothing.
///
/// Clamping rather than discarding keeps the user near where they were reading. `min` then `max`
/// rather than `Ord::clamp`, which panics when `min > max` — a grid with no rows produces exactly
/// that, and a pane mid-layout can have one.
fn clamp_to_grid<D: Dimensions>(dims: &D, p: Point) -> Point {
    let line = p.line.0.min(dims.bottommost_line().0).max(dims.topmost_line().0);
    let column = p.column.0.min(dims.columns().saturating_sub(1));
    Point::new(Line(line), Column(column))
}

/// Every match on the rows currently on screen.
///
/// Takes `&Term` rather than `&mut` so the caller can compute this and then hand the same guard to
/// `snapshot::capture`, which needs `&mut` — the immutable borrow has to end first.
pub fn highlights(term: &Term<EventProxy>, search: &mut Search) -> Highlights {
    let cols = term.columns();
    let rows = term.screen_lines();
    let mut map = Highlights::new(cols, rows);

    if !search.open || cols == 0 || rows == 0 {
        return map;
    }
    let current = search.current.clone();
    let Some(regex) = search.compiled.as_mut() else { return map };

    let display_offset = term.grid().display_offset();
    let top = Line(-(display_offset as i32));
    let bottom = Line(rows as i32 - 1 - display_offset as i32);
    let start = Point::new(top, Column(0));
    let end = Point::new(bottom, Column(cols.saturating_sub(1)));

    // A pattern that can match nothing (`a*`) yields one match per cell and the iterator only
    // advances by a cell each time, so the visible grid is the natural bound. Without it a
    // pathological pattern is an unbounded loop inside a frame.
    let ceiling = cols * rows + 1;

    for m in RegexIter::new(start, end, Direction::Right, term, regex).take(ceiling) {
        let is_current = current.as_ref().is_some_and(|c| *c == m);
        map.mark(&m, display_offset, is_current);
    }

    map
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    use crate::term::testing::{headless_term, type_into};
    use crate::term::GridSize;

    fn point(line: i32, col: usize) -> Point {
        Point::new(Line(line), Column(col))
    }

    /// A terminal behind the lock the real one lives behind.
    fn headless(size: GridSize) -> FairMutex<Term<EventProxy>> {
        FairMutex::new(headless_term(size))
    }

    /// The three steps in the order the app runs them.
    ///
    /// Production spreads them across a short lock, a thread and an `AppEvent`; this is the same
    /// three functions with nothing in between, so what these tests pin is the real sequence and
    /// not a second implementation of it.
    fn advance(
        term: &FairMutex<Term<EventProxy>>,
        search: &mut Search,
        pane: PaneId,
        direction: Direction,
    ) -> bool {
        let Some(pattern) = search.pattern().map(str::to_owned) else { return false };
        let (origin, side) = origin_for(&term.lock(), search, pane, direction);
        let found = scan(term, &pattern, origin, direction, side);
        apply(&mut term.lock(), search, found)
    }

    struct Dims {
        total: usize,
        screen: usize,
        cols: usize,
    }

    impl Dimensions for Dims {
        fn total_lines(&self) -> usize {
            self.total
        }

        fn screen_lines(&self) -> usize {
            self.screen
        }

        fn columns(&self) -> usize {
            self.cols
        }
    }

    #[test]
    fn a_match_from_before_a_resize_does_not_panic_the_ui_thread() {
        // `search.current` survives `Term::resize`, and `advance` handed it straight to
        // `search_next`, whose first act is `expand_wide` — which indexes the row by the stored
        // column. A column from a wider grid is out of bounds, and the panic lands on the UI
        // thread. Narrowing a window with the find bar open is all it takes.
        let pane = 1;
        let term = headless(GridSize::new(20, 4));
        let mut search = Search::default();
        search.open();
        search.set_query("x".into());
        search.owner = Some(pane);
        // Where a match on the old, wider grid would have left it.
        search.current = Some(point(0, 16)..=point(0, 18));

        term.lock().resize(GridSize::new(5, 4));

        // Finding nothing is a fine answer here; taking the window down is not.
        advance(&term, &mut search, pane, Direction::Right);
    }

    #[test]
    fn stepping_backwards_from_a_stale_match_does_not_panic_either() {
        // Direction::Left reads the match's *start* rather than its end, so it is a second path
        // to the same index.
        let pane = 1;
        let term = headless(GridSize::new(20, 4));
        let mut search = Search::default();
        search.open();
        search.set_query("x".into());
        search.owner = Some(pane);
        search.current = Some(point(0, 16)..=point(0, 18));

        term.lock().resize(GridSize::new(5, 4));

        advance(&term, &mut search, pane, Direction::Left);
    }

    fn searching(query: &str, pane: PaneId) -> Search {
        let mut search = Search::default();
        search.open();
        search.set_query(query.into());
        search.owner = Some(pane);
        search
    }

    #[test]
    fn next_steps_off_a_single_character_match() {
        // `next_match_right` accepts a match whose start is at or *after* the origin column, and
        // the origin was the current match's own end. For a match one cell wide those are the
        // same point, so the match re-accepted itself: Enter never left the first hit, which is
        // every search for a single letter.
        let pane = 1;
        let term = headless(GridSize::new(20, 4));
        type_into(&mut term.lock(), "a b a");
        let mut search = searching("a", pane);

        assert!(advance(&term, &mut search, pane, Direction::Right));
        assert_eq!(search.current.clone().unwrap(), point(0, 0)..=point(0, 0));

        assert!(advance(&term, &mut search, pane, Direction::Right));
        assert_eq!(
            search.current.clone().unwrap(),
            point(0, 4)..=point(0, 4),
            "next found the same one-cell match again"
        );
    }

    #[test]
    fn previous_steps_off_a_single_character_match_too() {
        // Direction::Left has the mirror of the same problem, against the match's start.
        let pane = 1;
        let term = headless(GridSize::new(20, 4));
        type_into(&mut term.lock(), "a b a");
        let mut search = searching("a", pane);

        assert!(advance(&term, &mut search, pane, Direction::Left));
        assert_eq!(search.current.clone().unwrap(), point(0, 4)..=point(0, 4));

        assert!(advance(&term, &mut search, pane, Direction::Left));
        assert_eq!(search.current.clone().unwrap(), point(0, 0)..=point(0, 0));
    }

    #[test]
    fn stepping_still_works_for_matches_wider_than_one_cell() {
        // The counterpart: these already worked, because a multi-cell match has its start
        // strictly before its end. Stepping the origin past the match must not skip one.
        let pane = 1;
        let term = headless(GridSize::new(20, 4));
        type_into(&mut term.lock(), "ab x ab");
        let mut search = searching("ab", pane);

        assert!(advance(&term, &mut search, pane, Direction::Right));
        assert_eq!(search.current.clone().unwrap(), point(0, 0)..=point(0, 1));

        assert!(advance(&term, &mut search, pane, Direction::Right));
        assert_eq!(search.current.clone().unwrap(), point(0, 5)..=point(0, 6));
    }

    #[test]
    fn clamping_pulls_a_stale_point_inside_the_current_grid() {
        // 5 columns, 4 visible rows, 2 of history: lines -2..=3, columns 0..=4.
        let d = Dims { total: 6, screen: 4, cols: 5 };

        // A column from a wider grid, and a line off either end of the history.
        assert_eq!(clamp_to_grid(&d, point(0, 16)), point(0, 4));
        assert_eq!(clamp_to_grid(&d, point(-99, 2)), point(-2, 2));
        assert_eq!(clamp_to_grid(&d, point(40, 2)), point(3, 2));

        // Anything already inside is left exactly where it is, including both corners.
        assert_eq!(clamp_to_grid(&d, point(-2, 0)), point(-2, 0));
        assert_eq!(clamp_to_grid(&d, point(3, 4)), point(3, 4));
        assert_eq!(clamp_to_grid(&d, point(1, 2)), point(1, 2));
    }

    #[test]
    fn clamping_survives_a_grid_with_no_rows_or_columns() {
        // `Ord::clamp` panics when min > max, which a grid with no rows produces; `last_column`
        // underflows on one with no columns. A pane mid-layout can be either.
        let d = Dims { total: 0, screen: 0, cols: 0 };
        let _ = clamp_to_grid(&d, point(7, 7));
    }

    #[test]
    fn a_match_on_one_row_marks_exactly_its_cells() {
        let mut h = Highlights::new(10, 3);
        h.mark(&(point(1, 2)..=point(1, 4)), 0, false);

        assert_eq!(h.at(1, 1), None);
        assert_eq!(h.at(1, 2), Some(Hit::Other));
        assert_eq!(h.at(1, 4), Some(Hit::Other));
        assert_eq!(h.at(1, 5), None);
        assert_eq!(h.visible, 1);
    }

    #[test]
    fn a_wrapped_match_fills_the_rows_between_its_ends() {
        // A regex crossing a line boundary covers the tail of the first row, all of any row in
        // between, and the head of the last.
        let mut h = Highlights::new(6, 4);
        h.mark(&(point(0, 4)..=point(2, 1)), 0, false);

        assert_eq!(h.at(0, 3), None);
        assert_eq!(h.at(0, 5), Some(Hit::Other));
        assert_eq!(h.at(1, 0), Some(Hit::Other));
        assert_eq!(h.at(1, 5), Some(Hit::Other));
        assert_eq!(h.at(2, 1), Some(Hit::Other));
        assert_eq!(h.at(2, 2), None);
    }

    #[test]
    fn scrollback_lines_map_onto_visible_rows_through_the_display_offset() {
        // Scrolled back five lines, buffer line -5 is the top visible row. Getting this backwards
        // highlights the wrong rows, which reads as "search finds the wrong thing".
        let mut h = Highlights::new(8, 4);
        h.mark(&(point(-5, 0)..=point(-5, 2)), 5, false);
        assert_eq!(h.at(0, 0), Some(Hit::Other));
        assert_eq!(h.at(1, 0), None);
    }

    #[test]
    fn a_match_entirely_off_screen_marks_nothing_and_is_not_counted() {
        let mut h = Highlights::new(8, 4);
        h.mark(&(point(50, 0)..=point(50, 2)), 0, false);
        assert_eq!(h.visible, 0);
    }

    #[test]
    fn the_current_match_outranks_an_overlapping_one() {
        let mut h = Highlights::new(8, 2);
        h.mark(&(point(0, 0)..=point(0, 3)), 0, false);
        h.mark(&(point(0, 2)..=point(0, 5)), 0, true);
        assert_eq!(h.at(0, 0), Some(Hit::Other));
        assert_eq!(h.at(0, 2), Some(Hit::Current));
        assert_eq!(h.at(0, 5), Some(Hit::Current));
    }

    #[test]
    fn out_of_range_lookups_are_none_rather_than_a_panic() {
        let h = Highlights::new(4, 2);
        assert_eq!(h.at(99, 0), None);
        assert_eq!(h.at(0, 99), None);
    }

    #[test]
    fn literal_mode_escapes_what_a_terminal_is_full_of() {
        // A Windows path and a shell error message both parse as broken regexes, which is why
        // literal is the default rather than the fallback.
        let escaped = escape_literal(r"C:\Users (2)");
        assert_eq!(escaped, r"C:\\Users \(2\)");
        assert!(RegexSearch::new(&escaped).is_ok());
        // ...and the unescaped form is exactly the thing that fails.
        assert!(RegexSearch::new(r"C:\Users (2)").is_err());
    }

    #[test]
    fn the_default_is_literal_so_an_ordinary_search_cannot_fail() {
        let mut s = Search::default();
        s.open();
        s.set_query(r"C:\Users\Nihil".into());
        assert!(s.error.is_none(), "{:?}", s.error);
        assert!(s.is_active());
    }

    #[test]
    fn a_broken_regex_reports_a_reason_a_person_can_act_on() {
        // The builder's own message is "error building NFA", which says nothing.
        let mut s = Search { regex_mode: true, ..Default::default() };
        s.open();
        s.set_query("(unclosed".into());
        assert_eq!(s.error.as_deref(), Some("unclosed group"));
        assert!(!s.is_active());

        s.set_query(r"C:\Users".into());
        assert_eq!(s.error.as_deref(), Some("invalid hexadecimal digit"));

        // ...and the same text in literal mode is a perfectly good search.
        s.toggle_regex();
        assert!(s.error.is_none());
        assert!(s.is_active());
    }

    #[test]
    fn changing_the_query_forgets_where_we_were() {
        // Otherwise `next` steps off a point belonging to a match that no longer exists.
        let mut s = Search { regex_mode: true, ..Default::default() };
        s.open();
        s.set_query("a".into());
        s.current = Some(point(0, 0)..=point(0, 0));
        s.set_query("ab".into());
        assert!(s.current.is_none());
    }


    #[test]
    fn a_scan_runs_somewhere_else_and_needs_the_lock_to_do_it() {
        // The whole of 2.2 in one test. The walk holds the pane's lock for as long as it takes,
        // which is why it cannot be on the UI thread — and why the UI in turn has to be able to
        // decline to wait for it, which is what `try_lock_unfair` in the render path is for.
        let term = Arc::new(headless(GridSize::new(20, 4)));
        type_into(&mut term.lock(), "a b a");

        let (tx, rx) = crossbeam_channel::unbounded();
        // Stands in for a PTY thread mid-write, or for the renderer taking its snapshot.
        let held = term.lock();

        let scanned = term.clone();
        std::thread::spawn(move || {
            let found =
                scan(&scanned, "a", Point::new(Line(0), Column(1)), Direction::Right, Side::Left);
            let _ = tx.send(found);
        });

        // It cannot have answered: it wants the lock this thread is holding. If the walk ever
        // stops taking the lock itself, or stops being handed to a thread, this is what notices.
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "the scan produced an answer without the lock it needs to read the grid"
        );

        drop(held);
        let found = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the scan never finished once the lock was free");
        assert_eq!(found, Some(point(0, 4)..=point(0, 4)));
    }

    #[test]
    fn a_superseded_scan_does_not_move_the_view() {
        // Type `a`, then `ab` while the walk for `a` is still going. The answer to `a` arrives
        // second and must be thrown away: applying it parks the view on a match of a query the
        // user has already finished replacing, which is the jump a debounced find bar exists to
        // avoid in the first place.
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let first = s.request(Direction::Right).expect("nothing was running");

        s.set_query("ab".into());
        assert_eq!(s.request(Direction::Right), None, "a second thread was started");

        let landed = s.land(first);
        assert!(!landed.apply, "the stale answer was applied to the view");
        let (second, _) = landed.next.expect("the request made while it ran was dropped");
        assert_ne!(second, first);

        // ...and that one is nobody's stale answer, so it is applied.
        let landed = s.land(second);
        assert!(landed.apply);
        assert_eq!(landed.next, None);
        assert!(!s.scanning());
    }

    #[test]
    fn only_the_newest_waiting_request_survives() {
        // Three more keystrokes while one scan runs cost one more scan, not three. Enter repeats
        // while it is held down, so this is also what stops a thread being started per repeat.
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let first = s.request(Direction::Right).unwrap();
        assert_eq!(s.request(Direction::Right), None);
        assert_eq!(s.request(Direction::Right), None);
        assert_eq!(s.request(Direction::Left), None);

        let landed = s.land(first);
        let (next, direction) = landed.next.expect("nothing was queued");
        assert_eq!(direction, Direction::Left, "the queue kept a request that had been replaced");
        assert_eq!(s.land(next).next, None, "the ones in between were queued too");
    }

    #[test]
    fn a_result_for_an_edited_query_is_not_applied_to_the_view() {
        // The debounce holds the replacement request back 100 ms, so a walk that is already
        // running when the query changes lands first. Its answer is to the *previous* query, and
        // applying it parks the view on a match the user has already moved past — the exact jump
        // the debounce exists to prevent. `recompile` resets `current`; the stale `apply` was the
        // one thing left that could put it back.
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let first = s.request(Direction::Right).expect("nothing was running");

        s.set_query("ab".into());
        let landed = s.land(first);
        assert!(!landed.apply, "the stale answer moved the view");
        assert!(s.current.is_none(), "the view parked on a match of the old query");
        // The bar stays usable: the debounced request still starts when it is due.
        assert!(s.request(Direction::Right).is_some());
    }

    #[test]
    fn an_emptied_query_drops_the_result_too() {
        // Emptying the query also empties the pattern, which must read as "not the answer to
        // this" rather than as "the answer to whatever".
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let seq = s.request(Direction::Right).unwrap();

        s.set_query(String::new());
        assert!(!s.land(seq).apply);
    }

    #[test]
    fn a_scan_clamps_an_origin_that_stopped_meaning_anything() {
        // `origin_for` clamps under its own short lock; `scan` re-locks afterwards. A resize in
        // that gap hands `search_next` a column the locked grid does not have, and it indexes the
        // row with it — the same out-of-bounds the stored `search.current` produced, one lock
        // later.
        let term = FairMutex::new(headless_term(GridSize::new(5, 4)));
        type_into(&mut term.lock(), "a b a");
        let stale = point(0, 16);

        let found = scan(&term, "a", stale, Direction::Right, Side::Left);
        // The clamped origin (0,16 -> 0,4) is a real cell, so the search runs from it and finds
        // the `a` that lives there. What it must never do is index past the grid and panic.
        assert_eq!(found, Some(point(0, 4)..=point(0, 4)));
    }

    #[test]
    fn a_result_for_a_closed_find_bar_is_ignored() {
        // Escape is the most likely thing to happen during a two-second walk over a million
        // lines, and the answer arrives afterwards to a bar that is gone.
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let seq = s.request(Direction::Right).unwrap();
        s.close();
        assert!(s.scanning(), "closing a bar cannot release its worker's lock");
        assert_eq!(s.scanning_pane(), Some(1));

        assert_eq!(s.land(seq), Landed::default());
        assert!(!s.scanning());
    }

    #[test]
    fn queued_searches_keep_the_pane_that_requested_them() {
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let first = s.request_in(17, Direction::Right).unwrap();
        assert_eq!(s.request_in(29, Direction::Left), None);
        assert_eq!(s.scanning_pane(), Some(17));
        let landed = s.land(first);
        assert!(!landed.apply);
        assert_eq!(s.scanning_pane(), Some(29));
        let (next, direction) = landed.next.unwrap();
        assert_eq!(direction, Direction::Left);
        assert!(s.land(next).apply);
        assert_eq!(s.scanning_pane(), None);
    }

    #[test]
    fn reopening_a_closed_bar_does_not_accept_its_old_result() {
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        let first = s.request_in(17, Direction::Right).unwrap();
        s.close();
        s.open();
        s.set_query("a".into());
        assert!(!s.land(first).apply);
    }

    #[test]
    fn a_scan_that_never_started_leaves_the_bar_usable() {
        // `request` marks a scan as running before the caller has managed to spawn it, because
        // it is what hands out the sequence number. If the spawn then cannot happen — the pane
        // closed in between — every later search would queue behind a walk that is not
        // happening, and the find bar would quietly stop finding anything at all.
        let mut s = Search::default();
        s.open();
        s.set_query("a".into());
        s.request(Direction::Right).unwrap();

        s.abandon();

        assert!(!s.scanning());
        assert!(s.request(Direction::Right).is_some(), "the next search queued behind nothing");
    }

    #[test]
    fn an_empty_query_is_not_an_error_and_matches_nothing() {
        let mut s = Search::default();
        s.open();
        s.set_query(String::new());
        assert!(s.error.is_none());
        assert!(!s.is_active());
    }
}
