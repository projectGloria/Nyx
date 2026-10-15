//! Keyboard routing: bindings, leader chords, and the chrome that gets first refusal.
//!
//! The terminal owns the keyboard, so every widget that wants a key has to be asked
//! before the grid is given it — and no chrome widget may hold egui's focus, which is
//! what `release_chrome_focus` is for.

use super::*;

impl NyxApp {
    /// App-level bindings, claimed before the shell sees the key. Returns true if consumed.
    pub(super) fn handle_bindings(&mut self, key: egui::Key, m: egui::Modifiers) -> bool {
        let hit = |b: &Option<Binding>| b.is_some_and(|b| b.matches(key, m));

        if hit(&self.keys.palette) {
            self.palette.open();
            return true;
        }
        if hit(&self.keys.find) {
            if self.search.open {
                self.search.close();
            } else {
                self.search.open();
            }
            // The highlight lives in the snapshot, so opening or closing the bar only shows on
            // screen once the pane is re-captured.
            self.mark_all_dirty();
            return true;
        }
        if hit(&self.keys.settings) {
            self.settings.open(&self.config);
            return true;
        }
        if hit(&self.keys.new_tab) {
            let profile = self.profiles.first().cloned();
            if let Err(err) = self.new_tab(profile.as_ref(), None) {
                self.toast(format!("could not open a tab: {err}"));
            }
            return true;
        }
        if hit(&self.keys.close_pane) {
            if let Some(pane) = self.focused_pane() {
                self.close_pane(pane);
            }
            return true;
        }
        // Same rule as the focus bindings below: a window with one tab has nowhere to step, and
        // Ctrl+Tab is a chord full-screen programs bind. Swallowing it to do nothing is the
        // defect, not the fallthrough.
        for (binding, forward) in [(self.keys.next_tab, true), (self.keys.prev_tab, false)] {
            if hit(&binding) {
                let Some(next) = tab_step(self.tabs.len(), self.active_tab, forward) else {
                    return false;
                };
                self.active_tab = next;
                self.relayout = true;
                return true;
            }
        }
        if hit(&self.keys.split_right) {
            self.split_focused(SplitDir::Horizontal);
            return true;
        }
        if hit(&self.keys.split_down) {
            self.split_focused(SplitDir::Vertical);
            return true;
        }
        if hit(&self.keys.zoom_pane) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.zoomed = match tab.zoomed {
                    Some(_) => None,
                    None => Some(tab.focused),
                };
                self.relayout = true;
            }
            return true;
        }

        let focused = self.tabs.get(self.active_tab).map(|tab| tab.focused);
        match focused.map(|from| directional_focus(&self.keys, key, m, &self.solved.panes, from)) {
            Some(Focus::Move(next)) => {
                self.set_focus(next);
                return true;
            }
            // The key is a focus binding with nowhere to go, so it is the program's.
            Some(Focus::PassThrough) | Some(Focus::NotBound) | None => {}
        }

        false
    }
    pub(super) fn run_chord(&mut self, command: chords::Command) {
        use chords::Command as C;
        match command {
            C::NewTab => {
                let profile = self.profiles.first().cloned();
                if let Err(err) = self.new_tab(profile.as_ref(), None) {
                    self.toast(format!("could not open a tab: {err}"));
                }
            }
            // A leader chord has already taken the keystroke, so there is nothing to hand back
            // here — the step simply does not happen. `tab_step` is shared with the bindings so
            // the wrap arithmetic has one implementation rather than three.
            C::NextTab | C::PrevTab => {
                if let Some(next) =
                    tab_step(self.tabs.len(), self.active_tab, matches!(command, C::NextTab))
                {
                    self.active_tab = next;
                    self.relayout = true;
                }
            }
            C::CloseTab => {
                let panes = self.tabs.get(self.active_tab).map(|t| t.panes());
                if let Some(panes) = panes {
                    for pane in panes {
                        self.shutdown_pane(pane);
                    }
                    let closed = self.active_tab;
                    self.tabs.remove(closed);
                    self.active_tab = crate::layout::active_after_removing(
                        self.active_tab,
                        closed,
                        self.tabs.len(),
                    );
                    self.relayout = true;
                }
            }
            C::SplitRight => self.split_focused(SplitDir::Horizontal),
            C::SplitDown => self.split_focused(SplitDir::Vertical),
            C::ClosePane => {
                if let Some(pane) = self.focused_pane() {
                    self.close_pane(pane);
                }
            }
            C::ZoomPane => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.zoomed = match tab.zoomed {
                        Some(_) => None,
                        None => Some(tab.focused),
                    };
                    self.relayout = true;
                }
            }
            C::FocusLeft | C::FocusRight | C::FocusUp | C::FocusDown => {
                let dir = match command {
                    C::FocusLeft => Direction::Left,
                    C::FocusRight => Direction::Right,
                    C::FocusUp => Direction::Up,
                    _ => Direction::Down,
                };
                let next = self.tabs.get(self.active_tab).and_then(|tab| {
                    crate::layout::focus::neighbour(&self.solved.panes, tab.focused, dir)
                });
                if let Some(next) = next {
                    self.set_focus(next);
                }
            }
            C::Palette => self.palette.open(),
            C::Shortcut(id) => match self.tables.shortcuts.iter().position(|s| s.id == id) {
                Some(index) => self.go_to_shortcut(index),
                None => self.toast(format!("no shortcut named \"{id}\"")),
            },
            // No catch-all, now that the tab arms are unguarded: a new `Command` should be a
            // compile error here rather than a chord that parses, binds, and does nothing.
        }
    }
    /// Palette keys while it is open. It owns the keyboard completely — a keystroke reaching the
    /// shell while a modal is up is how you end up typing your search query into a command line.
    pub(super) fn handle_palette_keys(&mut self, events: &[egui::Event]) {
        let entries = self.palette_entries();
        for event in events {
            let matches = palette::filter(&entries, &self.palette.query).len();
            if crate::ui::edit_query(&mut self.palette.query, event) {
                self.palette.selected = 0;
                continue;
            }
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event {
                match key {
                    egui::Key::Escape => { self.palette.close(); return; },
                    egui::Key::Enter => {
                        if let Some(action) = palette::selected_action(&entries, &self.palette) {
                            self.run_palette_action(action);
                        } else {
                            self.palette.close();
                        }
                        return;
                    }
                    egui::Key::ArrowDown => {
                        if matches > 0 {
                            self.palette.selected = (self.palette.selected + 1) % matches;
                        }
                    }
                    egui::Key::ArrowUp => {
                        if matches > 0 {
                            self.palette.selected = (self.palette.selected + matches - 1) % matches;
                        }
                    }
                    // The binding that opened it closes it, so it toggles. Asked of `self.keys`
                    // rather than hardcoded: this said `Ctrl+P` while the binding was
                    // configurable, so anyone who had rebound the palette opened it with their
                    // own chord and closed it with somebody else's — and the default moving to
                    // Ctrl+Shift+P would have left the stock install in exactly that state.
                    _ if self.keys.palette.is_some_and(|b| b.matches(*key, *modifiers)) => {
                        self.palette.close();
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
    /// Take egui's keyboard focus off the chrome. **The terminal owns the keyboard.**
    ///
    /// Nothing in the grid is an egui widget, so egui's focus lands on whatever chrome is
    /// focusable — every tab button, every tab's close button, the new-tab button and the footer's
    /// shortcut. **Tab** hands focus to the first of them (`Memory::begin_pass`), and egui then
    /// treats **Enter or Space** on a focused clickable widget as a primary click ("Space/enter
    /// works like a primary click for e.g. selected buttons").
    ///
    /// Both of those are ordinary terminal keys, and Tab is how you complete a path. Tab-completing
    /// a directory name handed the keyboard to the tab strip, and the Enter that ran the line
    /// pressed whichever button it had landed on: a new tab, or a tab's close button — which takes
    /// its shells with it — or a saved location, which types a `cd` nobody asked for. The arrow keys
    /// appear in the same match arm but are *directional* focus, relative to an already-focused
    /// widget, so they are not a way in on their own.
    ///
    /// Focus set during a pass only takes effect on the next one, so clearing it here, before any
    /// widget is drawn, is enough: nothing is ever focused at the moment an Enter is read.
    ///
    /// The settings window is the only chrome with real text fields, and it is the only thing that
    /// gets to keep focus. The find bar and the palette handle their own keys in `logic` and
    /// deliberately use no `TextEdit`, so for them egui focus is a liability rather than a need.
    pub(super) fn release_chrome_focus(&self, ctx: &egui::Context) {
        if self.settings.open || egui::Popup::is_any_open(ctx) {
            return;
        }
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
    }
    /// Settings-window keys. egui owns the text fields; this handles only the two things egui
    /// cannot: closing the window, and the binding recorder.
    pub(super) fn handle_settings_keys(&mut self, events: &[egui::Event]) -> bool {
        let mut changed = false;
        for event in events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } = event
            else {
                continue;
            };

            // An armed recorder claims the next keystroke whatever it is — including Ctrl+Shift+T,
            // which is the whole point of recording rather than typing. The exception is a
            // modifier press: Ctrl arrives *before* the key it modifies, so taking it would
            // record every chord as "Ctrl+ControlLeft".
            if self.settings.capture.is_some() {
                if crate::config::keys::is_modifier(*key) {
                    continue;
                }
                if *key == egui::Key::Escape {
                    self.settings.capture = None;
                } else {
                    changed |= self.settings.record(Binding::from_event(*key, *m));
                }
                continue;
            }

            if *key == egui::Key::Escape || self.keys.settings.is_some_and(|b| b.matches(*key, *m))
            {
                self.settings.close();
            }
        }
        changed
    }
    pub(super) fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        let Some(pane) = self.focused_pane() else {
            return false;
        };
        // Do not turn a copy chord into a terminal interrupt while a scan holds the grid.
        if self.search.scanning_pane() == Some(pane) {
            if !self.panes.get(&pane).is_some_and(|state| state.snapshot.selection_active) {
                return false;
            }
            self.toast("Copy is available when the current search finishes");
            return true;
        }
        let text = self
            .panes
            .get(&pane)
            .and_then(|s| select::text(&s.pane.term.lock()));
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.copy_text(ctx, text);
            return true;
        }
        false
    }
    pub(super) fn paste_clipboard(&mut self) {
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        let mode = self
            .panes
            .get(&pane)
            .map(|s| s.snapshot.mode)
            .unwrap_or(TermMode::empty());
        match clipboard.get_text() {
            Ok(text) if !text.is_empty() => {
                let mut bytes = Vec::new();
                input::push_paste(&mut bytes, &text, mode);
                if let Some(state) = self.panes.get_mut(&pane) {
                    state.shadow.feed(Feed::Desync);
                    state.pane.clear_selection();
                    state.pane.write(bytes);
                    state.pane.scroll(Scroll::Bottom);
                }
            }
            Ok(_) => {}
            Err(err) => log::warn!("clipboard read failed: {err}"),
        }
    }
}

impl NyxApp {
    pub(super) fn interrupt_focused(&mut self) {
        if let Some(state) = self.focused_pane().and_then(|p| self.panes.get_mut(&p)) {
            state.shadow.feed(Feed::Desync);
            state.pane.write(vec![0x03]);
            state.pane.scroll(Scroll::Bottom);
        }
    }
}

/// egui may deliver both a copy event and Ctrl+C for the same physical press.
pub(super) fn deduplicate_copy_events(events: Vec<egui::Event>) -> Vec<egui::Event> {
    let has_key = events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::C, pressed: true, modifiers: m, .. } if m.ctrl && !m.alt));
    let mut seen = false;
    events
        .into_iter()
        .filter(|e| {
            if matches!(e, egui::Event::Copy) {
                if has_key || seen {
                    return false;
                }
                seen = true;
            }
            true
        })
        .collect()
}
/// The tab to activate when stepping forwards or backwards, or `None` when there is nowhere to go.
///
/// `None` for a single tab rather than "the tab you are already on", because the caller uses it
/// to decide whether to *consume the key*. Ctrl+Tab in a one-tab window must reach the program.
pub(super) fn tab_step(len: usize, active: usize, forward: bool) -> Option<usize> {
    if len < 2 {
        return None;
    }
    Some(if forward {
        (active + 1) % len
    } else {
        (active + len - 1) % len
    })
}

/// What one of the four directional-focus bindings should do with a keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    /// Not a focus binding at all. Carry on down the chain.
    NotBound,
    /// A focus binding, but there is no pane that way — so the key belongs to the program.
    PassThrough,
    Move(PaneId),
}

/// Whether a directional-focus binding actually has somewhere to go.
///
/// Pure, so the pass-through rule is testable: `handle_bindings` is a method on `NyxApp`, which
/// no test can construct, and that is exactly why this rule was wrong for eight phases without
/// anything noticing.
///
/// **A binding that cannot act must not swallow the key.** The defaults put focus movement on
/// `Alt+Arrow`, and the overwhelmingly common layout is one pane — which has no neighbour in any
/// direction. Claiming the key regardless meant that in a single pane, Alt+Up, Alt+Down, Alt+Left
/// and Alt+Right all did nothing *and* never reached the shell: `ESC [ 1 ; 3 A` is a sequence
/// full-screen programs bind, and the terminal was eating all four of them to perform a
/// no-op. Consuming a key is only defensible when something happened.
pub(super) fn directional_focus(
    keys: &Keys,
    key: egui::Key,
    m: egui::Modifiers,
    panes: &[(PaneId, egui::Rect)],
    from: PaneId,
) -> Focus {
    let directions = [
        (keys.focus_left, Direction::Left),
        (keys.focus_right, Direction::Right),
        (keys.focus_up, Direction::Up),
        (keys.focus_down, Direction::Down),
    ];
    for (binding, dir) in directions {
        if binding.is_some_and(|b| b.matches(key, m)) {
            return match crate::layout::focus::neighbour(panes, from, dir) {
                Some(next) => Focus::Move(next),
                None => Focus::PassThrough,
            };
        }
    }
    Focus::NotBound
}

#[cfg(test)]
mod tests {
    use super::*;
    fn copy_key(shift: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                shift,
                ..Default::default()
            },
        }
    }
    #[test]
    fn copy_events_do_not_duplicate_interrupt_or_copy_keys() {
        for shift in [false, true] {
            for events in [
                vec![egui::Event::Copy, copy_key(shift)],
                vec![copy_key(shift), egui::Event::Copy],
            ] {
                let normalized = deduplicate_copy_events(events);
                assert_eq!(normalized.len(), 1);
                assert_eq!(normalized[0], copy_key(shift));
            }
        }
        assert_eq!(
            deduplicate_copy_events(vec![egui::Event::Copy, egui::Event::Copy]),
            vec![egui::Event::Copy]
        );
    }
    #[test]
    fn text_and_enter_remain_in_arrival_order() {
        let text = egui::Event::Text("@NYX".into());
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        assert_eq!(
            deduplicate_copy_events(vec![text.clone(), enter.clone()]),
            vec![text, enter]
        );
    }

    #[test]
    fn one_tab_has_nowhere_to_step_so_the_key_stays_the_programs() {
        assert_eq!(tab_step(1, 0, true), None);
        assert_eq!(tab_step(1, 0, false), None);
        assert_eq!(tab_step(0, 0, true), None);
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        assert_eq!(tab_step(3, 0, false), Some(2));
        assert_eq!(tab_step(3, 2, true), Some(0));
        assert_eq!(tab_step(3, 1, true), Some(2));
        assert_eq!(tab_step(3, 1, false), Some(0));
    }

    const ALT: egui::Modifiers = egui::Modifiers::ALT;

    fn rect(top: f32, bottom: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(100.0, bottom))
    }

    /// The single-pane case, which is the one nearly every session is in.
    #[test]
    fn a_focus_binding_with_nowhere_to_go_leaves_the_key_to_the_program() {
        let keys = Keys::default();
        let alone = [(1u64, rect(0.0, 100.0))];
        for key in [
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
        ] {
            assert_eq!(
                directional_focus(&keys, key, ALT, &alone, 1),
                Focus::PassThrough,
                "{key:?} was swallowed with only one pane on screen"
            );
        }
    }

    #[test]
    fn a_focus_binding_that_can_move_claims_the_key() {
        let keys = Keys::default();
        let stacked = [(1u64, rect(0.0, 50.0)), (2u64, rect(50.0, 100.0))];
        // From the bottom pane: up reaches the top one, down has nothing below it.
        assert_eq!(
            directional_focus(&keys, egui::Key::ArrowUp, ALT, &stacked, 2),
            Focus::Move(1)
        );
        assert_eq!(
            directional_focus(&keys, egui::Key::ArrowDown, ALT, &stacked, 2),
            Focus::PassThrough
        );
        // A split down the middle does not make the *sideways* keys ours either.
        assert_eq!(
            directional_focus(&keys, egui::Key::ArrowLeft, ALT, &stacked, 2),
            Focus::PassThrough
        );
    }

    #[test]
    fn a_chord_that_is_not_a_focus_binding_is_not_claimed_here() {
        let keys = Keys::default();
        let stacked = [(1u64, rect(0.0, 50.0)), (2u64, rect(50.0, 100.0))];
        // The bare arrow is the shell's history and cursor movement, not pane focus.
        assert_eq!(
            directional_focus(&keys, egui::Key::ArrowUp, egui::Modifiers::NONE, &stacked, 2),
            Focus::NotBound
        );
        assert_eq!(
            directional_focus(&keys, egui::Key::T, egui::Modifiers::ALT, &stacked, 2),
            Focus::NotBound
        );
    }

    /// An unbound direction must not be matched by a keystroke that carries no modifiers either.
    #[test]
    fn an_unbound_direction_claims_nothing() {
        let cfg = crate::config::keys::KeysConfig {
            focus_up: String::new(),
            ..Default::default()
        };
        let (keys, problems) = Keys::from_config(&cfg);
        assert!(problems.is_empty(), "{problems:?}");
        let stacked = [(1u64, rect(0.0, 50.0)), (2u64, rect(50.0, 100.0))];
        assert_eq!(
            directional_focus(&keys, egui::Key::ArrowUp, ALT, &stacked, 2),
            Focus::NotBound
        );
    }
}
