//! The Ctrl+P fuzzy palette over shortcuts, shell profiles and tabs.
//!
//! The matcher is hand-written rather than a dependency: a palette over a few dozen entries needs
//! subsequence matching with a sensible score, which is thirty lines, and `nucleo` would pull a
//! threaded matching engine to rank a list that never gets long.

/// What a palette entry does when chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `cd` the focused pane to a path.
    GoTo { shortcut: usize },
    /// Open a new tab running a profile.
    NewTab { profile: usize },
    /// Switch to a tab by index.
    SwitchTab { tab: usize },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    /// Shown dimmed after the label — the path, or the shell's program.
    pub detail: String,
    pub action: Action,
}

/// Score a query against a candidate, higher is better. `None` means no match.
///
/// Subsequence matching: every character of the query must appear in order. Consecutive matches
/// and matches at a word boundary score higher, so "ft" finds "Free Tibet" ahead of "drafts".
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let c: Vec<char> = candidate.chars().collect();

    let mut qi = 0;
    let mut total = 0;
    let mut last_hit: Option<usize> = None;

    for (i, ch) in c.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        let lower = ch.to_lowercase().next().unwrap_or(*ch);
        if lower != q[qi] {
            continue;
        }

        let mut points = 10;
        // Immediately after the previous match: strongly preferred, so a contiguous run wins.
        if last_hit == Some(i.wrapping_sub(1)) {
            points += 15;
        }
        // Start of the string, or start of a word.
        let boundary = i == 0
            || matches!(c[i - 1], ' ' | '-' | '_' | '/' | '\\' | '.')
            || (c[i - 1].is_lowercase() && ch.is_uppercase());
        if boundary {
            points += 12;
        }
        // Earlier matches are worth slightly more, so short labels beat long ones.
        points -= (i as i32).min(20) / 4;

        total += points;
        last_hit = Some(i);
        qi += 1;
    }

    (qi == q.len()).then_some(total)
}

/// Rank entries against a query, best first. Entries that do not match are dropped.
pub fn filter(entries: &[Entry], query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            // Match against the label first; fall back to the detail so a path fragment finds a
            // shortcut whose label does not contain it.
            let best = score(query, &e.label)
                .map(|s| s + 5)
                .into_iter()
                .chain(score(query, &e.detail))
                .max()?;
            Some((best, i))
        })
        .collect();

    // Stable by original order within equal scores, so the list does not jitter as you type.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// The palette's own state, owned by the app.
#[derive(Debug, Default)]
pub struct State {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

impl State {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }
}

/// Draw the palette over the terminal. Returns a chosen action.
pub fn show(
    ui: &mut egui::Ui,
    area: egui::Rect,
    state: &mut State,
    entries: &[Entry],
    theme: &crate::theme::Theme,
) -> Option<Action> {
    use crate::theme::visuals::to_color32;

    let ranked = filter(entries, &state.query);
    state.selected = state.selected.min(ranked.len().saturating_sub(1));

    let width = (area.width() * 0.6).clamp(320.0, 720.0);
    let row_h = 24.0;
    let rows = ranked.len().min(12);
    let height = 38.0 + rows as f32 * row_h + 8.0;

    let rect = egui::Rect::from_min_size(
        egui::pos2(area.center().x - width * 0.5, area.min.y + area.height() * 0.12),
        egui::vec2(width, height),
    );

    let painter = ui.painter();
    // A scrim, so the palette reads as modal rather than as text floating over a terminal.
    painter.rect_filled(area, 0.0, egui::Color32::from_black_alpha(120));
    painter.rect_filled(rect, 6.0, to_color32(theme.surface));
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, to_color32(theme.accent)),
        egui::StrokeKind::Inside,
    );

    let query_text = if state.query.is_empty() {
        "type to search…".to_string()
    } else {
        state.query.clone()
    };
    painter.text(
        egui::pos2(rect.min.x + 12.0, rect.min.y + 10.0),
        egui::Align2::LEFT_TOP,
        format!("> {query_text}"),
        egui::FontId::monospace(14.0),
        to_color32(if state.query.is_empty() { theme.palette[8] } else { theme.fg }),
    );

    let mut chosen = None;
    let window = super::selection_window(state.selected, ranked.len(), rows);
    for (row, &index) in ranked[window.clone()].iter().enumerate() {
        let entry = &entries[index];
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 4.0, rect.min.y + 36.0 + row as f32 * row_h),
            egui::vec2(rect.width() - 8.0, row_h),
        );
        let is_selected = window.start + row == state.selected;
        if is_selected {
            ui.painter().rect_filled(row_rect, 4.0, to_color32(theme.selection_bg));
        }

        let response =
            ui.interact(row_rect, egui::Id::new(("nyx.palette.row", index)), egui::Sense::click());
        if response.clicked() {
            chosen = Some(entry.action.clone());
        }

        let label_font = egui::FontId::monospace(13.0);
        let detail_font = egui::FontId::monospace(11.0);
        let label_end = ui
            .painter()
            .text(
                egui::pos2(row_rect.min.x + 8.0, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &entry.label,
                label_font,
                to_color32(if is_selected { theme.fg } else { theme.palette[7] }),
            )
            .max
            .x;

        // The detail is right-aligned and the label is left-aligned, so nothing stops the two
        // meeting in the middle: `painter.text` takes no width and draws what it is given. A
        // profile whose program is a full `C:\WINDOWS\System32\WindowsPowerShell\v1.0\...` path
        // was drawn straight through its own label, and two strings on top of each other are
        // less readable than either one alone. Elide against the space the label actually left.
        let room = row_rect.max.x - 8.0 - label_end - GAP;
        let measure = |text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_string(), detail_font.clone(), egui::Color32::WHITE)
                .rect
                .width()
        };
        if let Some(detail) = elide_front(&entry.detail, room, measure) {
            ui.painter().text(
                egui::pos2(row_rect.max.x - 8.0, row_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                detail,
                detail_font,
                to_color32(theme.palette[8]),
            );
        }
    }

    if ranked.is_empty() {
        ui.painter().text(
            egui::pos2(rect.min.x + 12.0, rect.min.y + 44.0),
            egui::Align2::LEFT_TOP,
            "no matches",
            egui::FontId::monospace(13.0),
            to_color32(theme.palette[8]),
        );
    }

    chosen
}

/// The gap kept between a row's label and its detail, so the two never touch even once the
/// detail has been elided down to fit.
const GAP: f32 = 12.0;

/// `detail` shortened from the *front* to fit `room`, or `None` when there is no room for it.
///
/// From the front because every detail here is a path or a program, and the tail is the half that
/// tells two of them apart: `…\v1.0\powershell.exe` says which PowerShell, where
/// `C:\WINDOWS\System32\Windows…` says only that it is somewhere under System32. That is the
/// opposite of `app::trim_title`, which keeps the head — a window title's subject comes first.
///
/// `width` is passed in rather than measured here so that the rule can be tested at all. egui's
/// test harness loads no font data and reports every string as zero pixels wide, so a test that
/// asked a `Ui` to measure would have watched every input "fit" and passed without exercising a
/// single line of this.
fn elide_front(detail: &str, room: f32, width: impl Fn(&str) -> f32) -> Option<String> {
    if detail.is_empty() || room <= 0.0 {
        return None;
    }
    let full = width(detail);
    if full <= room {
        return Some(detail.to_string());
    }

    // Monospace, so one proportional estimate lands on or beside the answer. The loop is what
    // keeps it correct when a fallback face makes that assumption false.
    let chars: Vec<char> = detail.chars().collect();
    let mut keep = ((room / full) * chars.len() as f32).floor() as usize;
    keep = keep.min(chars.len());
    loop {
        let candidate: String =
            std::iter::once('…').chain(chars[chars.len() - keep..].iter().copied()).collect();
        if width(&candidate) <= room {
            return Some(candidate);
        }
        if keep == 0 {
            return None;
        }
        keep -= 1;
    }
}

/// The entry the selection is currently on, for Enter.
pub fn selected_action(entries: &[Entry], state: &State) -> Option<Action> {
    let ranked = filter(entries, &state.query);
    ranked.get(state.selected).map(|&i| entries[i].action.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, detail: &str) -> Entry {
        Entry {
            label: label.into(),
            detail: detail.into(),
            action: Action::SwitchTab { tab: 0 },
        }
    }

    #[test]
    fn an_exact_prefix_matches() {
        assert!(score("free", "Free Tibet").is_some());
    }

    /// A stand-in for the font: every character is 7 px wide, which is what a monospace face is.
    /// The real one comes from the painter in `show`; what is under test is the rule, not egui.
    fn px(text: &str) -> f32 {
        text.chars().count() as f32 * 7.0
    }

    fn elided(detail: &str, room: f32) -> Option<String> {
        elide_front(detail, room, px)
    }

    const PWSH: &str = r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe";

    #[test]
    fn a_detail_that_fits_is_left_alone() {
        assert_eq!(elided(r"C:\Users\Nihil", 400.0).as_deref(), Some(r"C:\Users\Nihil"));
    }

    #[test]
    fn a_long_detail_is_cut_to_the_room_it_was_given() {
        // The defect this exists for: the detail is right-aligned and the label is left-aligned,
        // so an un-elided path is drawn straight back through the label rather than past the edge
        // of the row. Nothing is clipped and nothing overflows — the two strings simply overlap,
        // which is why it survived until someone opened the window and read it.
        let room = 120.0;
        let got = elided(PWSH, room).expect("120 px is room for something");
        assert!(px(&got) <= room, "elided to {} px, wider than the {room} px available", px(&got));
        assert!(got.starts_with('…'), "an elided detail says so: {got}");
        assert!(PWSH.ends_with(got.trim_start_matches('…')), "the tail is what was kept: {got}");
    }

    #[test]
    fn the_tail_is_what_survives_because_the_head_does_not_distinguish() {
        // Both stock shells live under System32, so a head-elided detail would render the two
        // identically and the palette would stop answering the question it exists to answer.
        let pwsh = elided(PWSH, 140.0).unwrap();
        let cmd = elided(r"C:\WINDOWS\System32\cmd.exe", 140.0).unwrap();
        assert_ne!(pwsh, cmd);
        assert!(pwsh.ends_with("powershell.exe"), "{pwsh}");
        assert!(cmd.ends_with("cmd.exe"), "{cmd}");

        // And when the room is genuinely tiny it is still the tail that is kept, even though what
        // is left no longer names the program: a suffix of the right answer beats a prefix of a
        // directory every other entry shares.
        assert!(PWSH.ends_with(elided(PWSH, 90.0).unwrap().trim_start_matches('…')));
    }

    #[test]
    fn the_result_never_exceeds_the_room_at_any_width() {
        // The estimate that opens the loop is proportional, so it can land either side of the
        // answer; this is the axis that says the loop closes the gap from both directions.
        for room in 1..200 {
            let room = room as f32;
            if let Some(got) = elided(PWSH, room) {
                assert!(px(&got) <= room, "{room} px produced {} px: {got}", px(&got));
            }
        }
    }

    #[test]
    fn no_room_means_nothing_is_drawn_rather_than_something_overlapping() {
        assert_eq!(elided(r"C:\WINDOWS\System32\cmd.exe", 0.0), None);
        assert_eq!(elided(r"C:\WINDOWS\System32\cmd.exe", -20.0), None);
    }

    #[test]
    fn a_subsequence_matches_but_a_missing_char_does_not() {
        assert!(score("ft", "Free Tibet").is_some());
        assert!(score("fz", "Free Tibet").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(score("FREE", "Free Tibet").is_some());
        assert!(score("free", "FREE TIBET").is_some());
    }

    #[test]
    fn initials_beat_a_scattered_match() {
        // "ft" as the initials of two words should rank above the same letters buried in a word.
        let initials = score("ft", "Free Tibet").unwrap();
        let scattered = score("ft", "shafted").unwrap();
        assert!(initials > scattered, "{initials} !> {scattered}");
    }

    #[test]
    fn a_contiguous_run_beats_a_gapped_one() {
        let contiguous = score("tib", "Tibet").unwrap();
        let gapped = score("tib", "Tall Icy Boulder").unwrap();
        assert!(contiguous > gapped, "{contiguous} !> {gapped}");
    }

    #[test]
    fn an_empty_query_keeps_everything_in_order() {
        let entries = [entry("a", ""), entry("b", ""), entry("c", "")];
        assert_eq!(filter(&entries, ""), vec![0, 1, 2]);
    }

    #[test]
    fn the_label_outranks_the_detail() {
        let entries = [
            entry("Downloads", r"C:\Users\Nihil\Downloads"),
            entry("Project", r"C:\dev\downloads-mirror"),
        ];
        let ranked = filter(&entries, "downloads");
        assert_eq!(ranked.first(), Some(&0));
    }

    #[test]
    fn a_path_fragment_finds_a_shortcut_whose_label_does_not_contain_it() {
        let entries = [entry("Work", r"C:\Users\Nihil\Desktop\Free Tibet")];
        assert_eq!(filter(&entries, "tibet"), vec![0]);
    }

    #[test]
    fn non_matching_entries_are_dropped() {
        let entries = [entry("alpha", "one"), entry("beta", "two")];
        assert_eq!(filter(&entries, "zzz"), Vec::<usize>::new());
    }
}
