//! Mouse position to grid coordinates, and the X10/SGR reporting protocols.
//!
//! Two separate concerns share this module because they share the coordinate maths: *we* use
//! grid points for selection, and *the program* wants them encoded when it has asked for mouse
//! reporting. Which of the two happens for a given click is decided in `app.rs`, because it also
//! depends on whether Shift is held.

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::TermMode;

use crate::render::metrics::CellMetrics;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// No button — xterm's button 3, which is what a hover in all-motion mode reports and what
    /// X10 reports for any release.
    None,
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl Button {
    fn code(self) -> u8 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
            Button::None => 3,
            // Wheel events set bit 6 rather than using a button number of their own.
            Button::WheelUp => 64,
            Button::WheelDown => 65,
        }
    }
}

/// Where a wheel notch should go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WheelTarget {
    /// The program asked for mouse events; send it one.
    Report,
    /// A pager with no scrollback of its own wants the wheel as cursor keys.
    ArrowKeys,
    /// Ours to scroll.
    Scrollback,
}

/// Decide what a wheel notch does, in priority order.
///
/// **Mouse reporting comes first.** A program that asked for wheel events and also left
/// alternate-scroll on — and several termcaps set both — was handed arrow keys it never asked
/// for, and its own wheel handling never ran at all.
pub fn wheel_target(mode: TermMode, shift: bool) -> WheelTarget {
    if wants_mouse(mode, shift) {
        WheelTarget::Report
    } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
        WheelTarget::ArrowKeys
    } else {
        WheelTarget::Scrollback
    }
}

/// Whether a plain pointer move should be reported, and as which button.
///
/// Mode 1003 (`MOUSE_MOTION`) reports every move; mode 1002 (`MOUSE_DRAG`) reports one only while
/// a button is held. Checking `MOUSE_MOTION` alone meant **1002 received no motion at all** — the
/// modes are mutually exclusive, so a program that asked for drag reporting got button presses and
/// then silence.
///
/// A hover with nothing held is `Button::None`, not `Left`. Reporting `Left` tells the program its
/// left button is down, which is how an application that tracks hover ends up drawing a selection
/// nobody started.
pub fn motion_report(mode: TermMode, shift: bool, held: Option<Button>) -> Option<Button> {
    if !wants_mouse(mode, shift) {
        None
    } else if mode.contains(TermMode::MOUSE_MOTION) {
        Some(held.unwrap_or(Button::None))
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        held
    } else {
        None
    }
}

/// Where a pixel position falls in the grid, and which half of the cell it is in.
///
/// The side matters for selection: clicking the right half of a cell and dragging right should
/// include that cell, clicking the left half should not.
pub fn point_at(
    x: f32,
    y: f32,
    origin: (f32, f32),
    metrics: &CellMetrics,
    cols: usize,
    rows: usize,
    display_offset: usize,
) -> (Point, Side) {
    let rel_x = x - origin.0 - metrics.padding;
    let rel_y = y - origin.1 - metrics.padding;

    let col_f = (rel_x / metrics.cell_w).floor();
    let row_f = (rel_y / metrics.cell_h).floor();

    let col = (col_f.max(0.0) as usize).min(cols.saturating_sub(1));
    let row = (row_f.max(0.0) as usize).min(rows.saturating_sub(1));

    // Visible row `r` is grid line `r - display_offset`: scrolled-back content has negative
    // line numbers, which is also how the selection and the grid iterator address it.
    let line = Line(row as i32 - display_offset as i32);

    let cell_start = col_f * metrics.cell_w;
    let side = if rel_x - cell_start > metrics.cell_w / 2.0 { Side::Right } else { Side::Left };

    (Point::new(line, Column(col)), side)
}

/// True when the program has asked to receive mouse events at all.
pub fn wants_mouse(mode: TermMode, shift: bool) -> bool {
    // Shift is the universal escape hatch: holding it lets you select text with the mouse even
    // inside a full-screen program that has grabbed the mouse.
    mode.intersects(TermMode::MOUSE_MODE) && !shift
}

fn modifier_bits(m: egui::Modifiers) -> u8 {
    u8::from(m.shift) * 4 + u8::from(m.alt) * 8 + u8::from(m.ctrl) * 16
}

/// Encode one mouse event, or `None` if it cannot be represented.
///
/// `press` distinguishes button-down from button-up; motion events set the drag bit.
pub fn encode(
    button: Button,
    press: bool,
    motion: bool,
    point: Point,
    display_offset: usize,
    m: egui::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    // Protocols are 1-based and address the *visible* screen, so scrolled-back lines are not
    // reportable at all.
    let row = point.line.0 + display_offset as i32 + 1;
    let col = point.column.0 as i32 + 1;
    if row < 1 || col < 1 {
        return None;
    }

    let mut code = button.code() + modifier_bits(m);
    if motion {
        code += 32;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        // SGR has no coordinate limit and reports button identity on release, which is why it is
        // the mode everything modern negotiates.
        let final_byte = if press { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{col};{row}{final_byte}").into_bytes());
    }

    // X10/normal mode: one byte each, offset by 32, so nothing past column 223 can be reported.
    // Release loses the button identity and is always code 3.
    if col > 223 || row > 223 {
        return None;
    }
    let code = if press { code } else { 3 + modifier_bits(m) };
    Some(vec![0x1b, b'[', b'M', 32 + code, 32 + col as u8, 32 + row as u8])
}

/// The wheel, when there is no scrollback to move.
///
/// In the alternate screen there is no history, so a program that set `ALTERNATE_SCROLL` wants
/// the wheel translated into cursor keys — that is what makes the wheel scroll in `less` and in
/// a pager that never implemented mouse support.
pub fn alternate_scroll(up: bool, lines: usize, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::ALT_SCREEN) || !mode.contains(TermMode::ALTERNATE_SCROLL) {
        return None;
    }
    let seq: &[u8] = if mode.contains(TermMode::APP_CURSOR) {
        if up {
            b"\x1bOA"
        } else {
            b"\x1bOB"
        }
    } else if up {
        b"\x1b[A"
    } else {
        b"\x1b[B"
    };
    Some(seq.repeat(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_reporting_beats_alternate_scroll() {
        // Alternate-scroll was tried first, so a program that asked for wheel events and also
        // left alternate-scroll on was handed arrow keys it never asked for -- and its own wheel
        // handling never ran.
        let both =
            TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(wheel_target(both, false), WheelTarget::Report);

        // Shift is still the escape hatch. There is no scrollback in the alternate screen, so it
        // lands on the pager's arrow keys rather than on nothing at all.
        assert_eq!(wheel_target(both, true), WheelTarget::ArrowKeys);

        // A pager that never asked for the mouse keeps its arrow keys.
        assert_eq!(
            wheel_target(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL, false),
            WheelTarget::ArrowKeys
        );

        // Alternate-scroll outside the alternate screen means nothing; scrollback is ours.
        assert_eq!(wheel_target(TermMode::ALTERNATE_SCROLL, false), WheelTarget::Scrollback);
        assert_eq!(wheel_target(TermMode::empty(), false), WheelTarget::Scrollback);
    }

    #[test]
    fn drag_mode_reports_motion_only_while_a_button_is_held() {
        // 1002 was not checked at all: the two motion modes are mutually exclusive, so a program
        // that asked for drag reporting got its button presses and then silence.
        let drag = TermMode::MOUSE_DRAG;
        assert_eq!(motion_report(drag, false, Some(Button::Left)), Some(Button::Left));
        assert_eq!(motion_report(drag, false, Some(Button::Right)), Some(Button::Right));
        assert_eq!(motion_report(drag, false, None), None);
    }

    #[test]
    fn all_motion_mode_reports_a_hover_as_no_button() {
        // 1003 hardcoded Left, which tells the program its left button is down for every hover.
        let all = TermMode::MOUSE_MOTION;
        assert_eq!(motion_report(all, false, None), Some(Button::None));
        assert_eq!(motion_report(all, false, Some(Button::Right)), Some(Button::Right));

        // On the wire that hover is button 3 plus the motion bit: 35, not 32.
        let bytes = encode(
            Button::None,
            true,
            true,
            Point::new(Line(0), Column(0)),
            0,
            egui::Modifiers::NONE,
            all | TermMode::SGR_MOUSE,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[<35;1;1M");
    }

    #[test]
    fn click_only_mode_reports_no_motion_and_shift_suppresses_it() {
        assert_eq!(motion_report(TermMode::MOUSE_REPORT_CLICK, false, Some(Button::Left)), None);
        assert_eq!(motion_report(TermMode::MOUSE_MOTION, true, None), None);
        assert_eq!(motion_report(TermMode::empty(), false, Some(Button::Left)), None);
    }

    fn metrics() -> CellMetrics {
        CellMetrics {
            cell_w: 10.0,
            cell_h: 20.0,
            baseline: 15.0,
            font_size: 16.0,
            padding: 5.0,
            underline_y: 17.0,
            underline_h: 1.0,
        }
    }

    #[test]
    fn a_click_maps_to_the_cell_under_it() {
        let (p, side) = point_at(5.0, 5.0, (0.0, 0.0), &metrics(), 80, 24, 0);
        assert_eq!(p, Point::new(Line(0), Column(0)));
        assert_eq!(side, Side::Left);

        // 5 padding + 2 cells + most of the way across the third.
        let (p, side) = point_at(5.0 + 28.0, 5.0 + 41.0, (0.0, 0.0), &metrics(), 80, 24, 0);
        assert_eq!(p, Point::new(Line(2), Column(2)));
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn scrolled_back_rows_have_negative_line_numbers() {
        // Row 0 of a screen scrolled back 100 lines is grid line -100, which is how the grid
        // iterator and the selection both address history.
        let (p, _) = point_at(5.0, 5.0, (0.0, 0.0), &metrics(), 80, 24, 100);
        assert_eq!(p.line, Line(-100));
    }

    #[test]
    fn a_click_outside_the_grid_is_clamped_rather_than_dropped() {
        let (p, _) = point_at(-500.0, -500.0, (0.0, 0.0), &metrics(), 80, 24, 0);
        assert_eq!(p, Point::new(Line(0), Column(0)));
        let (p, _) = point_at(9999.0, 9999.0, (0.0, 0.0), &metrics(), 80, 24, 0);
        assert_eq!(p, Point::new(Line(23), Column(79)));
    }

    #[test]
    fn sgr_reports_button_identity_on_release_and_x10_does_not() {
        let point = Point::new(Line(0), Column(0));
        let sgr = encode(
            Button::Left,
            false,
            false,
            point,
            0,
            egui::Modifiers::NONE,
            TermMode::SGR_MOUSE,
        );
        assert_eq!(sgr.unwrap(), b"\x1b[<0;1;1m");

        let x10 =
            encode(Button::Left, false, false, point, 0, egui::Modifiers::NONE, TermMode::empty());
        assert_eq!(x10.unwrap(), vec![0x1b, b'[', b'M', 32 + 3, 33, 33]);
    }

    #[test]
    fn x10_refuses_coordinates_it_cannot_encode() {
        // Past column 223 the single-byte encoding overflows; reporting a wrong cell is worse
        // than reporting nothing.
        let far = Point::new(Line(0), Column(300));
        assert!(encode(
            Button::Left,
            true,
            false,
            far,
            0,
            egui::Modifiers::NONE,
            TermMode::empty()
        )
        .is_none());
        // SGR has no such limit.
        assert!(encode(
            Button::Left,
            true,
            false,
            far,
            0,
            egui::Modifiers::NONE,
            TermMode::SGR_MOUSE
        )
        .is_some());
    }

    #[test]
    fn modifiers_and_motion_set_their_bits() {
        let p = Point::new(Line(0), Column(0));
        let ctrl_drag =
            encode(Button::Left, true, true, p, 0, egui::Modifiers::CTRL, TermMode::SGR_MOUSE);
        // 0 (left) + 16 (ctrl) + 32 (motion)
        assert_eq!(ctrl_drag.unwrap(), b"\x1b[<48;1;1M");
    }

    #[test]
    fn shift_overrides_a_program_that_grabbed_the_mouse() {
        assert!(wants_mouse(TermMode::MOUSE_REPORT_CLICK, false));
        assert!(!wants_mouse(TermMode::MOUSE_REPORT_CLICK, true));
        assert!(!wants_mouse(TermMode::empty(), false));
    }

    #[test]
    fn alternate_scroll_only_fires_in_the_alternate_screen() {
        assert!(alternate_scroll(true, 3, TermMode::ALTERNATE_SCROLL).is_none());
        let seq = alternate_scroll(true, 3, TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL);
        assert_eq!(seq.unwrap(), b"\x1b[A\x1b[A\x1b[A");
    }
}
