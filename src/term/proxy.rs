//! `EventListener` implementation. Every method here runs on a PTY thread, never the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use crossbeam_channel::Sender;

use crate::event::{AppEvent, RepaintCoalescer};
use crate::term::PaneId;

#[derive(Clone)]
pub struct EventProxy {
    pane: PaneId,
    tx: Sender<AppEvent>,
    repaint: Arc<RepaintCoalescer>,
    /// Set from the PTY thread, cleared by the UI thread once it has taken a snapshot.
    dirty: Arc<AtomicBool>,
}

impl EventProxy {
    pub fn new(
        pane: PaneId,
        tx: Sender<AppEvent>,
        repaint: Arc<RepaintCoalescer>,
        dirty: Arc<AtomicBool>,
    ) -> Self {
        Self { pane, tx, repaint, dirty }
    }

    fn wake(&self) {
        self.dirty.store(true, Ordering::Release);
        self.repaint.poke();
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            // The hot ones. No allocation, no channel send — just a flag and one gated poke.
            Event::Wakeup
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::ResetTitle => self.wake(),

            Event::Title(title) => {
                // From Phase 6 this is also the shell-integration handshake channel, and titles
                // beginning with `nyx:<our token>:` get consumed here instead of being shown.
                let _ = self.tx.send(AppEvent::Title { pane: self.pane, title });
                self.wake();
            },

            Event::PtyWrite(data) => {
                let _ = self.tx.send(AppEvent::PtyWrite { pane: self.pane, data });
                self.repaint.poke();
            },

            Event::ClipboardStore(kind, text) => {
                let _ = self.tx.send(AppEvent::ClipboardStore { pane: self.pane, kind, text });
                self.repaint.poke();
            },

            // Dropped, and it should never arrive: `term::spawn` sets `Osc52::OnlyCopy`, so the
            // crate refuses the read before it gets here. Answering it would write the user's
            // clipboard into the PTY on the say-so of anything that can print to the terminal —
            // a `cat` of a hostile file, or the far end of an ssh session. This arm is the second
            // of the two places that say no, and the one that survives a config change upstream.
            Event::ClipboardLoad(..) => {},

            Event::ColorRequest(index, format) => {
                let _ = self.tx.send(AppEvent::ColorRequest { pane: self.pane, index, format });
                self.repaint.poke();
            },

            Event::TextAreaSizeRequest(format) => {
                let _ = self.tx.send(AppEvent::TextAreaSizeRequest { pane: self.pane, format });
                self.repaint.poke();
            },

            Event::Bell => {
                let _ = self.tx.send(AppEvent::Bell { pane: self.pane });
                self.repaint.poke();
            },

            // No catch-all arm on purpose: every `Event` variant is handled above, so a new one
            // in a future alacritty_terminal is a compile error here rather than an event that
            // silently does nothing and takes a day to track down.
            Event::Exit | Event::ChildExit(_) => {
                let _ = self.tx.send(AppEvent::PaneExited { pane: self.pane });
                self.repaint.poke();
            },
        }
    }
}
