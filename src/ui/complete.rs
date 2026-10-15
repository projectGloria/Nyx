//! The sigil menu and the inline hint: the two things drawn *at the cursor* rather than as chrome.
//!
//! Both are display-only. Neither takes a click and neither holds egui focus — the terminal owns
//! the keyboard and the mouse, and the app subtracts these rectangles from the grid's own pointer
//! handling by hand, exactly as it does for the find bar.
//!
//! The placement rules are functions rather than inline arithmetic because a prompt lives on the
//! *last* row of the grid, so "below the cursor" is the case that never happens and the one that
//! would have been written first. Pure, so both can be pinned without a font or a window.

use crate::theme::visuals::to_color32;
use crate::theme::Theme;

/// One offered sigil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub sigil: String,
    pub label: String,
    pub path: String,
}

/// Which row is highlighted, and whether the menu has been waved away.
#[derive(Debug, Default)]
pub struct State {
    pub selected: usize,
    /// The token the selection and the dismissal belong to.
    ///
    /// Both are remembered per token rather than as bare flags, which is what makes them
    /// self-clearing: typing another character produces a different token, so the highlight goes
    /// back to the first row and a dismissed menu comes back. The alternative is a flag plus
    /// something to clear it, and the only honest candidate for that is "the line changed" —
    /// which is exactly the question this avoids having to ask.
    pub token: Option<String>,
    /// The token Escape was pressed against.
    pub dismissed: Option<String>,
    /// Where the cursor was when it was last visible, so the hint does not blink with it.
    pub anchor: Option<egui::Rect>,
    /// Last frame's menu rectangle, subtracted from the grid's pointer handling.
    pub blocked: Option<egui::Rect>,
}

impl State {
    /// Bring the selection into step with the token now being typed.
    ///
    /// Narrowing has to put the highlight back on the first row: keeping index 2 after `@n`
    /// became `@ny` leaves it pointing at whichever entry slid into that slot, which is the tab
    /// strip's removal bug in a smaller box.
    pub fn sync(&mut self, token: &str, hits: usize) {
        if self.token.as_deref() != Some(token) {
            self.token = Some(token.to_string());
            self.selected = 0;
            if self.dismissed.as_deref() != Some(token) {
                self.dismissed = None;
            }
        }
        self.selected = self.selected.min(hits.saturating_sub(1));
    }

    /// Whether the menu for `token` has been dismissed.
    pub fn is_dismissed(&self, token: &str) -> bool {
        self.dismissed.as_deref() == Some(token)
    }

    pub fn dismiss(&mut self, token: &str) {
        self.dismissed = Some(token.to_string());
    }

    pub fn step(&mut self, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let len = len as isize;
        self.selected = (((self.selected as isize + delta) % len + len) % len) as usize;
    }
}

pub const ROW_H: f32 = 22.0;
const PAD: f32 = 4.0;
const MAX_ROWS: usize = 8;
const MIN_WIDTH: f32 = 220.0;
const MAX_WIDTH: f32 = 460.0;

/// How many rows the menu actually shows.
pub fn visible_rows(hits: usize) -> usize {
    hits.min(MAX_ROWS)
}

/// Where the menu goes, given the cursor cell and the pane it belongs to.
///
/// Below the cursor when there is room and above it when there is not — and for a terminal there
/// is almost never room, because the prompt you are typing at is the bottom line of the grid. The
/// horizontal clamp is what keeps a menu opened near the right edge of a split from being drawn
/// across the pane next door.
pub fn menu_rect(anchor: egui::Rect, area: egui::Rect, rows: usize) -> egui::Rect {
    let height = rows as f32 * ROW_H + PAD * 2.0;
    let width = MAX_WIDTH.min(area.width()).max(MIN_WIDTH.min(area.width()));
    let x = anchor.min.x.min(area.max.x - width).max(area.min.x);

    let below = anchor.max.y + 2.0;
    let y = if below + height <= area.max.y {
        below
    } else {
        (anchor.min.y - 2.0 - height).max(area.min.y)
    };
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

/// Where the hint goes, and how much room it has.
///
/// Beside the cursor, which is where the eye already is. When the cursor has run far enough along
/// the row that there is no useful space left, it drops to the start of the next line instead of
/// being squeezed into twenty pixels — a hint elided to `↵ op…` is worse than no hint, because it
/// costs the same glance and answers nothing.
pub fn hint_pos(anchor: egui::Rect, area: egui::Rect, gap: f32) -> (egui::Pos2, f32) {
    let beside = anchor.max.x + gap;
    let room = area.max.x - beside;
    if room >= MIN_WIDTH {
        return (egui::pos2(beside, anchor.center().y), room);
    }
    let below = anchor.max.y + anchor.height() * 0.5;
    (egui::pos2(area.min.x + gap, below), area.width() - gap * 2.0)
}

/// Draw the menu. Returns nothing: it is a picture, and the keyboard decides everything.
pub fn draw_menu(
    painter: &egui::Painter,
    rect: egui::Rect,
    rows: &[Row],
    token: &str,
    selected: usize,
    theme: &Theme,
) {
    painter.rect_filled(rect, 6.0, to_color32(theme.surface));
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, to_color32(theme.accent)),
        egui::StrokeKind::Inside,
    );

    let sigil_font = egui::FontId::monospace(13.0);
    let detail_font = egui::FontId::monospace(11.0);

    let window = super::selection_window(selected, rows.len(), visible_rows(rows.len()));
    for (i, row) in rows[window.clone()].iter().enumerate() {
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + PAD, rect.min.y + PAD + i as f32 * ROW_H),
            egui::vec2(rect.width() - PAD * 2.0, ROW_H),
        );
        if window.start + i == selected {
            painter.rect_filled(row_rect, 4.0, to_color32(theme.selection_bg));
        }

        // The part already typed is drawn in the accent and the rest in the foreground, so the
        // list shows *why* each row is in it. Byte-sliced by the token's length, which is sound
        // because a sigil is required to be ASCII and the match was made on that basis.
        let matched = &row.sigil[..token.len().min(row.sigil.len())];
        let rest = &row.sigil[matched.len()..];
        let end = painter
            .text(
                egui::pos2(row_rect.min.x + 8.0, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                matched,
                sigil_font.clone(),
                to_color32(theme.accent),
            )
            .max
            .x;
        let end = painter
            .text(
                egui::pos2(end, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                rest,
                sigil_font.clone(),
                to_color32(theme.fg),
            )
            .max
            .x;

        // Label then path, both dimmed and both clipped against the room the sigil left. The
        // palette elides instead; here the row is wide and the sigil is short, so a clip is
        // honest and costs no measurement.
        let tail = format!("{}  {}", row.label, row.path);
        let clip = egui::Rect::from_min_max(
            egui::pos2(end + 10.0, row_rect.min.y),
            egui::pos2(row_rect.max.x - 4.0, row_rect.max.y),
        );
        if clip.width() > 0.0 {
            painter.with_clip_rect(clip).text(
                egui::pos2(clip.min.x, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                tail,
                detail_font.clone(),
                to_color32(theme.palette[8]),
            );
        }
    }

    if rows.len() > MAX_ROWS {
        painter.text(
            egui::pos2(rect.max.x - 8.0, rect.max.y - 2.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("+{}", rows.len() - MAX_ROWS),
            egui::FontId::monospace(10.0),
            to_color32(theme.palette[8]),
        );
    }
}

/// Draw the hint. Dimmed, clipped, and never given a background — it is an annotation on the
/// user's own line, not a widget sitting on top of it.
pub fn draw_hint(
    painter: &egui::Painter,
    anchor: egui::Rect,
    area: egui::Rect,
    text: &str,
    font_size: f32,
    theme: &Theme,
) -> egui::Rect {
    let (pos, room) = hint_pos(anchor, area, anchor.width().max(6.0));
    let clip = egui::Rect::from_min_max(
        egui::pos2(pos.x, pos.y - anchor.height() * 0.5),
        egui::pos2((pos.x + room).min(area.max.x), pos.y + anchor.height() * 0.5),
    );
    if clip.width() <= 0.0 {
        return egui::Rect::NOTHING;
    }
    painter.with_clip_rect(clip).text(
        pos,
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::monospace(font_size),
        to_color32(theme.palette[8]),
    );
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane, and a cursor cell on its last row — which is where a prompt is.
    fn area() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 400.0))
    }

    fn cell(x: f32, y: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(8.0, 16.0))
    }

    #[test]
    fn a_menu_at_the_bottom_of_the_pane_opens_upwards() {
        // The case that is not the exception: the prompt is the last row of the grid, so a menu
        // that only ever opened downwards would be drawn off the bottom of every pane.
        let rect = menu_rect(cell(120.0, 430.0), area(), 5);
        assert!(rect.max.y <= 430.0, "opened downwards off the pane: {rect:?}");
        assert!(area().contains_rect(rect), "{rect:?} escaped {:?}", area());
    }

    #[test]
    fn a_menu_with_room_below_opens_downwards() {
        let anchor = cell(120.0, 60.0);
        let rect = menu_rect(anchor, area(), 5);
        assert!(rect.min.y >= anchor.max.y, "{rect:?}");
    }

    #[test]
    fn a_menu_near_the_right_edge_is_pulled_back_inside_the_pane() {
        // Off the right of a split is over the pane next door, which is somebody else's output.
        let rect = menu_rect(cell(880.0, 430.0), area(), 3);
        assert!(rect.max.x <= area().max.x, "{rect:?}");
        assert!(rect.min.x >= area().min.x, "{rect:?}");
    }

    #[test]
    fn a_menu_taller_than_the_pane_still_starts_inside_it() {
        let rect = menu_rect(cell(120.0, 430.0), area(), MAX_ROWS);
        assert!(rect.min.y >= area().min.y, "{rect:?}");
    }

    #[test]
    fn the_list_is_capped_rather_than_growing_without_limit() {
        assert_eq!(visible_rows(100), MAX_ROWS);
        assert_eq!(visible_rows(3), 3);
    }

    #[test]
    fn a_hint_sits_beside_the_cursor_when_there_is_room() {
        let anchor = cell(120.0, 430.0);
        let (pos, room) = hint_pos(anchor, area(), 8.0);
        assert!(pos.x > anchor.max.x, "{pos:?}");
        assert_eq!(pos.y, anchor.center().y);
        assert!(room >= MIN_WIDTH);
    }

    #[test]
    fn a_hint_with_no_room_drops_to_the_next_line_rather_than_being_squeezed() {
        // Elided to five characters it costs the same glance and answers nothing, so it moves.
        let anchor = cell(870.0, 200.0);
        let (pos, room) = hint_pos(anchor, area(), 8.0);
        assert!(pos.x < anchor.min.x, "{pos:?}");
        assert!(pos.y > anchor.center().y, "{pos:?}");
        assert!(room > MIN_WIDTH, "{room}");
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        let mut state = State::default();
        state.step(-1, 3);
        assert_eq!(state.selected, 2, "up from the first row lands on the last");
        state.step(1, 3);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn stepping_an_empty_list_does_not_panic_or_move() {
        let mut state = State::default();
        state.step(1, 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn narrowing_the_list_puts_the_highlight_back_on_the_first_row() {
        let mut state = State::default();
        state.sync("@n", 3);
        state.step(2, 3);
        assert_eq!(state.selected, 2);
        state.sync("@ny", 1);
        assert_eq!(state.selected, 0, "a different token is a different list");
    }

    #[test]
    fn a_selection_is_clamped_rather_than_pointing_past_the_end() {
        // The same token can lose rows when the config is reloaded under it.
        let mut state = State::default();
        state.sync("@n", 3);
        state.step(2, 3);
        state.sync("@n", 2);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn a_dismissal_lasts_only_as_long_as_the_token_it_was_aimed_at() {
        let mut state = State::default();
        state.sync("@n", 3);
        state.dismiss("@n");
        state.sync("@n", 3);
        assert!(state.is_dismissed("@n"), "Escape has to survive the next frame");
        state.sync("@ny", 1);
        assert!(!state.is_dismissed("@ny"), "typing on brings the menu back");
    }
}
