//! Directional focus, computed geometrically over the flattened pane rects.
//!
//! Walking the tree instead would need special cases for every shape where one pane faces several
//! — the classic "one tall pane on the left, three stacked on the right" layout, where the tree
//! says "my sibling is a split" and gives no answer about which of the three you meant. Working
//! on rectangles makes that case ordinary.

use egui::Rect;

use crate::term::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The pane to focus when moving `dir` from `from`, if there is one that way.
pub fn neighbour(panes: &[(PaneId, Rect)], from: PaneId, dir: Direction) -> Option<PaneId> {
    let current = panes.iter().find(|(id, _)| *id == from)?.1;
    let origin = current.center();

    let mut best: Option<(f32, PaneId)> = None;

    for (id, rect) in panes {
        if *id == from {
            continue;
        }
        let c = rect.center();

        // Must actually lie in the requested direction, measured edge-to-centre so a pane that
        // merely overlaps on the axis does not count as being beside you.
        let primary = match dir {
            Direction::Left => origin.x - c.x,
            Direction::Right => c.x - origin.x,
            Direction::Up => origin.y - c.y,
            Direction::Down => c.y - origin.y,
        };
        if primary <= 0.0 {
            continue;
        }

        // How far off the travelling axis it sits. Weighted below 1.0 so a pane slightly to the
        // side but much closer still wins over a distant perfectly-aligned one — which is what
        // makes Alt+Right feel like it goes to "the next pane over" rather than to whichever
        // pane happens to share a centre line.
        let cross = match dir {
            Direction::Left | Direction::Right => (c.y - origin.y).abs(),
            Direction::Up | Direction::Down => (c.x - origin.x).abs(),
        };

        let score = primary + cross * 0.25;
        if best.is_none_or(|(b, _)| score < b) {
            best = Some((score, *id));
        }
    }

    best.map(|(_, id)| id)
}

/// The pane nearest to a rectangle, used when the focused pane is closed.
pub fn nearest(panes: &[(PaneId, Rect)], to: Rect) -> Option<PaneId> {
    let origin = to.center();
    panes
        .iter()
        .min_by(|(_, a), (_, b)| {
            let da = a.center().distance_sq(origin);
            let db = b.center().distance_sq(origin);
            da.total_cmp(&db)
        })
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    /// Left column is one tall pane; right column is three stacked. This is the layout a
    /// tree-walking implementation gets wrong.
    fn one_facing_three() -> Vec<(PaneId, Rect)> {
        vec![
            (1, rect(0.0, 0.0, 500.0, 600.0)),
            (2, rect(500.0, 0.0, 500.0, 200.0)),
            (3, rect(500.0, 200.0, 500.0, 200.0)),
            (4, rect(500.0, 400.0, 500.0, 200.0)),
        ]
    }

    #[test]
    fn moving_right_from_the_tall_pane_picks_the_vertically_closest() {
        let panes = one_facing_three();
        // Pane 1's centre is at y=300, which is inside pane 3.
        assert_eq!(neighbour(&panes, 1, Direction::Right), Some(3));
    }

    #[test]
    fn moving_left_from_any_of_the_three_returns_to_the_tall_one() {
        let panes = one_facing_three();
        for id in [2, 3, 4] {
            assert_eq!(neighbour(&panes, id, Direction::Left), Some(1));
        }
    }

    #[test]
    fn vertical_movement_within_the_column_works() {
        let panes = one_facing_three();
        assert_eq!(neighbour(&panes, 2, Direction::Down), Some(3));
        assert_eq!(neighbour(&panes, 3, Direction::Down), Some(4));
        assert_eq!(neighbour(&panes, 3, Direction::Up), Some(2));
    }

    #[test]
    fn there_is_no_neighbour_off_the_edge() {
        let panes = one_facing_three();
        assert_eq!(neighbour(&panes, 1, Direction::Left), None);
        assert_eq!(neighbour(&panes, 2, Direction::Up), None);
        assert_eq!(neighbour(&panes, 4, Direction::Down), None);
        assert_eq!(neighbour(&panes, 4, Direction::Right), None);
    }

    #[test]
    fn a_single_pane_has_no_neighbours_in_any_direction() {
        let panes = vec![(1, rect(0.0, 0.0, 100.0, 100.0))];
        for dir in [Direction::Left, Direction::Right, Direction::Up, Direction::Down] {
            assert_eq!(neighbour(&panes, 1, dir), None);
        }
    }

    #[test]
    fn nearest_finds_a_replacement_for_a_closed_pane() {
        let panes = one_facing_three();
        let closed = rect(500.0, 200.0, 500.0, 200.0);
        // Excluding the closed pane itself, 2 and 4 are equidistant; either is acceptable, but
        // it must not be the far-away pane 1.
        let remaining: Vec<_> =
            panes.iter().filter(|(id, _)| *id != 3).cloned().collect();
        let pick = nearest(&remaining, closed).unwrap();
        assert!(pick == 2 || pick == 4, "picked {pick}");
    }
}
