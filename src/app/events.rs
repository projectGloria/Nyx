//! Draining `AppEvent`, the one channel from every non-UI thread into the frame.
//!
//! Runs in `logic`, so it keeps working while the window is hidden — a shell that
//! exits behind a hidden Quake window still has to be noticed.

use super::*;

impl NyxApp {
    pub(super) fn drain_events(&mut self) {
        let events: Vec<AppEvent> = self.rx.try_iter().collect();
        for event in events {
            match event {
                AppEvent::Title { pane, title } => {
                    // A title carrying this pane's token is shell integration talking, not a
                    // name for the tab. It is consumed here and never displayed.
                    if let Some(state) = self.panes.get_mut(&pane) {
                        if let Some(mark) = handshake::parse(&title, &state.token) {
                            if matches!(
                                mark,
                                handshake::Mark::PromptStart | handshake::Mark::PromptEnd { .. }
                            ) {
                                state.copy_blocks.pending_prompt = true;
                                if matches!(mark, handshake::Mark::PromptEnd { .. })
                                    && state.shadow.line().is_none()
                                {
                                    state.shadow.feed(Feed::Submit);
                                }
                            }
                            state.integration.apply(mark);
                            continue;
                        }
                        state.pane.title = title.clone();
                    }
                    // The strip shows the focused pane's title, so only that one renames the tab.
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.focused == pane) {
                        tab.title = title;
                    }
                }
                AppEvent::PtyWrite { pane, data } => self.write_to(pane, data.into_bytes()),
                // A shell that exited is how you close a pane, so honour it.
                AppEvent::PaneExited { pane } => self.close_pane(pane),
                AppEvent::Bell { pane } => {
                    // Which tab rang decides what the bell looks like. A background tab is
                    // marked and left alone; flashing the window for it would draw the eye to
                    // the wrong tab and lose the only piece of information the bell carried.
                    match crate::layout::ring_bell(&mut self.tabs, self.active_tab, pane) {
                        crate::layout::Rang::Visible => {
                            self.bell_until = Some(Instant::now() + BELL_FLASH);
                        }
                        // `ring_bell` has already marked the tab, and the proxy poked the
                        // coalescer when it sent this — so the strip redraws with the mark on
                        // it and there is nothing further to do here.
                        crate::layout::Rang::Background { .. } => {}
                        crate::layout::Rang::Unknown => {}
                    }
                }

                AppEvent::ClipboardStore { pane, kind, text } => {
                    // `p`/`s` is the X11 primary selection, which Windows does not have. Treating
                    // it as the clipboard lets a program that mirrors its own selection there
                    // overwrite whatever the user last copied, over and over.
                    if !crate::event::reaches_system_clipboard(kind) {
                        continue;
                    }
                    if let Some(clipboard) = self.clipboard.as_mut() {
                        if let Err(err) = clipboard.set_text(text) {
                            log::warn!("pane {pane}: OSC 52 clipboard write failed: {err}");
                        }
                    }
                }

                AppEvent::ColorRequest {
                    pane,
                    index,
                    format,
                } => {
                    let c = self.color_for_index(index);
                    self.write_to(
                        pane,
                        format(Rgb {
                            r: c.r,
                            g: c.g,
                            b: c.b,
                        })
                        .into_bytes(),
                    );
                }

                AppEvent::TextAreaSizeRequest { pane, format } => {
                    let Some(state) = self.panes.get(&pane) else {
                        continue;
                    };
                    let size = state.pane.window_size(&self.metrics);
                    self.write_to(pane, format(size).into_bytes());
                }

                AppEvent::ConfigReloaded {
                    config,
                    problems,
                    hash,
                } => {
                    // Our own settings-window save, arriving back through the watcher. Applying
                    // it would at best be a no-op and at worst reset the draft under the cursor
                    // of whoever is still dragging a colour picker.
                    if self.last_written == Some(hash) {
                        continue;
                    }
                    self.pending_config = Some((config, problems));
                }

                AppEvent::ConfigInvalid { problems } => {
                    // The file on disk is broken, so nothing is applied and the running config
                    // stays exactly as it is. Say so, because the alternative is a save that
                    // appears to have done nothing.
                    for problem in problems {
                        self.toast(problem);
                    }
                    self.toast("config.toml was not applied; the running settings are unchanged");
                }

                AppEvent::SearchFinished { pane, seq, found } => {
                    let landed = self.search.land(seq);
                    if landed.apply && self.focused_pane() == Some(pane) {
                        if let Some(state) = self.panes.get(&pane) {
                            let mut term = state.pane.term.lock();
                            search::apply(&mut term, &mut self.search, found);
                            drop(term);
                            state.pane.mark_dirty();
                        }
                    }
                    if let Some((next, direction)) = landed.next {
                        if let Some(next_pane) = self.search.scanning_pane() {
                            self.spawn_scan(next_pane, next, direction);
                        }
                    }
                }
                AppEvent::WslDistros(distros) => {
                    self.detected.wsl_distros = distros;
                    self.rebuild_profiles();
                }

                AppEvent::HotkeyPressed => {
                    let geometry = self.quake_settings.geometry;
                    if self.quake.hidden() {
                        self.quake.show(geometry);
                        // Coming back from hidden, every pane is showing a snapshot from before
                        // whatever the shell printed while we were away.
                        self.mark_all_dirty();
                    } else {
                        self.quake.hide(geometry);
                    }
                }

                AppEvent::HotkeyRefused => {
                    self.toast(format!(
                        "another program already owns {} — Nyx could not claim it",
                        self.config.quake.hotkey
                    ));
                    // A thread with nothing registered is a thread doing nothing; dropping it
                    // also makes `unhide_if_unreachable` below see the truth.
                    self.hotkey = None;
                    self.pending_start_hidden = false;
                    self.unhide_if_unreachable();
                }

                AppEvent::OpenHere(request) => {
                    // A launch that named no folder is asking for the window, not for a tab —
                    // `nyx --attach` on its own, or a second launch under `single_instance` from
                    // a Start-menu shortcut. Opening a tab there would give someone a new shell
                    // every time they clicked the icon on the taskbar.
                    if !request.focus && !request.is_empty() {
                        let profile = self.profiles.first().cloned();
                        if let Err(err) = self.new_tab(profile.as_ref(), request.cwd) {
                            self.toast(format!("could not open a tab: {err}"));
                        }
                    }
                    self.reveal = true;
                }
            }
        }
    }
    /// Show the window again when there is no working way for the user to do it themselves.
    ///
    /// Hidden is a state only the hotkey can leave, so anything that takes the hotkey away —
    /// turning quake off in the settings, a chord another program already owns — has to undo the
    /// hiding on the way out. Otherwise the terminal is still running, still holding four shells,
    /// and completely unreachable.
    pub(super) fn unhide_if_unreachable(&mut self) {
        let reachable = self.quake_settings.enabled && self.hotkey.is_some();
        if !reachable && self.quake.hidden() {
            self.quake.show(quake::Geometry {
                // No slide: this is a rescue, not a gesture.
                animate: Duration::ZERO,
                ..self.quake_settings.geometry
            });
            self.mark_all_dirty();
        }
    }
    pub(super) fn color_for_index(&self, index: usize) -> crate::theme::Rgba {
        match index {
            i if i < 256 => self.theme.palette[i],
            i if i == NamedColor::Foreground as usize => self.theme.fg,
            i if i == NamedColor::Background as usize => self.theme.bg,
            i if i == NamedColor::Cursor as usize => self.theme.cursor_bg,
            _ => self.theme.fg,
        }
    }
}
