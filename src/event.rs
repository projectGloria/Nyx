//! The one channel from every non-UI thread into the frame, plus the repaint gate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::ClipboardType;
use alacritty_terminal::vte::ansi::Rgb;
use parking_lot::Mutex;

use crate::term::PaneId;

/// Ceiling on repaints driven by shell *output*.
///
/// Input-driven repaints are egui's own and are not throttled, so typing latency is unaffected.
/// This only governs how often a stream of output is allowed to redraw the screen, and 60 Hz is
/// already more distinct screens per second than anyone can read. Uncapped on a 144 Hz display
/// this reshapes the whole grid 144 times a second during a `cat`, which is 2.4x the work for
/// output that is a blur either way.
const MIN_OUTPUT_FRAME: Duration = Duration::from_micros(16_667);

/// Whether an OSC 52 store should reach the system clipboard.
///
/// `p` and `s` ask for the **primary selection**, an X11 concept with no Windows equivalent.
/// Writing those to the real clipboard is not a graceful degradation: a program that syncs its
/// own selection to primary — which is the whole point of primary — then overwrites whatever the
/// user last copied, every time they move the cursor over text. Nothing reports it and nothing
/// gets it back.
///
/// Alacritty, on the same crate, resolves it the same way: its selection provider is `None` on
/// Windows and macOS, and `(ClipboardType::Selection, None) => return` drops the store.
///
/// Reads are a separate question and are answered in `term::spawn` instead, where the terminal
/// config turns them off outright.
pub fn reaches_system_clipboard(kind: ClipboardType) -> bool {
    match kind {
        ClipboardType::Clipboard => true,
        ClipboardType::Selection => false,
    }
}

/// Everything a background thread can tell the UI.
///
/// High-frequency, payload-free signals (`Wakeup`, `MouseCursorDirty`) deliberately do **not**
/// appear here — they set the pane's dirty flag and poke the coalescer instead. Putting them on
/// the channel would allocate and enqueue thousands of times a second under `cat`.
///
/// Several variants carry a formatter closure rather than a value. That is how
/// `alacritty_terminal` hands back a reply it wants written to the PTY: it knows the escape
/// sequence shape, we know the answer, and the closure joins the two without either side
/// learning the other's business.
pub enum AppEvent {
    Title { pane: PaneId, title: String },
    /// The shell asked us to write bytes back to it.
    PtyWrite { pane: PaneId, data: String },
    /// OSC 52: the program wants text put on the clipboard.
    ///
    /// There is no `ClipboardLoad` counterpart on purpose. See `term::spawn`.
    ClipboardStore { pane: PaneId, kind: ClipboardType, text: String },
    /// OSC 4/10/11: the program is asking what a palette entry actually looks like.
    ColorRequest { pane: PaneId, index: usize, format: ColorFormatter },
    /// The program wants the text area size in pixels.
    TextAreaSizeRequest { pane: PaneId, format: SizeFormatter },
    /// The child process is gone. The pane is now showing a dead grid.
    PaneExited { pane: PaneId },
    Bell { pane: PaneId },
    /// `config.toml` changed on disk and parsed. `problems` is empty on a clean load, and `hash`
    /// is the fingerprint of the bytes that were read — the app uses it to tell an external edit
    /// from its own settings-window save coming back round.
    ConfigReloaded { config: Arc<crate::config::Config>, problems: Vec<String>, hash: u64 },
    /// `config.toml` changed on disk and did **not** parse. Carries no config on purpose: the
    /// running one stays exactly as it is, and all the user gets is the complaint. Saving from an
    /// editor mid-edit is the common way to see this, and reverting their whole file for the
    /// second between two keystrokes is not an acceptable answer to it.
    ConfigInvalid { problems: Vec<String> },
    /// WSL enumeration finished; it runs off the launch path because it shells out.
    WslDistros(Vec<String>),
    /// A scrollback search finished walking a pane's history.
    ///
    /// The walk holds that pane's lock for as long as it takes — 214-217 ms over 100k rows,
    /// against a scrollback ceiling of a million — so it runs on a thread of its own and the
    /// answer arrives here. `seq` is what separates a result for the query still being typed
    /// from one already moved past; `Search::land` is what decides which of the two it is.
    SearchFinished { pane: PaneId, seq: u64, found: Option<Match> },

    /// The global hotkey fired. The thread that owns it has already raised the window if it was
    /// hidden — see `win::hotkey` — so what is left is deciding whether this press meant show or
    /// hide, which needs the app's state and therefore happens here.
    HotkeyPressed,
    /// `RegisterHotKey` refused the chord, which almost always means another program owns it.
    /// Reported rather than swallowed: a hotkey that silently does nothing reads as Nyx being
    /// broken rather than as a chord that was already taken.
    HotkeyRefused,
    /// Another launch found this instance already running and handed us its folder — Explorer's
    /// "Open Nyx here", or a second `nyx --attach`. The payload has been checked to be an
    /// existing directory and is used as a working directory and nothing else.
    OpenHere(crate::win::ipc::Request),
}

pub type ColorFormatter = Arc<dyn Fn(Rgb) -> String + Send + Sync + 'static>;
pub type SizeFormatter = Arc<dyn Fn(WindowSize) -> String + Send + Sync + 'static>;

/// Wakes the UI thread at most once per frame.
///
/// `egui::Context::request_repaint` ends in a winit `EventLoopProxy::send_event`, which is a
/// syscall. `cat` of a large file fires `Event::Wakeup` thousands of times a second, and without
/// this gate the wake-up syscalls alone outweigh the terminal emulation — the terminal gets
/// slower the more output it receives, which is exactly backwards.
pub struct RepaintCoalescer {
    pending: AtomicBool,
    ctx: egui::Context,
    last_frame: Mutex<Instant>,
}

impl RepaintCoalescer {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            pending: AtomicBool::new(false),
            ctx,
            last_frame: Mutex::new(Instant::now() - MIN_OUTPUT_FRAME),
        }
    }

    pub fn poke(&self) {
        // swap, not store: only the thread that flips false->true pays for what follows, so the
        // body runs at most once per frame no matter how many times the PTY thread calls this.
        if !self.pending.swap(true, Ordering::AcqRel) {
            let since = self.last_frame.lock().elapsed();
            if since >= MIN_OUTPUT_FRAME {
                self.ctx.request_repaint();
            } else {
                // Ask for the frame at the cap instead of right now. egui coalesces these, so a
                // firehose of output still produces one wake-up per interval.
                self.ctx.request_repaint_after(MIN_OUTPUT_FRAME - since);
            }
        }
    }

    /// Called once at the top of the frame, before anything can set the flag again.
    pub fn consume(&self) {
        *self.last_frame.lock() = Instant::now();
        self.pending.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_primary_selection_store_does_not_reach_the_windows_clipboard() {
        // OSC 52 with `p` or `s` asks for the primary selection, which Windows does not have.
        // Treating it as the clipboard means a TUI that mirrors its selection there wipes out
        // whatever the user last copied, silently and repeatedly. Alacritty drops it; so do we.
        assert!(reaches_system_clipboard(ClipboardType::Clipboard));
        assert!(!reaches_system_clipboard(ClipboardType::Selection));
    }
}
