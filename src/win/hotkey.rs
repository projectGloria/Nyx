//! The system-wide hotkey that shows and hides Nyx.
//!
//! `RegisterHotKey` with a null window posts `WM_HOTKEY` to the **calling thread's** message
//! queue, so the plan's "message-only window" turned out to be one window class, one window
//! procedure and one `DestroyWindow` we do not need: a thread with a message loop is the whole
//! mechanism. The registration is bound to the thread that made it, which is why rebinding after
//! a config change is a message posted *to* this thread rather than a call into it.
//!
//! Nothing here touches the terminal. The thread raises the window — see the note on latency
//! below — and then reports the press through the ordinary `AppEvent` channel, so the decision of
//! what a press means stays in one place with the rest of the app's state.

use std::ptr;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY, WM_QUIT,
};

use crate::config::keys::Binding;
use crate::event::{AppEvent, RepaintCoalescer};
use crate::win::quake;

/// Our one hotkey id. Ids are per-thread, and there is only ever one.
const ID: i32 = 1;

/// "Re-register with these modifiers and this virtual key." `lParam == 0` means unregister.
const MSG_REBIND: u32 = WM_APP + 1;

/// What `RegisterHotKey` actually wants: a modifier mask and a virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: u32,
    pub vk: u32,
}

impl Chord {
    /// Translate a config binding, or say it cannot be one.
    ///
    /// `MOD_NOREPEAT` is not optional. Without it, holding the chord down fires the hotkey at the
    /// keyboard's repeat rate, which for a show/hide toggle is a window strobing thirty times a
    /// second.
    pub fn from_binding(binding: Binding) -> Option<Self> {
        let mut modifiers = MOD_NOREPEAT;
        if binding.ctrl {
            modifiers |= MOD_CONTROL;
        }
        if binding.shift {
            modifiers |= MOD_SHIFT;
        }
        if binding.alt {
            modifiers |= MOD_ALT;
        }
        // A bare key would claim it from every program on the machine, which is not a thing a
        // terminal gets to do — `F12` with no modifier is still the debugger's key everywhere.
        if !(binding.ctrl || binding.shift || binding.alt) {
            return None;
        }
        Some(Chord { modifiers, vk: vk_for(binding.key)? })
    }
}

/// A live registration. Dropping it unregisters the chord and stops the thread.
pub struct Hotkey {
    thread: u32,
    join: Option<JoinHandle<()>>,
}

impl Hotkey {
    /// Start the hotkey thread. `chord` may be `None`, which starts the thread with nothing
    /// registered so that a later `rebind` has somewhere to land.
    pub fn spawn(
        chord: Option<Chord>,
        quake: Arc<quake::Shared>,
        tx: Sender<AppEvent>,
        repaint: Arc<RepaintCoalescer>,
    ) -> std::io::Result<Self> {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let join = std::thread::Builder::new()
            .name("nyx-hotkey".into())
            .spawn(move || run(chord, quake, tx, repaint, ready_tx))?;

        // Block until the thread has a message queue. `PostThreadMessageW` to a thread that has
        // not created one yet silently drops the message, so a `rebind` racing startup would be
        // lost and the new chord would work only after the next restart.
        let thread = ready_rx.recv().map_err(|_| {
            std::io::Error::other("the hotkey thread stopped before it was ready")
        })?;

        Ok(Hotkey { thread, join: Some(join) })
    }

    /// Swap the registered chord. `None` unregisters without stopping the thread.
    pub fn rebind(&self, chord: Option<Chord>) {
        let (modifiers, vk) = match chord {
            Some(c) => (c.modifiers as usize, c.vk as isize),
            None => (0, 0),
        };
        // SAFETY: a thread id that has gone away makes this return 0, which is the case `drop`
        // already tolerates.
        unsafe { PostThreadMessageW(self.thread, MSG_REBIND, modifiers, vk) };
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        // SAFETY: posting to a dead thread fails harmlessly.
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(
    initial: Option<Chord>,
    quake: Arc<quake::Shared>,
    tx: Sender<AppEvent>,
    repaint: Arc<RepaintCoalescer>,
    ready: Sender<u32>,
) {
    // Force the queue into existence before anyone is told this thread exists.
    let mut msg = MSG::default();
    // SAFETY: `PM_NOREMOVE` inspects the queue without consuming anything; creating it is the
    // documented side effect we are here for.
    unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: no arguments, no failure mode.
    let id = unsafe { GetCurrentThreadId() };
    if ready.send(id).is_err() {
        return; // the app gave up on us before we started
    }

    let mut current: Option<Chord> = None;
    register(&mut current, initial, &tx, &repaint);

    loop {
        let mut msg = MSG::default();
        // SAFETY: a null window filter retrieves both window and thread messages, which is what
        // a null-`hwnd` hotkey registration and our own rebind message both produce.
        let got = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
        if got <= 0 {
            break; // 0 is WM_QUIT; -1 is an error we cannot do anything useful about
        }

        match msg.message {
            WM_HOTKEY => {
                // Done here rather than in the app, and this is the whole reason the thread
                // touches the window at all: eframe throttles repaints of an *invisible* window
                // to one every 100 ms, so a hidden Nyx would take up to a tenth of a second to
                // notice the press. Showing the window first means the very next repaint is an
                // ordinary one. The app still decides what the press means — this only undoes
                // the throttle.
                quake.raise_if_hidden();
                if tx.send(AppEvent::HotkeyPressed).is_err() {
                    break;
                }
                repaint.poke();
            },
            MSG_REBIND => {
                let chord = (msg.lParam != 0).then_some(Chord {
                    modifiers: msg.wParam as u32,
                    vk: msg.lParam as u32,
                });
                register(&mut current, chord, &tx, &repaint);
            },
            _ => {},
        }
    }

    if current.is_some() {
        // SAFETY: unregistering a chord this thread registered.
        unsafe { UnregisterHotKey(ptr::null_mut(), ID) };
    }
}

/// Move from whatever is registered to `wanted`, reporting a refusal.
fn register(
    current: &mut Option<Chord>,
    wanted: Option<Chord>,
    tx: &Sender<AppEvent>,
    repaint: &RepaintCoalescer,
) {
    if *current == wanted {
        return;
    }
    if current.take().is_some() {
        // SAFETY: the id was registered by this thread.
        unsafe { UnregisterHotKey(ptr::null_mut(), ID) };
    }
    let Some(chord) = wanted else { return };

    // SAFETY: a null window binds the hotkey to this thread's message queue.
    let ok = unsafe { RegisterHotKey(ptr::null_mut(), ID, chord.modifiers, chord.vk) };
    if ok != 0 {
        *current = Some(chord);
        return;
    }

    // The usual cause is another program already owning the chord, and the only way anyone finds
    // out is if we say so — a hotkey that silently does nothing reads as Nyx being broken.
    let _ = tx.send(AppEvent::HotkeyRefused);
    repaint.poke();
}

/// The virtual-key code for an egui key.
///
/// `None` for anything `RegisterHotKey` has no code for, so the caller reports the binding by name
/// instead of registering something else. Deliberately not derived from the key's character: a
/// hotkey is a scancode-level claim, and on a non-US layout the character under a key is not what
/// `VK_OEM_3` means.
pub fn vk_for(key: egui::Key) -> Option<u32> {
    use egui::Key;
    Some(match key {
        Key::A => 0x41, Key::B => 0x42, Key::C => 0x43, Key::D => 0x44, Key::E => 0x45,
        Key::F => 0x46, Key::G => 0x47, Key::H => 0x48, Key::I => 0x49, Key::J => 0x4A,
        Key::K => 0x4B, Key::L => 0x4C, Key::M => 0x4D, Key::N => 0x4E, Key::O => 0x4F,
        Key::P => 0x50, Key::Q => 0x51, Key::R => 0x52, Key::S => 0x53, Key::T => 0x54,
        Key::U => 0x55, Key::V => 0x56, Key::W => 0x57, Key::X => 0x58, Key::Y => 0x59,
        Key::Z => 0x5A,

        Key::Num0 => 0x30, Key::Num1 => 0x31, Key::Num2 => 0x32, Key::Num3 => 0x33,
        Key::Num4 => 0x34, Key::Num5 => 0x35, Key::Num6 => 0x36, Key::Num7 => 0x37,
        Key::Num8 => 0x38, Key::Num9 => 0x39,

        Key::F1 => 0x70, Key::F2 => 0x71, Key::F3 => 0x72, Key::F4 => 0x73,
        Key::F5 => 0x74, Key::F6 => 0x75, Key::F7 => 0x76, Key::F8 => 0x77,
        Key::F9 => 0x78, Key::F10 => 0x79, Key::F11 => 0x7A, Key::F12 => 0x7B,

        Key::Space => 0x20,
        Key::Tab => 0x09,
        Key::Enter => 0x0D,
        Key::Escape => 0x1B,
        Key::Backspace => 0x08,
        Key::Insert => 0x2D,
        Key::Delete => 0x2E,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::ArrowLeft => 0x25,
        Key::ArrowUp => 0x26,
        Key::ArrowRight => 0x27,
        Key::ArrowDown => 0x28,

        // The OEM keys, named for their position on a US layout because that is what the code
        // means — `VK_OEM_3` is "the key left of 1", whatever it prints on this keyboard.
        Key::Backtick => 0xC0,      // VK_OEM_3
        Key::Minus => 0xBD,         // VK_OEM_MINUS
        Key::Plus | Key::Equals => 0xBB, // VK_OEM_PLUS
        Key::OpenBracket => 0xDB,   // VK_OEM_4
        Key::Backslash => 0xDC,     // VK_OEM_5
        Key::CloseBracket => 0xDD,  // VK_OEM_6
        Key::Quote => 0xDE,         // VK_OEM_7
        Key::Semicolon | Key::Colon => 0xBA, // VK_OEM_1
        Key::Slash | Key::Questionmark => 0xBF, // VK_OEM_2
        Key::Comma => 0xBC,         // VK_OEM_COMMA
        Key::Period => 0xBE,        // VK_OEM_PERIOD

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_needs_a_modifier() {
        // Claiming a bare key system-wide is not something a terminal gets to do: F12 belongs to
        // whatever debugger the user is in front of.
        let bare = crate::config::keys::parse("F12").unwrap();
        assert!(Chord::from_binding(bare).is_none());

        let real = crate::config::keys::parse("Ctrl+Backtick").unwrap();
        let chord = Chord::from_binding(real).unwrap();
        assert_eq!(chord.vk, 0xC0);
        assert_eq!(chord.modifiers & MOD_CONTROL, MOD_CONTROL);
    }

    #[test]
    fn no_repeat_is_always_set() {
        // Without it, holding the chord toggles the window at the keyboard repeat rate.
        for spec in ["Ctrl+Backtick", "Alt+Space", "Ctrl+Shift+Alt+T"] {
            let chord = Chord::from_binding(crate::config::keys::parse(spec).unwrap()).unwrap();
            assert_eq!(chord.modifiers & MOD_NOREPEAT, MOD_NOREPEAT, "{spec}");
        }
    }

    #[test]
    fn a_key_with_no_virtual_key_code_is_refused_rather_than_mapped_to_something_else() {
        // `Copy` and `Cut` are egui keys with no `VK_`; binding one of them to "roughly C" would
        // claim Ctrl+C from every program on the machine.
        assert!(vk_for(egui::Key::Copy).is_none());
        assert!(vk_for(egui::Key::A).is_some());
    }

    #[test]
    fn every_key_the_config_grammar_spells_is_registrable_or_named() {
        // The settings window offers the same recorder the keybinding tab uses, so a chord it can
        // record has to either register or be reportable — never silently do nothing.
        use egui::Key;
        let spellable = [
            Key::A, Key::Z, Key::Num0, Key::F1, Key::F12, Key::Space, Key::Backtick,
            Key::ArrowUp, Key::Home, Key::Slash, Key::Semicolon, Key::Comma, Key::Period,
        ];
        for key in spellable {
            assert!(vk_for(key).is_some(), "{} has no VK code", key.name());
        }
    }
}
