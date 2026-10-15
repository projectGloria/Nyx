//! Sigils at the prompt: the completion menu, the inline hint, and `@shortcut` + Enter.
//!
//! All three read the same thing — `Shadow`'s model of the line — and all three are gated on
//! `Shadow::may_hint` rather than `may_expand`, because describing a line and rewriting it are
//! different rights. The shell's own integration holds the second and not the first.
//!
//! The keys taken here are taken narrowly. **The terminal owns the keyboard**, so Tab, Escape and
//! the two arrows are claimed only while a menu is actually on screen, and Enter only while a menu
//! is on screen or the line is *nothing but* a sigil. Outside those states every one of them
//! reaches the shell unchanged, which is what keeps Up as history recall and Tab as path
//! completion.

use super::*;

use crate::expand::table::Shortcut;
use crate::expand::{complete, hint};
use crate::ui::complete as menu_ui;

/// Everything to draw at the cursor this frame.
pub(super) struct Overlay {
    /// The cursor cell, in points.
    anchor: egui::Rect,
    /// The pane the cursor is in, which is what the menu is clamped against.
    area: egui::Rect,
    token: String,
    rows: Vec<menu_ui::Row>,
    hint: Option<String>,
}

impl NyxApp {
    /// What the focused pane's line means, as geometry and strings ready to draw.
    ///
    /// `&mut self` for one reason: the cursor snapshot is `None` while the cursor is blinked off,
    /// and a hint that blinks along with it is unreadable. The last position the cursor had is
    /// remembered here and used for those frames — sound because the cursor does not move between
    /// blink phases, and a frame that did move it carried a visible cursor with it.
    pub(super) fn sigil_overlay(&mut self) -> Option<Overlay> {
        // An unfocused window is not being typed at, and a scrolled-back view is not showing the
        // line the model describes — the cursor on screen is somewhere in the history.
        if !self.focused {
            return None;
        }
        let pane = self.focused_pane()?;
        let state = self.panes.get(&pane)?;
        if state.snapshot.display_offset != 0 {
            return None;
        }
        let mode = state.snapshot.mode;
        if state.integration.child_running || !state.shadow.may_hint(mode) || driven(mode) {
            return None;
        }
        let line = state.shadow.line()?.to_string();

        // Both switchable, and separately: the hint claims no keys, so somebody who turned the
        // menu off to keep Tab and the arrows has not asked to stop being told what `@nyx` means.
        let hint = self
            .config
            .expand
            .hint
            .then(|| hint::for_line(&line, &self.tables).map(|h| h.text()))
            .flatten();
        let menu = self
            .config
            .expand
            .completion
            .then(|| complete::menu(&line, &self.tables))
            .flatten();

        let (token, rows) = match menu {
            Some(menu) if !self.completion.is_dismissed(&menu.token) => {
                self.completion.sync(&menu.token, menu.hits.len());
                let rows = menu
                    .hits
                    .iter()
                    .filter_map(|&i| self.tables.shortcuts.get(i))
                    .map(|s| menu_ui::Row {
                        sigil: s.sigil.clone(),
                        label: s.label.clone(),
                        path: s.path.clone(),
                    })
                    .collect();
                (menu.token, rows)
            }
            _ => (String::new(), Vec::new()),
        };

        if hint.is_none() && rows.is_empty() {
            return None;
        }

        let area = self.solved.rect_of(pane)?;
        let anchor = self.focused_cursor_rect(pane).or(self.completion.anchor)?;
        self.completion.anchor = Some(anchor);

        Some(Overlay {
            anchor,
            area,
            token,
            rows,
            hint,
        })
    }

    /// Draw it, and record the menu's rectangle for the next frame's pointer handling.
    ///
    /// Recorded rather than returned because `handle_mouse` runs before the panes are painted:
    /// a rectangle one frame old is exact for a menu that has not moved, and a menu that just
    /// opened is one frame late to block a click nobody has made yet.
    pub(super) fn draw_sigil_overlay(&mut self, ui: &egui::Ui, overlay: &Overlay) {
        let painter = ui.painter();
        if let Some(text) = &overlay.hint {
            menu_ui::draw_hint(
                painter,
                overlay.anchor,
                overlay.area,
                text,
                self.metrics.font_size / self.scale,
                &self.theme,
            );
        }

        self.completion.blocked = None;
        if overlay.rows.is_empty() {
            return;
        }
        let rect = menu_ui::menu_rect(
            overlay.anchor,
            overlay.area,
            menu_ui::visible_rows(overlay.rows.len()),
        );
        menu_ui::draw_menu(
            painter,
            rect,
            &overlay.rows,
            &overlay.token,
            self.completion.selected,
            &self.theme,
        );
        self.completion.blocked = Some(rect);
    }

    /// Claim a keystroke for the menu or for `@shortcut` + Enter. True means the shell never sees
    /// it.
    pub(super) fn handle_sigil_key(&mut self, key: egui::Key, m: egui::Modifiers) -> bool {
        // Turned off, this layer takes nothing at all — which is the whole point of the switch.
        // Checked before anything else so there is no state to get out of step: no menu is
        // synced, nothing is dismissed, and Tab, Escape, Enter and the arrows go where they
        // would have gone if this file did not exist.
        if !self.config.expand.completion {
            return false;
        }
        // Only the bare key. A chord means something to the shell or to a binding, and Shift+Enter
        // in particular is how several line editors ask for a continuation line.
        if m.any() {
            return false;
        }
        let Some(pane) = self.focused_pane() else {
            return false;
        };
        // Read out of the pane and end the borrow: everything below needs `&mut self`.
        let (line, mode) = match self.panes.get(&pane) {
            Some(state)
                if !state.integration.child_running
                    && state.shadow.may_hint(state.snapshot.mode)
                    && !driven(state.snapshot.mode) =>
            {
                match state.shadow.line() {
                    Some(line) => (line.to_string(), state.snapshot.mode),
                    None => return false,
                }
            }
            _ => return false,
        };

        let menu = complete::menu(&line, &self.tables)
            .filter(|menu| !self.completion.is_dismissed(&menu.token));
        if let Some(menu) = &menu {
            self.completion.sync(&menu.token, menu.hits.len());
        }

        // Asked of `Shadow` rather than re-derived here, so there stays one implementation of
        // what "a line that is nothing but a sigil" means. `decide` orders the rules; it does
        // not own them.
        let whole_line = self
            .panes
            .get(&pane)
            .and_then(|s| s.shadow.whole_line_sigil(&self.tables, mode));

        match decide(
            self.config.expand.completion,
            key,
            m,
            &line,
            menu.as_ref(),
            self.completion.selected,
            whole_line,
            &self.tables,
        ) {
            Action::Ignore => false,
            Action::Dismiss => {
                if let Some(menu) = &menu {
                    self.completion.dismiss(&menu.token);
                }
                true
            }
            Action::Step(by) => {
                if let Some(menu) = &menu {
                    self.completion.step(by, menu.hits.len());
                }
                true
            }
            Action::Accept(expansion) => {
                self.accept_completion(pane, &expansion);
                true
            }
            Action::OpenTab(index) => {
                self.open_shortcut_tab(index);
                true
            }
        }
    }

    /// Put an accepted completion on the input line.
    ///
    /// The same bytes an expansion sends, for the same reason: what the shell echoed is what a
    /// backspace erases. Accepting a completion is an edit to the input line, not a new kind of
    /// thing — and Tab and Enter accept the same one, so there is one place that does it.
    fn accept_completion(&mut self, pane: PaneId, expansion: &crate::expand::line::Expansion) {
        if let Some(state) = self.panes.get_mut(&pane) {
            state.pane.write(shadow::bytes_for(expansion));
            state.pane.scroll(Scroll::Bottom);
            state.shadow.apply(expansion);
        }
    }

    /// Open a new tab at a shortcut's path, named after it.
    pub(super) fn open_shortcut_tab(&mut self, index: usize) {
        let Some(shortcut) = self.tables.shortcuts.get(index).cloned() else {
            return;
        };
        let cwd = std::path::PathBuf::from(&shortcut.path);
        if !cwd.is_dir() {
            self.toast(format!("Saved location does not exist: {}", shortcut.path));
            return;
        }

        // Take the line back off the prompt being left behind. The Enter that got us here never
        // reaches the shell, so without this the old pane keeps `@nyx` sitting at its prompt with
        // nothing to say it was consumed — and the next Enter runs it as a command.
        self.clear_input_line();

        let profile = self.profiles.first().cloned();
        match self.new_tab(profile.as_ref(), Some(cwd)) {
            Ok(()) => self.name_tab_after(&shortcut),
            Err(err) => self.toast(format!("Could not open {}: {err}", shortcut.label)),
        }
    }

    /// Give the active tab a shortcut's name and icon, and pin them.
    ///
    /// Pinned because the shell sets its own window title within a second of starting — cmd.exe
    /// announces its working directory — and an unpinned name is overwritten by it before the
    /// user has finished reading it. The shell's title is not discarded: it becomes the tooltip,
    /// which is where a path belongs.
    pub(super) fn name_tab_after(&mut self, shortcut: &Shortcut) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.pinned = Some(shortcut.label.clone());
            tab.icon = shortcut.icon.clone();
            tab.icon_color = shortcut.color.clone();
        }
    }

    /// Erase whatever is on the focused pane's input line, as far as the model knows it.
    fn clear_input_line(&mut self) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let Some(state) = self.panes.get_mut(&pane) else {
            return;
        };
        let Some(count) = state
            .shadow
            .line()
            .map(|l| l.chars().count())
            .filter(|n| *n > 0)
        else {
            return;
        };
        let erase = crate::expand::line::Expansion {
            backspaces: count,
            text: String::new(),
        };
        state.pane.write(shadow::bytes_for(&erase));
        state.shadow.apply(&erase);
    }

    /// What a tab's tooltip says: its name, and the path behind it.
    ///
    /// The path is the question a tooltip is being asked. The name is already on the tab — and
    /// the old tooltip showed only that, so hovering a tab told you what you could already read.
    pub(super) fn tab_tooltip(&self, tab: &Tab) -> String {
        let detail = self.panes.get(&tab.focused).and_then(|state| {
            state
                .integration
                .cwd
                .clone()
                // No integration and no `--cwd` means nothing has told us a directory, and the
                // shell's own title is the next best thing: cmd.exe puts its path there.
                .or_else(|| Some(state.pane.title.clone()))
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty() && d != tab.name())
        });
        match detail {
            Some(detail) => format!("{}\n{detail}", tab.name()),
            None => tab.name().to_string(),
        }
    }
}

/// Whether a program — rather than a shell prompt — is driving the keyboard.
///
/// A third answer to the question `child_running` can only answer under shell integration, and
/// unlike that one it needs no integration at all: **a prompt never turns the kitty keyboard
/// protocol on.** Only a program that wants every keystroke does, so a pane in that mode is a
/// pane where the line model is describing something that is not a command line, and where `@`
/// belongs to whatever asked for it. That closes the case this layer could not previously see —
/// an inline program in a plain cmd.exe, WSL or ssh pane — for every program modern enough to
/// ask, which is the same set of programs most likely to give `@` a meaning of its own.
///
/// The alternate screen is the older half of the same signal and is already handled by
/// `Shadow::may_hint`; this catches the inline programs that never switch screens.
fn driven(mode: TermMode) -> bool {
    crate::term::kitty::active(mode)
}

/// What a keystroke means to the sigil layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Action {
    /// The terminal owns this key. Nothing here touches it.
    Ignore,
    Dismiss,
    /// Move the highlight, wrapping.
    Step(isize),
    /// Put a whole sigil on the input line in place of the partial token.
    Accept(crate::expand::line::Expansion),
    /// Open a tab at this index into `Tables::shortcuts`.
    OpenTab(usize),
}

/// Which of those a key means, given the state the frame can see.
///
/// Pure, so the ordering is testable — and the ordering is the whole of the risk here. Every
/// rule this composes is tested on its own already (`complete::menu`, `complete::accept`,
/// `Shadow::whole_line_sigil`); what had nothing under it was the decision about which of them
/// wins, which is the part that decides whether Up still recalls history and whether Enter still
/// runs a command. `navigation_refusal` is the same shape for the same reason.
///
/// `enabled` is checked the same way and for the same reason: the guard at the top of
/// `handle_sigil_key` is what keeps a switched-off layer from touching `completion` state at
/// all, and this is the rule that says a switched-off layer claims nothing.
///
/// The argument list is long on purpose, and gathering it into a struct would undo the point:
/// every one of these is a piece of frame state that would otherwise be read off `self`, and
/// spelling them out is what makes the function callable from a test at all.
#[allow(clippy::too_many_arguments)]
fn decide(
    enabled: bool,
    key: egui::Key,
    modifiers: egui::Modifiers,
    line: &str,
    menu: Option<&complete::Menu>,
    selected: usize,
    whole_line: Option<usize>,
    tables: &crate::expand::table::Tables,
) -> Action {
    if !enabled {
        return Action::Ignore;
    }
    // A chord means something to the shell or to a binding, and Shift+Enter in particular is how
    // several line editors ask for a continuation line.
    if modifiers.any() {
        return Action::Ignore;
    }

    if key == egui::Key::Enter {
        // A menu on screen is a choice being offered, and Enter is how a menu is answered — so it
        // is taken here rather than left to the line as typed. A bare `@` is a token no table
        // holds, so falling through handed PowerShell a lone `@` and the user read `Unrecognized
        // token in source text` instead of opening the highlighted row.
        //
        // It outranks `whole_line` for the same reason: with `@nyx` highlighted under `@nyxlong`,
        // the line spells one shortcut and the user has pointed at another.
        if let Some(menu) = menu {
            // The token being the whole line makes this the same choice `@shortcut` + Enter
            // makes, so make it: open the location. Anything else is a command that merely
            // mentions a shortcut, and finishing the word is all Enter may do to it.
            if line.trim() == menu.token {
                if let Some(index) = menu.hits.get(selected).copied() {
                    return Action::OpenTab(index);
                }
            } else if let Some(expansion) = complete::accept(menu, tables, selected) {
                return Action::Accept(expansion);
            }
        }
        return match whole_line {
            Some(index) => Action::OpenTab(index),
            None => Action::Ignore,
        };
    }

    // No menu, no claim. This is what keeps Up as history recall and Tab as path completion.
    let Some(menu) = menu else { return Action::Ignore };

    match key {
        egui::Key::Escape => Action::Dismiss,
        egui::Key::ArrowDown => Action::Step(1),
        egui::Key::ArrowUp => Action::Step(-1),
        egui::Key::Tab => match complete::accept(menu, tables, selected) {
            Some(expansion) => Action::Accept(expansion),
            None => Action::Ignore,
        },
        _ => Action::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{AliasMap, ShortcutConfig};
    use crate::expand::shadow::{Feed, Shadow};
    use crate::expand::table::Tables;
    use alacritty_terminal::term::TermMode;

    fn cfg(id: &str, sigil: &str) -> ShortcutConfig {
        ShortcutConfig {
            id: id.into(),
            label: id.into(),
            sigil: sigil.into(),
            path: format!(r"C:\{id}"),
            icon: String::new(),
            color: None,
            order: 0,
        }
    }

    fn tables() -> Tables {
        Tables::from_config(
            &[cfg("nyx", "@nyx"), cfg("nyxlong", "@nyxlong"), cfg("tibet", "@tibet")],
            &AliasMap::new(),
        )
        .0
    }
    /// A program that asked for every keystroke is not a shell prompt, whatever the line model
    /// thinks it is holding.
    #[test]
    fn a_pane_driven_by_a_program_is_not_a_prompt() {
        assert!(driven(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(driven(TermMode::REPORT_ALL_KEYS_AS_ESC));
        assert!(!driven(TermMode::empty()));
        // Not a guess about every mode a program might set: an ordinary prompt with bracketed
        // paste and mouse reporting on is still a prompt.
        assert!(!driven(TermMode::BRACKETED_PASTE | TermMode::MOUSE_MODE));
    }

    /// With the layer switched off, every key it could have claimed goes to the program.
    #[test]
    fn a_switched_off_completion_layer_claims_nothing_at_all() {
        let t = tables();
        let menu = complete::menu("@n", &t);
        assert!(menu.is_some(), "the menu would otherwise be up for this line");

        for key in [
            egui::Key::Enter,
            egui::Key::Tab,
            egui::Key::Escape,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ] {
            assert_eq!(
                decide(false, key, egui::Modifiers::NONE, "@n", menu.as_ref(), 0, Some(0), &t),
                Action::Ignore,
                "{key:?} was still claimed with the layer off"
            );
            // And the same key is claimed with it on, or this test would pass for the wrong
            // reason on a day someone breaks the menu.
            assert_ne!(
                decide(true, key, egui::Modifiers::NONE, "@n", menu.as_ref(), 0, Some(0), &t),
                Action::Ignore,
                "{key:?} is not claimed even with the layer on"
            );
        }
    }


    /// The frame's own sequence, minus the side effects: model the line, then ask.
    fn act(typed: &str, key: egui::Key, selected: usize) -> Action {
        let t = tables();
        let mut shadow = Shadow::new();
        for c in typed.chars() {
            shadow.feed(Feed::Char(c));
        }
        let line = shadow.line().unwrap().to_string();
        let menu = complete::menu(&line, &t);
        let whole_line = shadow.whole_line_sigil(&t, TermMode::empty());
        decide(
            true,
            key,
            egui::Modifiers::NONE,
            &line,
            menu.as_ref(),
            selected,
            whole_line,
            &t,
        )
    }

    #[test]
    fn nothing_is_claimed_when_no_menu_is_on_screen() {
        // The regression this exists for: the terminal owns the keyboard, Up is history recall at
        // every prompt, and Tab completes a path. A layer that claimed them unconditionally would
        // make both stop working, which is the shape of the bug a real session already found once.
        for key in [
            egui::Key::Tab,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::Escape,
        ] {
            assert_eq!(act("git commit", key, 0), Action::Ignore, "{key:?}");
        }
    }

    #[test]
    fn a_chord_is_never_claimed_even_with_a_menu_up() {
        let t = tables();
        let menu = complete::menu("@n", &t);
        for m in [
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
            egui::Modifiers::SHIFT,
        ] {
            for key in [egui::Key::Enter, egui::Key::Tab, egui::Key::ArrowUp] {
                assert_eq!(
                    decide(true, key, m, "@n", menu.as_ref(), 0, None, &t),
                    Action::Ignore,
                    "{key:?} with {m:?}"
                );
            }
        }
    }

    #[test]
    fn the_menu_keys_are_claimed_only_while_it_is_up() {
        assert_eq!(act("@n", egui::Key::Escape, 0), Action::Dismiss);
        assert_eq!(act("@n", egui::Key::ArrowDown, 0), Action::Step(1));
        assert_eq!(act("@n", egui::Key::ArrowUp, 0), Action::Step(-1));
    }

    #[test]
    fn tab_accepts_the_highlighted_row_and_erases_exactly_what_was_typed() {
        let Action::Accept(expansion) = act("cd @n", egui::Key::Tab, 1) else {
            panic!("Tab did not accept the completion");
        };
        assert_eq!(expansion.backspaces, 2, "`@n` is two characters");
        assert_eq!(expansion.text, "@nyxlong");
    }

    #[test]
    fn enter_on_a_line_that_merely_mentions_a_shortcut_only_finishes_the_word() {
        // `cd @n` has to keep being a `cd`. Opening a tab here is the difference between "Enter
        // completes the word" and "Enter stopped running commands".
        let Action::Accept(expansion) = act("cd @n", egui::Key::Enter, 0) else {
            panic!("Enter did not accept the completion");
        };
        assert_eq!(expansion.text, "@nyx");
    }

    #[test]
    fn enter_on_a_bare_sigil_opens_the_highlighted_row_rather_than_the_line_as_typed() {
        // `@nyx` spells one shortcut while the highlight sits on `@nyxlong`; the user is pointing
        // at the second. The menu outranks `whole_line_sigil`, and getting that backwards opens
        // the wrong location with nothing on screen to explain why.
        assert_eq!(act("@nyx", egui::Key::Enter, 1), Action::OpenTab(1));
        assert_eq!(act("@nyx", egui::Key::Enter, 0), Action::OpenTab(0));
    }

    #[test]
    fn enter_on_a_bare_sigil_with_no_menu_still_opens_its_tab() {
        // `@tibet` is an exact single match, so `complete::menu` closes the list — and the
        // whole-line rule is then the only thing left to answer Enter.
        assert_eq!(act("@tibet", egui::Key::Enter, 0), Action::OpenTab(2));
        assert_eq!(act("  @tibet ", egui::Key::Enter, 0), Action::OpenTab(2));
    }

    #[test]
    fn enter_on_an_ordinary_command_reaches_the_shell() {
        assert_eq!(act("git commit", egui::Key::Enter, 0), Action::Ignore);
        assert_eq!(act("", egui::Key::Enter, 0), Action::Ignore);
    }

    #[test]
    fn a_bare_sigil_character_is_answered_by_the_menu_and_not_by_the_line() {
        // The defect this pins: `@` is a token no table holds, so falling through to the shell
        // ran a lone `@` and PowerShell replied `Unrecognized token in source text`.
        assert_eq!(act("@", egui::Key::Enter, 0), Action::OpenTab(0));
        assert_eq!(act("@", egui::Key::Enter, 2), Action::OpenTab(2));
    }

    #[test]
    fn a_selection_past_the_end_claims_nothing_rather_than_opening_the_wrong_tab() {
        // `completion.selected` is clamped by `sync`, but `decide` must not depend on that having
        // happened: guessing a row here opens somebody else's location.
        assert_eq!(act("@n", egui::Key::Tab, 99), Action::Ignore);
        assert_eq!(act("@n", egui::Key::Enter, 99), Action::Ignore);
    }

    #[test]
    fn an_unclaimed_key_falls_through_even_with_a_menu_up() {
        for key in [egui::Key::A, egui::Key::Backspace, egui::Key::ArrowLeft] {
            assert_eq!(act("@n", key, 0), Action::Ignore, "{key:?}");
        }
    }
}
