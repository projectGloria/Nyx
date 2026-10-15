//! Printing from a GUI-subsystem executable.
//!
//! Nyx is built `windows_subsystem = "windows"` so that launching it does not flash a console
//! behind the terminal — which means `--help` and `--install-context-menu` have nowhere obvious to
//! write. Two places, tried in order:
//!
//! **Our own standard output, when it is a real handle.** `nyx --help > out.txt` and
//! `nyx --help | more` both hand the process a valid stdout, and a GUI app that ignored it would
//! produce an empty file — the kind of thing people report as "--help is broken".
//!
//! **The console of whatever started us.** With no redirection a GUI process is created with no
//! console at all and its std handles are null; `AttachConsole` borrows the parent's and `CONOUT$`
//! writes to it.
//!
//! With neither — a double-click, a Start-menu shortcut — the text goes nowhere, which is the
//! right outcome rather than a dialog box nobody asked for.

use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};

use crate::win::wide;

/// Write to wherever this launch's output belongs, if anywhere.
pub fn print(text: &str) {
    // SAFETY: returns a borrowed handle or null; nothing is allocated and nothing is ours to
    // close.
    let stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if !stdout.is_null() && stdout != INVALID_HANDLE_VALUE {
        write(stdout, text);
        return;
    }

    // SAFETY: attaching to the parent's console. Fails harmlessly when there is none, or when one
    // is already attached — a debug build has its own.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

    let name = wide("CONOUT$");
    // SAFETY: `CONOUT$` is the active screen buffer of the attached console.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return;
    }
    write(handle, text);
    // SAFETY: a handle from `CreateFileW` that is ours. The std handle above deliberately is not
    // closed — it belongs to whoever gave it to us.
    unsafe { CloseHandle(handle) };
}

fn write(handle: HANDLE, text: &str) {
    let bytes = text.as_bytes();
    let mut written = 0u32;
    // SAFETY: `bytes` outlives the call and the length is its own. The console is UTF-8 because
    // the manifest sets `activeCodePage`.
    unsafe {
        WriteFile(handle, bytes.as_ptr(), bytes.len() as u32, &mut written, ptr::null_mut())
    };
}

/// `print`, with a newline, for one-line results.
pub fn line(text: &str) {
    print(&format!("{text}\r\n"));
}
