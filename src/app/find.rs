//! The find bar: its keys, its debounce, and the thread its walk runs on.
//!
//! The walk holds a pane's lock for as long as it takes, so it is not on this thread;
//! `Search::request` and `Search::land` in `term::search` are where the sequencing is
//! decided and tested.

use super::*;

impl NyxApp {
    /// Find-bar keys. Like the palette it owns the keyboard: a keystroke reaching the shell while
    /// a search box is up is how you end up running your query as a command.
    pub(super) fn handle_find_keys(&mut self, events: &[egui::Event]) {
        let mut query = self.search.query.clone();
        let mut query_changed = false;
        let mut step: Option<SearchDir> = None;
        let mut close = false;
        // Our own chords also arrive as `Text` on some layouts; letting that through would append
        // the chord letter to the query after acting on it.
        let mut chord_text: Option<char> = None;
        for event in events {
            if let egui::Event::Text(text) = event {
                if chord_text.take().is_some_and(|c| text.eq_ignore_ascii_case(&c.to_string())) { continue; }
            }
            if crate::ui::edit_query(&mut query, event) {
                chord_text = None;
                query_changed = true;
                continue;
            }
            if let egui::Event::Key { key, pressed: true, modifiers: m, .. } = event {
                chord_text = None;
                match key {
                    egui::Key::Escape => { close = true; break; },
                    egui::Key::Enter | egui::Key::F3 => {
                        step = Some(if m.shift { SearchDir::Left } else { SearchDir::Right });
                    },
                    egui::Key::R if m.alt => {
                        self.search.toggle_regex();
                        query_changed = true;
                        chord_text = Some('r');
                    },
                    _ if self.keys.find.is_some_and(|b| b.matches(*key, *m)) => { close = true; break; },
                    _ => {},
                }
            }
        }

        if close {
            self.search.close();
            self.search_at = None;
            self.mark_all_dirty();
            return;
        }

        if query_changed {
            self.search.set_query(query);
            // Unconditional, because the previous query's highlight is baked into the snapshot:
            // a pattern that stops compiling halfway through typing would otherwise leave the old
            // matches lit up.
            self.mark_all_dirty();
            // Incremental, but debounced: the jump to the first match is what makes a find bar
            // feel like one rather than a form you submit, and it is also a whole-buffer search
            // on the UI thread under the `Term` lock. Scheduled rather than run, so typing at
            // speed costs one search rather than one per key. A pattern that does not compile yet
            // leaves the view where it is.
            self.search_at = self.search.is_active().then(|| Instant::now() + SEARCH_DEBOUNCE);
        } else if let Some(direction) = step {
            self.find_step(direction);
        }
    }
    /// Run the debounced incremental search, if it is due.
    pub(super) fn flush_pending_search(&mut self, ctx: &egui::Context) {
        let Some(at) = self.search_at else { return };
        let now = Instant::now();
        if now < at {
            // Guarded by `search_at` being set, so an idle window still draws zero frames.
            ctx.request_repaint_after(at - now);
            return;
        }
        self.search_at = None;
        if self.search.is_active() {
            self.find_step(SearchDir::Right);
        }
    }
    /// Ask for the next match. The walk itself happens on a thread of its own.
    pub(super) fn find_step(&mut self, direction: SearchDir) {
        // An explicit step supersedes anything the debounce still had pending.
        self.search_at = None;
        if !self.search.is_active() {
            return;
        }
        let Some(pane) = self.focused_pane() else { return };
        let Some(seq) = self.search.request_in(pane, direction) else {
            // One is already walking this buffer; it will start this one when it lands. Enter
            // repeats while it is held, and a thread per repeat over a million lines is not a
            // thing to leave available.
            return;
        };
        self.spawn_scan(pane, seq, direction);
    }
    /// Hand one scan to a thread, or give up on it if there is nothing left to scan.
    ///
    /// The origin stays here, under a lock held for microseconds: it is an index into the grid
    /// as it is right now, and it is the half of the old `advance` that has to see that grid.
    /// What crosses to the thread is a pattern and two plain points.
    pub(super) fn spawn_scan(&mut self, pane: PaneId, seq: u64, direction: SearchDir) {
        let term = self.panes.get(&pane).map(|state| state.pane.term.clone());
        let pattern = self.search.pattern().map(str::to_owned);
        let (Some(term), Some(pattern)) = (term, pattern) else {
            // The pane closed, or the query emptied, between the request and this. `request` has
            // already marked a scan as running, so without this every later search would queue
            // behind a walk that is never going to happen and the bar would stop finding.
            self.search.abandon();
            return;
        };

        let (origin, side) = {
            let mut guard = term.lock();
            if let Some(state) = self.panes.get(&pane) { state.pane.apply_pending(&mut guard); }
            search::origin_for(&guard, &mut self.search, pane, direction)
        };

        let tx = self.tx.clone();
        let repaint = self.repaint.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("nyx-search-scan".into())
            .spawn(move || {
                let found = search::scan(&term, &pattern, origin, direction, side);
                let _ = tx.send(AppEvent::SearchFinished { pane, seq, found });
                repaint.poke();
            })
        {
            log::warn!("failed to spawn search-scan thread: {err}");
            self.search.abandon();
        }
    }
}
