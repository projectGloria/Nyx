//! Shortcut and alias expansion.
//!
//! Phase 5 is the terminal-side layer only: it works in every shell, including WSL and ssh, and
//! none of them especially well — the expansion appears at the moment you press space rather
//! than being offered by the line editor. Phase 6 adds generated shell integration that does it
//! properly where it can, and `Shadow::may_expand` is what keeps the two from both firing.

pub mod chords;
pub mod complete;
pub mod handshake;
pub mod hint;
pub mod line;
pub mod shadow;
pub mod shellgen;
pub mod table;

pub use handshake::Integration;
pub use shadow::{Feed, Shadow};
pub use table::Tables;
