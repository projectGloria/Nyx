//! Pointer routing: which pane a click landed in, and what it meant there.
//!
//! `handle_mouse` reads raw pointer state rather than going through `Ui::interact`,
//! which is what makes selection feel immediate — and also why everything drawn over
//! the grid has to be subtracted from it by hand. `block` is that subtraction.

use super::*;

impl NyxApp {
    /// Route pointer events to the grid.
    ///
    /// This reads raw events rather than going through `Ui::interact`, which is what makes
    /// selection feel immediate — and also means egui's layering does not protect the terminal
    /// from clicks meant for something drawn on top of it. `block` is that subtraction: without
    /// it, clicking the find bar's "next" arrow also starts dragging a selection through the
    /// output behind it. It is a list rather than one rectangle because the find bar is no longer
    /// the only thing over the grid — the sigil menu is there too, and a second parameter per
    /// overlay is how one of them ends up not being subtracted.
    pub(super) fn handle_mouse(&mut self, ui: &egui::Ui, block: &[egui::Rect], copy_buttons: &[egui::Rect]) {
        let events = ui.input(|i| i.events.clone());
        let modal = self.palette.open || self.settings.open || egui::Popup::is_any_open(ui.ctx())
            || !ui.input(|i| i.focused);
        let mut dirty: Option<PaneId> = None;
        let blocked = |pos: egui::Pos2| block.iter().chain(copy_buttons).any(|r| r.contains(pos));

        for event in &events {
            // Preserve event order: a press and release can both arrive in the same frame.
            // Releases reach captured programs even over chrome or while a modal is open.
            if let egui::Event::PointerButton { pos, button, pressed: false, .. } = event {
                if let Some(button) = to_button(*button) { self.release_mouse(Some(button), Some(*pos)); }
                if *button == egui::PointerButton::Primary { self.mouse.selecting = None; }
                continue;
            }
            if modal { continue; }
            match event {
                egui::Event::PointerButton { pos, button, pressed, modifiers }
                    if *pressed && !blocked(*pos) =>
                {
                    let Some(pane) = self.solved.pane_at(*pos)
                    else {
                        continue;
                    };

                    if *pressed {
                        // Clicking a pane focuses it, which is what makes a four-way split
                        // usable without touching the keyboard.
                        self.set_focus(pane);
                    }

                    let Some((point, side, mode, offset)) = self.locate(pane, *pos) else {
                        continue;
                    };

                    if mouse::wants_mouse(mode, modifiers.shift) {
                        if let Some(btn) = to_button(*button) {
                            // Remembered so 1002 knows whether to report motion at all, and so
                            // 1003 can say which button is down instead of assuming Left.
                            self.mouse.captures.retain(|capture| capture.button != btn);
                            self.mouse.captures.push(MouseCapture {
                                pane, button: btn, point, offset, mode, modifiers: *modifiers,
                            });
                            self.mouse.reported_cell = None;
                            if let Some(bytes) =
                                mouse::encode(btn, *pressed, false, point, offset, *modifiers, mode)
                            {
                                self.write_to(pane, bytes);
                            }
                        }
                        continue;
                    }
                    // Not reporting: nothing is held as far as the program is concerned.


                    // Ctrl+click opens a link. Ctrl rather than a bare click because the text
                    // came out of a pipe: a log file full of URLs must stay something you can
                    // select and scroll through without launching a browser by accident. Taken
                    // before the selection below so the click does not also start one.
                    if *button == egui::PointerButton::Primary && modifiers.ctrl {
                        if let Some(url) = self.link_at(pane, *pos) {
                            if let Err(err) = crate::win::open_url(&url) {
                                log::warn!("{err}");
                                self.toast(err);
                            }
                            continue;
                        }
                    }

                    match (button, pressed) {
                        (egui::PointerButton::Primary, true) => {
                            let now = Instant::now();
                            self.mouse.click_count = match self.mouse.last_click {
                                Some((at, p))
                                    if p == point && now.duration_since(at) < MULTI_CLICK =>
                                {
                                    self.mouse.click_count + 1
                                },
                                _ => 1,
                            };
                            self.mouse.last_click = Some((now, point));
                            self.mouse.selecting = Some(pane);

                            if let Some(state) = self.panes.get(&pane) {
                                let Some(mut term) = state.pane.term.try_lock_unfair() else {
                                    self.mouse.selecting = None;
                                    continue;
                                };
                                let ty = select::kind_for_clicks(
                                    self.mouse.click_count,
                                    modifiers.ctrl,
                                );
                                select::start(&mut term, point, side, ty);
                            }
                            dirty = Some(pane);
                        },
                        (egui::PointerButton::Primary, false) => self.mouse.selecting = None,
                        (egui::PointerButton::Middle, true) => self.paste_clipboard(),
                        _ => {},
                    }
                },

                egui::Event::PointerMoved(pos) => {
                    if let Some(pane) = self.mouse.selecting {
                        if let Some((point, side, _, _)) = self.locate(pane, *pos) {
                            if let Some(state) = self.panes.get(&pane) {
                                let Some(mut term) = state.pane.term.try_lock_unfair() else {
                                    self.mouse.selecting = None;
                                    continue;
                                };
                                select::update(&mut term, point, side);
                            }
                            dirty = Some(pane);
                        }
                        continue;
                    }

                    let capture = self.mouse.captures.last().copied();
                    if capture.is_none() && blocked(*pos) { continue; }
                    let Some(pane) = capture.map(|c| c.pane).or_else(|| self.solved.pane_at(*pos)) else { continue };
                    let Some((point, _, mode, offset)) = self.locate(pane, *pos) else { continue };

                    for captured in &mut self.mouse.captures {
                        if captured.pane == pane { captured.point = point; captured.offset = offset; }
                    }
                    // `PointerMoved` carries no modifiers of its own, so take the frame's.
                    let modifiers = ui.input(|i| i.modifiers);
                    let Some(button) = mouse::motion_report(mode, modifiers.shift, capture.map(|c| c.button))
                    else {
                        continue;
                    };
                    if self.mouse.reported_cell != Some((pane, point)) {
                        self.mouse.reported_cell = Some((pane, point));
                        if let Some(bytes) =
                            mouse::encode(button, true, true, point, offset, modifiers, mode)
                        {
                            self.write_to(pane, bytes);
                        }
                    }
                },

                // `phase` distinguishes trackpad momentum from a real gesture; scrollback does
                // not care which, so it is ignored rather than matched.
                egui::Event::MouseWheel { unit, delta, modifiers, .. } => {
                    let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else { continue };
                    let Some(pane) = self.solved.pane_at(pos) else { continue };
                    let Some((point, _, mode, offset)) = self.locate(pane, pos) else { continue };

                    let rows = self.panes.get(&pane).map(|s| s.snapshot.rows).unwrap_or(24);
                    let lines_f = match unit {
                        // egui reports one line per wheel notch. Windows' own convention is
                        // whatever the user set in Mouse Properties — three by default, and one
                        // line per notch makes scrollback feel broken.
                        egui::MouseWheelUnit::Line => delta.y * self.wheel.for_rows(rows),
                        egui::MouseWheelUnit::Point => delta.y / (self.metrics.cell_h / self.scale),
                        egui::MouseWheelUnit::Page => delta.y * rows as f32,
                    };
                    self.mouse.scroll_accum += lines_f;
                    let lines = self.mouse.scroll_accum.trunc();
                    if lines == 0.0 {
                        continue;
                    }
                    self.mouse.scroll_accum -= lines;
                    let lines = lines as i32;

                    // Three destinations, and which one wins is `mouse::wheel_target`.
                    match mouse::wheel_target(mode, modifiers.shift) {
                        mouse::WheelTarget::Report => {
                            let btn = if lines > 0 {
                                mouse::Button::WheelUp
                            } else {
                                mouse::Button::WheelDown
                            };
                            for _ in 0..lines.abs() {
                                if let Some(bytes) =
                                    mouse::encode(btn, true, false, point, offset, *modifiers, mode)
                                {
                                    self.write_to(pane, bytes);
                                }
                            }
                            continue;
                        },
                        mouse::WheelTarget::ArrowKeys => {
                            if let Some(bytes) = mouse::alternate_scroll(
                                lines > 0,
                                lines.unsigned_abs() as usize,
                                mode,
                            ) {
                                self.write_to(pane, bytes);
                            }
                            continue;
                        },
                        mouse::WheelTarget::Scrollback => {},
                    }

                    if let Some(state) = self.panes.get(&pane) {
                        state.pane.scroll(Scroll::Delta(lines));
                    }
                },

                _ => {},
            }
        }

        if let Some(pane) = dirty {
            if let Some(state) = self.panes.get(&pane) {
                state.pane.mark_dirty();
            }
        }
    }
    /// Complete reports using the press protocol and pane, even if either is now hidden.
    pub(super) fn release_mouse(&mut self, button: Option<mouse::Button>, pos: Option<egui::Pos2>) {
        for capture in self.mouse.take_releases(button) {
            let (point, offset) = pos.and_then(|pos| self.locate(capture.pane, pos))
                .map(|(point, _, _, offset)| (point, offset)).unwrap_or((capture.point, capture.offset));
            if let Some(bytes) = mouse::encode(capture.button, false, false, point, offset,
                capture.modifiers, capture.mode) {
                self.write_to(capture.pane, bytes);
            }
        }
    }
    /// Grid point, cell side, terminal mode and scroll offset for a screen position in a pane.
    /// The URL under the pointer, if there is one.
    ///
    /// Read out of the snapshot rather than from the `Term`, so hovering costs no lock at all —
    /// this is asked once a frame while the pointer is over a pane, and a mutex on that path is
    /// the thing `snapshot` exists to avoid.
    ///
    /// The column is mapped to a character position through the row's `cells` rather than used
    /// as one. They agree for a URL, which is ASCII, and they do not agree for a row that has a
    /// wide character or a combining mark earlier in it — which is most rows in a CJK locale,
    /// and would put the link a few columns from where it is drawn.
    pub(super) fn link_at(&self, pane: PaneId, pos: egui::Pos2) -> Option<String> {
        let (point, _, _, offset) = self.locate(pane, pos)?;
        let state = self.panes.get(&pane)?;
        let row = state
            .snapshot
            .rows_data
            .get(usize::try_from(point.line.0 + offset as i32).ok()?)?;

        let column = point.column.0;
        let span = row
            .cells
            .iter()
            .find(|c| column >= c.col as usize && column < (c.col + c.cols) as usize)?;
        let index = row.text.get(..span.start as usize)?.chars().count();
        crate::term::links::at(&row.text, index)
    }

    pub(super) fn locate(
        &self,
        pane: PaneId,
        pos: egui::Pos2,
    ) -> Option<(Point, alacritty_terminal::index::Side, TermMode, usize)> {
        let rect = self.solved.rect_of(pane)?;
        let state = self.panes.get(&pane)?;
        let snapshot = &state.snapshot;
        let (point, side) = mouse::point_at(
            pos.x * self.scale,
            pos.y * self.scale,
            (rect.min.x * self.scale, rect.min.y * self.scale),
            &self.metrics,
            snapshot.cols.max(1),
            snapshot.rows.max(1),
            snapshot.display_offset,
        );
        Some((point, side, snapshot.mode, snapshot.display_offset))
    }
}
