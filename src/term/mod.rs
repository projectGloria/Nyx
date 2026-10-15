//! The PTY side of Nyx. Nothing in here draws.
//!
//! The seam to `render/` is [`snapshot::GridSnapshot`]: a plain owned struct with no lifetimes,
//! built under the `Term` lock and consumed on the GPU thread long after the lock is gone.
//! Nothing hands out a `&Term`.

use alacritty_terminal::grid::Dimensions;

pub mod cursor;
pub mod blocks;
pub mod detect;
pub mod input;
pub mod kitty;
pub mod links;
pub mod mouse;
pub mod pane;
pub mod profile;
pub mod proxy;
pub mod search;
pub mod select;
pub mod snapshot;
pub mod spawn;

pub use pane::Pane;

pub type PaneId = u64;

#[cfg(test)]
pub mod testing {
    //! A real `Term` with nothing behind it, for tests.
    //!
    //! `Term` is the grid and the parser — no PTY, no process, no GPU — so anything that needs
    //! one can still run under `cargo test`. That covers more of the codebase than it looks:
    //! snapshots, search and selection all take a `&mut Term` and nothing else.

    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use alacritty_terminal::term::{Config as TermConfig, Term};

    use super::proxy::EventProxy;
    use super::GridSize;
    use crate::event::RepaintCoalescer;

    /// A `Term` of the given size. The proxy's sends are all `let _ =`, so dropping the receiver
    /// is deliberate and harmless.
    pub fn headless_term(size: GridSize) -> Term<EventProxy> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let repaint = Arc::new(RepaintCoalescer::new(egui::Context::default()));
        let proxy = EventProxy::new(1, tx, repaint, Arc::new(AtomicBool::new(false)));
        Term::new(TermConfig::default(), &size, proxy)
    }

    /// Type into the grid at the cursor. `Term` implements `Handler`, so no parser is needed.
    pub fn type_into(term: &mut Term<EventProxy>, text: &str) {
        use alacritty_terminal::vte::ansi::Handler;
        for c in text.chars() {
            term.input(c);
        }
    }
}

/// Our own [`Dimensions`], because `alacritty_terminal`'s `TermSize` lives in its `term::test`
/// module and is not public API to build on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub screen_lines: usize,
    pub total_lines: usize,
}

impl GridSize {
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        Self { columns, screen_lines, total_lines: screen_lines }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.total_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}
