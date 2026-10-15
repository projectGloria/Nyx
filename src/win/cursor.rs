//! Imported mouse pointers. Handles and the window subclass belong to the UI thread.
use std::{path::{Path, PathBuf}, ptr::null_mut};
use windows_sys::Win32::{Foundation::{HWND, LPARAM, LRESULT, WPARAM}, UI::{
    Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
    WindowsAndMessaging::{DestroyCursor, HCURSOR, HTCLIENT, IDC_ARROW, LoadCursorW,
        LoadImageW, PostMessageW, SetCursor, IMAGE_CURSOR, LR_DEFAULTSIZE, LR_LOADFROMFILE, WM_NCDESTROY, WM_SETCURSOR},
}};

#[derive(Default)]
struct State { hwnd: HWND, cursor: HCURSOR, active: bool }
#[derive(Default)]
pub struct CustomCursor { state: Box<State>, attempted: Option<String> }
const SUBCLASS_ID: usize = 0x4e595843;

unsafe extern "system" fn cursor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    // SAFETY: data points to the stable Box owned by CustomCursor. Drop removes this callback
    // before freeing it; WM_NCDESTROY clears the handle before the window disappears.
    let state = unsafe { &mut *(data as *mut State) };
    if msg == WM_SETCURSOR && (lp as u32 & 0xffff) == HTCLIENT && state.active && !state.cursor.is_null() {
        unsafe { SetCursor(state.cursor); }
        return 1;
    }
    if msg == WM_NCDESTROY {
        state.hwnd = null_mut();
        unsafe { RemoveWindowSubclass(hwnd, Some(cursor_proc), SUBCLASS_ID); }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

fn load(path: &Path) -> std::io::Result<HCURSOR> {
    let wide = super::wide(path);
    // SAFETY: path is NUL-terminated, and no module is required for LR_LOADFROMFILE.
    let cursor = unsafe { LoadImageW(null_mut(), wide.as_ptr(), IMAGE_CURSOR, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE) };
    if cursor.is_null() { Err(std::io::Error::last_os_error()) } else { Ok(cursor) }
}

impl CustomCursor {
    pub fn configure(&mut self, frame: &eframe::Frame, path: &str) -> std::io::Result<()> {
        if self.attempted.as_deref() == Some(path) { return Ok(()); }
        let Some(window) = super::Window::of(frame) else { return Ok(()); };
        self.attempted = Some(path.to_owned());
        self.activate(false);
        if !self.state.cursor.is_null() {
            unsafe { DestroyCursor(self.state.cursor); }
            self.state.cursor = null_mut();
        }
        if path.is_empty() { return Ok(()); }
        let cursor = load(Path::new(path))?;
        self.state.hwnd = window.as_isize() as HWND;
        self.state.cursor = cursor;
        let data = (&mut *self.state) as *mut State as usize;
        // SAFETY: window, subclass and Box all live on eframe's UI thread.
        if unsafe { SetWindowSubclass(self.state.hwnd, Some(cursor_proc), SUBCLASS_ID, data) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { DestroyCursor(cursor); }
            self.state.cursor = null_mut();
            return Err(error);
        }
        Ok(())
    }
    /// Called every frame, so it acts only on a change.
    ///
    /// The subclass is what actually keeps the pointer ours: it answers every `WM_SETCURSOR`.
    /// Re-posting one on each of 60 frames a second is a syscall and a queued message per frame to
    /// say what the subclass already says, and the whole claim of this program is that it does no
    /// work while nothing is happening.
    pub fn activate(&mut self, active: bool) {
        let active = active && !self.state.cursor.is_null();
        if active == self.state.active {
            return;
        }
        self.state.active = active;
        if active {
            unsafe {
                SetCursor(self.state.cursor);
                // eframe applies its platform cursor after ui() returns. Queue the native
                // cursor refresh behind that update so it cannot replace our custom image.
                PostMessageW(self.state.hwnd, WM_SETCURSOR, self.state.hwnd as usize, HTCLIENT as isize);
            }
        } else {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_ARROW)); }
        }
    }
}
impl Drop for CustomCursor {
    fn drop(&mut self) {
        self.activate(false);
        if !self.state.hwnd.is_null() {
            unsafe { RemoveWindowSubclass(self.state.hwnd, Some(cursor_proc), SUBCLASS_ID); }
        }
        if !self.state.cursor.is_null() { unsafe { DestroyCursor(self.state.cursor); } }
    }
}

/// Keep a private copy beside nyx.exe, or in %LOCALAPPDATA%\Nyx if write-protected.
pub fn import(source: &Path) -> std::io::Result<PathBuf> {
    if !source.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cur")) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Choose a .cur file"));
    }
    let cursor = load(source)?;
    unsafe { DestroyCursor(cursor); }
    let bytes = std::fs::read(source)?;
    let exe = std::env::current_exe()?;
    let exe_parent = exe.parent().ok_or_else(|| std::io::Error::other("Nyx executable has no parent folder"))?;
    let beside_exe = exe_parent.join("cursors");
    let root = if std::fs::create_dir_all(&beside_exe).is_ok() {
        beside_exe
    } else {
        let fallback = crate::config::paths::data_dir().join("cursors");
        std::fs::create_dir_all(&fallback)?;
        fallback
    };
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    let name = source.file_stem().unwrap_or_default().to_string_lossy();
    let target = root.join(format!("{name}-{:016x}.cur", hash.finish()));
    crate::util::write_atomic(&target, &bytes)?;
    Ok(target)
}

pub fn choose() -> std::io::Result<Option<PathBuf>> {
    use windows_sys::Win32::UI::Controls::Dialogs::{CommDlgExtendedError, GetOpenFileNameW,
        OPENFILENAMEW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST};
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0u16; 32768];
    let filter: Vec<u16> = "Windows cursors (*.cur)\0*.cur\0\0".encode_utf16().collect();
    let title = super::wide("Choose a custom mouse cursor");
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() };
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: dialog buffers remain alive until this modal call returns.
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 { Ok(None) } else { Err(std::io::Error::other(format!("File picker error {error:#x}"))) };
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..end]))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn importing_a_cursor_keeps_a_loadable_private_copy() {
        // A 16x16 CUR with a 32-bit DIB and an AND mask, independent of machine assets.
        let mut bytes = vec![0u8; 22 + 40 + 16 * 16 * 4 + 16 * 4];
        bytes[2] = 2; bytes[4] = 1; bytes[6] = 16; bytes[7] = 16;
        let size = (bytes.len() - 22) as u32;
        bytes[14..18].copy_from_slice(&size.to_le_bytes());
        bytes[18..22].copy_from_slice(&22u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&40u32.to_le_bytes());
        bytes[26..30].copy_from_slice(&16i32.to_le_bytes());
        bytes[30..34].copy_from_slice(&32i32.to_le_bytes());
        bytes[34..36].copy_from_slice(&1u16.to_le_bytes());
        bytes[36..38].copy_from_slice(&32u16.to_le_bytes());
        for pixel in bytes[62..62 + 16 * 16 * 4].chunks_mut(4) { pixel.copy_from_slice(&[255, 255, 255, 255]); }
        let source = std::env::temp_dir().join(format!("nyx-cursor-test-{}.CUR", std::process::id()));
        std::fs::write(&source, &bytes).unwrap();
        let target = import(&source).unwrap();
        assert_eq!(target.parent().unwrap(), std::env::current_exe().unwrap().parent().unwrap().join("cursors"));
        std::fs::remove_file(&source).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        let handle = load(&target).unwrap();
        unsafe { DestroyCursor(handle); }
        std::fs::remove_file(&target).unwrap();
    }
    #[test]
    fn invalid_cursor_files_are_rejected() {
        assert!(import(Path::new("image.png")).is_err());
        let path = std::env::temp_dir().join(format!("nyx-invalid-cursor-{}.cur", std::process::id()));
        std::fs::write(&path, b"not cursor data").unwrap();
        assert!(import(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
