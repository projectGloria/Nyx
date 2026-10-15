//! Quake mode: the window drops down from the top of the screen and goes away again.
//!
//! Everything here works in **physical pixels through `SetWindowPos`**, not in egui points through
//! `ViewportCommand`. Two reasons, and both are bugs avoided rather than taste:
//!
//! `ViewportCommand::OuterPosition` is multiplied by the *current* window's `pixels_per_point`,
//! so dropping onto a second monitor at a different scale places the window using the old
//! monitor's factor — off by a third on a 100%-to-150% pair, which is most laptops with an
//! external screen. The monitor's work area comes back from Win32 in physical pixels, and keeping
//! it that way means there is no conversion to get wrong.
//!
//! And the surface must survive. Show and hide are `ShowWindow`, never a window rebuild: winit
//! reads `IsWindowVisible` rather than caching a flag, so going around it leaves eframe's idea of
//! visibility correct, while destroying and recreating the window would take the wgpu surface,
//! every pane's `TextRenderer` and the atlas with it.
//!
//! The slide moves **Y only, at a fixed size**. Animating the height would reconfigure the
//! surface and resize every ConPTY eight times in a tenth of a second, and ConPTY reflows
//! asynchronously — the drop-down would land on a grid still catching up with itself.

use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, SetForegroundWindow, SetWindowPos, ShowWindow, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_HIDE, SW_SHOW,
};

use crate::win::Window;

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn with_y(self, y: i32) -> Self {
        Rect { y, ..self }
    }
}

/// Which screen to drop down on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monitor {
    /// The one the pointer is on — "wherever I am looking".
    Cursor,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Monitor {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "cursor" | "mouse" | "active" => Monitor::Cursor,
            "primary" | "main" => Monitor::Primary,
            _ => return None,
        })
    }
}

impl Align {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "left" => Align::Left,
            "center" | "centre" | "middle" => Align::Center,
            "right" => Align::Right,
            _ => return None,
        })
    }
}

/// The work area of the chosen monitor, in physical pixels.
///
/// The *work* area, not the monitor rectangle: a dropdown that covers the taskbar is a dropdown
/// you cannot get out from behind.
pub fn work_area(monitor: Monitor) -> Option<Rect> {
    let (point, flags) = match monitor {
        Monitor::Cursor => {
            let mut p = POINT { x: 0, y: 0 };
            // SAFETY: writes one POINT. A failure leaves the zeroed value, which lands on the
            // primary monitor — the same answer the other branch gives.
            unsafe { GetCursorPos(&mut p) };
            (p, MONITOR_DEFAULTTONEAREST)
        },
        Monitor::Primary => (POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
    };

    // SAFETY: `MonitorFromPoint` cannot fail with a DEFAULTTO* flag — it always returns a handle.
    let handle = unsafe { MonitorFromPoint(point, flags) };

    // SAFETY: `cbSize` tells the API which version of the struct it is filling, and getting it
    // wrong is the documented way to have this fail.
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: `info` is a correctly sized MONITORINFO.
    if unsafe { GetMonitorInfoW(handle, &mut info) } == 0 {
        return None;
    }

    let work = info.rcWork;
    Some(Rect {
        x: work.left,
        y: work.top,
        w: (work.right - work.left).max(1),
        h: (work.bottom - work.top).max(1),
    })
}

/// Where the dropdown sits when it is fully down.
///
/// Pure, so the fractions and the alignment can be tested without a monitor.
pub fn target(work: Rect, width: f32, height: f32, align: Align) -> Rect {
    // Clamped rather than trusted: a `height = 0` in the config is a window with no rows in it,
    // which looks exactly like a crash.
    let w = ((work.w as f32) * width.clamp(0.1, 1.0)).round() as i32;
    let h = ((work.h as f32) * height.clamp(0.1, 1.0)).round() as i32;
    let x = match align {
        Align::Left => work.x,
        Align::Center => work.x + (work.w - w) / 2,
        Align::Right => work.x + work.w - w,
    };
    Rect { x, y: work.y, w: w.max(1), h: h.max(1) }
}

/// What the hotkey thread and the UI thread agree on.
///
/// Two atomics rather than a mutex because one side of every access is a signal handler in all but
/// name: the hotkey thread is inside `GetMessageW` and must not be able to block on the UI thread,
/// which spends most of its time holding a `Term` lock.
#[derive(Default)]
pub struct Shared {
    /// Our top-level window, or 0 before the first frame has published it.
    hwnd: AtomicIsize,
    hidden: AtomicBool,
}

impl Shared {
    pub fn publish(&self, window: Window) {
        self.hwnd.store(window.as_isize(), Ordering::Release);
    }

    pub fn window(&self) -> Option<Window> {
        Window::from_isize(self.hwnd.load(Ordering::Acquire))
    }

    pub fn hidden(&self) -> bool {
        self.hidden.load(Ordering::Acquire)
    }

    fn set_hidden(&self, hidden: bool) {
        self.hidden.store(hidden, Ordering::Release);
    }

    /// Show the window immediately, from whichever thread is asking.
    ///
    /// Called by the hotkey thread purely to beat eframe's 100 ms repaint throttle on invisible
    /// windows; the flag is left alone so the UI thread still decides what the press meant. If the
    /// app then decides to hide again the window was parked above the screen anyway, so nothing
    /// is visible in between.
    pub fn raise_if_hidden(&self) {
        if !self.hidden() {
            return;
        }
        let Some(window) = self.window() else { return };
        // SAFETY: a window handle owned by this process. Both calls are no-ops on a stale one.
        unsafe {
            ShowWindow(window.raw(), SW_SHOW);
            // Legitimate here rather than a foreground steal: the documented conditions include
            // "the process is processing a hotkey", which is exactly where this runs.
            SetForegroundWindow(window.raw());
        }
    }
}

/// How the window is moving right now.
struct Slide {
    from: i32,
    to: i32,
    started: Instant,
    duration: Duration,
    /// Hide the window when the slide finishes. Sliding up and then vanishing is the whole
    /// difference between a dropdown retracting and a window blinking out.
    hide_at_end: bool,
}

/// The dropdown, as the app sees it.
pub struct Quake {
    pub shared: std::sync::Arc<Shared>,
    /// The rectangle the current slide is moving within, so a frame never re-derives it — the
    /// monitor under the cursor can change mid-slide and the window would jump sideways.
    rect: Option<Rect>,
    slide: Option<Slide>,
}

impl Default for Quake {
    fn default() -> Self {
        Self { shared: std::sync::Arc::new(Shared::default()), rect: None, slide: None }
    }
}

/// The parts of `[quake]` this module needs, already validated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub dropdown: bool,
    pub width: f32,
    pub height: f32,
    pub align: Align,
    pub monitor: Monitor,
    pub animate: Duration,
}

/// `[quake]`, resolved once, with every bad value reported by name.
///
/// Same rule as the rest of the config: a typo costs that key and nothing else. `align = "up"`
/// falls back to centred and says so, rather than turning the dropdown off.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// `None` when the chord could not be turned into one — reported, and the hotkey simply is
    /// not registered.
    pub chord: Option<crate::win::hotkey::Chord>,
    pub geometry: Geometry,
    pub hide_on_focus_loss: bool,
    pub start_hidden: bool,
}

impl Settings {
    pub fn from_config(cfg: &crate::config::schema::QuakeConfig) -> (Self, Vec<String>) {
        let mut problems = Vec::new();

        let chord = if !cfg.enabled || cfg.hotkey.trim().is_empty() {
            None
        } else {
            match crate::config::keys::parse(&cfg.hotkey) {
                None => {
                    problems.push(format!(
                        "quake.hotkey: \"{}\" is not a key combination",
                        cfg.hotkey
                    ));
                    None
                },
                Some(binding) => {
                    let chord = crate::win::hotkey::Chord::from_binding(binding);
                    if chord.is_none() {
                        problems.push(format!(
                            "quake.hotkey: \"{}\" cannot be a global hotkey — it needs at least \
                             one of Ctrl, Shift or Alt, and a key Windows has a code for",
                            cfg.hotkey
                        ));
                    }
                    chord
                },
            }
        };

        let align = Align::parse(&cfg.align).unwrap_or_else(|| {
            problems.push(format!(
                "quake.align: \"{}\" is not left, center or right",
                cfg.align
            ));
            Align::Center
        });

        let monitor = Monitor::parse(&cfg.monitor).unwrap_or_else(|| {
            problems.push(format!("quake.monitor: \"{}\" is not cursor or primary", cfg.monitor));
            Monitor::Cursor
        });

        let settings = Settings {
            enabled: cfg.enabled,
            chord,
            geometry: Geometry {
                dropdown: cfg.dropdown,
                width: cfg.width,
                height: cfg.height,
                align,
                monitor,
                animate: Duration::from_millis(cfg.animate_ms),
            },
            hide_on_focus_loss: cfg.hide_on_focus_loss,
            start_hidden: cfg.start_hidden,
        };
        (settings, problems)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_config(&crate::config::schema::QuakeConfig::default()).0
    }
}

impl Quake {
    pub fn hidden(&self) -> bool {
        self.shared.hidden()
    }

    pub fn animating(&self) -> bool {
        self.slide.is_some()
    }

    /// Bring the window down. A no-op if it is already on its way down.
    pub fn show(&mut self, geometry: Geometry) {
        let Some(window) = self.shared.window() else { return };

        if !geometry.dropdown {
            self.slide = None;
            self.shared.set_hidden(false);
            // SAFETY: our own window.
            unsafe {
                ShowWindow(window.raw(), SW_SHOW);
                SetForegroundWindow(window.raw());
            }
            return;
        }

        let rect = self.place(window, geometry);
        let top = rect.y;
        let parked = rect.y - rect.h;

        // Start from wherever it actually is, so pressing the hotkey during a retract reverses
        // the slide instead of restarting it from off-screen.
        let from = self.current_y(window).unwrap_or(parked).min(top);
        move_to(window, rect.with_y(from));

        self.shared.set_hidden(false);
        // SAFETY: our own window. `SW_SHOW` before the slide, so the first moved frame is drawn.
        unsafe {
            ShowWindow(window.raw(), SW_SHOW);
            SetForegroundWindow(window.raw());
        }
        self.slide = begin(from, top, geometry.animate, false);
        if self.slide.is_none() {
            move_to(window, rect);
        }
    }

    /// Retract, then hide.
    pub fn hide(&mut self, geometry: Geometry) {
        let Some(window) = self.shared.window() else { return };

        if !geometry.dropdown {
            self.slide = None;
            self.shared.set_hidden(true);
            // SAFETY: our own window.
            unsafe { ShowWindow(window.raw(), SW_HIDE) };
            return;
        }

        let rect = self.place(window, geometry);
        let from = self.current_y(window).unwrap_or(rect.y);
        let parked = rect.y - rect.h;

        self.slide = begin(from, parked, geometry.animate, true);
        if self.slide.is_none() {
            self.finish_hide(window, rect.with_y(parked));
        }
    }

    /// Advance the slide. Returns how long until the next frame is wanted, if it is still moving.
    pub fn tick(&mut self) -> Option<Duration> {
        let slide = self.slide.as_ref()?;
        let window = self.shared.window()?;
        let rect = self.rect?;

        let elapsed = slide.started.elapsed();
        let t = (elapsed.as_secs_f32() / slide.duration.as_secs_f32()).clamp(0.0, 1.0);
        // Ease out: fast at the start, settling at the end. A linear slide reads as mechanical
        // and, at 120 ms, as slower than it is.
        let eased = 1.0 - (1.0 - t).powi(3);
        let y = slide.from as f32 + (slide.to - slide.from) as f32 * eased;
        move_to(window, rect.with_y(y.round() as i32));

        if t < 1.0 {
            // ~144 Hz. The slide is the one thing in Nyx that asks for frames it did not earn,
            // and it stops asking the instant it is over.
            return Some(Duration::from_millis(7));
        }

        let done = self.slide.take().expect("checked above");
        if done.hide_at_end {
            self.finish_hide(window, rect.with_y(done.to));
        }
        None
    }

    /// Compute the target rectangle, remember it for the slide, and apply the parts that are not
    /// animated (x and size) so the slide only has to move y.
    fn place(&mut self, window: Window, geometry: Geometry) -> Rect {
        let work = work_area(geometry.monitor).unwrap_or(Rect { x: 0, y: 0, w: 1280, h: 720 });
        let rect = target(work, geometry.width, geometry.height, geometry.align);
        // Keep the current y: moving it here would snap the window before the slide starts.
        let keep_y = self.current_y(window).unwrap_or(rect.y);
        if self.rect != Some(rect) {
            move_to(window, rect.with_y(keep_y));
        }
        self.rect = Some(rect);
        rect
    }

    fn finish_hide(&mut self, window: Window, parked: Rect) {
        // Parked above the screen *before* it vanishes, which is what lets the hotkey thread call
        // `ShowWindow` on the next press without a frame of the window appearing at the old spot.
        move_to(window, parked);
        self.shared.set_hidden(true);
        // SAFETY: our own window.
        unsafe { ShowWindow(window.raw(), SW_HIDE) };
    }

    fn current_y(&self, window: Window) -> Option<i32> {
        let mut rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: writes one RECT; returns 0 for a handle it does not like.
        if unsafe { GetWindowRect(window.raw(), &mut rect) } == 0 {
            return None;
        }
        Some(rect.top)
    }
}

fn begin(from: i32, to: i32, duration: Duration, hide_at_end: bool) -> Option<Slide> {
    if duration.is_zero() || from == to {
        return None;
    }
    Some(Slide { from, to, started: Instant::now(), duration, hide_at_end })
}

fn move_to(window: Window, rect: Rect) {
    // SAFETY: our own window. `SWP_NOACTIVATE` keeps a move from stealing focus, and
    // `SWP_NOZORDER` keeps the dropdown from jumping in front of a window the user put on top.
    unsafe {
        SetWindowPos(
            window.raw(),
            ptr::null_mut(),
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };

    #[test]
    fn a_full_width_dropdown_covers_the_work_area_and_not_the_taskbar() {
        let r = target(WORK, 1.0, 0.5, Align::Center);
        assert_eq!(r.x, 0);
        assert_eq!(r.w, 1920);
        assert_eq!(r.y, 0);
        assert_eq!(r.h, 520);
    }

    #[test]
    fn alignment_places_a_narrow_dropdown() {
        let left = target(WORK, 0.5, 0.4, Align::Left);
        let centre = target(WORK, 0.5, 0.4, Align::Center);
        let right = target(WORK, 0.5, 0.4, Align::Right);
        assert_eq!(left.x, 0);
        assert_eq!(centre.x, 480);
        assert_eq!(right.x, 960);
        assert_eq!(right.x + right.w, WORK.w);
    }

    #[test]
    fn a_monitor_that_is_not_at_the_origin_is_handled() {
        // A second screen to the left of the primary has a negative x, and a dropdown that
        // ignored it would land on the wrong monitor entirely.
        let work = Rect { x: -1920, y: -200, w: 1920, h: 1080 };
        let r = target(work, 1.0, 0.5, Align::Center);
        assert_eq!(r.x, -1920);
        assert_eq!(r.y, -200);
    }

    #[test]
    fn degenerate_fractions_are_clamped_rather_than_producing_an_empty_window() {
        // `height = 0` in a hand-edited config is a window with no rows in it, which looks
        // exactly like a crash.
        let zero = target(WORK, 0.0, 0.0, Align::Center);
        assert!(zero.w > 0 && zero.h > 0);
        let huge = target(WORK, 5.0, 5.0, Align::Center);
        assert_eq!(huge.w, WORK.w);
        assert_eq!(huge.h, WORK.h);
    }

    #[test]
    fn names_parse_the_way_the_config_spells_them() {
        assert_eq!(Align::parse("Centre"), Some(Align::Center));
        assert_eq!(Align::parse("center"), Some(Align::Center));
        assert_eq!(Align::parse("sideways"), None);
        assert_eq!(Monitor::parse("cursor"), Some(Monitor::Cursor));
        assert_eq!(Monitor::parse("PRIMARY"), Some(Monitor::Primary));
        assert_eq!(Monitor::parse("second"), None);
    }

    #[test]
    fn the_shipped_default_claims_no_hotkey_and_complains_about_nothing() {
        // A global hotkey is a claim on every keystroke on the machine. The default config must
        // not take one, and must not produce a warning for having a chord written in it that it
        // is deliberately not using.
        let (settings, problems) =
            Settings::from_config(&crate::config::schema::QuakeConfig::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!settings.enabled);
        assert!(settings.chord.is_none(), "nothing may be registered while quake is off");
    }

    #[test]
    fn a_bad_value_costs_that_value_and_nothing_else() {
        // The same rule as the rest of the config: `align = "up"` falls back to centred and says
        // so, rather than switching the dropdown off.
        let cfg = crate::config::schema::QuakeConfig {
            enabled: true,
            align: "up".into(),
            monitor: "the big one".into(),
            ..Default::default()
        };
        let (settings, problems) = Settings::from_config(&cfg);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("quake.align")));
        assert!(problems.iter().any(|p| p.contains("quake.monitor")));
        assert_eq!(settings.geometry.align, Align::Center);
        assert_eq!(settings.geometry.monitor, Monitor::Cursor);
        // And the hotkey still works, because it is a different key.
        assert!(settings.chord.is_some());
    }

    #[test]
    fn a_hotkey_that_cannot_be_global_is_named_rather_than_silently_dropped() {
        let cfg = crate::config::schema::QuakeConfig {
            enabled: true,
            hotkey: "F12".into(),
            ..Default::default()
        };
        let (settings, problems) = Settings::from_config(&cfg);
        assert!(settings.chord.is_none());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("F12"), "{}", problems[0]);
    }

    #[test]
    fn the_shared_flag_starts_visible() {
        // A Nyx that started hidden with no way to know the hotkey works would look like a
        // launch that failed.
        let shared = Shared::default();
        assert!(!shared.hidden());
        assert!(shared.window().is_none());
    }

    #[test]
    fn real_work_area_is_plausible() {
        // Runs against this machine's actual monitors; the only claim is that the call works and
        // returns something a window could be placed inside.
        let work = work_area(Monitor::Primary).expect("a primary monitor exists");
        assert!(work.w > 100 && work.h > 100, "{work:?}");
    }
}
