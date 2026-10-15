//! Everything drawn that is not the grid: the tab strip, the palette's contents, the
//! toasts and the perf overlay.

use super::*;

/// How wide one tab is.
///
/// Wide enough for a name. An icon-only strip is unreadable the moment two tabs run the same
/// shell, which is the normal case — five `cmd.exe` tabs were five identical squares, and the
/// only way to tell them apart was to hover each one in turn.
const TAB_WIDTH: f32 = 150.0;

/// `title` shortened from the *back* to fit `room`, with an ellipsis.
///
/// From the back because a window title's subject comes first: a title truncated to its
/// tail says the same thing about every tab in one repository. That is the opposite of the
/// palette's `elide_front`, which keeps a path's tail for exactly the same reason — there it is
/// the end of the path that identifies it.
///
/// `width` is a parameter rather than a measurement taken here because egui's test harness loads
/// no font data and reports every string as zero pixels wide. A test that asked a `Ui` to measure
/// would watch every input "fit" and pin nothing.
fn elide_back(title: &str, room: f32, width: impl Fn(&str) -> f32) -> String {
    let title = title.trim();
    if title.is_empty() || room <= 0.0 {
        return String::new();
    }
    if width(title) <= room {
        return title.to_string();
    }
    let chars: Vec<char> = title.chars().collect();
    // Monospace-ish first guess, then walked down: one estimate lands on or beside the answer,
    // and the loop is what keeps it right when a proportional face makes the estimate false.
    let mut keep = ((room / width(title)) * chars.len() as f32).floor() as usize;
    keep = keep.min(chars.len());
    loop {
        let candidate: String = chars[..keep].iter().collect::<String>() + "…";
        if keep == 0 {
            return String::new();
        }
        if width(&candidate) <= room {
            return candidate;
        }
        keep -= 1;
    }
}

fn footer_icon(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    icon: icons::Builtin,
    hint: &str,
    theme: &Theme,
) -> egui::Response {
    let response = ui
        .interact(rect, egui::Id::new(id), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals::to_color32(theme.selection_bg));
    }
    icons::draw_builtin(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
        icon,
        visuals::to_color32(theme.accent),
    );
    response
}

impl NyxApp {
    pub(super) fn location_footer(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        ui.painter()
            .rect_filled(rect, 0.0, visuals::to_color32(self.theme.surface));
        ui.painter().text(
            rect.left_center() + egui::vec2(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "NYX",
            egui::FontId::monospace(14.0),
            visuals::to_color32(self.theme.accent),
        );
        let location = self
            .focused_pane()
            .and_then(|pane| self.panes.get(&pane))
            .and_then(|state| state.integration.cwd.as_deref())
            .unwrap_or("Location unavailable")
            .to_string();
        let menu_width = 76.0;
        let path_width = (rect.width() - 2.0 * (menu_width + 12.0)).max(0.0);
        let path_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(path_width, rect.height()));
        ui.painter().with_clip_rect(path_rect).text(
            path_rect.center(),
            egui::Align2::CENTER_CENTER,
            &location,
            egui::FontId::monospace(12.0),
            visuals::to_color32(self.theme.fg),
        );
        ui.interact(
            path_rect,
            egui::Id::new("nyx.current_location"),
            egui::Sense::hover(),
        )
        .on_hover_text(&location);
        // Disjoint field borrows rather than a clone of the table. This runs every frame the
        // footer is drawn, and cloning the shortcut list to *ask whether the menu is open* is a
        // heap allocation per shortcut per frame for a menu that is almost always closed.
        let mut chosen: Option<crate::expand::table::Shortcut> = None;
        let gear_rect = egui::Rect::from_center_size(
            egui::pos2(rect.max.x - 20.0, rect.center().y),
            egui::vec2(32.0, 26.0),
        );
        let saved_rect = gear_rect.translate(egui::vec2(-36.0, 0.0));
        let gear = footer_icon(
            ui,
            gear_rect,
            "nyx.options",
            icons::Builtin::Gear,
            "Options",
            &self.theme,
        );
        if gear.clicked() {
            self.settings.open(&self.config);
        }
        let saved = footer_icon(
            ui,
            saved_rect,
            "nyx.locations",
            icons::Builtin::Folder,
            "Saved locations",
            &self.theme,
        );
        {
            let Self {
                tables,
                icon_cache,
                theme,
                ..
            } = self;
            egui::Popup::menu(&saved).width(260.0).show(|ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        if tables.shortcuts.is_empty() {
                            ui.weak("No saved locations");
                        }
                        for shortcut in tables.shortcuts.iter() {
                            let (row_rect, row) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 28.0),
                                egui::Sense::click(),
                            );
                            let row = row
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(&shortcut.path);
                            if row.hovered() {
                                ui.painter().rect_filled(
                                    row_rect,
                                    4.0,
                                    visuals::to_color32(theme.selection_bg),
                                );
                                let marker = egui::Rect::from_min_size(
                                    row_rect.min,
                                    egui::vec2(3.0, row_rect.height()),
                                );
                                ui.painter().rect_filled(
                                    marker,
                                    2.0,
                                    visuals::to_color32(theme.accent),
                                );
                            }
                            let icon_rect = egui::Rect::from_center_size(
                                egui::pos2(row_rect.min.x + 18.0, row_rect.center().y),
                                egui::vec2(20.0, 20.0),
                            );
                            let tint = shortcut
                                .color
                                .as_deref()
                                .and_then(crate::theme::Rgba::parse_hex)
                                .unwrap_or(theme.accent);
                            let texture = match icons::Icon::parse(&shortcut.icon) {
                                icons::Icon::File(path) => icon_cache.file(ui.ctx(), &path),
                                icons::Icon::Exe(path) => icon_cache.exe(ui.ctx(), &path),
                                icons::Icon::Builtin(builtin) => {
                                    icons::draw_builtin(
                                        ui.painter(),
                                        icon_rect,
                                        builtin,
                                        visuals::to_color32(tint),
                                    );
                                    None
                                }
                            };
                            if let Some(texture) = texture {
                                ui.painter().image(
                                    texture.id(),
                                    icons::fitted_rect(icon_rect, texture),
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else if !matches!(
                                icons::Icon::parse(&shortcut.icon),
                                icons::Icon::Builtin(_)
                            ) {
                                icons::draw_builtin(
                                    ui.painter(),
                                    icon_rect,
                                    icons::Builtin::Folder,
                                    visuals::to_color32(tint),
                                );
                            }
                            ui.painter().with_clip_rect(row_rect).text(
                                egui::pos2(row_rect.min.x + 36.0, row_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &shortcut.label,
                                egui::FontId::proportional(14.0),
                                visuals::to_color32(theme.fg),
                            );
                            if row.clicked() {
                                // The one entry that was actually chosen, so the `&mut self` calls
                                // below are not blocked by a borrow of the table.
                                chosen = Some(shortcut.clone());
                                ui.close();
                            }
                        }
                    });
            });
        }
        if let Some(shortcut) = chosen {
            let cwd = std::path::PathBuf::from(&shortcut.path);
            if !cwd.is_dir() {
                self.toast(format!("Saved location does not exist: {}", shortcut.path));
            } else {
                let profile = self.profiles.first().cloned();
                if let Err(err) = self.new_tab(profile.as_ref(), Some(cwd)) {
                    self.toast(format!("Could not open saved location: {err}"));
                } else {
                    // The same naming as `@sigil` + Enter, from the same place: the name has to
                    // be *pinned*, or the shell's first title overwrites it a second later.
                    self.name_tab_after(&shortcut);
                }
            }
        }
    }
    /// Everything the palette can offer, rebuilt each time it is drawn.
    pub(super) fn palette_entries(&self) -> Vec<palette::Entry> {
        let mut entries = Vec::new();
        for (i, shortcut) in self.tables.shortcuts.iter().enumerate() {
            entries.push(palette::Entry {
                label: shortcut.label.clone(),
                detail: shortcut.path.clone(),
                action: palette::Action::GoTo { shortcut: i },
            });
        }
        for (i, profile) in self.profiles.iter().enumerate() {
            entries.push(palette::Entry {
                label: format!("New tab: {}", profile.name),
                detail: profile.program.to_string_lossy().into_owned(),
                action: palette::Action::NewTab { profile: i },
            });
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            entries.push(palette::Entry {
                label: format!("Tab {}: {}", i + 1, trim_title(tab.name())),
                detail: String::new(),
                action: palette::Action::SwitchTab { tab: i },
            });
        }
        entries
    }
    pub(super) fn run_palette_action(&mut self, action: palette::Action) {
        self.palette.close();
        match action {
            palette::Action::GoTo { shortcut } => self.go_to_shortcut(shortcut),
            palette::Action::NewTab { profile } => {
                let profile = self.profiles.get(profile).cloned();
                if let Err(err) = self.new_tab(profile.as_ref(), None) {
                    self.toast(format!("could not open a tab: {err}"));
                }
            }
            palette::Action::SwitchTab { tab } => {
                if tab < self.tabs.len() {
                    self.active_tab = tab;
                    self.relayout = true;
                }
            }
        }
    }
    /// Send a `cd` to the focused pane, spelled for that pane's shell.
    pub(super) fn go_to_shortcut(&mut self, index: usize) {
        let Some(shortcut) = self.tables.shortcuts.get(index) else {
            return;
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let Some(state) = self.panes.get_mut(&pane) else {
            return;
        };

        // The keystrokes land wherever the shell is looking, so they may only be typed at a
        // command line. On the alternate screen there is no command line — the `cd` would be
        // inserted into a vim buffer — and while a command is running (known only under shell
        // integration) its stdin is not one either. Saying so beats typing somewhere destructive.
        if let Some(reason) = navigation_refusal(state.snapshot.mode, state.integration.child_running, state.shadow.line())
        {
            self.toast(reason.to_string());
            return;
        }

        let line = navigation_command(state.style, &shortcut.expansion(state.style));
        state.pane.write(line.into_bytes());
        state.pane.scroll(Scroll::Bottom);
        // The line we just sent ends in Enter, so the model is back at a known prompt.
        state.shadow.feed(Feed::Submit);
    }
    pub(super) fn toast(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        self.toasts.push((message, Instant::now() + TOAST_LIFETIME));
    }
    /// Drawn into a slice of the `Ui` eframe hands us rather than as an `egui::Panel`.
    ///
    /// A panel registered on the context would allocate its space *outside* the `Ui` whose
    /// `max_rect` we have already taken as the terminal area, and the two would overlap.
    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui, rect: egui::Rect) -> Vec<egui::Rect> {
        let bg = visuals::to_color32(self.theme.surface);
        let fg = visuals::to_color32(self.theme.fg);
        let active_bg = visuals::to_color32(self.theme.bg);
        let accent = visuals::to_color32(self.theme.accent);

        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut open_new = false;
        let mut buttons = Vec::new();

        ui.painter().rect_filled(rect, 0.0, bg);

        // Looking at a tab is what clears its bell, and this is the single place that says so.
        // Seven different things set `active_tab` — the strip, Ctrl+Tab both ways, the palette,
        // a close, a new tab — and clearing the mark at each of them is how one of them ends up
        // not doing it and a tab stays lit for the rest of the session.
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.bell = false;
        }

        // Built before the loop because `on_hover_text` takes its string eagerly and the loop
        // holds a borrow of `self.tabs` that a `&self` call inside it would have to share with
        // the icon cache's `&mut`.
        let tooltips: Vec<String> = self.tabs.iter().map(|tab| self.tab_tooltip(tab)).collect();

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(4.0, 3.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("nyx.top_tabs")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (i, tab) in self.tabs.iter().enumerate() {
                            let is_active = i == self.active_tab;
                            let (tab_rect, response) = ui.allocate_exact_size(
                                egui::vec2(TAB_WIDTH, 26.0),
                                egui::Sense::click(),
                            );
                            buttons.push(tab_rect.intersect(rect));
                            let hovered = ui
                                .ctx()
                                .pointer_hover_pos()
                                .is_some_and(|pos| tab_rect.contains(pos));
                            ui.painter().rect_filled(
                                tab_rect,
                                4.0,
                                if is_active {
                                    active_bg
                                } else if hovered {
                                    visuals::to_color32(self.theme.selection_bg)
                                } else {
                                    bg
                                },
                            );
                            if is_active {
                                ui.painter().line_segment(
                                    [tab_rect.left_bottom(), tab_rect.right_bottom()],
                                    egui::Stroke::new(2.0, accent),
                                );
                            }
                            let icon_rect = egui::Rect::from_center_size(
                                egui::pos2(tab_rect.min.x + 20.0, tab_rect.center().y),
                                egui::vec2(20.0, 20.0),
                            );
                            let tint = tab
                                .icon_color
                                .as_deref()
                                .and_then(crate::theme::Rgba::parse_hex)
                                .map(visuals::to_color32)
                                .unwrap_or(accent);
                            let texture = match icons::Icon::parse(&tab.icon) {
                                icons::Icon::File(path) => self.icon_cache.file(ui.ctx(), &path),
                                icons::Icon::Exe(path) => self.icon_cache.exe(ui.ctx(), &path),
                                icons::Icon::Builtin(builtin) => {
                                    icons::draw_builtin(ui.painter(), icon_rect, builtin, tint);
                                    None
                                }
                            };
                            if let Some(texture) = texture {
                                ui.painter().image(
                                    texture.id(),
                                    icons::fitted_rect(icon_rect, texture),
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else if !matches!(
                                icons::Icon::parse(&tab.icon),
                                icons::Icon::Builtin(_)
                            ) {
                                icons::draw_builtin(
                                    ui.painter(),
                                    icon_rect,
                                    icons::Builtin::Terminal,
                                    tint,
                                );
                            }
                            if tab.bell {
                                ui.painter()
                                    .circle_filled(icon_rect.right_top(), 3.0, accent);
                            }
                            let close_rect = egui::Rect::from_center_size(
                                egui::pos2(tab_rect.max.x - 12.0, tab_rect.center().y),
                                egui::vec2(18.0, 20.0),
                            );
                            // Between the icon and the close button. `Painter::text` takes no width, so
                            // the room has to be worked out here or a long title is drawn straight
                            // through both of them.
                            let title_font = egui::FontId::proportional(13.0);
                            let title_left = icon_rect.max.x + 6.0;
                            let room = close_rect.min.x - 4.0 - title_left;
                            let measure = |text: &str| {
                                ui.painter()
                                    .layout_no_wrap(
                                        text.to_string(),
                                        title_font.clone(),
                                        egui::Color32::WHITE,
                                    )
                                    .rect
                                    .width()
                            };
                            let label = elide_back(tab.name(), room, measure);
                            if !label.is_empty() {
                                ui.painter().text(
                                    egui::pos2(title_left, tab_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    title_font,
                                    if is_active {
                                        fg
                                    } else {
                                        visuals::to_color32(self.theme.palette[8])
                                    },
                                );
                            }
                            let close_response = ui.interact(
                                close_rect,
                                egui::Id::new(("nyx.tab.close", tab.id)),
                                egui::Sense::click(),
                            );
                            if hovered {
                                if close_response.hovered() {
                                    ui.painter().rect_filled(
                                        close_rect,
                                        3.0,
                                        visuals::to_color32(self.theme.palette[1]),
                                    );
                                }
                                let x = egui::Rect::from_center_size(
                                    close_rect.center(),
                                    egui::vec2(7.0, 7.0),
                                );
                                ui.painter().line_segment(
                                    [x.left_top(), x.right_bottom()],
                                    egui::Stroke::new(1.3, fg),
                                );
                                ui.painter().line_segment(
                                    [x.right_top(), x.left_bottom()],
                                    egui::Stroke::new(1.3, fg),
                                );
                                close_response.clone().on_hover_text("Close tab");
                            }
                            if response.clicked() && !close_response.clicked() {
                                select = Some(i);
                            }
                            if response.middle_clicked()
                                || close_response.middle_clicked()
                                || close_response.clicked()
                            {
                                close = Some(i);
                            }
                            // The name is already on the tab, so a tooltip repeating it says nothing.
                            // What a hover is asking is *where* — `tab_tooltip` answers with the path.
                            response.on_hover_text(&tooltips[i]);
                        }

                        let new_tab = ui
                            .add(
                                egui::Button::new(egui::RichText::new(" + ").monospace().color(fg))
                                    .fill(bg),
                            )
                            .on_hover_text("New tab");
                        buttons.push(new_tab.rect.intersect(rect));
                        if new_tab.clicked() {
                            open_new = true;
                        }
                    });
            },
        );

        if let Some(i) = select {
            self.active_tab = i;
            self.relayout = true;
        }
        if let Some(i) = close {
            let panes = self.tabs.get(i).map(|t| t.panes());
            if let Some(panes) = panes {
                for pane in panes {
                    self.shutdown_pane(pane);
                }
                self.tabs.remove(i);
                // `i` is whichever tab's close button was clicked, not necessarily the active
                // one: clamping alone would move the user to a different tab when they closed
                // one to the left of the one they were using.
                self.active_tab =
                    crate::layout::active_after_removing(self.active_tab, i, self.tabs.len());
                self.relayout = true;
            }
        }
        if open_new {
            let profile = self.profiles.first().cloned();
            if let Err(err) = self.new_tab(profile.as_ref(), None) {
                self.toast(format!("could not open a tab: {err}"));
            }
        }
        buttons
    }
    pub(super) fn perf_overlay(&mut self, ui: &egui::Ui) {
        let Some(perf) = &mut self.perf else { return };

        let now = Instant::now();
        perf.frame_ms = now.duration_since(perf.last).as_secs_f32() * 1000.0;
        perf.last = now;
        perf.frames += 1;
        let elapsed = now.duration_since(perf.window_start).as_secs_f32();
        if elapsed >= 0.5 {
            perf.fps = perf.frames as f32 / elapsed;
            perf.frames = 0;
            perf.window_start = now;
        }

        let focused = self
            .tabs
            .get(self.active_tab)
            .map(|t| t.focused)
            .unwrap_or(0);
        let size = self
            .panes
            .get(&focused)
            .map(|s| s.pane.size())
            .unwrap_or(GridSize::new(0, 0));
        let offset = self
            .panes
            .get(&focused)
            .map(|s| s.snapshot.display_offset)
            .unwrap_or(0);

        // Which expansion layer is live for the focused pane. "active" means PSReadLine/Clink is
        // doing it and the shadow layer is standing down, which is otherwise indistinguishable
        // from the outside because both produce the same text.
        let integration = match self.panes.get(&focused).map(|s| s.integration.integration) {
            Some(crate::expand::Integration::Active) => "shell",
            Some(crate::expand::Integration::Pending) => "wait",
            _ => "shadow",
        };

        let text = format!(
            "{:>5.1} fps  {:>5.2} ms  {}x{}  +{}  {} panes  exp:{integration}\n\
             snap {:>5}us  shape {:>5}us ({} rows)\n\
             text {:>5}us  quads {:>5}us",
            perf.fps,
            perf.frame_ms,
            size.columns,
            size.screen_lines,
            offset,
            self.panes.len(),
            self.frame_timings.snapshot_us,
            self.frame_timings.shape_us,
            self.frame_timings.rows_reshaped,
            self.frame_timings.text_prepare_us,
            self.frame_timings.quads_us,
        );
        // Below the find bar when there is one: both want the top-right corner, and the overlay
        // is an `Area` so it is drawn over the bar rather than beside it.
        let top = window::TITLE_HEIGHT + 8.0 + if self.search.open { 38.0 } else { 0.0 };
        egui::Area::new(egui::Id::new("nyx.perf"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, top))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .monospace()
                            .color(visuals::to_color32(self.theme.palette[3]))
                            .background_color(visuals::to_color32(self.theme.surface)),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
    }
}

/// Why a `cd` must not be typed into the focused pane right now, if it must not.
///
/// Pure, so the rule is testable: it depends only on what the pane's terminal and its shell
/// integration are reporting, never on where the request came from — the palette, a leader chord
/// and the footer menu all go through `go_to_shortcut`, and they all get the same answer.
fn navigation_refusal(mode: TermMode, child_running: bool, line: Option<&str>) -> Option<&'static str> {
    if mode.contains(TermMode::ALT_SCREEN) {
        Some("the program on screen owns this pane — exit it, then pick a location")
    } else if child_running {
        Some("a command is still running in this pane — it does not have a prompt to `cd`")
    } else if line != Some("") {
        Some("Finish or clear the current command before navigating")
    } else {
        None
    }
}

fn navigation_command(style: PathStyle, path: &str) -> String {
    let command = match style {
        PathStyle::Windows => "cd /d",
        PathStyle::PowerShell => "Set-Location -LiteralPath",
        _ => "cd",
    };
    format!("{command} {path}\r")
}

#[cfg(test)]
mod tests {
    use super::{elide_back, navigation_refusal};
    use alacritty_terminal::term::TermMode;

    #[test]
    fn navigation_never_submits_unfinished_or_unknown_input() {
        let mut shadow = crate::expand::shadow::Shadow::new();
        shadow.feed(crate::expand::shadow::Feed::Char('x'));
        assert!(navigation_refusal(TermMode::empty(), false, shadow.line()).is_some());
        shadow.feed(crate::expand::shadow::Feed::Desync);
        assert!(navigation_refusal(TermMode::empty(), false, shadow.line()).is_some());
        shadow.feed(crate::expand::shadow::Feed::Submit);
        assert!(navigation_refusal(TermMode::empty(), false, shadow.line()).is_none());
    }

    #[test]
    fn navigation_changes_cmd_drives_and_uses_literal_powershell_paths() {
        use crate::term::profile::PathStyle;
        assert_eq!(super::navigation_command(PathStyle::Windows, r"D:\work"), "cd /d D:\\work\r");
        assert_eq!(super::navigation_command(PathStyle::PowerShell, "'D:\\a[b]'"),
            "Set-Location -LiteralPath 'D:\\a[b]'\r");
    }

    #[test]
    fn navigation_is_refused_where_there_is_no_command_line() {
        // Typing `cd ...` into vim inserts it into the buffer; into a running command it feeds
        // a program that did not ask for it. Both are worse than a message saying why not.
        assert!(navigation_refusal(TermMode::empty(), false, Some("")).is_none());
        assert!(navigation_refusal(TermMode::ALT_SCREEN, false, Some("")).is_some());
        assert!(navigation_refusal(TermMode::empty(), true, Some("")).is_some());
        // The two together: the alternate screen is the answer that matters first.
        assert!(navigation_refusal(TermMode::ALT_SCREEN, true, Some("")).is_some());
    }

    /// One pixel per character, so the rule is exercised rather than watched to "fit".
    fn width(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn a_tab_title_is_cut_from_the_back_and_keeps_its_subject() {
        // The head is what identifies a tab: every title in a repository ends the same way, and
        // cutting from the front would make them all read alike — which is the icon-only strip
        // this replaced.
        assert_eq!(elide_back("cmd", 20.0, width), "cmd");
        let cut = elide_back("Windows PowerShell", 10.0, width);
        assert!(cut.starts_with("Windows"), "{cut}");
        assert!(cut.ends_with('…'), "{cut}");
        assert!(width(&cut) <= 10.0, "{cut} is {} wide", width(&cut));
    }

    #[test]
    fn a_title_with_no_room_is_dropped_rather_than_drawn_over_the_close_button() {
        assert_eq!(elide_back("anything", 0.0, width), "");
        assert_eq!(elide_back("anything", -5.0, width), "");
        assert_eq!(elide_back("   ", 100.0, width), "");
    }

    #[test]
    fn eliding_a_multi_byte_title_lands_on_a_character_boundary() {
        // Cut by chars, not bytes: slicing a CJK or accented title by byte index is a panic on
        // the UI thread, and a tab title is whatever the shell decided to print.
        let cut = elide_back("世界のディレクトリ", 4.0, width);
        assert!(cut.chars().count() <= 4, "{cut}");
        assert!(cut.ends_with('…'), "{cut}");
    }
}
