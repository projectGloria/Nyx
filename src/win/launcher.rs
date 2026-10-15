//! Per-user sign-in launcher, independent of the terminal's lifetime.
use crate::config::schema::LauncherConfig;
use std::{
    os::windows::process::CommandExt,
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, WAIT_FAILED},
    System::{
        Registry::{RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_SET_VALUE},
        Threading::{
            CreateEventW, CreateMutexW, OpenEventW, SetEvent, INFINITE,
            EVENT_MODIFY_STATE,
        },
    },
    UI::{
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey},
        WindowsAndMessaging::{
            AllowSetForegroundWindow, PeekMessageW, MsgWaitForMultipleObjectsEx, ASFW_ANY, MSG, PM_REMOVE, WM_HOTKEY, QS_ALLINPUT, MWMO_INPUTAVAILABLE,
        },
    },
};
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
fn name(suffix: &str) -> Vec<u16> {
    super::wide(format!(
        r"Local\NyxLauncher-{}-{suffix}",
        std::env::var("USERNAME").unwrap_or_default()
    ))
}
fn status_path() -> std::path::PathBuf {
    crate::config::paths::config_file().with_file_name("launcher-status.txt")
}
fn report(message: &str) {
    let p = status_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = crate::util::write_atomic(&p, message.as_bytes());
}
pub fn status() -> Option<String> {
    std::fs::read_to_string(status_path()).ok()
}
pub fn ensure(config: &LauncherConfig) -> Result<(), String> {
    if !config.enabled {
        return Ok(());
    }
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name("stop").as_ptr()) };
    if !event.is_null() {
        unsafe {
            CloseHandle(event);
        }
        return Ok(());
    }
    configure(config)
}
pub fn stop() {
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name("stop").as_ptr()) };
    if !event.is_null() {
        unsafe {
            SetEvent(event);
            CloseHandle(event);
        }
    }
}
pub fn configure(config: &LauncherConfig) -> Result<(), String> {
    if config.enabled {
        let binding = crate::config::keys::parse(&config.hotkey)
            .and_then(super::hotkey::Chord::from_binding)
            .ok_or_else(|| "Launcher key needs a valid key and Ctrl, Shift, or Alt".to_string())?;
        let key = super::registry::create(RUN)?;
        let command = format!(
            "\"{}\" --launcher {} {}",
            super::registry::executable().display(),
            binding.modifiers,
            binding.vk
        );
        let result = super::registry::set_string(key, Some("NyxLauncher"), &command);
        super::registry::close(key);
        result?;
        stop();
        report("Starting Windows launcher...");
        std::process::Command::new(super::registry::executable())
            .args([
                "--launcher",
                &binding.modifiers.to_string(),
                &binding.vk.to_string(),
            ])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|e| format!("Could not start launcher: {e}"))?;
    } else {
        let mut key = ptr::null_mut();
        let result = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                super::wide(RUN).as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if result == 0 {
            unsafe {
                RegDeleteValueW(key, super::wide("NyxLauncher").as_ptr());
            }
            super::registry::close(key);
        }
        stop();
        report("Windows launcher disabled");
    }
    Ok(())
}
pub fn run(modifiers: u32, vk: u32) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let claim = loop {
        let claim = unsafe { CreateMutexW(ptr::null(), 0, name("instance").as_ptr()) };
        if claim.is_null() {
            return Err("Could not create launcher mutex".into());
        }
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            break claim;
        }
        unsafe {
            CloseHandle(claim);
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    let stop = unsafe { CreateEventW(ptr::null(), 1, 0, name("stop").as_ptr()) };
    if stop.is_null() {
        unsafe {
            CloseHandle(claim);
        }
        return Err("Could not create launcher stop event".into());
    }
    if unsafe { RegisterHotKey(ptr::null_mut(), 7, modifiers, vk) } == 0 {
        report("Launcher hotkey unavailable: another application may own it.");
        unsafe {
            CloseHandle(stop);
            CloseHandle(claim);
        }
        return Err("Launcher hotkey unavailable".into());
    }
    report("Windows launcher registered");
    let mut launching: Option<std::process::Child> = None;
    let mut msg = MSG::default();
    let mut wait_error = None;
    loop {
        // SAFETY: stop is our live event handle; the message queue belongs to this thread.
        let status = unsafe { MsgWaitForMultipleObjectsEx(1, &stop, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        if status == 0 { break; }
        if status == WAIT_FAILED {
            // SAFETY: reads this thread's error immediately after the failed wait.
            wait_error = Some(format!("Launcher wait failed (error {})", unsafe { GetLastError() }));
            break;
        }
        // SAFETY: msg is writable storage and only this thread's queue is drained.
        while unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_HOTKEY {
                // SAFETY: ASFW_ANY is the documented permission constant, not a process handle.
                unsafe {
                    AllowSetForegroundWindow(ASFW_ANY);
                }
                if super::ipc::send(&super::ipc::Request {
                    focus: true,
                    ..Default::default()
                }) {
                    launching = None;
                    continue;
                }
                if let Some(child) = &mut launching {
                    if matches!(child.try_wait(), Ok(None)) {
                        continue;
                    }
                }
                match std::process::Command::new(super::registry::executable())
                    .arg("--focus")
                    .creation_flags(0x08000000)
                    .spawn()
                {
                    Ok(child) => launching = Some(child),
                    Err(err) => report(&format!("Could not launch Nyx: {err}")),
                }
            }
        }
    }
    if let Some(err) = &wait_error { report(err); }
    // SAFETY: registration and both handles belong to this thread and are released once.
    unsafe {
        UnregisterHotKey(ptr::null_mut(), 7);
        CloseHandle(stop);
        CloseHandle(claim);
    }
    wait_error.map_or(Ok(()), Err)
}
