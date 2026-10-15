//! Mouse selection, held inside `Term` where the renderer already looks for it.
//!
//! `alacritty_terminal` owns the hard part: `Selection` tracks anchors through scrolling and
//! resizing, and `to_range` turns them into something the renderer can test per cell. This module
//! is the thin layer that decides *when* to start, extend and clear one.

use alacritty_terminal::index::{Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::Term;

use crate::term::proxy::EventProxy;

/// What a click should select, by click count.
pub fn kind_for_clicks(clicks: u32, block: bool) -> SelectionType {
    if block {
        return SelectionType::Block;
    }
    match clicks {
        1 => SelectionType::Simple,
        2 => SelectionType::Semantic, // word
        _ => SelectionType::Lines,    // whole line
    }
}

pub fn start(term: &mut Term<EventProxy>, point: Point, side: Side, ty: SelectionType) {
    term.selection = Some(Selection::new(ty, point, side));
}

pub fn update(term: &mut Term<EventProxy>, point: Point, side: Side) {
    if let Some(selection) = term.selection.as_mut() {
        selection.update(point, side);
    }
}

pub fn clear(term: &mut Term<EventProxy>) {
    term.selection = None;
}

pub fn is_active(term: &Term<EventProxy>) -> bool {
    term.selection.is_some()
}

/// The selected text, or `None` if nothing is selected.
///
/// Returns `None` for an empty selection too — a bare click starts a zero-width selection, and
/// copying that would silently wipe whatever was on the clipboard.
pub fn text(term: &Term<EventProxy>) -> Option<String> {
    let selection = term.selection.as_ref()?;
    if selection.is_empty() {
        return None;
    }
    let range = selection.to_range(term)?;
    let text = term.bounds_to_string(range.start, range.end);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
