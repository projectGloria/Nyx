//! One shell: its `Term`, its PTY channel, and its size bookkeeping.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoopSender, Msg};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use crossbeam_channel::Sender;

use crate::event::{AppEvent, RepaintCoalescer};
use crate::render::metrics::CellMetrics;
use crate::term::proxy::EventProxy;
use crate::term::spawn::{self, ShellSpec};
use crate::term::{GridSize, PaneId};

/// ConPTY reflows its buffer asynchronously and re-emits content as it does. A window drag that
/// fires forty resizes a second produces forty overlapping reflows and visible garbage, so a
/// size change waits for the drag to settle.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(60);

pub struct Pane {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub title: String,

    sender: EventLoopSender,
    dirty: Arc<AtomicBool>,
    scrollback: usize,

    size: GridSize,
    pending: Option<(GridSize, Instant)>,
    pending_scroll: parking_lot::Mutex<Vec<Scroll>>,
    clear_selection: AtomicBool,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        spec: &ShellSpec,
        size: GridSize,
        metrics: &CellMetrics,
        scrollback: usize,
        tx: Sender<AppEvent>,
        repaint: Arc<RepaintCoalescer>,
    ) -> anyhow::Result<Self> {
        let dirty = Arc::new(AtomicBool::new(true));
        let proxy = EventProxy::new(id, tx, repaint, dirty.clone());

        let spawned = spawn::spawn(
            id,
            spec,
            size,
            metrics.cell_w.round() as u16,
            metrics.cell_h.round() as u16,
            scrollback,
            proxy,
        )?;

        Ok(Self {
            term: spawned.term,
            title: spec.program.clone(),
            sender: spawned.sender,
            dirty,
            scrollback,
            size,
            pending: None,
            pending_scroll: parking_lot::Mutex::new(Vec::new()),
            clear_selection: AtomicBool::new(false),
        })
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    /// The scrollback depth this shell was started with.
    ///
    /// `Term` fixes its history size at construction, so a config change cannot move it — the
    /// app compares against this to say so rather than silently ignoring the key.
    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// True if the PTY thread has touched the grid since the last snapshot.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Send bytes to the shell. This is a channel send plus a poller wake — it never touches the
    /// `Term` lock, which is why a keystroke cannot be delayed by a redraw.
    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    /// Note a new pixel rect. Cheap, and called every frame: the common case is that the derived
    /// column/row count has not changed and this returns immediately.
    pub fn reconcile_size(&mut self, cols: usize, rows: usize, now: Instant) {
        let next = GridSize::new(cols, rows);
        if next == self.size {
            self.pending = None;
            return;
        }
        match self.pending {
            Some((p, _)) if p == next => {},
            _ => self.pending = Some((next, now)),
        }
    }

    /// Apply a settled resize. `force` skips the debounce for changes that are not drags —
    /// a split, a zoom, or the initial layout — where waiting only looks like lag.
    pub fn flush_size(&mut self, metrics: &CellMetrics, now: Instant, force: bool) {
        let Some((next, at)) = self.pending else { return };
        if !force && now.duration_since(at) < RESIZE_DEBOUNCE {
            return;
        }
        // A background search owns this lock for its entire walk; retry on a later frame.
        let Some(mut term) = self.term.try_lock_unfair() else { return };
        self.pending = None;
        self.size = next;

        // Term first, then the PTY, and always both: resizing the grid alone leaves the shell
        // writing at the old width into a grid of the new one, and every wrapped line is wrong
        // until something forces a full redraw.
        term.resize(next);
        drop(term);
        let _ = self.sender.send(Msg::Resize(WindowSize {
            num_lines: next.screen_lines as u16,
            num_cols: next.columns as u16,
            cell_width: metrics.cell_w.round() as u16,
            cell_height: metrics.cell_h.round() as u16,
        }));
        self.mark_dirty();
    }

    /// True while a resize is waiting out the debounce, so the app knows to schedule one more
    /// frame rather than going idle with the change unapplied.
    pub fn resize_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }

    /// Move the viewport through scrollback.
    ///
    /// Queue changes so a background scan cannot block input handling.
    pub fn scroll(&self, scroll: Scroll) {
        let mut pending = self.pending_scroll.lock();
        if matches!(scroll, Scroll::Top | Scroll::Bottom) { pending.clear(); }
        if let Scroll::Delta(delta) = scroll {
            if let Some(Scroll::Delta(previous)) = pending.last_mut() {
                *previous = previous.saturating_add(delta);
                drop(pending);
                self.mark_dirty();
                return;
            }
        }
        pending.push(scroll);
        drop(pending);
        self.mark_dirty();
    }

    pub fn clear_selection(&self) {
        self.clear_selection.store(true, Ordering::Release);
        self.mark_dirty();
    }

    pub fn apply_pending(&self, term: &mut Term<EventProxy>) {
        if self.clear_selection.swap(false, Ordering::AcqRel)
            && crate::term::select::is_active(term)
        {
            crate::term::select::clear(term);
        }
        for scroll in self.pending_scroll.lock().drain(..) { term.scroll_display(scroll); }
    }

    /// The window size as the shell sees it, for a `TextAreaSizeRequest` reply.
    pub fn window_size(&self, metrics: &CellMetrics) -> WindowSize {
        WindowSize {
            num_lines: self.size.screen_lines as u16,
            num_cols: self.size.columns as u16,
            cell_width: metrics.cell_w.round() as u16,
            cell_height: metrics.cell_h.round() as u16,
        }
    }
}
