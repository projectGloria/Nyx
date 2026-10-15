//! The kitty keyboard protocol: `CSI <code> ; <modifiers> : <event> u`.
//!
//! Why a terminal needs this at all. The legacy encoding is a fixed table with no room left in
//! it — `Ctrl+I` and `Tab` are both `0x09`, `Ctrl+M` and `Enter` are both `0x0d`, and
//! `Shift+Enter`, `Ctrl+Enter` and `Alt+Enter` have no spelling whatsoever, so a program cannot
//! be told they happened. That is not a gap anyone notices in a shell, and it is the first thing
//! anyone notices in a full-screen program: "newline without submitting" is the most asked-for
//! key in every editor and every agent CLI, and on a terminal without this protocol the honest
//! answer is that the keystroke does not exist.
//!
//! A program opts in with `CSI > <flags> u`, and the flags are progressive — it asks only for
//! what it can use. `alacritty_terminal` has always tracked the negotiation, the mode stack and
//! the queries; `TermConfig::kitty_keyboard` gates the whole thing and Nyx left it at its `false`
//! default, so every request was refused and every program correctly fell back. Turning it on is
//! half the work: the crate parses the request, and what is below is the answer.
//!
//! **What is implemented, and what is not.** `DISAMBIGUATE_ESC_CODES`, `REPORT_EVENT_TYPES`,
//! `REPORT_ALL_KEYS_AS_ESC` and `REPORT_ASSOCIATED_TEXT` are honoured. `REPORT_ALTERNATE_KEYS`
//! reports the shifted key when the layout said what it produced, and never the base-layout key:
//! egui reports a logical key, and Windows does not hand us another layout's mapping for it, so
//! that sub-field is omitted rather than guessed. Every one of those sub-fields is optional in
//! the specification, so a program that asked for alternates and gets one field instead of two
//! reads a well-formed sequence and loses only a layout-independent shortcut it can still take
//! from the primary code.
//!
//! The numeric keypad is absent for the reason `input` already records: egui does not
//! distinguish it from the number row, so `KP_Enter` cannot be told from `Enter`.

use alacritty_terminal::term::TermMode;

/// What happened to the key. `1`, `2`, `3` in the protocol; press is the default and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Press,
    Repeat,
    Release,
}

impl Event {
    pub fn of(pressed: bool, repeat: bool) -> Self {
        match (pressed, repeat) {
            (false, _) => Event::Release,
            (true, true) => Event::Repeat,
            (true, false) => Event::Press,
        }
    }

    fn param(self) -> u8 {
        match self {
            Event::Press => 1,
            Event::Repeat => 2,
            Event::Release => 3,
        }
    }
}

/// Whether a program in this pane has turned the protocol on.
pub fn active(mode: TermMode) -> bool {
    mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL)
}

/// `1 + shift + 2*alt + 4*ctrl`, the same base the legacy encoding uses.
///
/// Super, hyper, meta, caps lock and num lock are the protocol's higher bits and are never set:
/// egui reports none of them here, and a bit we cannot observe is worse stated than omitted — a
/// program would read an absent bit as "caps lock is off" rather than "unknown".
fn modifier_param(m: egui::Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl)
}

/// The number and final byte of a key that keeps its legacy escape shape.
///
/// These stay `CSI 1 ; <mods> A` and `CSI <n> ; <mods> ~` rather than becoming `CSI u`, which is
/// what the specification asks for and what every program already parses.
fn legacy_shape(key: egui::Key) -> Option<(u8, u8)> {
    use egui::Key;
    Some(match key {
        Key::ArrowUp => (1, b'A'),
        Key::ArrowDown => (1, b'B'),
        Key::ArrowRight => (1, b'C'),
        Key::ArrowLeft => (1, b'D'),
        Key::Home => (1, b'H'),
        Key::End => (1, b'F'),
        Key::F1 => (1, b'P'),
        Key::F2 => (1, b'Q'),
        Key::F3 => (1, b'R'),
        Key::F4 => (1, b'S'),
        Key::Insert => (2, b'~'),
        Key::Delete => (3, b'~'),
        Key::PageUp => (5, b'~'),
        Key::PageDown => (6, b'~'),
        Key::F5 => (15, b'~'),
        Key::F6 => (17, b'~'),
        Key::F7 => (18, b'~'),
        Key::F8 => (19, b'~'),
        Key::F9 => (20, b'~'),
        Key::F10 => (21, b'~'),
        Key::F11 => (23, b'~'),
        Key::F12 => (24, b'~'),
        _ => return None,
    })
}

/// The unshifted codepoint the protocol identifies a key by.
///
/// Spelled out rather than taken from whatever the layout produced, because that is what the
/// protocol asks for: `Shift+1` is key `49` with a shifted alternate of `33`, and a program
/// matching on "the 1 key" has to see `49` on every layout that has one.
fn key_code(key: egui::Key) -> Option<u32> {
    use egui::Key;
    let c = match key {
        Key::Escape => return Some(27),
        Key::Enter => return Some(13),
        Key::Tab => return Some(9),
        Key::Backspace => return Some(127),

        Key::Space => ' ',

        Key::A => 'a', Key::B => 'b', Key::C => 'c', Key::D => 'd', Key::E => 'e',
        Key::F => 'f', Key::G => 'g', Key::H => 'h', Key::I => 'i', Key::J => 'j',
        Key::K => 'k', Key::L => 'l', Key::M => 'm', Key::N => 'n', Key::O => 'o',
        Key::P => 'p', Key::Q => 'q', Key::R => 'r', Key::S => 's', Key::T => 't',
        Key::U => 'u', Key::V => 'v', Key::W => 'w', Key::X => 'x', Key::Y => 'y',
        Key::Z => 'z',

        Key::Num0 => '0', Key::Num1 => '1', Key::Num2 => '2', Key::Num3 => '3',
        Key::Num4 => '4', Key::Num5 => '5', Key::Num6 => '6', Key::Num7 => '7',
        Key::Num8 => '8', Key::Num9 => '9',

        Key::Minus => '-',
        Key::Equals => '=',
        Key::OpenBracket => '[',
        Key::CloseBracket => ']',
        Key::Backslash => '\\',
        Key::Semicolon => ';',
        Key::Quote => '\'',
        Key::Comma => ',',
        Key::Period => '.',
        Key::Slash => '/',
        Key::Backtick => '`',

        // Shifted spellings that egui reports as keys of their own. A program matches on the
        // unshifted partner, so they resolve to the same code and the shift is in the modifier.
        Key::Plus => '=',
        Key::Colon => ';',
        Key::Questionmark => '/',
        Key::Pipe => '\\',
        Key::Exclamationmark => '1',
        Key::OpenCurlyBracket => '[',
        Key::CloseCurlyBracket => ']',

        // Reported only under `REPORT_ALL_KEYS_AS_ESC`; the protocol's private range for them.
        Key::ShiftLeft => return Some(57441),
        Key::ControlLeft => return Some(57442),
        Key::AltLeft => return Some(57443),
        Key::SuperLeft => return Some(57444),
        Key::ShiftRight => return Some(57447),
        Key::ControlRight => return Some(57448),
        Key::AltRight => return Some(57449),
        Key::SuperRight => return Some(57450),

        _ => return None,
    };
    Some(c as u32)
}

/// How this key should be spelled, or `None` to leave it to the ordinary encoding.
///
/// **`None` is the important half.** With only `DISAMBIGUATE_ESC_CODES` set — which is what
/// nearly every program asks for — ordinary typing must still arrive as plain text and an
/// unmodified Enter must still be `\r`. So the protocol takes exactly the keys the legacy
/// encoding cannot express and hands back the rest, and a pane whose editor exited without
/// resetting the mode still has a working shell in it.
///
/// `text` is what the layout produced for this keystroke, when it produced anything; it supplies
/// the shifted alternate and the associated-text field.
pub fn encode(
    key: egui::Key,
    m: egui::Modifiers,
    mode: TermMode,
    event: Event,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    if !active(mode) {
        return None;
    }
    let all_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);

    // A release nobody asked to hear about. `None` rather than an empty answer, because the
    // ordinary path drops releases too — this is a fallthrough, not a special case.
    if event == Event::Release && !events {
        return None;
    }

    // A modifier key pressed by itself is an event only `REPORT_ALL_KEYS_AS_ESC` asks for.
    if crate::config::keys::is_modifier(key) && !all_esc {
        return None;
    }

    // Asked before `key_code`, because a legacy-shaped key is identified by its own number and
    // final byte and has no `CSI u` codepoint at all — looking one up first returned `None` for
    // every arrow and function key, and the branch below could never run.
    // Keys that keep a legacy shape. Owned here only when there is something to add to it;
    // otherwise the legacy encoder already produces identical bytes, and one encoding with one
    // implementation is worth more than a second that happens to agree today.
    if let Some((number, final_byte)) = legacy_shape(key) {
        if !events && !all_esc {
            return None;
        }
        let mut out = format!("\x1b[{number};{}", modifier_param(m));
        if events {
            out.push_str(&format!(":{}", event.param()));
        }
        out.push(final_byte as char);
        return Some(out.into_bytes());
    }

    let code = key_code(key)?;

    // Enter, Tab, Backspace, Escape and the printable keys. Unmodified they are text or their
    // own control byte and taking those would break every shell; modified, they are the gap.
    //
    // Escape is always taken: telling `Esc` apart from the start of an escape sequence is the
    // single thing `DISAMBIGUATE_ESC_CODES` is named after.
    let owned = all_esc
        || events
        || m.ctrl
        || m.alt
        || key == egui::Key::Escape
        || (m.shift && matches!(key, egui::Key::Enter | egui::Key::Tab | egui::Key::Backspace));
    if !owned {
        return None;
    }

    let mut out = format!("\x1b[{code}");

    // The shifted alternate, when the layout said what shift produced. The base-layout key is
    // the third sub-field and is deliberately never written — see the module comment.
    if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && m.shift {
        if let Some(shifted) = text
            .and_then(|t| t.chars().next())
            .filter(|c| *c as u32 != code)
        {
            out.push_str(&format!(":{}", shifted as u32));
        }
    }

    let param = modifier_param(m);
    let with_text = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT)
        && event != Event::Release
        && text.is_some_and(|t| !t.is_empty());

    if param != 1 || events || with_text {
        out.push_str(&format!(";{param}"));
        if events {
            out.push_str(&format!(":{}", event.param()));
        }
    }

    if with_text {
        let codepoints: Vec<String> = text
            .unwrap_or_default()
            .chars()
            .map(|c| (c as u32).to_string())
            .collect();
        out.push(';');
        out.push_str(&codepoints.join(":"));
    }

    out.push('u');
    Some(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: egui::Modifiers = egui::Modifiers::NONE;
    const DISAMBIGUATE: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;

    fn enc(key: egui::Key, m: egui::Modifiers, mode: TermMode) -> Option<String> {
        encode(key, m, mode, Event::Press, None).map(|b| String::from_utf8(b).unwrap())
    }

    /// The reason the protocol exists, and the thing a terminal without it cannot express.
    #[test]
    fn the_enter_chords_that_have_no_legacy_spelling_finally_have_one() {
        assert_eq!(enc(egui::Key::Enter, egui::Modifiers::SHIFT, DISAMBIGUATE).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(enc(egui::Key::Enter, egui::Modifiers::ALT, DISAMBIGUATE).as_deref(), Some("\x1b[13;3u"));
        assert_eq!(enc(egui::Key::Enter, egui::Modifiers::CTRL, DISAMBIGUATE).as_deref(), Some("\x1b[13;5u"));
    }

    /// `Ctrl+I` and `Tab` are the same byte in the legacy encoding. Here they are not.
    #[test]
    fn control_chords_stop_colliding_with_the_keys_they_shared_a_byte_with() {
        assert_eq!(enc(egui::Key::I, egui::Modifiers::CTRL, DISAMBIGUATE).as_deref(), Some("\x1b[105;5u"));
        assert_eq!(enc(egui::Key::M, egui::Modifiers::CTRL, DISAMBIGUATE).as_deref(), Some("\x1b[109;5u"));
        assert_eq!(enc(egui::Key::Tab, NONE, DISAMBIGUATE), None, "plain Tab stays 0x09");
        assert_eq!(enc(egui::Key::Tab, egui::Modifiers::SHIFT, DISAMBIGUATE).as_deref(), Some("\x1b[9;2u"));
    }

    /// The half that keeps every shell working: a mode left set by a crashed editor must not
    /// turn ordinary typing into escape sequences.
    #[test]
    fn plain_typing_and_a_bare_enter_are_left_entirely_alone() {
        for key in [egui::Key::A, egui::Key::Num1, egui::Key::Space, egui::Key::Comma] {
            assert_eq!(enc(key, NONE, DISAMBIGUATE), None, "{key:?}");
            assert_eq!(enc(key, egui::Modifiers::SHIFT, DISAMBIGUATE), None, "shifted {key:?}");
        }
        assert_eq!(enc(egui::Key::Enter, NONE, DISAMBIGUATE), None);
        assert_eq!(enc(egui::Key::Backspace, NONE, DISAMBIGUATE), None);
        // And nothing at all happens when no program asked.
        assert_eq!(enc(egui::Key::Enter, egui::Modifiers::SHIFT, TermMode::empty()), None);
    }

    #[test]
    fn escape_is_always_taken_because_telling_it_apart_is_the_whole_point() {
        assert_eq!(enc(egui::Key::Escape, NONE, DISAMBIGUATE).as_deref(), Some("\x1b[27u"));
    }

    /// Arrows and function keys keep the shape programs already parse, and are only taken over
    /// when there is an event type to add to them.
    #[test]
    fn keys_with_a_legacy_shape_keep_it() {
        assert_eq!(enc(egui::Key::ArrowUp, NONE, DISAMBIGUATE), None);
        assert_eq!(enc(egui::Key::ArrowUp, egui::Modifiers::ALT, DISAMBIGUATE), None);

        let with_events = DISAMBIGUATE | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(enc(egui::Key::ArrowUp, egui::Modifiers::ALT, with_events).as_deref(), Some("\x1b[1;3:1A"));
        assert_eq!(enc(egui::Key::PageUp, NONE, with_events).as_deref(), Some("\x1b[5;1:1~"));
    }

    #[test]
    fn releases_and_repeats_are_reported_only_to_a_program_that_asked() {
        let key = egui::Key::A;
        assert_eq!(encode(key, NONE, DISAMBIGUATE, Event::Release, None), None);

        let with_events = DISAMBIGUATE | TermMode::REPORT_EVENT_TYPES;
        let release = encode(key, NONE, with_events, Event::Release, None).unwrap();
        assert_eq!(String::from_utf8(release).unwrap(), "\x1b[97;1:3u");
        let repeat = encode(key, NONE, with_events, Event::Repeat, None).unwrap();
        assert_eq!(String::from_utf8(repeat).unwrap(), "\x1b[97;1:2u");
    }

    #[test]
    fn event_of_reads_egui_flags_the_way_the_protocol_numbers_them() {
        assert_eq!(Event::of(true, false), Event::Press);
        assert_eq!(Event::of(true, true), Event::Repeat);
        assert_eq!(Event::of(false, false), Event::Release);
        // A release carrying egui's repeat flag is still a release.
        assert_eq!(Event::of(false, true), Event::Release);
    }

    #[test]
    fn asking_for_every_key_gets_every_key_including_the_modifiers_themselves() {
        let mode = TermMode::REPORT_ALL_KEYS_AS_ESC | DISAMBIGUATE;
        assert_eq!(enc(egui::Key::A, NONE, mode).as_deref(), Some("\x1b[97u"));
        assert_eq!(enc(egui::Key::ShiftLeft, egui::Modifiers::SHIFT, mode).as_deref(), Some("\x1b[57441;2u"));
        // ...and not otherwise: a bare Shift is not an event most programs want.
        assert_eq!(enc(egui::Key::ShiftLeft, egui::Modifiers::SHIFT, DISAMBIGUATE), None);
    }

    #[test]
    fn associated_text_and_the_shifted_alternate_come_from_what_the_layout_produced() {
        let mode = DISAMBIGUATE | TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_ASSOCIATED_TEXT;
        let bytes = encode(egui::Key::A, NONE, mode, Event::Press, Some("a")).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\x1b[97;1;97u");

        let alt = DISAMBIGUATE | TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_ALTERNATE_KEYS;
        let bytes = encode(egui::Key::Num1, egui::Modifiers::SHIFT, alt, Event::Press, Some("!")).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\x1b[49:33;2u");

        // No alternate when shift produced the same character, rather than a field saying so.
        let bytes = encode(egui::Key::Num1, egui::Modifiers::SHIFT, alt, Event::Press, Some("1")).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\x1b[49;2u");
    }
}
