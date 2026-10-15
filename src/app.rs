//! The `eframe::App` loop.
//!
//! eframe 0.36 splits a frame into `logic` (no painting; also runs while the window is hidden)
//! and `ui` (the paint pass). That split maps cleanly onto what a terminal needs: everything
//! that must happen even when nothing is drawn — draining PTY events, forwarding keystrokes —
//! belongs in `logic`, and only the grid itself belongs in `ui`.
//!
//! The frame is: drain events, lay out the chrome, solve pane rects, reconcile sizes, snapshot
//! each dirty pane, add one paint callback per pane.

mod chrome;
mod copy_blocks;
mod events;
mod find;
mod keys;
mod panes;
mod pointer;
mod settings_apply;
mod sigil;

use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
// Aliased because `layout::focus::Direction` is already `Direction` here, and the two mean
// genuinely different things: one is "which pane is to the left", the other "forwards or
// backwards through the scrollback".
use alacritty_terminal::index::{Direction as SearchDir, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{NamedColor, Rgb};
use crossbeam_channel::{Receiver, Sender};
use rustc_hash::FxHashMap;

use crate::config::keys::{Binding, Keys};
use crate::config::{self, watch, Config};
use crate::event::{AppEvent, RepaintCoalescer};
use crate::expand::{chords, handshake, shadow, Feed, Shadow, Tables};
use crate::layout::focus::Direction;
use crate::layout::geometry::{self, Solved};
use crate::layout::{SplitDir, Tab, TabId};
use crate::render::metrics::CellMetrics;
use crate::render::{GridCallback, GridResources, Timings};
use crate::term::cursor::CursorSettle;
use crate::term::detect::{self, Detected};
use crate::term::profile::{PathStyle, Profile};
use crate::term::search::{self, Search};
use crate::term::snapshot::{self, GridSnapshot};
use crate::term::{input, kitty, mouse, select, GridSize, Pane, PaneId};
use crate::theme::{visuals, Theme};
use crate::ui::{findbar, icons, palette, settings, window};
use crate::win::{hotkey, ipc, quake, WheelStep};

/// How long a config problem stays on screen. Long enough to read a colour name, short enough
/// not to sit over the terminal while you fix it.
const TOAST_LIFETIME: Duration = Duration::from_secs(8);

/// How long to keep waking up while a resize is settling, so the debounce in `Pane::flush_size`
/// actually fires.
const RESIZE_POLL: Duration = Duration::from_millis(20);

/// Long enough to notice, short enough not to be a strobe when a build script rings it in a loop.
const BELL_FLASH: Duration = Duration::from_millis(120);

/// Two clicks inside this window on the same cell mean "select the word".
const MULTI_CLICK: Duration = Duration::from_millis(400);

/// How many frames must be painted before the window may be hidden.
///
/// `start_hidden` cannot fire on the first one: hiding before anything has been drawn means the
/// wgpu surface is configured against a window that is not on screen, and what comes back when it
/// reappears is a frame of whatever was in the buffer.
const FRAMES_BEFORE_HIDING: u64 = 3;

/// How long the settings window waits before writing `config.toml`.
///
/// A colour-picker drag produces a change every frame, and each one is a file write plus a
/// watcher round trip. Long enough to collapse a drag into one save, short enough that closing
/// the window immediately after a change still catches it.
const SETTINGS_SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// How long the find bar waits, after the last keystroke, before searching the whole buffer.
///
/// A scan walks the entire history — 214–217 ms over 100k rows, against a configurable ceiling of
/// a million — so one per keystroke is that much work per keystroke. It no longer *stalls*
/// anything, because the walk is on a thread of its own and `Search::request` keeps a second one
/// from starting, but a burst of typing would still queue a scan behind every character. Short
/// enough to feel incremental, long enough that ordinary typing collapses to one search.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);

const GUTTER: f32 = 5.0;

struct PaneState {
    pane: Pane,
    snapshot: Arc<GridSnapshot>,
    /// Best-effort model of the current input line, for terminal-side expansion.
    shadow: Shadow,
    /// How this pane's shell wants a Windows path spelled.
    style: PathStyle,
    /// Whether this shell announced native expansion, and what it has told us since.
    integration: handshake::State,
    /// Per-pane, per-launch secret, so nothing else can impersonate the shell.
    token: String,
    /// Where the cursor is *drawn*, which trails the terminal's across an unproven jump.
    cursor: CursorSettle,
    copy_blocks: crate::term::blocks::Blocks,
}

pub struct NyxApp {
    config: Arc<Config>,
    keys: Keys,
    theme: Arc<Theme>,
    metrics: CellMetrics,
    scale: f32,
    srgb: bool,

    panes: FxHashMap<PaneId, PaneState>,
    /// Panes whose shells are gone and whose GPU resources are still to be released.
    ///
    /// Releasing them needs the wgpu render state, which is only in hand inside a frame, and
    /// panes are closed from event handling and key bindings that do not have one — so the id is
    /// queued here and `retire_panes` does the work on the next pass.
    retired: Vec<PaneId>,
    tabs: Vec<Tab>,
    active_tab: usize,
    next_pane_id: PaneId,
    next_tab_id: TabId,

    profiles: Vec<Profile>,
    detected: Detected,
    tables: Tables,
    /// Rewrites the generated shell files. Off the UI thread because they cost 26 ms of disk.
    shellgen: crate::expand::shellgen::Writer,
    chords: chords::Chords,
    chord_state: chords::ChordState,
    palette: palette::State,
    /// The sigil menu's highlight, its dismissal and the cursor cell it hangs from.
    completion: crate::ui::complete::State,
    /// One search, applied to whichever pane has focus — `Search::owner` is what keeps a `Point`
    /// from one grid being used to scroll another.
    search: Search,
    /// Matches on screen at the last snapshot, for the find bar's counter.
    search_visible: usize,
    settings: settings::State,
    /// Fingerprint of the last file we wrote ourselves, so the watcher's echo can be ignored.
    last_written: Option<u64>,
    /// When the debounced settings save is due.
    save_at: Option<Instant>,
    /// When the debounced incremental search is due.
    search_at: Option<Instant>,
    /// Last rectangle handed to the IME, so the viewport command is only sent when it moves.
    ime_rect: Option<egui::Rect>,

    /// Last frame's solved layout, so mouse events can be routed without re-solving.
    solved: Solved,

    rx: Receiver<AppEvent>,
    tx: Sender<AppEvent>,
    repaint: Arc<RepaintCoalescer>,

    clipboard: Option<arboard::Clipboard>,
    copied_at: Option<Instant>,
    mouse: MouseState,
    /// How many lines one wheel notch scrolls, from the system setting rather than a guess.
    wheel: WheelStep,
    bell_until: Option<Instant>,
    focused: bool,

    /// The dropdown. Inert — no hotkey thread, no geometry — unless `[quake] enabled`.
    quake: quake::Quake,
    quake_settings: quake::Settings,
    /// The thread that owns the global hotkey. `None` when quake is off, or when the chord could
    /// not be claimed.
    hotkey: Option<hotkey::Hotkey>,
    sounds: crate::win::sounds::Sounds,
    /// Whether the window handle has reached the hotkey thread yet; it only exists once eframe
    /// has made the window, which is after `NyxApp::new`.
    window_published: bool,
    /// `[quake] start_hidden`, waiting for enough frames to have been painted.
    pending_start_hidden: bool,
    /// Something wants the window in front — an Explorer click that landed in this instance.
    /// Deferred because raising it is a viewport command and `drain_events` has no context.
    reveal: bool,
    /// Textures for `icon = "exe:..."`, so a tab or menu entry does not ask the shell about the same
    /// executable sixty times a second.
    icon_cache: icons::Cache,

    _config_watcher: Option<watch::ConfigWatcher>,
    pending_config: Option<(Arc<Config>, Vec<String>)>,
    toasts: Vec<(String, Instant)>,
    custom_cursor: crate::win::cursor::CustomCursor,
    cursor_on: bool,
    cursor_phase: Instant,

    frame: u64,
    relayout: bool,
    perf: Option<Perf>,
    timings: Arc<Timings>,
    /// The last complete frame's totals, summed over every pane. See `Timings::take_frame`.
    frame_timings: crate::render::FrameTimings,
}

#[derive(Clone, Copy)]
struct MouseCapture {
    pane: PaneId,
    button: mouse::Button,
    point: Point,
    offset: usize,
    mode: TermMode,
    modifiers: egui::Modifiers,
}

#[derive(Default)]
struct MouseState {
    selecting: Option<PaneId>,
    /// Each press belongs to its originating pane until release, even outside the grid.
    captures: Vec<MouseCapture>,
    /// The last cell a motion report was sent for, and the pane it belonged to. Two panes can
    /// hold the same cell coordinate, so the pane is part of the key: without it, a hover that
    /// moved between two panes at matching coordinates would be mistaken for "no change" and
    /// the second pane would never hear about the pointer arriving.
    reported_cell: Option<(PaneId, Point)>,
    last_click: Option<(Instant, Point)>,
    click_count: u32,
    scroll_accum: f32,
}

impl MouseState {
    fn take_releases(&mut self, button: Option<mouse::Button>) -> Vec<MouseCapture> {
        let (released, retained) = std::mem::take(&mut self.captures).into_iter()
            .partition(|capture| button.is_none_or(|button| button == capture.button));
        self.captures = retained;
        self.reported_cell = None;
        if button.is_none() || button == Some(mouse::Button::Left) { self.selecting = None; }
        released
    }
}

struct Perf {
    last: Instant,
    frame_ms: f32,
    frames: u32,
    window_start: Instant,
    fps: f32,
}

impl NyxApp {
    /// `config` and `problems` come from `Loaded::or_defaults` — the first load is the one place
    /// where a file that did not parse legitimately means the defaults, because nothing is
    /// running yet for it to keep.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: config::Config,
        problems: Vec<String>,
        startup: ipc::Startup,
    ) -> anyhow::Result<Self> {
        let config = Arc::new(config);
        let (theme, theme_problems) = Theme::from_config(&config.theme);
        let (keys, key_problems) = Keys::from_config(&config.keys);
        let (tables, table_problems) = Tables::from_config(&config.shortcuts, &config.aliases);
        let (chords, chord_problems) = chords::Chords::from_config(&config.leader);
        let (quake_settings, quake_problems) = quake::Settings::from_config(&config.quake);
        log::info!(
            "leader chords: enabled={} key={:?} count={}",
            !chords.is_empty(),
            config.leader.key,
            config.leader.chords.len()
        );
        let theme = Arc::new(theme);

        let mut toasts: Vec<(String, Instant)> = Vec::new();
        let deadline = Instant::now() + TOAST_LIFETIME;
        for problem in problems
            .into_iter()
            .chain(theme_problems)
            .chain(key_problems)
            .chain(table_problems)
            .chain(chord_problems)
            .chain(quake_problems)
        {
            log::warn!("config: {problem}");
            toasts.push((problem, deadline));
        }

        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Nyx must be built with eframe's wgpu backend"))?;

        let timings = Arc::new(Timings::default());
        let mut resources = GridResources::new(
            &render_state.device,
            &render_state.queue,
            render_state.target_format,
            String::new(),
            timings.clone(),
        );

        if !resources.set_font(&config.font.family, config.font.ligatures) {
            let message = format!(
                "font.family \"{}\" is not installed — using monospace",
                config.font.family
            );
            log::warn!("{message}");
            toasts.push((message, deadline));
        }

        let scale = cc.egui_ctx.pixels_per_point();
        let metrics = resources.measure(
            config.font.size * scale,
            config.font.line_height,
            config.render.padding * scale,
        );

        cc.egui_ctx.set_visuals(visuals::visuals(&theme));

        // egui's default quit shortcut is Ctrl+Q (`Options::quit_shortcuts`), and `end_pass`
        // answers it with `ViewportCommand::Close` — closing a terminal *and every shell in it*
        // on a chord that at a prompt is the flow-control resume, Ctrl+Q after Ctrl+S. Disabled
        // here rather than filtered out of the event list every frame: the per-frame filter also
        // stopped Ctrl+Q ever reaching the shell, which is Nyx taking the key away to protect
        // egui from it.
        cc.egui_ctx.options_mut(|o| o.quit_shortcuts.clear());

        let (tx, rx) = crossbeam_channel::unbounded();
        let repaint = Arc::new(RepaintCoalescer::new(cc.egui_ctx.clone()));

        // Filesystem-only, so it is sub-millisecond and the first tab gets a real profile.
        let detected = detect::detect();
        let (profiles, profile_problems) =
            crate::term::profile::resolve(&detected.profiles(), &config.profiles);
        for problem in profile_problems {
            log::warn!("config: {problem}");
            toasts.push((problem, deadline));
        }

        // WSL enumeration shells out, so it happens off the launch path and the profile list
        // grows when the answer arrives.
        if detected.wsl.is_some() {
            let tx = tx.clone();
            let repaint = repaint.clone();
            std::thread::Builder::new()
                .name("nyx-wsl-detect".into())
                .spawn(move || {
                    let distros = detect::wsl_distros();
                    if !distros.is_empty() {
                        let _ = tx.send(AppEvent::WslDistros(distros));
                        repaint.poke();
                    }
                })
                .ok();
        }

        render_state
            .renderer
            .write()
            .callback_resources
            .insert(resources);

        let clipboard = match arboard::Clipboard::new() {
            Ok(c) => Some(c),
            Err(err) => {
                log::warn!("clipboard unavailable: {err}");
                None
            }
        };

        let config_watcher = watch::spawn(tx.clone(), repaint.clone());

        // Written before the first shell starts: a pane spawned against a missing script would
        // be told it has integration and then wait 2.5 s for a greeting that cannot arrive.
        if let Err(err) = crate::expand::shellgen::regenerate(&tables) {
            log::warn!("could not write shell integration scripts: {err}");
            toasts.push((format!("shell integration unavailable: {err}"), deadline));
        }

        // Started before the hotkey thread is handed a clone of its shared state, so there is one
        // `Quake` and one `Arc` for the whole run.
        let quake = quake::Quake::default();
        let hotkey = spawn_hotkey(
            &quake_settings,
            &quake,
            &tx,
            &repaint,
            &mut toasts,
            deadline,
        );

        // Only the process holding the single-instance claim listens. A second window that got
        // this far did so *because* the claim was already taken, and two servers on one pipe name
        // is a coin toss over which window an Explorer click lands in.
        if startup.primary {
            if let Err(err) = ipc::serve(tx.clone(), repaint.clone()) {
                log::warn!("could not start the IPC listener: {err}");
            }
        }

        let sounds = crate::win::sounds::Sounds::new(&config.sounds);
        let mut app = Self {
            config,
            keys,
            theme,
            metrics,
            scale,
            srgb: render_state.target_format.is_srgb(),
            panes: FxHashMap::default(),
            retired: Vec::new(),
            tabs: Vec::new(),
            active_tab: 0,
            next_pane_id: 1,
            next_tab_id: 1,
            profiles,
            detected,
            tables,
            shellgen: crate::expand::shellgen::Writer::spawn(),
            chords,
            chord_state: chords::ChordState::default(),
            palette: palette::State::default(),
            completion: crate::ui::complete::State::default(),
            search: Search::default(),
            search_visible: 0,
            settings: settings::State::default(),
            last_written: None,
            save_at: None,
            search_at: None,
            ime_rect: None,
            solved: Solved::default(),
            rx,
            tx,
            repaint,
            clipboard,
            copied_at: None,
            mouse: MouseState::default(),
            wheel: crate::win::wheel_lines(),
            bell_until: None,
            focused: true,
            pending_start_hidden: quake_settings.enabled
                && quake_settings.start_hidden
                // A window hidden with no working hotkey to bring it back is a window the user
                // has lost, so this refuses to start hidden rather than trusting the chord.
                && quake_settings.chord.is_some()
                && hotkey.is_some(),
            quake,
            quake_settings,
            sounds,
            hotkey,
            window_published: false,
            reveal: false,
            icon_cache: icons::Cache::default(),
            _config_watcher: config_watcher,
            pending_config: None,
            toasts,
            custom_cursor: Default::default(),
            cursor_on: true,
            cursor_phase: Instant::now(),
            frame: 0,
            relayout: true,
            perf: std::env::var_os("NYX_PERF").map(|_| Perf {
                last: Instant::now(),
                frame_ms: 0.0,
                frames: 0,
                window_start: Instant::now(),
                fps: 0.0,
            }),
            timings,
            frame_timings: crate::render::FrameTimings::default(),
        };

        let profile = app.profiles.first().cloned();
        // `--cwd` from the command line, or from Explorer's context menu on a cold start.
        app.new_tab(profile.as_ref(), startup.request.sanitized().cwd)?;
        Ok(app)
    }
}

/// Start the hotkey thread, if there is anything for it to register.
///
/// Free rather than a method because `NyxApp::new` calls it before there is a `self`, and having
/// two versions of "how do we start the hotkey" is how the constructor and a config reload end up
/// disagreeing about what enabled means.
fn spawn_hotkey(
    settings: &quake::Settings,
    quake: &quake::Quake,
    tx: &Sender<AppEvent>,
    repaint: &Arc<RepaintCoalescer>,
    toasts: &mut Vec<(String, Instant)>,
    deadline: Instant,
) -> Option<hotkey::Hotkey> {
    if !settings.enabled || settings.chord.is_none() {
        return None;
    }
    match hotkey::Hotkey::spawn(
        settings.chord,
        quake.shared.clone(),
        tx.clone(),
        repaint.clone(),
    ) {
        Ok(hotkey) => Some(hotkey),
        Err(err) => {
            let message = format!("could not start the global hotkey: {err}");
            log::warn!("{message}");
            toasts.push((message, deadline));
            None
        }
    }
}

fn trim_title(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= 24 {
        return title.to_string();
    }
    let kept: String = title.chars().take(23).collect();
    format!("{kept}…")
}

/// Encode a frame's input, expanding shortcuts and aliases on the way past.
///
/// Walked event by event rather than handed to `input::translate` wholesale, because an expansion
/// has to be emitted *before* the space or Enter that triggered it — the ordering is the whole
/// mechanism, and a batch translate would put the rewrite in the wrong place.
///
/// `typed` is what the layout produced for the key being translated, when it produced anything.
/// The caller already works it out for the leader chords, and the kitty encoding needs the same
/// thing for its associated-text and shifted-alternate fields — passed in rather than looked up
/// here, because this is called with one event at a time and cannot see the `Text` beside it.
#[allow(clippy::too_many_arguments)]
fn translate_with_expansion(
    shadow: &mut Shadow,
    tables: &Tables,
    style: PathStyle,
    integration_active: bool,
    events: &[egui::Event],
    modifiers: egui::Modifiers,
    mode: TermMode,
    typed: Option<&str>,
) -> input::Translated {
    let mut out = input::Translated::default();
    // The same rule `input::translate` applies, from the same function. This arm exists only
    // because an expansion has to be emitted *before* the space that triggered it; what a `Text`
    // event means under Ctrl or Alt is not this path's business to decide a second time.
    let rule = input::text_rule(modifiers);

    for event in events {
        // The kitty keyboard protocol, when a program has turned it on. Asked before the match
        // below rather than as an arm in it, because the keys it exists for — Shift+Enter above
        // all — are ones those arms would otherwise claim and flatten back into the very byte
        // the protocol was asked to replace.
        //
        // It hands most keys straight back (`None`): plain typing and an unmodified Enter still
        // take the ordinary path, which is what keeps a shell usable in a pane whose last
        // full-screen program exited without resetting the mode.
        if let egui::Event::Key { key, pressed, modifiers: m, repeat, .. } = event {
            if let Some(bytes) =
                kitty::encode(*key, *m, mode, kitty::Event::of(*pressed, *repeat), typed)
            {
                // A line edited through a protocol whose effects this model does not read is a
                // line it cannot describe. Shift+Enter in particular is emphatically *not* a
                // submit: treating it as one would leave the model believing the line is empty
                // while the program is still holding two lines of it.
                shadow.feed(Feed::Desync);
                out.bytes.extend(bytes);
                out.scroll_to_bottom = true;
                out.swallow_text = true;
                continue;
            }
        }

        match event {
            // Ctrl already spoke as a C0 byte through the `other` arm below; the text is a
            // duplicate of it.
            egui::Event::Text(_) if rule.suppress => {}

            // Meta sends escape, and an Alt chord is never an expansion trigger — the `Key` event
            // beside it has already desynced the model.
            egui::Event::Text(text) if rule.meta => {
                out.bytes.push(0x1b);
                out.bytes.extend_from_slice(text.as_bytes());
                out.scroll_to_bottom = true;
            }

            egui::Event::Text(text) => {
                for c in text.chars() {
                    if c == ' ' {
                        match shadow.on_space(tables, style, mode, integration_active) {
                            Some(expansion) => {
                                out.bytes.extend(shadow::bytes_for(&expansion));
                                shadow.apply(&expansion);
                            }
                            // Under integration the shell does the rewriting and this layer
                            // stands down — but the model is still the only description of the
                            // line that anything has, and it was left holding `@nyx` while the
                            // line editor showed a path. Follow the shell's edit and emit
                            // nothing; see `Shadow::mirror_shell_expansion`.
                            None if integration_active => {
                                shadow.mirror_shell_expansion(tables, style, mode)
                            }
                            None => {}
                        }
                    }
                    shadow.feed(Feed::Char(c));
                    let mut buf = [0u8; 4];
                    out.bytes
                        .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                out.scroll_to_bottom = true;
            }

            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                modifiers: m,
                ..
            } if !m.ctrl && !m.alt => {
                if let Some(expansion) = shadow.on_submit(tables, style, mode, integration_active) {
                    out.bytes.extend(shadow::bytes_for(&expansion));
                    shadow.apply(&expansion);
                }
                shadow.feed(Feed::Submit);
                out.bytes.push(0x0d);
                out.scroll_to_bottom = true;
            }

            other => {
                if let Some(feed) = feed_for(other) {
                    shadow.feed(feed);
                }
                let one = input::translate(std::slice::from_ref(other), modifiers, mode);
                out.bytes.extend(one.bytes);
                out.copy |= one.copy;
                out.paste |= one.paste;
                out.scroll = one.scroll.or(out.scroll);
                out.scroll_to_bottom |= one.scroll_to_bottom;
            }
        }
    }

    out
}

/// How an event affects the shadow model, if it can be followed at all.
fn feed_for(event: &egui::Event) -> Option<Feed> {
    match event {
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            if crate::config::keys::is_modifier(*key) {
                return None;
            }
            // Any control or alt chord can do arbitrary things to the line — kill-to-end,
            // word-rubout, reverse search — so the model gives up rather than guessing.
            if modifiers.ctrl || modifiers.alt {
                return Some(Feed::Desync);
            }
            match key {
                egui::Key::Backspace => Some(Feed::Backspace),
                // History recall, completion and cursor movement all rewrite the line in ways
                // nothing here can see.
                egui::Key::ArrowUp
                | egui::Key::ArrowDown
                | egui::Key::ArrowLeft
                | egui::Key::ArrowRight
                | egui::Key::Tab
                | egui::Key::Delete
                | egui::Key::Home
                | egui::Key::End
                | egui::Key::PageUp
                | egui::Key::PageDown
                | egui::Key::Escape => Some(Feed::Desync),
                _ => None,
            }
        }
        // Pasted text can contain newlines and control characters; following it is not worth the
        // risk of being subtly wrong about where the line now ends.
        egui::Event::Paste(_) | egui::Event::Ime(egui::ImeEvent::Commit(_)) => Some(Feed::Desync),
        _ => None,
    }
}

fn to_button(b: egui::PointerButton) -> Option<mouse::Button> {
    match b {
        egui::PointerButton::Primary => Some(mouse::Button::Left),
        egui::PointerButton::Middle => Some(mouse::Button::Middle),
        egui::PointerButton::Secondary => Some(mouse::Button::Right),
        _ => None,
    }
}

impl eframe::App for NyxApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.bg.to_target(self.srgb)
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Err(err) = self
            .custom_cursor
            .configure(frame, &self.config.render.cursor_icon)
        {
            self.toast(format!("Could not load cursor: {err}"));
        }
        self.repaint.consume();
        self.frame += 1;
        // Close the previous frame's timings here, before any pane starts adding to the next
        // one. Taken unconditionally: five atomic swaps, and gating it on the overlay being on
        // would leave the counters climbing for the whole run when it is off.
        self.frame_timings = self.timings.take_frame();
        self.release_chrome_focus(ctx);

        // The window only exists once eframe has made it, which is after `NyxApp::new`. The
        // hotkey thread needs the handle to raise a hidden window without waiting for a frame.
        if !self.window_published {
            if let Some(window) = crate::win::Window::of(frame) {
                self.quake.shared.publish(window);
                self.window_published = true;
            }
            // A terminal is a text field for its whole surface, but nothing here is an egui text
            // widget — so nothing ever asked the platform to start an input method, and CJK and
            // Korean input had no way in at all.
            ctx.send_viewport_cmd(egui::ViewportCommand::IMEAllowed(true));
        }

        self.drain_events();
        self.retire_panes(frame);

        // A Pending integration expires into Absent once its greeting is late — 2.5 s of a
        // script that never loaded. Once per frame for every pane, not per keystroke: a pane
        // whose script failed at spawn must get its shadow layer back whether or not anyone has
        // typed into it yet.
        for state in self.panes.values_mut() {
            state.integration.tick();
        }

        if let Some((config, problems)) = self.pending_config.take() {
            self.apply_config(ctx, frame, config, problems, true);
            // Somebody edited the file in another editor. Keeping the draft would mean the next
            // keystroke in the settings window silently reverted their edit.
            if self.settings.open {
                self.settings.adopt(&self.config);
            }
        }

        self.flush_settings_save(ctx);
        self.flush_pending_search(ctx);
        self.toasts.retain(|(_, until)| Instant::now() < *until);

        // Every pane gone means every shell exited.
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if std::mem::take(&mut self.reveal) {
            if self.quake.hidden() {
                self.quake.show(self.quake_settings.geometry);
                self.mark_all_dirty();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // The slide is the one thing in Nyx that asks for frames it did not earn, and it stops
        // asking the moment it lands.
        if let Some(next) = self.quake.tick() {
            ctx.request_repaint_after(next);
        }

        // `start_hidden`, once there has actually been something to hide.
        if self.pending_start_hidden && self.window_published && self.frame >= FRAMES_BEFORE_HIDING
        {
            self.pending_start_hidden = false;
            // No slide on the way in: a window that visibly retracts a tenth of a second after
            // launching looks like a crash caught halfway.
            self.quake.hide(quake::Geometry {
                animate: Duration::ZERO,
                ..self.quake_settings.geometry
            });
        }

        if self.quake.hidden() {
            // eframe keeps calling `logic` while the window is hidden — that is how the hotkey
            // gets a chance to ask for it back — but the input it hands us is the *last shown
            // frame's*, replayed every tick until a real pass consumes it. Going any further
            // would type that frame's keystrokes into the shell again on every tick.
            return;
        }

        let focused = ctx.input(|i| i.focused);
        if focused != self.focused {
            if !focused { self.release_mouse(None, None); }
            self.focused = focused;
            for state in self.panes.values() {
                // Every pane in the window, not just the focused one. The mode reports what the
                // *window* did, and in a split every pane is equally on screen — telling the
                // other two they lost focus would have them dim or pause content the user is
                // still looking at. Moving focus between panes is deliberately not reported for
                // the same reason.
                if let Some(report) = input::focus_report(state.snapshot.mode, focused) {
                    state.pane.write(report.to_vec());
                }
                state.pane.mark_dirty();
            }
            if !focused
                && self.quake_settings.enabled
                && self.quake_settings.hide_on_focus_loss
                && self.window_published
                && !self.quake.animating()
            {
                self.quake.hide(self.quake_settings.geometry);
                ctx.request_repaint();
                return;
            }
        }

        let scale = ctx.pixels_per_point();
        if (scale - self.scale).abs() > 1e-4 {
            self.scale = scale;
            if let Some(render_state) = frame.wgpu_render_state() {
                let mut renderer = render_state.renderer.write();
                if let Some(res) = renderer.callback_resources.get_mut::<GridResources>() {
                    self.metrics = res.measure(
                        self.config.font.size * scale,
                        self.config.font.line_height,
                        self.config.render.padding * scale,
                    );
                }
            }
            self.relayout = true;
            for state in self.panes.values() {
                state.pane.mark_dirty();
            }
        }

        // Whether the *terminal* wants a cursor at all — not whether the last snapshot drew one.
        // A snapshot is captured with the blink phase folded in, so `snapshot.cursor` is `None`
        // for a cursor that is merely blinked off. Reading that made the controller see its own
        // off-frame as the program having hidden the cursor, fall into the branch below, and turn
        // it straight back on: the off phase lasted exactly one frame and the cursor never
        // visibly blinked.
        let cursor_visible = self
            .focused_pane()
            .and_then(|p| self.panes.get(&p))
            .is_some_and(|s| s.snapshot.mode.contains(TermMode::SHOW_CURSOR));
        if self.config.render.cursor_blink && self.focused && cursor_visible {
            let period = Duration::from_millis(self.config.render.cursor_blink_ms);
            if self.cursor_phase.elapsed() >= period {
                self.cursor_phase = Instant::now();
                self.cursor_on = !self.cursor_on;
                if let Some(p) = self.focused_pane() {
                    if let Some(state) = self.panes.get(&p) {
                        state.pane.mark_dirty();
                    }
                }
            }
            ctx.request_repaint_after(period.saturating_sub(self.cursor_phase.elapsed()));
        } else if !self.cursor_on {
            self.cursor_on = true;
            if let Some(p) = self.focused_pane() {
                if let Some(state) = self.panes.get(&p) {
                    state.pane.mark_dirty();
                }
            }
        }

        // ---- keyboard ----
        let (events, modifiers) = ctx.input(|i| (i.events.clone(), i.modifiers));
        if ctx.input(|i| i.focused) && (self.settings.open || self.palette.open || self.search.open)
        {
            self.sounds.events(&events);
        }

        if egui::Popup::is_any_open(ctx) {
            return;
        }

        // Three modal layers, in priority order. Each owns the keyboard completely while it is
        // up: a keystroke reaching the shell from behind a search box or a settings window is how
        // you end up running your query, or your new keybinding, as a command.
        if self.settings.open {
            if self.handle_settings_keys(&events) {
                self.apply_draft(ctx, frame);
            }
            return;
        }
        if self.palette.open {
            self.handle_palette_keys(&events);
            return;
        }
        if self.search.open {
            self.handle_find_keys(&events);
            return;
        }

        let events = keys::deduplicate_copy_events(events);
        // Route in arrival order: a Text event must update the line before Enter inspects it.
        let has_copy_key = events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::C, pressed: true, modifiers: m, .. } if m.ctrl && !m.alt));
        let mut swallow_text = false;
        let mut copied = false;
        let mut text_modifiers = modifiers;
        for (index, event) in events.iter().enumerate() {
            // What the layout produced for this key, if anything: the `Text` egui delivers
            // after it and before the next key press. The leader chords want the first
            // character of it; the kitty encoding wants the whole string, for its
            // associated-text and shifted-alternate fields.
            let typed_text = events[index + 1..]
                .iter()
                .take_while(|e| !matches!(e, egui::Event::Key { pressed: true, .. }))
                .find_map(|e| {
                    if let egui::Event::Text(t) = e {
                        Some(t.as_str())
                    } else {
                        None
                    }
                });
            let typed = typed_text.and_then(|t| t.chars().next());
            if matches!(event, egui::Event::Copy) {
                if has_copy_key || copied {
                    continue;
                }
                copied = true;
                if !self.copy_selection(ctx) && !modifiers.shift {
                    self.interrupt_focused();
                }
                continue;
            }
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } = event
            {
                if crate::config::keys::is_modifier(*key) {
                    continue;
                }
                text_modifiers = *m;
                swallow_text = false;
                // AltGr produces printable Text; its Ctrl+Alt Key must not desync the line.
                if m.ctrl && m.alt && typed.is_some() {
                    if ctx.input(|i| i.focused) {
                        self.sounds.events(std::slice::from_ref(event));
                    }
                    continue;
                }
                if *key == egui::Key::C && m.ctrl && !m.alt {
                    if !copied {
                        copied = true;
                        if !self.copy_selection(ctx) && !m.shift {
                            self.interrupt_focused();
                        }
                    }
                    continue;
                }
                if !self.chords.is_empty() {
                    match self.chord_state.press(&self.chords, *key, *m, typed) {
                        chords::Outcome::Ignored => {}
                        chords::Outcome::Armed | chords::Outcome::Cancelled => {
                            swallow_text = true;
                            continue;
                        }
                        chords::Outcome::Run(command) => {
                            swallow_text = true;
                            self.run_chord(command);
                            continue;
                        }
                    }
                }
                if self.handle_bindings(*key, *m) {
                    swallow_text = true;
                    if self.settings.open || self.palette.open || self.search.open {
                        break;
                    }
                    continue;
                }
                if ctx.input(|i| i.focused) {
                    self.sounds.events(std::slice::from_ref(event));
                }
                if self.handle_sigil_key(*key, *m) {
                    continue;
                }
            }
            if matches!(event, egui::Event::Text(_)) && swallow_text {
                swallow_text = false;
                continue;
            }
            let Some(pane) = self.focused_pane() else {
                continue;
            };
            let Some(state) = self.panes.get_mut(&pane) else {
                continue;
            };
            let translated = translate_with_expansion(
                &mut state.shadow,
                &self.tables,
                state.style,
                state.integration.suppresses_shadow(),
                std::slice::from_ref(event),
                text_modifiers,
                state.snapshot.mode,
                typed_text,
            );
            // The kitty encoding put this key's character inside the sequence it sent, so the
            // `Text` arriving next would type it a second time.
            swallow_text |= translated.swallow_text;
            if translated.copy {
                self.copy_selection(ctx);
            }
            if translated.paste {
                self.paste_clipboard();
            }
            if let Some(state) = self.panes.get(&pane) {
                if let Some(scroll) = translated.scroll {
                    state.pane.scroll(scroll);
                }
                if !translated.bytes.is_empty() {
                    if translated.scroll_to_bottom {
                        state.pane.scroll(Scroll::Bottom);
                    }
                    state.pane.clear_selection();
                    state.pane.write(translated.bytes);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let popup_owned_pointer = egui::Popup::is_any_open(&ctx);

        let chrome = window::regions(full);
        let window_drag = window::title_bar(ui, chrome.title, &self.theme);
        let tab_buttons = self.tab_strip(ui, chrome.tabs);
        window::drag_action(ui, &window_drag, &tab_buttons);
        self.location_footer(ui, chrome.footer);
        let rect = chrome.terminal;
        ui.painter()
            .rect_filled(rect, 0.0, visuals::to_color32(self.theme.bg));

        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let root = tab.layout_root();
        let focused_pane = tab.focused;
        self.solved = geometry::solve(&root, rect, GUTTER);

        // Gutter drags. Done before the panes so the cursor feedback is not masked by the
        // terminal's own text cursor.
        let mut ratio_change: Option<(Vec<bool>, f32)> = None;
        for gutter in &self.solved.gutters {
            let id = egui::Id::new(("nyx.gutter", self.active_tab, &gutter.path));
            let response = ui.interact(gutter.rect, id, egui::Sense::drag());
            if response.hovered() || response.dragged() {
                ctx.set_cursor_icon(match gutter.dir {
                    SplitDir::Horizontal => egui::CursorIcon::ResizeHorizontal,
                    SplitDir::Vertical => egui::CursorIcon::ResizeVertical,
                });
                ui.painter().rect_filled(
                    gutter.rect,
                    0.0,
                    visuals::to_color32(self.theme.palette[8]),
                );
            }
            if response.dragged() {
                if let Some(pos) = ctx.pointer_interact_pos() {
                    // Recomputed from the pointer rather than accumulated from the delta, so a
                    // fast drag cannot desynchronise the gutter from the cursor.
                    let parent = gutter.parent;
                    let raw = match gutter.dir {
                        SplitDir::Horizontal => (pos.x - parent.min.x - GUTTER / 2.0) / (parent.width() - GUTTER).max(1.0),
                        SplitDir::Vertical => (pos.y - parent.min.y - GUTTER / 2.0) / (parent.height() - GUTTER).max(1.0),
                    };
                    let total = match gutter.dir {
                        SplitDir::Horizontal => parent.width(),
                        SplitDir::Vertical => parent.height(),
                    };
                    let min_px = match gutter.dir {
                        SplitDir::Horizontal => self.metrics.cell_w * 8.0 / self.scale,
                        SplitDir::Vertical => self.metrics.cell_h * 2.0 / self.scale,
                    };
                    ratio_change = Some((
                        gutter.path.clone(),
                        geometry::clamp_ratio(raw, total, GUTTER, min_px),
                    ));
                }
            }
        }

        if let Some((path, ratio)) = ratio_change {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                if let Some(crate::layout::Node::Split { ratio: r, .. }) = tab.root.at_mut(&path) {
                    *r = ratio;
                }
            }
            self.relayout = true;
            // Re-solve immediately so the frame being drawn already reflects the drag.
            if let Some(tab) = self.tabs.get(self.active_tab) {
                self.solved = geometry::solve(&tab.layout_root(), rect, GUTTER);
            }
        }

        let copy_hover = self.copy_block_hover(ui);
        let copy_buttons = copy_hover
            .as_ref()
            .map(|hover| crate::ui::copyblock::buttons(hover.rect))
            .unwrap_or_default();
        if !popup_owned_pointer {
            // Everything drawn over the grid has to be subtracted from the grid's own pointer
            // handling by hand — `handle_mouse` reads raw events rather than going through
            // `Ui::interact`, so egui's layering does not protect the terminal.
            let blocks: Vec<egui::Rect> = self
                .search
                .open
                .then(|| findbar::rect(rect))
                .into_iter()
                .chain(self.completion.blocked)
                .collect();
            self.handle_mouse(ui, &blocks, &copy_buttons);
        }

        // Re-read after the click: `handle_mouse` may have just moved focus, and this frame's
        // captures are what decide whether each pane's cursor is solid or hollow. Using the value
        // from before the click leaves the pane that lost focus still drawn as focused — and
        // `set_focus` has already spent its `mark_dirty` this frame, so the panes are re-captured
        // with the stale answer and then marked clean, with nothing scheduled to correct them.
        let focused_pane = self.focused_pane().unwrap_or(focused_pane);

        if ui.rect_contains_pointer(rect) {
            let pos = ctx.pointer_interact_pos().unwrap_or_default();
            if let Some(pane) = self.solved.pane_at(pos) {
                // A pointing hand is the whole discoverability story for Ctrl+click: holding
                // Ctrl over a link is how anyone finds out the link is one. Only while Ctrl is
                // held, so an ordinary pass over a wall of log output does not flicker.
                let link = ctx.input(|i| i.modifiers.ctrl) && self.link_at(pane, pos).is_some();
                ctx.set_cursor_icon(if link {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Text
                });
            }
        }

        let now = Instant::now();
        let force = self.relayout;
        self.relayout = false;

        let panes: Vec<(PaneId, egui::Rect)> = self.solved.panes.clone();
        for (pane_id, pane_rect) in &panes {
            let rect_px = [
                pane_rect.min.x * self.scale,
                pane_rect.min.y * self.scale,
                pane_rect.width() * self.scale,
                pane_rect.height() * self.scale,
            ];
            let (cols, rows) = self.metrics.grid_size(rect_px[2], rect_px[3]);

            let is_focused = *pane_id == focused_pane && self.focused;

            let Some(state) = self.panes.get_mut(pane_id) else {
                continue;
            };
            state.pane.reconcile_size(cols, rows, now);
            state.pane.flush_size(&self.metrics, now, force);

            if state.pane.take_dirty() {
                let t = Instant::now();

                // A scan holds this pane's lock for as long as its walk takes. Waiting for it
                // here would put the stall straight back on the UI thread by another route, so
                // the frame is drawn from the snapshot this pane already has and the pane stays
                // dirty for the next one: a few frames of slightly old output, rather than a
                // window that stops answering.
                //
                // Only that pane, and only while its own scan is running. `FairMutex` is what
                // keeps the renderer and the PTY thread taking turns under `cat`, and going
                // around it in the ordinary case would undo exactly that.
                let scanned = self.search.scanning_pane() == Some(*pane_id);
                let locked = if scanned {
                    state.pane.term.try_lock_unfair()
                } else {
                    Some(state.pane.term.lock())
                };

                match locked {
                    Some(mut term) => {
                        state.pane.apply_pending(&mut term);
                        state.copy_blocks.invalidate();
                        if !state.integration.child_running {
                            state.copy_blocks.observe_prompt(
                                &term,
                                state.integration.integration == handshake::Integration::Active,
                            );
                        }
                        // Only the focused pane is searched: the query belongs to it, and running
                        // a DFA over three other grids to highlight nothing is work for no
                        // picture. The immutable borrow has to end before `capture` takes the
                        // mutable one.
                        let hits = if is_focused && self.search.open {
                            search::highlights(&term, &mut self.search)
                        } else {
                            search::Highlights::default()
                        };
                        if is_focused {
                            self.search_visible = hits.visible;
                        }

                        state.snapshot = Arc::new(snapshot::capture_with_style(
                            &mut term,
                            &self.theme,
                            is_focused,
                            self.cursor_on || !is_focused,
                            &hits,
                            &mut state.cursor,
                            self.config.render.cursor_style,
                        ));
                        drop(term);
                        Timings::add(&self.timings.snapshot_us, t.elapsed());

                        // A deferred cursor position is drawn by a frame of its own: the output
                        // that would otherwise bring one along is exactly what has stopped
                        // arriving. Both halves are needed — the repaint to have a frame, and the
                        // dirty mark for that frame to re-capture instead of reusing this one.
                        if let Some(remaining) = state.cursor.deadline(Instant::now()) {
                            state.pane.mark_dirty();
                            ctx.request_repaint_after(remaining);
                        }
                    }
                    None => state.pane.mark_dirty(),
                }
            }

            let snapshot = state.snapshot.clone();
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                *pane_rect,
                GridCallback {
                    repaint: ctx.clone(),
                    pane: *pane_id,
                    snapshot,
                    metrics: self.metrics,
                    theme: self.theme.clone(),
                    rect_px,
                    focused: is_focused,
                    frame: self.frame,
                },
            ));
        }

        self.draw_copy_block(ui, copy_hover);

        // After the panes, so it is drawn over the grid rather than under it, and computed here
        // rather than before them so the cursor it hangs from is this frame's.
        if let Some(overlay) = self.sigil_overlay() {
            self.draw_sigil_overlay(ui, &overlay);
        } else {
            self.completion.blocked = None;
        }

        // Park the IME's candidate window on the focused pane's cursor. Without a rect it opens
        // wherever the platform last left it — usually the window's top-left corner, which for a
        // terminal means the candidate list covers the output you are typing against.
        if let Some(cursor) = self.focused_cursor_rect(focused_pane) {
            if self.ime_rect != Some(cursor) {
                self.ime_rect = Some(cursor);
                ctx.send_viewport_cmd(egui::ViewportCommand::IMERect(cursor));
            }
        }

        // An unfocused pane in a split gets a dim outline so it is obvious where the keyboard
        // is going. With one pane there is nothing to disambiguate, so it is skipped.
        if panes.len() > 1 {
            if let Some(r) = self.solved.rect_of(focused_pane) {
                ui.painter().rect_stroke(
                    r,
                    0.0,
                    egui::Stroke::new(1.0, visuals::to_color32(self.theme.palette[4])),
                    egui::StrokeKind::Inside,
                );
            }
        }

        if let Some(until) = self.bell_until {
            if Instant::now() < until {
                let c = self.theme.fg;
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, 28),
                );
                ctx.request_repaint_after(Duration::from_millis(16));
            } else {
                self.bell_until = None;
            }
        }

        if !self.toasts.is_empty() {
            let messages: Vec<String> = self
                .toasts
                .iter()
                .map(|(m, _)| format!("  {m}  "))
                .collect();
            egui::Area::new(egui::Id::new("nyx.toasts"))
                .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
                .interactable(false)
                .show(&ctx, |ui| {
                    for message in messages {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(message)
                                    .monospace()
                                    .color(visuals::to_color32(self.theme.bg))
                                    .background_color(visuals::to_color32(self.theme.palette[1])),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    }
                });
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        if self.search.open {
            let action = findbar::show(ui, rect, &self.search, self.search_visible, &self.theme);
            match action {
                Some(findbar::Action::Next) => self.find_step(SearchDir::Right),
                Some(findbar::Action::Prev) => self.find_step(SearchDir::Left),
                Some(findbar::Action::ToggleRegex) => {
                    self.search.toggle_regex();
                    self.find_step(SearchDir::Right);
                }
                Some(findbar::Action::Close) => {
                    self.search.close();
                    self.mark_all_dirty();
                }
                None => {}
            }
        }

        if self.palette.open {
            let entries = self.palette_entries();
            let chosen = palette::show(ui, rect, &mut self.palette, &entries, &self.theme);
            if let Some(action) = chosen {
                self.run_palette_action(action);
            }
        }

        if self.settings.open {
            // Enumerating installed families needs the render state, which the keyboard path that
            // opens the window does not have — so the list is filled in on the first frame.
            if self.settings.families.is_empty() {
                if let Some(render_state) = frame.wgpu_render_state() {
                    let renderer = render_state.renderer.read();
                    if let Some(res) = renderer.callback_resources.get::<GridResources>() {
                        self.settings.families = res.monospace_families();
                    }
                }
            }
            self.settings.sound_message = self.sounds.status.lock().ok().and_then(|s| s.clone());
            let changed = settings::show(
                &ctx,
                &mut self.settings,
                &self.theme,
                &self.profiles,
                &self.detected,
            );
            if changed {
                self.apply_draft(&ctx, frame);
            }
        }

        self.perf_overlay(ui);
        self.draw_copy_feedback(ui);
        self.custom_cursor.activate(
            ctx.output(|o| o.cursor_icon == egui::CursorIcon::Text)
                && !self.settings.open
                && !self.palette.open
                && !egui::Popup::is_any_open(&ctx),
        );

        if self.panes.values().any(|s| s.pane.resize_pending()) {
            ctx.request_repaint_after(RESIZE_POLL);
        }
    }

    fn on_exit(&mut self) {
        // The settings window applies live and saves on a 400 ms debounce, so a change made and
        // then immediately followed by closing the window was visibly applied and never written.
        // There is no frame after this one to flush it in.
        if self.save_at.is_some() {
            if let Err(err) = self.write_config_now() {
                // A toast has nowhere to appear from here, so the log is the only place left.
                log::error!("could not save config.toml on exit: {err}");
            }
        }

        for state in self.panes.values() {
            state.pane.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reported_buttons_keep_their_panes_until_release_or_focus_loss() {
        use super::*;
        use alacritty_terminal::index::{Column, Line};
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let mut mouse = MouseState::default();
        for (pane, button) in [(1, mouse::Button::Left), (2, mouse::Button::Right)] {
            mouse.captures.push(MouseCapture { pane, button, point: Point::new(Line(0), Column(0)),
                offset: 0, mode, modifiers: egui::Modifiers::NONE });
        }
        let released = mouse.take_releases(Some(mouse::Button::Left));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].pane, 1);
        assert_eq!(mouse.captures[0].pane, 2);
        assert_eq!(mouse::encode(released[0].button, false, false, released[0].point, 0,
            egui::Modifiers::NONE, mode).unwrap(), b"\x1b[<0;1;1m");
        let released = mouse.take_releases(None);
        assert_eq!(released[0].pane, 2);
        assert!(mouse.captures.is_empty());
    }

    #[test]
    fn ime_and_paste_desynchronize_the_application_input_model() {
        use super::*;
        for event in [egui::Event::Paste("tail".into()),
            egui::Event::Ime(egui::ImeEvent::Commit("世界".into()))] {
            let mut shadow = Shadow::new();
            shadow.feed(Feed::Char('@'));
            let translated = translate_with_expansion(&mut shadow, &Tables::default(),
                PathStyle::Windows, false, &[event], egui::Modifiers::NONE, TermMode::empty(), None);
            assert!(!translated.bytes.is_empty());
            assert!(shadow.line().is_none());
        }
    }
    /// One egui pass that draws a button, reporting whether it was clicked.
    ///
    /// `surrender` mirrors what `release_chrome_focus` does, and at the same point in the pass.
    fn pass(ctx: &egui::Context, events: Vec<egui::Event>, surrender: bool) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut clicked = false;
        let mut output = ctx.run_ui(input, |ui| {
            if surrender {
                if let Some(id) = ui.ctx().memory(|m| m.focused()) {
                    ui.ctx().memory_mut(|m| m.surrender_focus(id));
                }
            }
            clicked = ui.button("new tab").clicked();
        });
        // A `TexturesDelta` insists on being handled before it is dropped; there is no renderer
        // here to hand it to.
        output.textures_delta.clear();
        clicked
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn modifier_presses_preserve_sigil_tracking_for_altgr_and_copy() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let altgr = egui::Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        for (code, modifiers) in [(egui::Key::ControlLeft, ctrl), (egui::Key::AltRight, altgr)] {
            assert_eq!(super::feed_for(&key(code, modifiers)), None);
        }
    }

    #[test]
    fn tab_then_enter_presses_a_chrome_button_unless_focus_is_released() {
        // Why `release_chrome_focus` exists, pinned against egui rather than described in a
        // comment. Both of these are ordinary terminal keys: Tab completes a path, Enter runs the
        // line. Together they pressed the new-tab button, which is what "typing cd <dir> and
        // pressing Enter opens another tab" turned out to be.
        let ctx = egui::Context::default();
        assert!(
            !pass(&ctx, vec![key(egui::Key::Tab)], false),
            "the Tab pass clicks nothing itself"
        );
        assert!(
            pass(&ctx, vec![key(egui::Key::Enter)], false),
            "egui no longer clicks a focused button on Enter — release_chrome_focus may be moot"
        );

        // Releasing focus at the top of the pass, the same two keys reach nothing.
        let ctx = egui::Context::default();
        assert!(!pass(&ctx, vec![key(egui::Key::Tab)], true));
        assert!(
            !pass(&ctx, vec![key(egui::Key::Enter)], true),
            "Enter still reached the chrome"
        );
    }

    #[test]
    fn tab_is_the_way_in_and_an_arrow_alone_is_not() {
        // Worth pinning which key actually hands the keyboard over, because the arrow keys are
        // listed alongside Tab in `Memory::begin_pass` and look like they would do it too. They
        // do not: egui's directional focus is relative to an already-focused widget, so an arrow
        // from a clean state moves nothing. It is Tab — which is exactly what completes a path at
        // a shell prompt, and is why this showed up on a long directory name and not a short one.
        let ctx = egui::Context::default();
        let _ = pass(&ctx, vec![key(egui::Key::ArrowDown)], false);
        assert!(
            !pass(&ctx, vec![key(egui::Key::Enter)], false),
            "an arrow from a clean state should not have focused anything"
        );
    }

    /// The whole path, not just the encoder: a program that turned the protocol on gets the
    /// sequence, and the line model is told it can no longer follow the line.
    #[test]
    fn under_the_kitty_protocol_shift_enter_is_a_sequence_and_not_a_submit() {
        use crate::expand::Tables;
        use crate::term::profile::PathStyle;
        use crate::expand::Feed;
        use alacritty_terminal::term::TermMode;

        let shift_enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        let mode = TermMode::DISAMBIGUATE_ESC_CODES;

        let mut shadow = super::Shadow::new();
        for c in "hello".chars() {
            shadow.feed(Feed::Char(c));
        }
        let out = super::translate_with_expansion(
            &mut shadow,
            &Tables::default(),
            PathStyle::Windows,
            false,
            std::slice::from_ref(&shift_enter),
            egui::Modifiers::SHIFT,
            mode,
            None,
        );
        assert_eq!(out.bytes, b"\x1b[13;2u");
        // Not a submit: the program is holding a two-line buffer the model cannot see, so it
        // must stop claiming to know what the line says rather than believe it is empty.
        assert!(shadow.line().is_none(), "the model still thinks it knows the line");

        // The same keystroke with no program asking is the terminal's ordinary Enter.
        let mut shadow = super::Shadow::new();
        shadow.feed(Feed::Char('x'));
        let out = super::translate_with_expansion(
            &mut shadow,
            &Tables::default(),
            PathStyle::Windows,
            false,
            std::slice::from_ref(&shift_enter),
            egui::Modifiers::SHIFT,
            TermMode::empty(),
            None,
        );
        assert_eq!(out.bytes, b"\r");
        assert_eq!(shadow.line(), Some(""), "a real submit leaves an empty line");
    }

    /// The other half: with only disambiguation asked for, ordinary typing is untouched and the
    /// `Text` beside a key is not swallowed.
    #[test]
    fn the_kitty_protocol_leaves_ordinary_typing_to_the_ordinary_path() {
        use crate::expand::Tables;
        use crate::term::profile::PathStyle;
        use alacritty_terminal::term::TermMode;

        let events = vec![
            egui::Event::Key {
                key: egui::Key::B,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("b".into()),
        ];
        let mut shadow = super::Shadow::new();
        let out = super::translate_with_expansion(
            &mut shadow,
            &Tables::default(),
            PathStyle::Windows,
            false,
            &events,
            egui::Modifiers::NONE,
            TermMode::DISAMBIGUATE_ESC_CODES,
            Some("b"),
        );
        assert_eq!(out.bytes, b"b");
        assert!(!out.swallow_text);
        assert_eq!(shadow.line(), Some("b"));
    }

    /// Every modifier combination, encoded both ways.
    ///
    /// The expansion path is the one every keystroke in every pane actually takes, and it has a
    /// `Text` arm of its own because an expansion must be emitted before the space that triggered
    /// it. That arm used to decide nothing about modifiers, so Alt+B — readline's "back one word",
    /// and the most used Alt chord in any shell — sent a literal `b` into the command line. The
    /// two are pinned against each other rather than against a table of expected bytes, because
    /// the property that matters is that they agree.
    #[test]
    fn the_expansion_path_encodes_modified_text_exactly_as_the_plain_one_does() {
        use crate::expand::Tables;
        use crate::term::input;
        use crate::term::profile::PathStyle;
        use alacritty_terminal::term::TermMode;

        for (name, m) in [
            ("plain", egui::Modifiers::NONE),
            (
                "alt",
                egui::Modifiers {
                    alt: true,
                    ..egui::Modifiers::NONE
                },
            ),
            (
                "ctrl",
                egui::Modifiers {
                    ctrl: true,
                    ..egui::Modifiers::NONE
                },
            ),
            // AltGr, which Windows reports as Ctrl+Alt: this is how `@` and `€` are typed.
            (
                "altgr",
                egui::Modifiers {
                    ctrl: true,
                    alt: true,
                    ..egui::Modifiers::NONE
                },
            ),
        ] {
            let events = vec![
                egui::Event::Key {
                    key: egui::Key::B,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: m,
                },
                egui::Event::Text("b".into()),
            ];
            let mut shadow = super::Shadow::new();
            let expanded = super::translate_with_expansion(
                &mut shadow,
                &Tables::default(),
                PathStyle::Windows,
                false,
                &events,
                m,
                TermMode::empty(),
                None,
            );
            let plain = input::translate(&events, m, TermMode::empty());
            assert_eq!(expanded.bytes, plain.bytes, "{name} encoded differently");
        }
    }

    /// The shell's own rewrite, followed by the model but never echoed back at the shell.
    ///
    /// Two halves, and the bug is only visible when both are asserted: under integration this
    /// path must emit *nothing* (a second rewrite gives `cd "C:\...""C:\..."`) and must still
    /// leave the model holding what the line editor now shows. It used to do the first and not
    /// the second, and nothing read the model in that state, so nothing could report it.
    #[test]
    fn under_integration_the_model_follows_the_shell_without_writing_anything() {
        use crate::config::schema::{AliasMap, ShortcutConfig};
        use crate::expand::Tables;
        use crate::term::profile::PathStyle;
        use alacritty_terminal::term::TermMode;

        let tables = Tables::from_config(
            &[ShortcutConfig {
                id: "tibet".into(),
                label: "Free Tibet".into(),
                sigil: "@tibet".into(),
                path: r"C:\Free Tibet".into(),
                icon: String::new(),
                color: None,
                order: 0,
            }],
            &AliasMap::new(),
        )
        .0;

        let typed: Vec<egui::Event> = "cd @tibet "
            .chars()
            .map(|c| egui::Event::Text(c.to_string()))
            .collect();

        for integration in [false, true] {
            let mut shadow = super::Shadow::new();
            let out = super::translate_with_expansion(
                &mut shadow,
                &tables,
                PathStyle::Windows,
                integration,
                &typed,
                egui::Modifiers::NONE,
                TermMode::empty(),
                None,
            );
            assert_eq!(
                shadow.line(),
                Some(r#"cd "C:\Free Tibet" "#),
                "integration={integration}: the model has to describe the line on screen"
            );
            let wrote_expansion = out.bytes.contains(&0x7f);
            assert_eq!(
                wrote_expansion, !integration,
                "integration={integration}: exactly one layer may rewrite the line"
            );
        }
    }

    #[test]
    fn space_activates_a_focused_button_just_as_enter_does() {
        // So it is not only the Enter at the end of a line: a space in the middle of one would do
        // it too.
        let ctx = egui::Context::default();
        let _ = pass(&ctx, vec![key(egui::Key::Tab)], false);
        assert!(pass(&ctx, vec![key(egui::Key::Space)], false));

        let ctx = egui::Context::default();
        let _ = pass(&ctx, vec![key(egui::Key::Tab)], true);
        assert!(!pass(&ctx, vec![key(egui::Key::Space)], true));
    }
}
