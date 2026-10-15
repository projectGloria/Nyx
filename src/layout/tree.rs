//! The split tree for one tab.
//!
//! A `Box` tree rather than an arena: realistic depth is under six, an arena buys nothing at that
//! size and costs readability. There are deliberately **no parent pointers** — everything that
//! would need one is done on the flattened result from `geometry::solve` instead, which is also
//! what makes directional focus work across arbitrary shapes without special cases.

use crate::term::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// `a` left, `b` right.
    Horizontal,
    /// `a` top, `b` bottom.
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split { dir: SplitDir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// Addresses a node by the left/right (or top/bottom) choices taken to reach it.
///
/// `false` is the `a` side. Used to identify a gutter across frames so egui can track a drag,
/// and to walk back down to the node a drag is resizing.
pub type NodePath = Vec<bool>;

impl Node {
    pub fn leaf(pane: PaneId) -> Self {
        Node::Leaf(pane)
    }

    /// Every pane in this subtree, left-to-right / top-to-bottom.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            },
        }
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(id) => *id == pane,
            Node::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    /// `#[cfg(test)]` because only the tests ask. Production code that has a `Node` always
    /// wants to know *which* leaf, and gets that from `leaves`, `contains` or a match.
    #[cfg(test)]
    pub fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf(_))
    }

    pub fn at_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        let mut node = self;
        for step in path {
            match node {
                Node::Split { a, b, .. } => node = if *step { b } else { a },
                Node::Leaf(_) => return None,
            }
        }
        Some(node)
    }

    /// Replace `target`'s leaf with a split holding `target` and `new_pane`.
    ///
    /// Returns false if `target` is not in this tree, which is the caller's bug rather than a
    /// state to recover from.
    pub fn split(&mut self, target: PaneId, dir: SplitDir, new_pane: PaneId) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new_pane)),
                };
                true
            },
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.split(target, dir, new_pane) || b.split(target, dir, new_pane)
            },
        }
    }

    /// Remove a pane, collapsing its parent split into the surviving sibling.
    ///
    /// Returns `false` when the pane is the whole tree — the caller has to close the tab instead,
    /// because a tab with an empty layout has nothing to draw and no pane to focus.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                if **a == Node::Leaf(pane) {
                    *self = (**b).clone();
                    return true;
                }
                if **b == Node::Leaf(pane) {
                    *self = (**a).clone();
                    return true;
                }
                a.remove(pane) || b.remove(pane)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_a_leaf_puts_both_panes_in_the_tree() {
        let mut root = Node::leaf(1);
        assert!(root.split(1, SplitDir::Horizontal, 2));
        assert_eq!(root.leaves(), vec![1, 2]);

        // The new pane goes on the `b` side, so it is right of / below the one you split.
        assert!(root.split(2, SplitDir::Vertical, 3));
        assert_eq!(root.leaves(), vec![1, 2, 3]);
    }

    #[test]
    fn splitting_an_absent_pane_changes_nothing() {
        let mut root = Node::leaf(1);
        assert!(!root.split(99, SplitDir::Horizontal, 2));
        assert_eq!(root.leaves(), vec![1]);
    }

    #[test]
    fn removing_collapses_the_parent_into_the_sibling() {
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);
        assert_eq!(root.leaves(), vec![1, 2, 3]);

        assert!(root.remove(2));
        assert_eq!(root.leaves(), vec![1, 3]);

        assert!(root.remove(3));
        assert_eq!(root.leaves(), vec![1]);
        assert!(root.is_leaf());
    }

    #[test]
    fn removing_the_last_pane_is_refused() {
        // There is no such thing as a tab with no panes; the caller closes the tab instead.
        let mut root = Node::leaf(1);
        assert!(!root.remove(1));
        assert_eq!(root.leaves(), vec![1]);
    }

    #[test]
    fn at_mut_walks_the_path() {
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);

        // false = the `a` side of the root, which is the original leaf 1.
        assert_eq!(root.at_mut(&[false]).unwrap(), &mut Node::Leaf(1));
        // true = the `b` side, which is now the nested vertical split.
        assert!(matches!(root.at_mut(&[true]).unwrap(), Node::Split { .. }));
        // Walking off the end of a leaf is None, not a panic.
        assert!(root.at_mut(&[false, true]).is_none());
    }
}
