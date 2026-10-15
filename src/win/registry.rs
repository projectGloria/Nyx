//! "Open Nyx here" in Explorer's context menu.
//!
//! **HKCU only.** Every key written here is under `HKEY_CURRENT_USER\Software\Classes`, which
//! needs no elevation, affects nobody else on the machine, and can be removed by the same user
//! that added it. The machine-wide equivalent under HKLM would mean a UAC prompt on install and
//! an orphaned menu entry for every other account if the uninstall ever missed.
//!
//! Three places, because Explorer treats them as three different objects: a folder you
//! right-click, the empty space *inside* a folder, and a drive in "This PC". All three use `%V`,
//! which is the verb's target — for `Background` it is the folder being looked at, which `%1`
//! would give as the desktop instead.
//!
//! On Windows 11 this lands under "Show more options" rather than in the top-level sheet. The
//! short menu only accepts an `IExplorerCommand` from a registered package, which means an MSIX
//! identity Nyx does not have and should not need to launch a terminal.

use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::win::wide;

/// The verb's key name. Also what the uninstall deletes, so it must never change.
const VERB: &str = "NyxHere";

/// Every object type the entry is registered under, as a subkey of `Software\Classes`.
const ROOTS: [&str; 3] = ["Directory", "Directory\\Background", "Drive"];

fn key_paths() -> Vec<String> {
    ROOTS
        .iter()
        .map(|root| format!("Software\\Classes\\{root}\\shell\\{VERB}"))
        .collect()
}

/// The `.exe` to launch. Falls back to the plain name, so an installed copy on PATH still works
/// if `current_exe` ever fails.
pub fn executable() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("nyx.exe"))
}

/// The command line Explorer runs.
///
/// `--attach` is the whole point of the entry: without it every right-click opens another window
/// rather than a tab in the one already on screen.
fn command_for(exe: &Path) -> String {
    format!("\"{}\" --attach --cwd \"%V\"", exe.display())
}

/// Add the menu entry. Returns what went wrong, in a sentence that can go on screen.
pub fn install(label: &str, exe: &Path) -> Result<(), String> {
    install_at(label, exe, &key_paths())
}

fn install_at(label: &str, exe: &Path, paths: &[String]) -> Result<(), String> {
    let label = if label.trim().is_empty() { "Open Nyx here" } else { label.trim() };
    let command = command_for(exe);
    // `,0` is the first icon resource in the executable. Nyx has no icon resource yet, so Explorer
    // falls back to the generic application icon rather than showing nothing.
    let icon = format!("{},0", exe.display());

    for path in paths {
        let key = create(path)?;
        let result = (|| {
            set_string(key, None, label)?;
            set_string(key, Some("Icon"), &icon)?;
            let command_key = create(&format!("{path}\\command"))?;
            let result = set_string(command_key, None, &command);
            close(command_key);
            result
        })();
        close(key);
        result?;
    }
    Ok(())
}

/// Take it out again. Succeeds when the entry was not there to begin with — an uninstall that
/// fails because something is already gone is an uninstall people stop trusting.
pub fn remove() -> Result<(), String> {
    remove_at(&key_paths())
}

fn remove_at(paths: &[String]) -> Result<(), String> {
    for path in paths {
        let wide_path = wide(path);
        // SAFETY: deletes a subtree under HKCU. A missing key reports ERROR_FILE_NOT_FOUND.
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide_path.as_ptr()) };
        // 2 is ERROR_FILE_NOT_FOUND: nothing to remove.
        if status != ERROR_SUCCESS && status != 2 {
            return Err(format!("could not remove {path} (error {status})"));
        }
    }
    Ok(())
}

/// Whether the entry is currently registered, and pointing at *this* executable.
///
/// The second half matters: after moving or reinstalling Nyx the menu still exists but launches
/// the old path, and a settings window that said "installed" would be lying about what happens
/// when you click it.
pub fn installed_for(exe: &Path) -> bool {
    installed_at(exe, &key_paths())
}

fn installed_at(exe: &Path, paths: &[String]) -> bool {
    let Some(path) = paths.first() else { return false };
    read_string(&format!("{path}\\command")).is_some_and(|current| current == command_for(exe))
}

pub(super) fn create(path: &str) -> Result<HKEY, String> {
    let wide_path = wide(path);
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: creates or opens the key and writes the handle through `key`. Default security,
    // non-volatile, write access only.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide_path.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("could not write {path} (error {status})"));
    }
    Ok(key)
}

pub(super) fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<(), String> {
    let wide_name = name.map(wide);
    let wide_value = wide(value);
    // The byte count includes the terminating NUL; a REG_SZ written without it is the classic
    // "the value has garbage after it" bug.
    let bytes = (wide_value.len() * 2) as u32;
    // SAFETY: `wide_value` outlives the call and `bytes` is its true length in bytes.
    let status = unsafe {
        RegSetValueExW(
            key,
            wide_name.as_ref().map_or(ptr::null(), |n| n.as_ptr()),
            0,
            REG_SZ,
            wide_value.as_ptr().cast(),
            bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("could not set {} (error {status})", name.unwrap_or("(default)")));
    }
    Ok(())
}

pub(super) fn close(key: HKEY) {
    // SAFETY: a handle from `create` that has not been closed.
    unsafe { RegCloseKey(key) };
}

fn read_string(path: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    let wide_path = wide(path);
    let mut size: u32 = 0;
    // SAFETY: a null buffer with a null size is the documented way to ask for the length.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide_path.as_ptr(),
            ptr::null(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }

    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` is `size` bytes long.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide_path.as_ptr(),
            ptr::null(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_attaches_and_passes_the_folder() {
        let command = command_for(Path::new(r"C:\Program Files\Nyx\nyx.exe"));
        // Quoted, because the path has a space in it and Explorer does not add them.
        assert!(command.starts_with("\"C:\\Program Files\\Nyx\\nyx.exe\""), "{command}");
        // `%V` and not `%1`: for the Background verb, `%1` is the desktop rather than the folder
        // the user is standing in.
        assert!(command.contains("\"%V\""), "{command}");
        // Without --attach every right-click opens a window instead of a tab.
        assert!(command.contains("--attach"), "{command}");
    }

    #[test]
    fn all_three_object_types_are_covered() {
        let paths = key_paths();
        assert_eq!(paths.len(), 3);
        // Right-clicking a folder, right-clicking inside one, and right-clicking a drive are
        // three different objects to Explorer and need three keys.
        assert!(paths.iter().any(|p| p.contains("Directory\\shell")));
        assert!(paths.iter().any(|p| p.contains("Directory\\Background\\shell")));
        assert!(paths.iter().any(|p| p.contains("Drive\\shell")));
        // Never anywhere but the current user.
        assert!(paths.iter().all(|p| p.starts_with("Software\\Classes\\")));
    }

    #[test]
    fn install_and_remove_round_trip() {
        // A disposable namespace must never replace an existing Explorer registration.
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = format!(r"Software\NyxTests\Registry-{}-{unique}", std::process::id());
        let paths = vec![format!(r"{root}\Directory"), format!(r"{root}\Background"), format!(r"{root}\Drive")];
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = remove_at(&[self.0.clone()]); }
        }
        let _cleanup = Cleanup(root);
        let exe = PathBuf::from(r"C:\nyx-test-does-not-exist\nyx.exe");
        assert!(!installed_at(&exe, &paths));
        install_at("Open Nyx here (test)", &exe, &paths).expect("HKCU is writable without elevation");
        assert!(installed_at(&exe, &paths));
        assert!(!installed_at(Path::new(r"C:\elsewhere\nyx.exe"), &paths));
        remove_at(&paths).expect("removing what we just wrote");
        assert!(!installed_at(&exe, &paths));
        remove_at(&paths).expect("removing nothing is not an error");
    }
}
