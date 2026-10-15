//! egui input events to PTY bytes, plus the app-level actions that are not bytes at all.
//!
//! The encoding follows xterm: a key with modifiers carries a parameter of
//! `1 + shift + 2*alt + 4*ctrl`, and the unmodified forms collapse to the short sequences that
//! every shell and full-screen program expects.
//!
//! Known gap: egui does not distinguish the numeric keypad from the number row, so application
//! keypad mode (DECKPAM) cannot be honoured. Nothing in a normal shell session depends on it.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;

/// Everything one frame of input asked for.
#[derive(Default)]
pub struct Translated {
    pub bytes: Vec<u8>,
    pub copy: bool,
    pub paste: bool,
    pub scroll: Option<Scroll>,
    /// Typing jumps back to the prompt, the way every terminal behaves.
    pub scroll_to_bottom: bool,
    /// The `Text` egui delivers beside this key has already been spoken for — by the kitty
    /// encoding, which puts the character in the sequence itself.
    ///
    /// Reported rather than handled here because the caller walks the events and this function
    /// is handed one at a time, so the `Text` to drop arrives on the *next* call. The caller
    /// already keeps a `swallow_text` flag for its own bindings; this joins it.
    pub swallow_text: bool,
}

/// What a frame's modifiers make an `Event::Text` mean.
///
/// **The only place that decides**, because `app::translate_with_expansion` walks the same events
/// one at a time — expansion has to be emitted before the space that triggered it — and so has its
/// own `Text` arm. A second copy of this rule is how the two paths end up disagreeing: they did,
/// and the live one was the one without it, so Alt+B typed a `b` instead of moving back a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRule {
    /// Drop the text entirely: Ctrl already spoke through `encode_key`'s C0 branch.
    pub suppress: bool,
    /// Prefix one ESC — "meta sends escape", what every readline word motion expects.
    pub meta: bool,
}

/// egui emits `Text` alongside `Key` for printable input. Taking both would double every
/// character, so `Text` is the source for ordinary typing and `Key` handles the rest.
///
/// The exception is AltGr, which Windows reports as Ctrl+Alt: on a UK or German layout that is how
/// you type `@`, `\` or `€`, so a plain "skip Text when Ctrl is down" rule makes those keys
/// silently dead.
///
/// Alt without Ctrl is meta. The character has to come from `Text` rather than from `Key`, because
/// `Text` is the only one that knows what the layout produced — `Key` is a logical key and would
/// spell `@` as `Num2` on a UK keyboard. egui-winit emits it for an Alt chord even where Windows
/// delivers no character event of its own, because it falls back to the logical key's own text,
/// and it suppresses `Text` only for Ctrl. Suppressing it here as well left Alt+B and Alt+F — the
/// most used Alt chords in any shell — emitting nothing at all.
pub fn text_rule(modifiers: egui::Modifiers) -> TextRule {
    let altgr = modifiers.ctrl && modifiers.alt;
    TextRule {
        suppress: modifiers.ctrl && !altgr,
        meta: modifiers.alt && !modifiers.ctrl,
    }
}

pub fn translate(
    events: &[egui::Event],
    modifiers: egui::Modifiers,
    mode: TermMode,
) -> Translated {
    let mut out = Translated::default();
    let rule = text_rule(modifiers);

    for event in events {
        match event {
            egui::Event::Text(text) if !rule.suppress => {
                if rule.meta {
                    out.bytes.push(0x1b);
                }
                out.bytes.extend_from_slice(text.as_bytes());
                out.scroll_to_bottom = true;
            },

            egui::Event::Paste(text) => {
                push_paste(&mut out.bytes, text, mode);
                out.scroll_to_bottom = true;
            },

            // The finished text from an input method, delivered instead of `Text`. Not handling it
            // at all is why CJK and Korean input produced nothing: the candidate window worked,
            // the characters were chosen, and then they went nowhere.
            //
            // `Preedit` is deliberately ignored. Showing a composition inline would mean writing
            // it into the grid and taking it out again on every keystroke of it, and the IME draws
            // its own candidate window — which `ViewportCommand::IMERect` parks over the cursor.
            // `DeleteSurrounding` is ignored for the same reason: the shell owns the line, and we
            // have no way to delete from it that is not a guess about the line editor.
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                out.bytes.extend_from_slice(text.as_bytes());
                out.scroll_to_bottom = true;
            },

            egui::Event::Copy => out.copy = true,

            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                // App-level bindings are claimed before the shell ever sees them.
                if m.ctrl && m.shift {
                    match key {
                        egui::Key::C => {
                            out.copy = true;
                            continue;
                        },
                        egui::Key::V => {
                            out.paste = true;
                            continue;
                        },
                        _ => {},
                    }
                }

                // The other paste chord everyone has in their fingers, from X11 by way of every
                // Windows console. `ESC [ 2 ; 2 ~` is what it would otherwise send, and nothing
                // reads that.
                //
                // Only the paste half. Ctrl+Insert is the CUA partner for copy, but copy already
                // has Ctrl+Shift+C and Ctrl+C-with-a-selection, and taking a second chord away
                // from the program to duplicate a binding that exists is the thing this
                // terminal's defaults are trying not to do.
                if *key == egui::Key::Insert && m.shift && !m.ctrl && !m.alt {
                    out.paste = true;
                    continue;
                }

                // Scrollback navigation. Only meaningful on the primary screen: the alternate
                // screen has no history, and these keys belong to the application there.
                if m.shift && !mode.contains(TermMode::ALT_SCREEN) {
                    match key {
                        egui::Key::PageUp => {
                            out.scroll = Some(Scroll::PageUp);
                            continue;
                        },
                        egui::Key::PageDown => {
                            out.scroll = Some(Scroll::PageDown);
                            continue;
                        },
                        egui::Key::Home => {
                            out.scroll = Some(Scroll::Top);
                            continue;
                        },
                        egui::Key::End => {
                            out.scroll = Some(Scroll::Bottom);
                            continue;
                        },
                        _ => {},
                    }
                }

                let before = out.bytes.len();
                encode_key(*key, *m, mode, &mut out.bytes);
                if out.bytes.len() > before {
                    out.scroll_to_bottom = true;
                }
            },

            _ => {},
        }
    }

    out
}

/// Wrap pasted text so the shell can tell it apart from typing.
///
/// Without bracketed paste a shell runs every newline in the pasted text immediately, which turns
/// a mis-paste into a sequence of executed commands. Programs that support it opt in, so the
/// wrapper is only added when the mode is actually set.
pub fn push_paste(out: &mut Vec<u8>, text: &str, mode: TermMode) {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out.extend_from_slice(b"\x1b[200~");
        // The terminator must not appear inside the payload, or the paste ends early and the
        // remainder is executed as keystrokes.
        out.extend_from_slice(strip_paste_terminator(text).as_bytes());
        out.extend_from_slice(b"\x1b[201~");
    } else {
        // Normalise newlines: a bare \n leaves most line editors waiting for more input.
        out.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
    }
}

/// Remove every `ESC [ 2 0 1 ~` from pasted text, *including* the ones removing another one
/// creates.
///
/// A single `str::replace` is not enough. Deleting the inner occurrence from
/// `ESC[20` + `ESC[201~` + `1~` leaves the two halves around it adjacent, and they spell a fresh
/// terminator — so the sanitiser hands the shell the very sequence it was meant to remove, the
/// paste ends there, and everything after it is executed as typing with any Enter in it honoured.
///
/// Checking the tail after each character is what closes that: a splice can only be completed by
/// the character that completes it, so the check runs again on whatever the removal brought
/// together. Nesting is handled to any depth for the same reason.
fn strip_paste_terminator(text: &str) -> String {
    const END: &str = "\x1b[201~";

    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.push(ch);
        if out.ends_with(END) {
            // END is ASCII and `out` ends with it, so this lands on a char boundary.
            out.truncate(out.len() - END.len());
        }
    }
    out
}

/// What a program is told when the window gains or loses focus, if it asked to be told.
///
/// `CSI ? 1004 h` is how it asks, and `alacritty_terminal` has tracked the mode all along —
/// nothing ever read it, so the answer was never sent. Programs use it to re-read a file that
/// changed under them (vim's `FocusGained`), to stop animating while nobody is looking, and to
/// redraw a prompt that another window may have scribbled over. A program that asked and is
/// never answered waits for an event that cannot arrive.
///
/// `None` for a program that did not ask. Sending it unasked puts a literal `[I` on the line of
/// anything that is not expecting an escape sequence there.
pub fn focus_report(mode: TermMode, focused: bool) -> Option<&'static [u8]> {
    mode.contains(TermMode::FOCUS_IN_OUT)
        .then(|| if focused { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] })
}

/// Whether this chord would mean anything to the program on the other end of the PTY.
///
/// The question a keybinding default has to answer before it claims a chord. `Ctrl+Comma` writes
/// no bytes at all — there is no C0 control for it and no escape sequence — so a terminal keeping
/// it for itself costs the program nothing, while `Ctrl+P` is `previous-history` and keeping that
/// one is taking something. Exposed rather than inlined into the test so the rule is asked of the
/// encoder that actually runs, instead of a second list that can drift out from under it.
///
/// Printable keys answer `false` here because they arrive as `Event::Text` rather than through
/// this path, so the caller must also require at least one modifier.
///
/// `cfg(test)`: nothing at runtime asks this — the encoder simply runs. It exists so the rule
/// about default bindings in `config::load` is checked against the encoder itself rather than
/// against a hand-kept list of which chords matter, which is the thing that would drift.
#[cfg(test)]
pub fn means_something_to_the_program(key: egui::Key, m: egui::Modifiers) -> bool {
    let mut out = Vec::new();
    encode_key(key, m, TermMode::empty(), &mut out);
    !out.is_empty()
}

/// `1 + shift + 2*alt + 4*ctrl`, the xterm modifier parameter. 1 means "no modifiers".
fn modifier_param(m: egui::Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl)
}

/// Cursor and Home/End keys: `ESC [ A`, `ESC O A` in application cursor mode, or
/// `ESC [ 1 ; <mod> A` when modified.
fn csi_letter(letter: u8, m: egui::Modifiers, app_cursor: bool, out: &mut Vec<u8>) {
    let p = modifier_param(m);
    if p == 1 {
        out.extend_from_slice(if app_cursor { b"\x1bO" } else { b"\x1b[" });
        out.push(letter);
    } else {
        // A modified cursor key is always CSI, never SS3, even in application cursor mode.
        out.extend_from_slice(format!("\x1b[1;{p}").as_bytes());
        out.push(letter);
    }
}

/// Navigation and F5+ keys: `ESC [ <n> ~`, or `ESC [ <n> ; <mod> ~`.
fn csi_tilde(n: u8, m: egui::Modifiers, out: &mut Vec<u8>) {
    let p = modifier_param(m);
    if p == 1 {
        out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[{n};{p}~").as_bytes());
    }
}

/// F1-F4: `ESC O P`, or `ESC [ 1 ; <mod> P`.
fn ss3_fn(letter: u8, m: egui::Modifiers, out: &mut Vec<u8>) {
    let p = modifier_param(m);
    if p == 1 {
        out.extend_from_slice(b"\x1bO");
        out.push(letter);
    } else {
        out.extend_from_slice(format!("\x1b[1;{p}").as_bytes());
        out.push(letter);
    }
}

fn ctrl_byte(key: egui::Key) -> Option<u8> {
    use egui::Key;
    Some(match key {
        Key::A => 0x01,
        Key::B => 0x02,
        Key::C => 0x03,
        Key::D => 0x04,
        Key::E => 0x05,
        Key::F => 0x06,
        Key::G => 0x07,
        Key::H => 0x08,
        Key::I => 0x09,
        Key::J => 0x0a,
        Key::K => 0x0b,
        Key::L => 0x0c,
        Key::M => 0x0d,
        Key::N => 0x0e,
        Key::O => 0x0f,
        Key::P => 0x10,
        Key::Q => 0x11,
        Key::R => 0x12,
        Key::S => 0x13,
        Key::T => 0x14,
        Key::U => 0x15,
        Key::V => 0x16,
        Key::W => 0x17,
        Key::X => 0x18,
        Key::Y => 0x19,
        Key::Z => 0x1a,
        // The classic C0 aliases. Ctrl+Space and Ctrl+2 both send NUL, Ctrl+/ sends US.
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Space | Key::Num2 => 0x00,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Slash => 0x1f,
        _ => return None,
    })
}

fn encode_key(key: egui::Key, m: egui::Modifiers, mode: TermMode, out: &mut Vec<u8>) {
    use egui::Key;

    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let start = out.len();

    // Ctrl+<key> maps to the C0 controls. Nothing else produces a SIGINT, so this branch is what
    // makes the terminal interruptible. AltGr (Ctrl+Alt) is excluded: it is a text-entry
    // modifier, not a control one.
    if m.ctrl && !m.alt {
        if let Some(b) = ctrl_byte(key) {
            out.push(b);
            return;
        }
    }

    match key {
        Key::Enter => out.push(0x0d),
        Key::Escape => out.push(0x1b),
        // DEL, not BS. Readline and every Unix line editor expect 0x7f here; sending 0x08 makes
        // backspace behave like Ctrl+H, which in some configurations is "help".
        Key::Backspace => out.push(if m.ctrl { 0x08 } else { 0x7f }),
        Key::Tab => {
            if m.shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                out.push(0x09);
            }
        },

        Key::ArrowUp => csi_letter(b'A', m, app_cursor, out),
        Key::ArrowDown => csi_letter(b'B', m, app_cursor, out),
        Key::ArrowRight => csi_letter(b'C', m, app_cursor, out),
        Key::ArrowLeft => csi_letter(b'D', m, app_cursor, out),
        Key::Home => csi_letter(b'H', m, app_cursor, out),
        Key::End => csi_letter(b'F', m, app_cursor, out),

        Key::Insert => csi_tilde(2, m, out),
        Key::Delete => csi_tilde(3, m, out),
        Key::PageUp => csi_tilde(5, m, out),
        Key::PageDown => csi_tilde(6, m, out),

        Key::F1 => ss3_fn(b'P', m, out),
        Key::F2 => ss3_fn(b'Q', m, out),
        Key::F3 => ss3_fn(b'R', m, out),
        Key::F4 => ss3_fn(b'S', m, out),
        Key::F5 => csi_tilde(15, m, out),
        Key::F6 => csi_tilde(17, m, out),
        Key::F7 => csi_tilde(18, m, out),
        Key::F8 => csi_tilde(19, m, out),
        Key::F9 => csi_tilde(20, m, out),
        Key::F10 => csi_tilde(21, m, out),
        Key::F11 => csi_tilde(23, m, out),
        Key::F12 => csi_tilde(24, m, out),

        // Everything printable arrives as Event::Text; handling it here too would double it.
        _ => {},
    }

    // Alt means "meta sends escape", which is what every readline word-motion binding expects.
    // Only applied when the key actually produced something, or a bare Alt+<unhandled key>
    // would leave a stray ESC for the shell to read as the start of a sequence never finished.
    if m.alt && !m.ctrl && out.len() > start {
        // A key that already emitted its own CSI/SS3 introducer must not be double-escaped: the
        // modifier is already encoded in its parameter.
        if out[start] != 0x1b {
            out.insert(start, 0x1b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: egui::Modifiers = egui::Modifiers::NONE;

    fn enc(key: egui::Key, m: egui::Modifiers, mode: TermMode) -> Vec<u8> {
        let mut out = Vec::new();
        encode_key(key, m, mode, &mut out);
        out
    }

    #[test]
    fn focus_is_reported_only_to_a_program_that_asked() {
        assert_eq!(focus_report(TermMode::FOCUS_IN_OUT, true), Some(&b"\x1b[I"[..]));
        assert_eq!(focus_report(TermMode::FOCUS_IN_OUT, false), Some(&b"\x1b[O"[..]));
        // Unasked, the bytes would land on the input line of whatever is running.
        assert_eq!(focus_report(TermMode::empty(), true), None);
        assert_eq!(focus_report(TermMode::ALT_SCREEN, false), None);
    }

    #[test]
    fn shift_insert_pastes_and_plain_insert_still_reaches_the_program() {
        let key = |m: egui::Modifiers| egui::Event::Key {
            key: egui::Key::Insert,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        };

        let pasted = translate(&[key(egui::Modifiers::SHIFT)], egui::Modifiers::SHIFT, TermMode::empty());
        assert!(pasted.paste);
        assert!(pasted.bytes.is_empty(), "the sequence must not go out as well as pasting");

        // Unmodified Insert is the program's — overwrite mode in a line editor, and a key vim
        // binds. Taking it would be the defect this change exists to stop repeating.
        let plain = translate(&[key(NONE)], NONE, TermMode::empty());
        assert!(!plain.paste);
        assert_eq!(plain.bytes, b"\x1b[2~");
    }

    #[test]
    fn ctrl_c_is_a_single_interrupt_byte() {
        assert_eq!(enc(egui::Key::C, egui::Modifiers::CTRL, TermMode::empty()), vec![0x03]);
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(enc(egui::Key::ArrowUp, NONE, TermMode::empty()), b"\x1b[A");
        assert_eq!(enc(egui::Key::ArrowUp, NONE, TermMode::APP_CURSOR), b"\x1bOA");
    }

    #[test]
    fn a_modified_cursor_key_is_csi_even_in_application_mode() {
        // ESC O A with a modifier parameter is not a valid sequence; xterm switches to CSI.
        let ctrl_up = enc(egui::Key::ArrowUp, egui::Modifiers::CTRL, TermMode::APP_CURSOR);
        assert_eq!(ctrl_up, b"\x1b[1;5A");
    }

    #[test]
    fn modifier_parameters_match_xterm() {
        assert_eq!(enc(egui::Key::ArrowUp, egui::Modifiers::SHIFT, TermMode::empty()), b"\x1b[1;2A");
        assert_eq!(enc(egui::Key::ArrowUp, egui::Modifiers::ALT, TermMode::empty()), b"\x1b[1;3A");
        assert_eq!(enc(egui::Key::ArrowUp, egui::Modifiers::CTRL, TermMode::empty()), b"\x1b[1;5A");
        assert_eq!(enc(egui::Key::Delete, egui::Modifiers::CTRL, TermMode::empty()), b"\x1b[3;5~");
    }

    #[test]
    fn alt_escapes_a_bare_key_but_never_doubles_an_escape() {
        // Alt+Left already carries its modifier in the CSI parameter.
        assert_eq!(enc(egui::Key::ArrowLeft, egui::Modifiers::ALT, TermMode::empty()), b"\x1b[1;3D");
        // Alt+Enter has no parameterised form, so it gets the ESC prefix.
        assert_eq!(enc(egui::Key::Enter, egui::Modifiers::ALT, TermMode::empty()), b"\x1b\r");
        // A key that emits nothing must not leave a stray ESC behind.
        assert!(enc(egui::Key::BrowserBack, egui::Modifiers::ALT, TermMode::empty()).is_empty());
    }

    #[test]
    fn backspace_sends_del_not_backspace() {
        assert_eq!(enc(egui::Key::Backspace, NONE, TermMode::empty()), vec![0x7f]);
        assert_eq!(enc(egui::Key::Backspace, egui::Modifiers::CTRL, TermMode::empty()), vec![0x08]);
    }

    #[test]
    fn shift_tab_is_a_back_tab() {
        assert_eq!(enc(egui::Key::Tab, egui::Modifiers::SHIFT, TermMode::empty()), b"\x1b[Z");
    }

    #[test]
    fn bracketed_paste_is_only_used_when_the_program_asked_for_it() {
        let mut plain = Vec::new();
        push_paste(&mut plain, "a\nb", TermMode::empty());
        assert_eq!(plain, b"a\rb");

        let mut wrapped = Vec::new();
        push_paste(&mut wrapped, "a\nb", TermMode::BRACKETED_PASTE);
        assert_eq!(wrapped, b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn a_paste_cannot_smuggle_its_own_terminator() {
        // Otherwise the paste ends early and the rest is executed as if typed.
        let mut out = Vec::new();
        push_paste(&mut out, "safe\x1b[201~rm -rf /", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~saferm -rf /\x1b[201~");
    }

    /// The payload between the two brackets, which is the part that must never spell a
    /// terminator.
    fn pasted_payload(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        push_paste(&mut out, text, TermMode::BRACKETED_PASTE);
        assert!(out.starts_with(b"\x1b[200~"), "{out:?}");
        assert!(out.ends_with(b"\x1b[201~"), "{out:?}");
        out[6..out.len() - 6].to_vec()
    }

    #[test]
    fn stripping_the_terminator_cannot_manufacture_a_new_one() {
        // A single pass of str::replace removes the inner occurrence and leaves the two halves
        // around it adjacent -- and they spell a fresh terminator. Everything after it leaves
        // the paste and is executed as keystrokes, Enter included, which is exactly the
        // "paste a snippet from a web page" attack the wrapper exists to prevent.
        let spliced = pasted_payload("\x1b[20\x1b[201~1~\rwhoami\r");
        assert!(
            !spliced.windows(6).any(|w| w == b"\x1b[201~"),
            "payload still spells a terminator: {:?}",
            String::from_utf8_lossy(&spliced)
        );

        // And it has to hold however deeply the halves are nested, not just one layer down.
        let nested = pasted_payload("\x1b[20\x1b[20\x1b[201~1~1~");
        assert!(
            !nested.windows(6).any(|w| w == b"\x1b[201~"),
            "payload still spells a terminator: {:?}",
            String::from_utf8_lossy(&nested)
        );
    }

    #[test]
    fn stripping_the_terminator_leaves_the_rest_of_the_paste_alone() {
        // The counterpart to the test above: it would also pass if the sanitiser threw the
        // whole payload away. Ordinary text, including lone ESCs and other CSI sequences, has
        // to come through byte for byte.
        assert_eq!(pasted_payload("cd /tmp\nls -la\n"), b"cd /tmp\nls -la\n");
        assert_eq!(pasted_payload("\x1b[200~"), b"\x1b[200~");
        assert_eq!(pasted_payload("\x1b[20"), b"\x1b[20");
        assert_eq!(pasted_payload("caf\u{e9} \u{1f600}"), "caf\u{e9} \u{1f600}".as_bytes());
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn alt_letter_sends_meta_escape() {
        // Readline's word motions are Alt+B and Alt+F, and both mean ESC then the letter. A
        // printable key never reached `encode_key`'s escaping — it falls through to `_ => {}`,
        // which emits nothing, and `Text` is suppressed because Alt is down. So the two most
        // common word motions in a shell did nothing at all.
        let alt = egui::Modifiers::ALT;
        let events = [egui::Event::Text("b".into()), key_event(egui::Key::B, alt)];
        assert_eq!(translate(&events, alt, TermMode::empty()).bytes, b"\x1bb");

        // Shift is part of the character, not a separate escape.
        let alt_shift = egui::Modifiers { alt: true, shift: true, ..Default::default() };
        let shifted = [egui::Event::Text("B".into()), key_event(egui::Key::B, alt_shift)];
        assert_eq!(translate(&shifted, alt_shift, TermMode::empty()).bytes, b"\x1bB");

        // And a digit, which is Alt+1 in tmux and friends.
        let digits = [egui::Event::Text("1".into()), key_event(egui::Key::Num1, alt)];
        assert_eq!(translate(&digits, alt, TermMode::empty()).bytes, b"\x1b1");
    }

    #[test]
    fn an_ime_commit_reaches_the_shell() {
        // The candidate window worked and the chosen characters went nowhere, because the event
        // was not handled at all. This is the whole of CJK and Korean input.
        let events = [egui::Event::Ime(egui::ImeEvent::Commit("\u{4f60}\u{597d}".into()))];
        let out = translate(&events, egui::Modifiers::NONE, TermMode::empty());
        assert_eq!(out.bytes, "\u{4f60}\u{597d}".as_bytes());
        assert!(out.scroll_to_bottom, "committing text is typing, so it returns to the prompt");
    }

    #[test]
    fn an_ime_preedit_sends_nothing() {
        // A composition in progress is the IME's to draw, not ours to put in the grid — and
        // sending it would type the intermediate letters and then the finished word.
        let events = [egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "ni".into(),
            active_range_chars: None,
        })];
        let out = translate(&events, egui::Modifiers::NONE, TermMode::empty());
        assert!(out.bytes.is_empty(), "{:?}", out.bytes);
    }

    #[test]
    fn altgr_still_types_text() {
        // Windows reports AltGr as Ctrl+Alt. Suppressing Text there kills `@` and `\` on several
        // European layouts.
        let events = [egui::Event::Text("@".into())];
        let altgr = egui::Modifiers { ctrl: true, alt: true, ..Default::default() };
        assert_eq!(translate(&events, altgr, TermMode::empty()).bytes, b"@");

        let ctrl_only = egui::Modifiers::CTRL;
        assert!(translate(&events, ctrl_only, TermMode::empty()).bytes.is_empty());
    }

    #[test]
    fn shift_pageup_scrolls_the_terminal_but_not_in_the_alternate_screen() {
        let events = [egui::Event::Key {
            key: egui::Key::PageUp,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }];

        let scrollback = translate(&events, egui::Modifiers::SHIFT, TermMode::empty());
        assert!(matches!(scrollback.scroll, Some(Scroll::PageUp)));
        assert!(scrollback.bytes.is_empty());

        // Inside vim or less, Shift+PageUp belongs to the program.
        let alt = translate(&events, egui::Modifiers::SHIFT, TermMode::ALT_SCREEN);
        assert!(alt.scroll.is_none());
        assert_eq!(alt.bytes, b"\x1b[5;2~");
    }
}
