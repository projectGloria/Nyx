//! The Ctrl+F bar.
//!
//! Painted rather than built from egui widgets, for the same reason the palette is: the keyboard
//! is handled in `logic()` before anything reaches the shell, so a `TextEdit` would be a second
//! place that owns the same keystrokes and the two would disagree about who has focus.

use crate::term::search::Search;
use crate::theme::visuals::to_color32;
use crate::theme::Theme;

/// Something the user clicked. Keys are handled by the app, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Next,
    Prev,
    ToggleRegex,
    Close,
}

const HEIGHT: f32 = 30.0;
const WIDTH: f32 = 420.0;

/// Where the bar sits inside the terminal area.
///
/// Exposed because the grid reads raw pointer events rather than going through egui's layering,
/// so it has to be told which rectangle is not its own — otherwise clicking "next match" also
/// drags a selection through the output behind the bar.
pub fn rect(area: egui::Rect) -> egui::Rect {
    let width = WIDTH.min(area.width() - 16.0).max(200.0);
    egui::Rect::from_min_size(
        egui::pos2(area.max.x - width - 8.0, area.min.y + 8.0),
        egui::vec2(width, HEIGHT),
    )
}

/// Draw the bar in the top-right of the terminal area. Returns a clicked action.
pub fn show(
    ui: &mut egui::Ui,
    area: egui::Rect,
    search: &Search,
    visible: usize,
    theme: &Theme,
) -> Option<Action> {
    let rect = rect(area);

    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, to_color32(theme.surface));
    painter.rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, to_color32(theme.accent)),
        egui::StrokeKind::Inside,
    );

    let mut clicked = None;
    let mid = rect.center().y;
    let mut x = rect.min.x + 8.0;

    // The regex/literal toggle. It is a button rather than a hidden binding because the moment
    // anyone needs it is the moment their pattern just failed to compile and they are looking
    // right here for why.
    let (toggle_label, toggle_hint) = if search.regex_mode {
        (".*", "Regular expression \u{2014} click or Alt+R to match literal text")
    } else {
        ("ab", "Literal text \u{2014} click or Alt+R for a regular expression")
    };
    let toggle_rect = egui::Rect::from_min_size(egui::pos2(x, mid - 9.0), egui::vec2(26.0, 18.0));
    let response = ui
        .interact(toggle_rect, egui::Id::new("nyx.find.regex"), egui::Sense::click())
        .on_hover_text(toggle_hint);
    if response.clicked() {
        clicked = Some(Action::ToggleRegex);
    }
    ui.painter().rect_filled(
        toggle_rect,
        3.0,
        to_color32(if search.regex_mode { theme.selection_bg } else { theme.surface }),
    );
    ui.painter().text(
        toggle_rect.center(),
        egui::Align2::CENTER_CENTER,
        toggle_label,
        egui::FontId::monospace(11.0),
        to_color32(if search.regex_mode { theme.fg } else { theme.palette[8] }),
    );
    x = toggle_rect.max.x + 8.0;

    // The status sits on the right and the query grows into whatever is left, so a long query
    // never pushes the match count off the end of the bar.
    let (status, color) = status(search, visible);
    let status_color = theme.palette[color];
    let status_width = status.chars().count() as f32 * 6.5 + 8.0;

    let buttons_w = 66.0;
    let query_max = rect.max.x - 8.0 - buttons_w - status_width - x;

    let query = if search.query.is_empty() { "find…" } else { &search.query };
    let query_color = if search.query.is_empty() { theme.palette[8] } else { theme.fg };
    // Clipped to its own column: a query longer than the bar must not draw over the match count.
    let clip = egui::Rect::from_min_size(
        egui::pos2(x, rect.min.y),
        egui::vec2(query_max.max(40.0), rect.height()),
    );
    ui.painter().with_clip_rect(clip.intersect(rect)).text(
        egui::pos2(x, mid),
        egui::Align2::LEFT_CENTER,
        query,
        egui::FontId::monospace(13.0),
        to_color32(query_color),
    );

    ui.painter().text(
        egui::pos2(rect.max.x - 8.0 - buttons_w, mid),
        egui::Align2::RIGHT_CENTER,
        &status,
        egui::FontId::monospace(11.0),
        to_color32(status_color),
    );

    let buttons = [
        ("\u{2191}", Action::Prev, "Previous match (Shift+Enter)"),
        ("\u{2193}", Action::Next, "Next match (Enter)"),
        ("\u{00d7}", Action::Close, "Close (Esc)"),
    ];
    for (i, (glyph, action, hint)) in buttons.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - buttons_w + i as f32 * 22.0, mid - 9.0),
            egui::vec2(20.0, 18.0),
        );
        let response = ui
            .interact(r, egui::Id::new(("nyx.find.button", i)), egui::Sense::click())
            .on_hover_text(hint);
        if response.clicked() {
            clicked = Some(action);
        }
        if response.hovered() {
            ui.painter().rect_filled(r, 3.0, to_color32(theme.selection_bg));
        }
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::monospace(12.0),
            to_color32(theme.fg),
        );
    }

    clicked
}

/// The right-hand status text, and which palette entry colours it.
///
/// Separated from the drawing so it can be tested: what the bar *says* is the only part of it
/// that can be wrong rather than merely ugly.
fn status(search: &Search, visible: usize) -> (String, usize) {
    match (&search.error, search.query.is_empty(), visible) {
        // A pattern that did not compile can only happen in regex mode, and the way out is the
        // toggle two inches to the left — so the message names it rather than only the fault.
        (Some(err), _, _) => (format!("{err} \u{2014} Alt+R"), 1),
        (None, true, _) => (String::new(), 8),
        // "No matches" is a claim about the whole buffer, and while a scan is still walking it
        // nobody knows that yet. Moving the walk off the UI thread is what made this reachable:
        // it can now take two seconds over a million lines without freezing the window, and for
        // those two seconds the bar was confidently saying the wrong thing.
        (None, false, 0) if search.scanning() => ("searching\u{2026}".into(), 8),
        (None, false, 0) => ("no matches here".into(), 3),
        (None, false, n) => (format!("{n} on screen"), 8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Direction;

    fn bar(query: &str) -> Search {
        let mut search = Search::default();
        search.open();
        search.set_query(query.into());
        search
    }

    #[test]
    fn a_scan_still_walking_is_not_the_same_as_nothing_to_find() {
        // Both look like "no matches on screen" from in here. Saying so while the buffer is
        // still being read is a wrong answer the user acts on, by giving up on a search that was
        // about to succeed.
        let mut search = bar("needle");
        let seq = search.request(Direction::Right).expect("nothing was running");
        assert!(search.scanning());
        assert_eq!(status(&search, 0).0, "searching\u{2026}");

        // Once it lands having found nothing, the claim is true and gets made.
        search.land(seq);
        assert!(!search.scanning());
        assert_eq!(status(&search, 0).0, "no matches here");
    }

    #[test]
    fn a_count_is_shown_even_while_another_scan_is_running() {
        // Pressing Enter starts a scan with matches already on screen. The count is still the
        // more useful thing to say, and a walk being in progress does not make it wrong.
        let mut search = bar("needle");
        search.request(Direction::Right).unwrap();
        assert_eq!(status(&search, 3).0, "3 on screen");
    }

    #[test]
    fn a_pattern_that_did_not_compile_outranks_everything_else() {
        let mut search = Search::default();
        search.open();
        search.toggle_regex();
        search.set_query("(unclosed".into());
        assert_eq!(status(&search, 0).0, "unclosed group \u{2014} Alt+R");
    }
}
