//! Panes and tabs: spawning, splitting, focusing and tearing down.
//!
//! The decisions that can be wrong in a way no window shows are in `layout`, tested
//! there; what is left here is the part that owns the `Pane` values themselves.

use super::*;

impl NyxApp {
    /// `cwd` is a directory the *user* named — `--cwd`, or Explorer's "Open Nyx here". It wins
    /// over a profile's own `cwd`, which is only a default: someone who right-clicked a folder
    /// asked for that folder, not for wherever their PowerShell profile usually starts.
    pub(super) fn spawn_pane(
        &mut self,
        profile: Option<&Profile>,
        cwd: Option<std::path::PathBuf>,
    ) -> anyhow::Result<PaneId> {
        let id = self.next_pane_id;
        self.next_pane_id += 1;

        let (mut spec, injected, token) = match profile {
            Some(p) => {
                let launch = p.launch(id, dirs::home_dir(), self.detected.clink.as_deref());
                (launch.spec, launch.injected, launch.token)
            },
            None => (crate::term::spawn::ShellSpec::cmd(), false, String::new()),
        };
        if cwd.is_some() {
            spec.cwd = cwd;
        }
        let style = profile.map(|p| p.kind.path_style()).unwrap_or(PathStyle::Windows);
        let mut integration = handshake::State::new(injected);
        integration.cwd = spec.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());

        let pane = Pane::spawn(
            id,
            &spec,
            GridSize::new(80, 24),
            &self.metrics,
            self.config.render.scrollback,
            self.tx.clone(),
            self.repaint.clone(),
        )?;

        let snapshot = Arc::new(GridSnapshot::empty(&self.theme));
        self.panes.insert(
            id,
            PaneState {
                pane,
                snapshot,
                shadow: Shadow::new(),
                style,
                integration,
                token,
                cursor: CursorSettle::default(),
                copy_blocks: crate::term::blocks::Blocks::new(),
            },
        );
        Ok(id)
    }
    pub(super) fn new_tab(
        &mut self,
        profile: Option<&Profile>,
        cwd: Option<std::path::PathBuf>,
    ) -> anyhow::Result<()> {
        let pane = self.spawn_pane(profile, cwd)?;
        let title = profile.map(|p| p.name.clone()).unwrap_or_else(|| "Shell".into());
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, pane, title);
        if let Some(profile) = profile {
            tab.icon = format!("exe:{}", profile.program.display());
        }
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.relayout = true;
        Ok(())
    }
    pub(super) fn split_focused(&mut self, dir: SplitDir) {
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let target = tab.focused;
        // Splitting inherits the current tab's profile by matching nothing — the first profile
        // is the sensible default, and Phase 5's palette will offer the rest.
        let profile = self.profiles.first().cloned();
        let Ok(new_pane) = self.spawn_pane(profile.as_ref(), None) else {
            self.toast("could not start a shell for the new pane");
            return;
        };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            // Zoom and splitting are contradictory: the new pane would be invisible.
            tab.zoomed = None;
            tab.root.split(target, dir, new_pane);
            tab.focused = new_pane;
            if tab.pinned.is_none() {
                if let Some(state) = self.panes.get(&new_pane) {
                    tab.title = state.pane.title.clone();
                }
            }
        }
        // The pane that was split loses focus, so its cursor has to be redrawn hollow.
        if let Some(state) = self.panes.get(&target) {
            state.pane.mark_dirty();
        }
        self.relayout = true;
    }
    /// End a pane's shell and queue its renderer state for release.
    ///
    /// The renderer keys its per-pane row cache, `TextRenderer` and quad buffer by pane id, and
    /// pane ids only ever increase — so without the queueing half, opening and closing panes
    /// accumulates all three for the life of the process. Nothing is pushed for an id that was
    /// not there, which keeps a duplicate `PaneExited` from queueing the same work twice.
    pub(super) fn shutdown_pane(&mut self, pane: PaneId) {
        if let Some(state) = self.panes.remove(&pane) {
            state.pane.shutdown();
            self.retired.push(pane);
        }
    }
    /// Hand the queued panes to the renderer. Called once a frame, where the render state exists.
    pub(super) fn retire_panes(&mut self, frame: &mut eframe::Frame) {
        if self.retired.is_empty() {
            return;
        }
        if let Some(render_state) = frame.wgpu_render_state() {
            let mut renderer = render_state.renderer.write();
            if let Some(res) = renderer.callback_resources.get_mut::<GridResources>() {
                for pane in self.retired.drain(..) {
                    res.forget_pane(pane);
                }
            }
        }
        // Cleared either way: a queue that cannot be drained is the leak it was meant to fix.
        self.retired.clear();
    }
    pub(super) fn close_pane(&mut self, pane: PaneId) {
        self.shutdown_pane(pane);

        // Whichever tab holds it, which is not necessarily the one being looked at.
        match crate::layout::close_pane(&mut self.tabs, pane) {
            // Already closed — a duplicate PaneExited, or a tab that went before its panes did.
            crate::layout::Closed::Unknown => return,
            crate::layout::Closed::Tab { tab: index, id } => {
                log::info!("tab {id} closed: its last pane ({pane}) exited");
                self.active_tab =
                    crate::layout::active_after_removing(self.active_tab, index, self.tabs.len());
            },
            crate::layout::Closed::Pane { tab: index } => {
                // Nearest survivor by geometry, using the layout we last drew.
                let closed = self.solved.rect_of(pane);
                let Some(tab) = self.tabs.get(index) else { return };
                if tab.focused == pane {
                    let remaining: Vec<_> = self
                        .solved
                        .panes
                        .iter()
                        .filter(|(id, _)| *id != pane && tab.root.contains(*id))
                        .cloned()
                        .collect();
                    let next = closed
                        .and_then(|r| crate::layout::focus::nearest(&remaining, r))
                        .or_else(|| tab.root.leaves().first().copied())
                        .unwrap_or(pane);
                    if let Some(tab) = self.tabs.get_mut(index) {
                        tab.focused = next;
                        if tab.pinned.is_none() {
                            if let Some(state) = self.panes.get(&next) {
                                tab.title = state.pane.title.clone();
                            }
                        }
                    }
                }
            },
        }

        self.relayout = true;
    }
    /// Where the focused pane's cursor is on screen, in points, for the IME candidate window.
    pub(super) fn focused_cursor_rect(&self, pane: PaneId) -> Option<egui::Rect> {
        let rect = self.solved.rect_of(pane)?;
        let snapshot = &self.panes.get(&pane)?.snapshot;
        let cursor = snapshot.cursor.as_ref()?;
        let pad = self.metrics.padding / self.scale;
        let (w, h) = (self.metrics.cell_w / self.scale, self.metrics.cell_h / self.scale);
        let min = egui::pos2(
            rect.min.x + pad + cursor.col as f32 * w,
            rect.min.y + pad + cursor.row as f32 * h,
        );
        Some(egui::Rect::from_min_size(min, egui::vec2(w, h)))
    }
    pub(super) fn focused_pane(&self) -> Option<PaneId> {
        self.tabs.get(self.active_tab).map(|t| t.focused)
    }
    /// Move focus, invalidating both panes' snapshots.
    ///
    /// The cursor is drawn solid in the focused pane and hollow everywhere else, and that is
    /// baked into the snapshot — so a focus change with no other activity would leave the old
    /// pane showing a solid cursor until something happened to redraw it.
    pub(super) fn set_focus(&mut self, pane: PaneId) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if tab.focused == pane || !tab.root.contains(pane) {
            return;
        }
        let previous = tab.focused;
        tab.focused = pane;
        if tab.pinned.is_none() {
            if let Some(state) = self.panes.get(&pane) {
                tab.title = state.pane.title.clone();
            }
        }
        for id in [previous, pane] {
            if let Some(state) = self.panes.get(&id) {
                state.pane.mark_dirty();
            }
        }
    }
    pub(super) fn mark_all_dirty(&self) {
        for state in self.panes.values() {
            state.pane.mark_dirty();
        }
    }
    pub(super) fn write_to(&self, pane: PaneId, bytes: Vec<u8>) {
        if let Some(state) = self.panes.get(&pane) {
            state.pane.write(bytes);
        }
    }
}
