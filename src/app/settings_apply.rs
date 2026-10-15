//! Applying config: to the running app, to the draft the settings window edits, and
//! back out to disk.
//!
//! The lifecycle is deliberate and stated in CLAUDE.md: the window edits a draft,
//! applies it live so it is a theme editor rather than a form, and saves on a debounce
//! that `on_exit` flushes because there is no frame after the last one.

use super::*;

impl NyxApp {
    /// Re-derive the profile list from detection plus the config's `[[profile]]` overrides.
    pub(super) fn rebuild_profiles(&mut self) -> Vec<String> {
        let (profiles, problems) =
            crate::term::profile::resolve(&self.detected.profiles(), &self.config.profiles);
        self.profiles = profiles;
        problems
    }
    /// Apply a whole config.
    ///
    /// `announce` distinguishes the two callers. A file reload is news and its complaints belong
    /// on screen as toasts; a live edit from the settings window is not — a half-typed hex colour
    /// is invalid for three keystrokes, and toasting each one buries the terminal in warnings
    /// about a value the user is still typing. Those go into the window instead.
    pub(super) fn apply_config(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        config: Arc<Config>,
        problems: Vec<String>,
        announce: bool,
    ) {
        let (theme, theme_problems) = Theme::from_config(&config.theme);
        let (keys, key_problems) = Keys::from_config(&config.keys);
        let (tables, table_problems) = Tables::from_config(&config.shortcuts, &config.aliases);
        let (new_chords, chord_problems) = chords::Chords::from_config(&config.leader);
        let (quake_settings, quake_problems) = quake::Settings::from_config(&config.quake);
        let tables_changed = tables != self.tables;
        self.tables = tables;
        self.chords = new_chords;

        let mut all: Vec<String> = problems
            .into_iter()
            .chain(theme_problems)
            .chain(key_problems)
            .chain(table_problems)
            .chain(chord_problems)
            .chain(quake_problems)
            .collect();

        self.apply_quake(quake_settings);
        if config.sounds != self.config.sounds {
            self.sounds.configure(&config.sounds);
        }
        if config.launcher != self.config.launcher {
            if let Err(err) = crate::win::launcher::configure(&config.launcher) {
                all.push(err);
            }
        }

        // Rewrite the data files. Already-running shells re-read them at their next prompt, so
        // editing config.toml changes shortcuts in open sessions rather than only in new ones.
        //
        // Gated on the tables actually moving: the settings window applies its draft on every
        // keystroke, and six atomic file writes per character typed into a colour field is disk
        // traffic for a change no shell can see.
        if tables_changed {
            // Off the UI thread: seven atomic writes measure 26 ms, and this runs on the
            // keystroke that changed the tables — the settings window applies its draft as you
            // type, so editing a shortcut's path used to drop a frame per character. The worker
            // keeps only the newest request, so a twenty-character path is one rewrite.
            self.shellgen.request(self.tables.clone());
            // Retained, not cleared. The cache is keyed by path, so a shortcut that now points
            // its `exe:` icon somewhere else is already a miss — clearing bought nothing and
            // cost a shell call or a decode and a texture upload per drawn icon, on every
            // keystroke, because the settings window applies its draft as you type. What is
            // dropped here is only what nothing asks for any more, which is also what keeps the
            // failed lookups from a half-typed path from accumulating for the life of the run.
            let live: std::collections::HashSet<String> = self
                .tables
                .shortcuts
                .iter()
                .map(|s| s.icon.as_str())
                .chain(self.tabs.iter().map(|t| t.icon.as_str()))
                .filter_map(|spec| match icons::Icon::parse(spec) {
                    icons::Icon::File(path) | icons::Icon::Exe(path) => Some(path),
                    icons::Icon::Builtin(_) => None,
                })
                .collect();
            self.icon_cache.retain(|path| live.contains(path));
        }

        let font_changed = config.font.family != self.config.font.family
            || config.font.ligatures != self.config.font.ligatures;
        let metrics_changed = config.font.size != self.config.font.size
            || config.font.line_height != self.config.font.line_height
            || config.render.padding != self.config.render.padding;

        self.theme = Arc::new(theme);
        self.keys = keys;
        self.config = config;

        ctx.set_visuals(visuals::visuals(&self.theme));

        all.extend(self.rebuild_profiles());

        // `Term` fixes its history size when it is constructed, so this key cannot reach a shell
        // that is already running. Saying so is the difference between "a setting that does
        // nothing" and "a setting that applies to the next tab".
        if self
            .panes
            .values()
            .any(|s| s.pane.scrollback() != self.config.render.scrollback)
        {
            all.push(format!(
                "render.scrollback = {} applies to new shells; tabs already open keep the depth \
                 they started with",
                self.config.render.scrollback
            ));
        }

        if font_changed || metrics_changed {
            if let Some(render_state) = frame.wgpu_render_state() {
                let mut renderer = render_state.renderer.write();
                if let Some(res) = renderer.callback_resources.get_mut::<GridResources>() {
                    if font_changed
                        && !res.set_font(&self.config.font.family, self.config.font.ligatures)
                    {
                        all.push(format!(
                            "font.family \"{}\" is not installed — using monospace",
                            self.config.font.family
                        ));
                    }
                    self.metrics = res.measure(
                        self.config.font.size * self.scale,
                        self.config.font.line_height,
                        self.config.render.padding * self.scale,
                    );
                }
            }
            self.relayout = true;
        }

        for problem in &all {
            log::warn!("config: {problem}");
        }
        if announce {
            let deadline = Instant::now() + TOAST_LIFETIME;
            self.toasts.extend(all.into_iter().map(|p| (p, deadline)));
        } else {
            self.settings.problems = all;
        }

        for state in self.panes.values() {
            state.pane.mark_dirty();
        }
    }
    /// Move the hotkey registration to match new settings.
    ///
    /// Rebinding rather than restarting the thread: the registration belongs to the thread that
    /// made it, and tearing the thread down for every keystroke typed into the settings window's
    /// hotkey field would mean a thread created and joined per character.
    pub(super) fn apply_quake(&mut self, settings: quake::Settings) {
        // The wheel setting is the system's, not ours, and a config reload is the natural moment
        // to notice someone changed it in Mouse Properties. Above the early return, because
        // below it this only ran on a reload that *also* moved a Quake setting — which is
        // almost none of them, and not what the line was for.
        self.wheel = crate::win::wheel_lines();
        if settings == self.quake_settings {
            return;
        }
        let want_thread = settings.enabled && settings.chord.is_some();

        match (&self.hotkey, want_thread) {
            (Some(hotkey), true) => hotkey.rebind(settings.chord),
            (Some(_), false) => self.hotkey = None,
            (None, true) => {
                let mut toasts = Vec::new();
                self.hotkey = spawn_hotkey(
                    &settings,
                    &self.quake,
                    &self.tx,
                    &self.repaint,
                    &mut toasts,
                    Instant::now() + TOAST_LIFETIME,
                );
                self.toasts.extend(toasts);
            }
            (None, false) => {}
        }

        self.quake_settings = settings;
        // Turning quake off while the window is hidden would otherwise lose it for good.
        self.unhide_if_unreachable();
    }
    /// Adopt the settings window's draft: apply it now, and schedule the disk write.
    pub(super) fn apply_draft(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let draft = Arc::new(self.settings.draft.clone());
        self.apply_config(ctx, frame, draft, Vec::new(), false);
        self.save_at = Some(Instant::now() + SETTINGS_SAVE_DEBOUNCE);
        // The grid callback for this frame already captured the old theme, so the change lands on
        // the next frame — and without asking for one, "the next frame" might be whenever the
        // shell next prints something.
        ctx.request_repaint();
    }
    pub(super) fn flush_settings_save(&mut self, ctx: &egui::Context) {
        let Some(at) = self.save_at else { return };
        let now = Instant::now();
        if now < at {
            ctx.request_repaint_after(at - now);
            return;
        }
        if let Err(err) = self.write_config_now() {
            self.toast(format!("could not save config.toml: {err}"));
        }
    }
    /// Write the config immediately, whatever the debounce has to say about it.
    ///
    /// Shared with `on_exit`, which has no later frame to flush a pending save in.
    pub(super) fn write_config_now(&mut self) -> std::io::Result<()> {
        self.save_at = None;
        let bytes = crate::config::write::save(&self.config)?;
        // Remembered so the watcher's echo of our own write is not taken for an external edit.
        self.last_written = Some(crate::config::watch::hash_bytes(&bytes));
        Ok(())
    }
}
