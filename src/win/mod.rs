//! Windows-specific behaviour: the global hotkey, Quake geometry, the single-instance pipe, the
//! Explorer context menu, and lifting an icon out of an executable.
//!
//! Every module here is a thin wrapper over `windows-sys`. The rule is that a call fails by
//! reporting itself and returning `None`, never by panicking or by taking the terminal down with
//! it: none of this is load-bearing. A hotkey that cannot register, a pipe that cannot open and a
//! registry key that cannot be written all leave a working terminal behind.

pub mod assets;
pub mod launcher;
pub mod sounds;
pub mod console;
pub mod cursor;
pub mod hotkey;
pub mod ipc;
pub mod paths;
pub mod quake;
pub mod registry;
pub mod shellicon;

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// A NUL-terminated UTF-16 string, for the `W` half of the API.
///
/// Always the `W` functions, never the `A` ones: the `A` family goes through the process code
/// page, and a path with a non-ASCII character in it — which on this machine is most of them —
/// comes back mangled or not at all.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// How many lines one wheel notch scrolls, from the system setting.
///
/// Windows' default is three, which is what Nyx hardcoded until now. The setting is per user and
/// people who change it notice immediately — and `-1` is the documented value for "one screen at
/// a time", which a terminal has to translate into the pane's own height rather than pass on.
pub fn wheel_lines() -> WheelStep {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWHEELSCROLLLINES,
    };

    let mut lines: u32 = 3;
    // SAFETY: `SPI_GETWHEELSCROLLLINES` writes one `u32` through `pvparam`, which is what is
    // passed. The last argument is the "also update the user profile" flag, which a read must
    // not set.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            (&mut lines as *mut u32).cast(),
            0,
        )
    };
    if ok == 0 {
        return WheelStep::Lines(3.0);
    }
    match lines {
        // The documented sentinel for "scroll one page".
        u32::MAX => WheelStep::Page,
        0 => WheelStep::Lines(0.0), // the user turned wheel scrolling off
        n => WheelStep::Lines(n as f32),
    }
}

/// What one wheel notch means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelStep {
    Lines(f32),
    /// `WHEEL_PAGESCROLL`: as many lines as the thing under the pointer is tall.
    Page,
}

impl WheelStep {
    /// Lines per notch for a pane `rows` tall.
    ///
    /// One row short of a screen, the way every pager does it: a page that scrolls the whole
    /// height leaves nothing to read against, and you lose your place on every notch.
    pub fn for_rows(self, rows: usize) -> f32 {
        match self {
            WheelStep::Lines(n) => n,
            WheelStep::Page => rows.saturating_sub(1).max(1) as f32,
        }
    }
}

/// Our own top-level window, as something that can cross a thread boundary.
///
/// `HWND` is `*mut c_void` and so neither `Send` nor `Sync`, but a window handle is not a pointer
/// to memory we own — it is a process-wide token the window manager hands back, and calling
/// `ShowWindow` on it from another thread is exactly what the API is for. The newtype is where
/// that reasoning lives, so no call site has to repeat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window(isize);

// SAFETY: see above — an HWND is a handle, not a borrow.
unsafe impl Send for Window {}
unsafe impl Sync for Window {}

impl Window {
    /// The `HWND` behind eframe's frame, if there is one.
    ///
    /// `None` on a headless run, which is the only case the test harness produces.
    pub fn of(frame: &eframe::Frame) -> Option<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match frame.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(Window(handle.hwnd.get())),
            _ => None,
        }
    }

    pub fn raw(self) -> windows_sys::Win32::Foundation::HWND {
        self.0 as _
    }

    pub fn as_isize(self) -> isize {
        self.0
    }

    pub fn from_isize(raw: isize) -> Option<Self> {
        (raw != 0).then_some(Window(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated() {
        let w = wide("ab");
        assert_eq!(w, vec![b'a' as u16, b'b' as u16, 0]);
    }

    #[test]
    fn a_page_scroll_leaves_one_line_of_context() {
        // WHEEL_PAGESCROLL means "a screen", but a screen that scrolls the full height leaves
        // nothing to read against and you lose your place on every notch.
        assert_eq!(WheelStep::Page.for_rows(30), 29.0);
        assert_eq!(WheelStep::Lines(3.0).for_rows(30), 3.0);
        // Degenerate panes still scroll by something rather than by zero.
        assert_eq!(WheelStep::Page.for_rows(1), 1.0);
        assert_eq!(WheelStep::Page.for_rows(0), 1.0);
    }

    #[test]
    fn the_wheel_setting_is_readable_and_sane() {
        // Not asserting a value — it is the machine's setting — only that the call succeeds and
        // the result is something a scroll can be computed from.
        let step = wheel_lines();
        assert!(step.for_rows(30) >= 0.0);
    }
}

/// Hand a URL to whatever the user has set as their browser.
///
/// **`http` and `https` only, checked again here.** `term::links` will not offer anything else,
/// and this is the second of the two places that say no — the same shape as `term::proxy`
/// refusing an OSC 52 read that `term::spawn` has already refused. The text being opened came
/// out of a pipe, and every other scheme on a Windows machine is a registered launcher for some
/// local program; one of the two checks surviving a careless edit is the point of having both.
///
/// Failure is reported and never fatal, like everything else in this module.
pub fn open_url(url: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if !is_web_url(url) {
        return Err(format!("refusing to open {url}: only http and https are opened"));
    }

    let verb = wide("open");
    let target = wide(url);
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call, and the window,
    // parameter and directory arguments are documented as optional and passed null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a fake HINSTANCE; anything over 32 is success. The API is this old.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(format!("could not open {url} (ShellExecute returned {})", result as isize))
    }
}

/// Whether this is a URL Nyx will open at all.
///
/// Pure, so the rule can be tested without launching a browser on the machine running the suite.
pub fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    // A scheme is ASCII and case-insensitive; everything after it is left exactly as it came.
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && url.len() > "https://".len()
        && !url.contains(|c: char| c.is_control())
}

#[cfg(test)]
mod url_tests {
    #[test]
    fn only_web_urls_are_opened() {
        assert!(super::is_web_url("https://example.com/a"));
        assert!(super::is_web_url("HTTP://Example.com/a"));

        for refused in [
            "file:///C:/Windows/System32/cmd.exe",
            "ms-settings:windowsupdate",
            "steam://run/440",
            "javascript:alert(1)",
            "https://",
            "",
        ] {
            assert!(!super::is_web_url(refused), "{refused} would have been opened");
        }
    }

    /// A control character in a URL is a terminal escape that survived into the text.
    #[test]
    fn a_url_carrying_a_control_character_is_refused() {
        assert!(!super::is_web_url("https://example.com/\u{1b}]0;x\u{7}"));
        assert!(super::open_url("https://example.com/\u{1b}x").is_err());
    }
}
