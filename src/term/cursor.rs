//! When the drawn cursor is allowed to follow the terminal's.
//!
//! ConPTY does not hand us a repaint in one piece. conhost paints its changed cells, leaves the
//! cursor wherever that painting finished, and only puts it back where the program actually
//! wants it in its *next* frame — 8 to 22 ms later, measured against `codex` on this machine.
//! Both halves arrive as ordinary output, so a terminal that redraws whenever output arrives
//! draws the parked cursor for a frame or two every time the program repaints. A TUI that
//! animates a spinner repaints ten times a second, and the cursor is then seen flicking to the
//! top-left corner and back, ten times a second, for as long as the program is working.
//!
//! Nothing in the byte stream separates that parked position from a real one: the parking frame
//! is a content update with a cursor move in it, which is exactly what echoing a typed character
//! is. Only time tells them apart — the parked position never survives the next frame — so a
//! jump is drawn only once it has held still for `SETTLE`.
//!
//! The cost of that would be `SETTLE` of cursor lag on every keystroke, which is the one place a
//! terminal cannot afford any, so a move of a single column along the same row is taken
//! immediately. That is what typing, backspacing and a line editor's own left/right all look
//! like, and taking a position immediately is what the code did before this existed — the
//! exemption can only decline to defer, never move the cursor somewhere it was not.

use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point};

/// How long a jump must hold still before it is drawn.
///
/// Above the 8–22 ms conhost was measured taking to put the cursor back, and below the point
/// where a deferred jump reads as lag.
const SETTLE: Duration = Duration::from_millis(30);

/// A ceiling on how long the drawn cursor may be held behind the real one.
///
/// A program that moves the cursor somewhere new on every frame would otherwise reset the settle
/// timer forever and freeze the cursor where it last stood still.
const MAX_HOLD: Duration = Duration::from_millis(90);

/// Per pane, and owned by the UI thread: the drawn cursor position, which trails the terminal's
/// only across a jump that has not proved itself yet.
#[derive(Default, Debug)]
pub struct CursorSettle {
    drawn: Option<Point>,
    /// A position seen but not yet drawn, and when it was first seen.
    pending: Option<(Point, Instant)>,
    /// When the current hold began, for `MAX_HOLD`.
    holding_since: Option<Instant>,
}

impl CursorSettle {
    /// Where to draw the cursor, given where the terminal says it is.
    ///
    /// `cols` and `rows` are the current grid, because a held position from a larger grid is not
    /// a cell any more.
    pub fn resolve(&mut self, point: Point, cols: usize, rows: usize, now: Instant) -> Point {
        let in_grid =
            |p: Point| p.line >= Line(0) && p.line.0 < rows as i32 && p.column < Column(cols);

        let hold = match self.drawn {
            Some(drawn)
                if drawn != point
                    && in_grid(drawn)
                    && !adjacent(drawn, point)
                    && self.holding_since.is_none_or(|t| now.duration_since(t) < MAX_HOLD) =>
            {
                match self.pending {
                    Some((p, since)) if p == point => now.duration_since(since) < SETTLE,
                    _ => {
                        self.pending = Some((point, now));
                        true
                    },
                }
            },
            _ => false,
        };

        if hold {
            self.holding_since.get_or_insert(now);
            self.drawn.unwrap_or(point)
        } else {
            self.pending = None;
            self.holding_since = None;
            self.drawn = Some(point);
            point
        }
    }

    /// How long until a deferred position would be drawn, for scheduling the frame that draws it.
    ///
    /// `None` when nothing is deferred. Without this the cursor would sit at the old position
    /// until some unrelated output happened to bring a frame along.
    pub fn deadline(&self, now: Instant) -> Option<Duration> {
        self.pending.map(|(_, since)| SETTLE.saturating_sub(now.duration_since(since)))
    }
}

/// One column left or right along the same row: typing, backspacing, or a line editor's own
/// left/right. Never a repaint parking the cursor somewhere else.
fn adjacent(a: Point, b: Point) -> bool {
    a.line == b.line && a.column.0.abs_diff(b.column.0) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: i32, col: usize) -> Point {
        Point::new(Line(line), Column(col))
    }

    #[test]
    fn typing_is_never_deferred() {
        // The whole point of the adjacency exemption: a cursor that lags the character it is
        // sitting behind is the one artefact worse than the one this module exists to remove.
        let mut s = CursorSettle::default();
        let t = Instant::now();
        assert_eq!(s.resolve(p(0, 4), 80, 24, t), p(0, 4));
        assert_eq!(s.resolve(p(0, 5), 80, 24, t), p(0, 5));
        assert_eq!(s.resolve(p(0, 6), 80, 24, t), p(0, 6));
        assert_eq!(s.resolve(p(0, 5), 80, 24, t), p(0, 5), "backspace is adjacent too");
        assert_eq!(s.deadline(t), None);
    }

    #[test]
    fn a_parked_cursor_that_comes_straight_back_is_never_drawn() {
        // The measured ConPTY sequence: the program's cursor sits at (10, 20), conhost paints and
        // leaves it at (8, 0), and 15 ms later puts it back.
        let mut s = CursorSettle::default();
        let t = Instant::now();
        assert_eq!(s.resolve(p(10, 20), 80, 24, t), p(10, 20));

        let parked = t + Duration::from_millis(1);
        assert_eq!(s.resolve(p(8, 0), 80, 24, parked), p(10, 20), "the park must not be drawn");
        assert!(s.deadline(parked).is_some());

        let back = t + Duration::from_millis(16);
        assert_eq!(s.resolve(p(10, 20), 80, 24, back), p(10, 20));
        assert_eq!(s.deadline(back), None, "nothing is deferred once the real position returns");
    }

    #[test]
    fn the_measured_conpty_trace_never_draws_a_parked_cursor() {
        // Timestamps and positions lifted from a real ConPTY carrying `codex` while a character
        // was typed every 120 ms: conhost paints, leaving the cursor at (8, 0), and puts it back
        // one column further along ~15 ms later. Every restore is adjacent to what is drawn, so
        // the typed character and its cursor still arrive together.
        let trace: &[(u64, i32, usize)] = &[
            (5981, 10, 19),
            (6074, 8, 0),
            (6090, 10, 20),
            (6200, 8, 0),
            (6215, 10, 21),
            (6317, 8, 0),
            (6331, 10, 22),
            (6442, 8, 0),
            (6458, 10, 23),
            (6561, 8, 0),
            (6578, 10, 24),
        ];

        let mut s = CursorSettle::default();
        let base = Instant::now();
        for &(ms, line, col) in trace {
            let now = base + Duration::from_millis(ms - trace[0].0);
            let drawn = s.resolve(p(line, col), 120, 34, now);
            assert_eq!(drawn.line, Line(10), "the cursor left the composer at t={ms}ms");
        }
    }

    #[test]
    fn a_jump_that_holds_still_is_drawn() {
        let mut s = CursorSettle::default();
        let t = Instant::now();
        s.resolve(p(10, 20), 80, 24, t);
        assert_eq!(s.resolve(p(2, 0), 80, 24, t), p(10, 20));
        assert_eq!(s.resolve(p(2, 0), 80, 24, t + SETTLE - Duration::from_millis(1)), p(10, 20));
        assert_eq!(s.resolve(p(2, 0), 80, 24, t + SETTLE), p(2, 0));
        assert_eq!(s.deadline(t + SETTLE), None);
    }

    #[test]
    fn a_cursor_that_never_settles_is_still_drawn_eventually() {
        // Otherwise a program moving the cursor somewhere new on every frame resets the timer
        // forever and the drawn cursor stays where it last stood still.
        let mut s = CursorSettle::default();
        let t = Instant::now();
        s.resolve(p(0, 0), 80, 24, t);

        let mut stale_for = Duration::ZERO;
        let mut longest = Duration::ZERO;
        for i in 1..40 {
            let step = Duration::from_millis(5);
            let now = t + step * i;
            let want = p(i as i32 % 20, i as usize);
            if s.resolve(want, 80, 24, now) == want {
                stale_for = Duration::ZERO;
            } else {
                stale_for += step;
                longest = longest.max(stale_for);
            }
        }
        assert!(longest > Duration::ZERO, "a wandering cursor is deferred at all");
        assert!(longest <= MAX_HOLD, "and never for longer than MAX_HOLD, got {longest:?}");
    }

    #[test]
    fn a_held_position_outside_the_grid_is_dropped() {
        // A resize is the other way a stored point stops meaning a cell: holding one from the
        // wider grid would draw the cursor off the end of a row that no longer has that column.
        let mut s = CursorSettle::default();
        let t = Instant::now();
        s.resolve(p(10, 100), 120, 24, t);
        assert_eq!(s.resolve(p(4, 2), 40, 24, t), p(4, 2));
    }

    #[test]
    fn the_first_position_is_drawn_at_once() {
        let mut s = CursorSettle::default();
        let t = Instant::now();
        assert_eq!(s.resolve(p(7, 3), 80, 24, t), p(7, 3));
    }
}
