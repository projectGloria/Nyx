//! Tabs and the split tree inside each one. No terminal state, no rendering.

pub mod focus;
pub mod geometry;
pub mod tree;

pub use tree::{Node, SplitDir};

use crate::term::PaneId;

pub type TabId = u64;

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub root: Node,
    pub focused: PaneId,
    /// Set from the focused pane's title, which the shell rewrites whenever it likes.
    pub title: String,
    /// A name the *user* asked for, which the shell may not overwrite.
    ///
    /// A tab opened at a shortcut is called after the shortcut. Without somewhere for that name
    /// to be safe it survives about a second: cmd.exe announces its working directory as the
    /// window title on its first prompt, and `AppEvent::Title` writes that straight over it. The
    /// shell's title is not lost — it becomes the tooltip, which is where a path belongs.
    pub pinned: Option<String>,
    pub icon: String,
    pub icon_color: Option<String>,
    /// While set, this pane gets the whole tab area. Changes the pane's size, so it drives a
    /// real PTY resize — which is correct, and what tmux does.
    pub zoomed: Option<PaneId>,
    /// A pane in this tab rang while the user was looking at a different one.
    pub bell: bool,
}

impl Tab {
    pub fn new(id: TabId, pane: PaneId, title: String) -> Self {
        Self { id, root: Node::leaf(pane), focused: pane, title, pinned: None,
            icon: "builtin:terminal".into(), icon_color: None, zoomed: None, bell: false }
    }

    /// What the tab is called: what the user asked for, or what the shell last said.
    pub fn name(&self) -> &str {
        self.pinned.as_deref().unwrap_or(&self.title)
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.root.leaves()
    }

    /// The tree to lay out: either the whole thing, or just the zoomed pane.
    pub fn layout_root(&self) -> Node {
        match self.zoomed {
            Some(pane) if self.root.contains(pane) => Node::leaf(pane),
            _ => self.root.clone(),
        }
    }
}

/// What closing a pane did to the tab strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// No tab held that pane. Nothing was changed and nothing should be.
    Unknown,
    /// The pane came out of the tab at this index, which survives with its other panes.
    Pane { tab: usize },
    /// The pane was the whole tab, so the tab at this index went with it. The `id` is carried
    /// because an index stops meaning anything the moment the tab is gone, and a teardown worth
    /// logging is worth naming — nothing recorded which tab went, which is part of why closing
    /// the wrong one went unnoticed.
    Tab { tab: usize, id: TabId },
}

/// Remove `pane` from whichever tab holds it.
///
/// **The owning tab is found by search, never assumed to be the active one.** A shell can exit in
/// a background tab, and `Node::remove` answers `false` for *any* leaf — not only a leaf holding
/// the pane it was asked about. Applying the removal to the active tab therefore reads that
/// `false` as "the pane was the whole tree" and closes a tab that had nothing to do with it.
///
/// For the same reason a pane no tab holds is `Unknown` rather than a fall-through.
/// `Event::Exit` and `Event::ChildExit` both map to `AppEvent::PaneExited` and one ordinary exit
/// on Windows emits both, so the second arrival always names a pane that is already gone — and as
/// a fall-through it would close a tab per duplicate event.
///
/// Choosing the tab's next focused pane is left to the caller: that wants the solved geometry to
/// pick the nearest survivor, and geometry is not known here.
pub fn close_pane(tabs: &mut Vec<Tab>, pane: PaneId) -> Closed {
    let Some(index) = tabs.iter().position(|t| t.root.contains(pane)) else {
        return Closed::Unknown;
    };

    let tab = &mut tabs[index];
    // A zoom naming a pane that no longer exists would lay out an empty tree.
    if tab.zoomed == Some(pane) {
        tab.zoomed = None;
    }

    if tab.root.remove(pane) {
        Closed::Pane { tab: index }
    } else {
        // The pane was the whole tree, so the tab goes with it.
        let id = tab.id;
        tabs.remove(index);
        Closed::Tab { tab: index, id }
    }
}

/// What a bell from `pane` should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rang {
    /// No tab holds that pane — it exited between ringing and being heard. Change nothing.
    Unknown,
    /// The pane is in the tab on screen, so the window flash is about something the user can see.
    Visible,
    /// The pane is in a background tab, which has been marked. A window flash here would be worse
    /// than nothing: it draws the eye to a tab that did not ring and says nothing about the one
    /// that did.
    Background { tab: usize },
}

/// Record a bell, and say whether it is the on-screen kind.
///
/// **The owning tab is found by search, exactly as in `close_pane`.** A bell is most useful
/// precisely when it comes from a tab the user is *not* watching — a long build finishing — and
/// that is the case the single window-wide flash could not express at all.
pub fn ring_bell(tabs: &mut [Tab], active: usize, pane: PaneId) -> Rang {
    let Some(index) = tabs.iter().position(|t| t.root.contains(pane)) else {
        return Rang::Unknown;
    };
    if index == active {
        return Rang::Visible;
    }
    tabs[index].bell = true;
    Rang::Background { tab: index }
}

/// Where the active tab lands once the tab at `removed` is gone and `remaining` are left.
///
/// Removing an entry shifts every later one down a place, so clamping alone is not enough: close
/// a tab to the left of the one being looked at and the clamp leaves `active` pointing at
/// whichever tab slid into the slot. The user did not ask to be moved.
pub fn active_after_removing(active: usize, removed: usize, remaining: usize) -> usize {
    let active = if removed < active { active - 1 } else { active };
    active.min(remaining.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(spec: &[(TabId, PaneId)]) -> Vec<Tab> {
        spec.iter().map(|(id, pane)| Tab::new(*id, *pane, format!("tab {id}"))).collect()
    }

    #[test]
    fn a_bell_in_a_background_tab_marks_that_tab_instead_of_flashing_the_window() {
        // The bell used to be a single window-wide flash, which is the one thing a bell must not
        // be: it is most useful exactly when it comes from a tab the user is *not* watching, and
        // a flash over the foreground tab says nothing about which tab rang. `AppEvent::Bell`
        // had carried the pane id the whole time and nothing read it.
        let mut t = tabs(&[(1, 10), (2, 20)]);

        // The user is on tab 0; tab 1 rings.
        assert_eq!(ring_bell(&mut t, 0, 20), Rang::Background { tab: 1 });
        assert!(t[1].bell, "the tab that rang was not marked");
        assert!(!t[0].bell, "the tab the user is looking at was marked");

        // ...and a bell from the tab on screen is the visible kind, so it flashes and marks
        // nothing. Marking it would light up a tab the user is already reading.
        assert_eq!(ring_bell(&mut t, 0, 10), Rang::Visible);
        assert!(!t[0].bell);
    }

    #[test]
    fn a_bell_from_a_pane_no_tab_holds_changes_nothing() {
        // A shell can ring and exit in the same breath, and the event arrives after the pane is
        // gone. The same reasoning as `Closed::Unknown`: an id nobody holds must fall through to
        // nothing rather than to whichever tab happens to be at that index.
        let mut t = tabs(&[(1, 10), (2, 20)]);
        assert_eq!(ring_bell(&mut t, 0, 999), Rang::Unknown);
        assert!(t.iter().all(|tab| !tab.bell));
    }

    #[test]
    fn a_shell_exiting_in_a_background_tab_does_not_take_the_foreground_one() {
        // The removal used to be applied to the active tab whatever tab the pane was in, and
        // `Node::remove` answers false for *any* leaf -- not only one holding the pane asked
        // about. So "that pane was the whole tree" was read off the wrong tab, and a shell
        // exiting in the background tore down the tab the user was looking at.
        //
        // The user is on tab 2 here; the shell exits in tab 1.
        let mut t = tabs(&[(1, 10), (2, 20)]);

        // Named by id, not just by index: which tab went is the whole question here.
        assert_eq!(close_pane(&mut t, 10), Closed::Tab { tab: 0, id: 1 });
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, 2, "the foreground tab was destroyed");
        assert_eq!(t[0].focused, 20);
    }

    #[test]
    fn a_pane_that_has_already_gone_changes_nothing() {
        // Event::Exit and Event::ChildExit both map to PaneExited, and one normal exit on
        // Windows emits both. The second arrival names a pane no tab holds any more; falling
        // through to the active tab turns it into a second, unrelated tab deletion.
        let mut t = tabs(&[(1, 10), (2, 20)]);
        assert_eq!(close_pane(&mut t, 99), Closed::Unknown);
        assert_eq!(t.len(), 2, "an id belonging to no tab deleted one anyway");
    }

    #[test]
    fn closing_one_pane_of_a_split_leaves_its_tab_standing() {
        // The split is in the background tab, so this also fails if the active tab is assumed.
        let mut t = tabs(&[(1, 10), (2, 20)]);
        assert!(t[0].root.split(10, SplitDir::Horizontal, 11));

        assert_eq!(close_pane(&mut t, 11), Closed::Pane { tab: 0 });
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].panes(), vec![10]);
    }

    #[test]
    fn closing_a_pane_clears_a_zoom_that_was_on_it() {
        // A zoom naming a pane that no longer exists would lay out an empty tree.
        let mut t = tabs(&[(1, 10), (2, 20)]);
        assert!(t[0].root.split(10, SplitDir::Vertical, 11));
        t[0].zoomed = Some(11);

        assert_eq!(close_pane(&mut t, 11), Closed::Pane { tab: 0 });
        assert_eq!(t[0].zoomed, None);
    }

    #[test]
    fn where_the_active_tab_lands_after_a_removal() {
        // An earlier tab closed: follow the one being looked at down a place.
        assert_eq!(active_after_removing(2, 0, 2), 1);
        assert_eq!(active_after_removing(1, 0, 2), 0);
        // A later tab closed: nothing moves.
        assert_eq!(active_after_removing(0, 1, 2), 0);
        assert_eq!(active_after_removing(1, 2, 2), 1);
        // The active tab itself: the same slot, clamped to the end of what is left.
        assert_eq!(active_after_removing(1, 1, 1), 0);
        assert_eq!(active_after_removing(0, 0, 2), 0);
        // The last tab of all.
        assert_eq!(active_after_removing(0, 0, 0), 0);
    }
}
