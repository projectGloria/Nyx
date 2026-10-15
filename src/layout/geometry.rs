//! Turning a split tree plus a rectangle into pane rects and draggable gutters.
//!
//! Pure: no egui state, no terminal, no side effects. `egui::Rect` is used as the value type
//! because converting to and from a private one would only create two ways to be wrong about
//! the same rectangle.

use egui::{Rect, Vec2};

use crate::layout::tree::{Node, NodePath, SplitDir};
use crate::term::PaneId;

#[derive(Debug, Clone)]
pub struct Gutter {
    pub path: NodePath,
    pub rect: Rect,
    pub parent: Rect,
    pub dir: SplitDir,
}

#[derive(Debug, Clone, Default)]
pub struct Solved {
    pub panes: Vec<(PaneId, Rect)>,
    pub gutters: Vec<Gutter>,
}

impl Solved {
    pub fn rect_of(&self, pane: PaneId) -> Option<Rect> {
        self.panes.iter().find(|(id, _)| *id == pane).map(|(_, r)| *r)
    }

    /// Which pane contains a point, if any.
    pub fn pane_at(&self, pos: egui::Pos2) -> Option<PaneId> {
        self.panes.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id)
    }
}

/// Lay the tree out inside `area`, reserving `gutter` pixels between siblings.
pub fn solve(root: &Node, area: Rect, gutter: f32) -> Solved {
    let mut out = Solved::default();
    walk(root, area, gutter, &mut Vec::new(), &mut out);
    out
}

fn walk(node: &Node, area: Rect, gutter: f32, path: &mut NodePath, out: &mut Solved) {
    match node {
        Node::Leaf(id) => out.panes.push((*id, area)),
        Node::Split { dir, ratio, a, b } => {
            let ratio = ratio.clamp(0.0, 1.0);
            match dir {
                SplitDir::Horizontal => {
                    // The gutter comes out of the middle, so neither side silently gains half a
                    // gutter's width and drifts out of step with its sibling.
                    let usable = (area.width() - gutter).max(0.0);
                    let left_w = usable * ratio;
                    // Clamped to the area: a pane narrower than the gutter would otherwise put
                    // the right-hand side's min past the area's max, and `from_min_max` happily
                    // produces a negative rect that every consumer then has to defend against.
                    let b_x = (area.min.x + left_w + gutter).min(area.max.x);
                    let a_rect = Rect::from_min_size(area.min, Vec2::new(left_w, area.height()));
                    let g_rect = Rect::from_min_size(
                        egui::pos2((area.min.x + left_w).min(area.max.x), area.min.y),
                        Vec2::new(gutter.min(area.width()), area.height()),
                    );
                    let b_rect =
                        Rect::from_min_max(egui::pos2(b_x, area.min.y), area.max);
                    out.gutters.push(Gutter { path: path.clone(), rect: g_rect, parent: area, dir: *dir });
                    descend(a, a_rect, gutter, path, false, out);
                    descend(b, b_rect, gutter, path, true, out);
                },
                SplitDir::Vertical => {
                    let usable = (area.height() - gutter).max(0.0);
                    let top_h = usable * ratio;
                    let b_y = (area.min.y + top_h + gutter).min(area.max.y);
                    let a_rect = Rect::from_min_size(area.min, Vec2::new(area.width(), top_h));
                    let g_rect = Rect::from_min_size(
                        egui::pos2(area.min.x, (area.min.y + top_h).min(area.max.y)),
                        Vec2::new(area.width(), gutter.min(area.height())),
                    );
                    let b_rect =
                        Rect::from_min_max(egui::pos2(area.min.x, b_y), area.max);
                    out.gutters.push(Gutter { path: path.clone(), rect: g_rect, parent: area, dir: *dir });
                    descend(a, a_rect, gutter, path, false, out);
                    descend(b, b_rect, gutter, path, true, out);
                },
            }
        },
    }
}

fn descend(
    node: &Node,
    area: Rect,
    gutter: f32,
    path: &mut NodePath,
    side: bool,
    out: &mut Solved,
) {
    path.push(side);
    walk(node, area, gutter, path, out);
    path.pop();
}

/// The smallest ratio that still leaves both sides at least `min_px`.
///
/// A gutter that can be dragged to nothing lets you create a two-column pane, which
/// `alacritty_terminal` rejects outright — so the gutter stops rather than the terminal panicking.
pub fn clamp_ratio(ratio: f32, total: f32, gutter: f32, min_px: f32) -> f32 {
    let usable = (total - gutter).max(1.0);
    let min_ratio = (min_px / usable).min(0.49);
    ratio.clamp(min_ratio, 1.0 - min_ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_same_axis_splits_record_the_complete_parent() {
        for dir in [SplitDir::Horizontal, SplitDir::Vertical] {
            let mut root = Node::leaf(1);
            root.split(1, dir, 2);
            root.split(1, dir, 3);
            let solved = solve(&root, area(), 4.0);
            assert_eq!(solved.gutters[0].parent, area());
            assert!(solved.gutters[1].parent.width() <= area().width());
            assert!(solved.gutters[1].parent.height() <= area().height());
            let gutter = &solved.gutters[0];
            let (position, start, total) = match dir {
                SplitDir::Horizontal => (gutter.rect.center().x, gutter.parent.min.x, gutter.parent.width()),
                SplitDir::Vertical => (gutter.rect.center().y, gutter.parent.min.y, gutter.parent.height()),
            };
            assert!(((position - start - 2.0) / (total - 4.0) - 0.5).abs() < 0.001);
        }
    }

    fn area() -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1000.0, 500.0))
    }

    #[test]
    fn a_single_leaf_fills_the_area() {
        let solved = solve(&Node::leaf(1), area(), 4.0);
        assert_eq!(solved.panes.len(), 1);
        assert_eq!(solved.panes[0].1, area());
        assert!(solved.gutters.is_empty());
    }

    #[test]
    fn a_horizontal_split_leaves_exactly_one_gutter_between_the_halves() {
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        let solved = solve(&root, area(), 4.0);

        let left = solved.rect_of(1).unwrap();
        let right = solved.rect_of(2).unwrap();
        assert_eq!(solved.gutters.len(), 1);

        // No overlap, no gap beyond the gutter, and the whole width accounted for.
        assert!((left.width() + 4.0 + right.width() - 1000.0).abs() < 0.01);
        assert!((right.min.x - left.max.x - 4.0).abs() < 0.01);
        assert_eq!(left.height(), 500.0);
        assert_eq!(right.height(), 500.0);
    }

    #[test]
    fn nested_splits_partition_the_area_without_overlapping() {
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);
        let solved = solve(&root, area(), 4.0);

        assert_eq!(solved.panes.len(), 3);
        for (i, (_, a)) in solved.panes.iter().enumerate() {
            for (_, b) in solved.panes.iter().skip(i + 1) {
                let overlap = a.intersect(*b);
                assert!(
                    overlap.width() <= 0.01 || overlap.height() <= 0.01,
                    "panes overlap: {a:?} and {b:?}"
                );
            }
        }
    }

    #[test]
    fn gutter_paths_address_the_node_they_came_from() {
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);
        let solved = solve(&root, area(), 4.0);

        // The root's gutter has an empty path; the nested one is on the `b` side.
        let mut paths: Vec<_> = solved.gutters.iter().map(|g| g.path.clone()).collect();
        paths.sort();
        assert_eq!(paths, vec![vec![], vec![true]]);

        // And walking each path reaches a Split, which is what a drag needs to mutate.
        for gutter in &solved.gutters {
            assert!(matches!(root.at_mut(&gutter.path), Some(Node::Split { .. })));
        }
    }

    #[test]
    fn a_degenerate_area_does_not_produce_negative_rects() {
        let tiny = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(2.0, 2.0));
        let mut root = Node::leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        let solved = solve(&root, tiny, 4.0);
        for (_, r) in &solved.panes {
            assert!(r.width() >= 0.0 && r.height() >= 0.0, "negative rect {r:?}");
        }
    }

    #[test]
    fn clamp_ratio_keeps_both_sides_alive() {
        // Dragging a gutter all the way left must still leave room for a usable pane.
        let r = clamp_ratio(0.0, 1000.0, 4.0, 100.0);
        assert!(r > 0.0);
        assert!((r * 996.0) >= 99.0);
        let r = clamp_ratio(1.0, 1000.0, 4.0, 100.0);
        assert!(r < 1.0);
    }
}
