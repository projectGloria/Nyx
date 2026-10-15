//! The channel a shell uses to tell Nyx what it is doing.
//!
//! **The transport is the window title, not a private OSC.** `alacritty_terminal` routes OSC 0
//! and 2 to `Event::Title` through its public `EventListener` and routes unknown OSC codes
//! nowhere at all — using a private number would mean replacing `EventLoop` with our own read
//! thread and a hand-written sixty-method `vte::ansi::Handler` shim. The title costs nothing and
//! uses only public API.
//!
//! Every message is `nyx:<token>:<kind>[:payload]`. The token is per pane and per launch, so a
//! program that prints `nyx:` cannot pretend to be shell integration, and it never appears in a
//! file the user might paste.

use std::time::{Duration, Instant};

/// How long to wait for a `hello` before concluding there is no integration.
///
/// Only matters for a shell we *did* inject into — a profile we did not inject into starts at
/// `Absent` immediately, so nothing waits on a script that was never loaded.
const HELLO_TIMEOUT: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integration {
    /// An init script was injected and its greeting has not arrived yet. The terminal-side layer
    /// stays quiet in the meantime rather than racing a `hello` that is milliseconds away and
    /// double-expanding the first command.
    Pending,
    Active,
    Absent,
}

#[derive(Debug, Clone)]
pub struct State {
    pub integration: Integration,
    since: Instant,
    /// Reported by the shell at each prompt. Used for opening a new pane in the same place.
    pub cwd: Option<String>,
    /// True between a command being submitted and it finishing.
    pub child_running: bool,
}

impl State {
    /// `injected` says whether an init script was actually handed to this shell.
    pub fn new(injected: bool) -> Self {
        Self {
            integration: if injected { Integration::Pending } else { Integration::Absent },
            since: Instant::now(),
            cwd: None,
            child_running: false,
        }
    }

    /// Whether the terminal-side layer should stand down.
    ///
    /// True while waiting for a greeting as well as after one arrives: both mean "something
    /// better is about to handle this, or already is".
    pub fn suppresses_shadow(&self) -> bool {
        matches!(self.integration, Integration::Pending | Integration::Active)
    }

    /// Give up waiting once the timeout has passed.
    pub fn tick(&mut self) {
        if self.integration == Integration::Pending && self.since.elapsed() >= HELLO_TIMEOUT {
            log::debug!("no shell integration greeting; falling back to the shadow layer");
            self.integration = Integration::Absent;
        }
    }

    pub fn apply(&mut self, mark: Mark) {
        match mark {
            Mark::Hello { .. } => self.integration = Integration::Active,
            Mark::PromptStart => self.child_running = false,
            Mark::PromptEnd { cwd } => {
                self.child_running = false;
                if let Some(cwd) = cwd {
                    self.cwd = Some(cwd);
                }
            },
            Mark::CommandStart => self.child_running = true,
            Mark::CommandEnd { .. } => self.child_running = false,
        }
    }
}

/// A parsed message from a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Hello { caps: Vec<String> },
    /// The prompt is about to be drawn.
    PromptStart,
    /// The prompt is drawn and input begins; carries the working directory.
    PromptEnd { cwd: Option<String> },
    /// A command was submitted; something is running.
    CommandStart,
    CommandEnd { exit: Option<i32> },
}

/// Parse a window title as a mark, if it is one addressed to this pane.
///
/// Returns `None` for an ordinary title, which is then shown as the tab's name. A title that
/// starts with `nyx:` but carries the wrong token is *also* `None` — it is some other program's
/// output and belongs on the tab, not in our state machine.
pub fn parse(title: &str, token: &str) -> Option<Mark> {
    let rest = title.strip_prefix("nyx:")?;
    let (got_token, rest) = rest.split_once(':')?;
    if got_token != token {
        return None;
    }

    let (kind, payload) = match rest.split_once(':') {
        Some((kind, payload)) => (kind, Some(payload)),
        None => (rest, None),
    };

    Some(match kind {
        "hello" => Mark::Hello {
            caps: payload
                .and_then(|p| {
                    p.split(':').find_map(|field| field.strip_prefix("caps=").map(|v| v.to_string()))
                })
                .map(|caps| caps.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default(),
        },
        "A" => Mark::PromptStart,
        "B" => Mark::PromptEnd {
            cwd: payload.and_then(|p| p.strip_prefix("cwd=")).map(decode_percent),
        },
        "C" => Mark::CommandStart,
        "D" => Mark::CommandEnd { exit: payload.and_then(|p| p.trim().parse().ok()) },
        _ => return None,
    })
}

/// The working directory is percent-encoded so a path containing `:` cannot split the message.
///
/// Decoding works on bytes throughout and never slices the `&str`. `&s[i + 1..i + 3]` panics
/// whenever either end lands inside a multi-byte character — `%` followed by `€` is enough — and
/// this runs on the UI thread with a payload a child process controls, because the pane's token
/// is handed to the shell in `NYX_TOKEN`.
///
/// An escape that is not exactly two hex digits stays literal. `u8::from_str_radix` is not the
/// same test: it accepts a leading sign, so `%+1` decoded to byte 0x01 in the middle of a path.
fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A fresh per-pane token. 128 bits from the system clock and the pane id is not cryptographic,
/// and does not need to be: it only has to be unguessable by a program printing text into its own
/// terminal, which has no way to observe it.
pub fn new_token(pane: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = (nanos as u64) ^ (pane.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    format!("{:016x}{:016x}", mixed, nanos as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_greeting_activates_integration() {
        let mark = parse("nyx:abc:hello:v=1:caps=sigil,alias,marks", "abc").unwrap();
        assert_eq!(
            mark,
            Mark::Hello { caps: vec!["sigil".into(), "alias".into(), "marks".into()] }
        );

        let mut state = State::new(true);
        assert_eq!(state.integration, Integration::Pending);
        state.apply(mark);
        assert_eq!(state.integration, Integration::Active);
    }

    #[test]
    fn a_percent_escape_never_panics_on_a_multibyte_character() {
        // `&s[i + 1..i + 3]` sliced the `&str` by byte offset, so a `%` with a multi-byte
        // character anywhere in the next two bytes landed inside one and panicked — on the UI
        // thread, taking the window down with it. The pane's token is handed to the shell in
        // NYX_TOKEN, so any child process can print a title carrying this payload.
        for payload in [
            "%\u{20ac}",                  // % then a three-byte char
            "%a\u{20ac}",                 // one hex digit then a three-byte char
            "%\u{e9}",                    // % then a two-byte char
            "C:\\%\u{4e2d}\u{6587}",      // a realistic path
            "%",                          // nothing after it at all
            "%a",                         // one digit and then the end
        ] {
            let mark = parse(&format!("nyx:tok:B:cwd={payload}"), "tok");
            assert!(mark.is_some(), "{payload:?} did not parse");
        }
    }

    #[test]
    fn a_well_formed_percent_escape_still_decodes() {
        // The counterpart: not panicking would also be satisfied by decoding nothing.
        let mark = parse("nyx:tok:B:cwd=C%3A%5CUsers%5CNihil", "tok").unwrap();
        assert_eq!(mark, Mark::PromptEnd { cwd: Some(r"C:\Users\Nihil".into()) });
    }

    #[test]
    fn a_malformed_percent_escape_stays_literal() {
        // Two hex digits or nothing. `u8::from_str_radix` accepts a leading sign, so the old
        // version quietly turned `%+1` into byte 0x01 in the middle of a path.
        let mark = parse("nyx:tok:B:cwd=100%+1 and 50%", "tok").unwrap();
        assert_eq!(mark, Mark::PromptEnd { cwd: Some("100%+1 and 50%".into()) });
    }

    #[test]
    fn a_wrong_token_is_an_ordinary_title() {
        // Otherwise anything that prints "nyx:..." could switch off the expansion layer.
        assert!(parse("nyx:other:hello", "abc").is_none());
        assert!(parse("just a title", "abc").is_none());
        assert!(parse("nyx:abc:unknown-kind", "abc").is_none());
    }

    #[test]
    fn the_working_directory_survives_a_path_containing_colons_and_spaces() {
        let mark = parse("nyx:abc:B:cwd=C%3A%5CFree%20Tibet", "abc").unwrap();
        assert_eq!(mark, Mark::PromptEnd { cwd: Some(r"C:\Free Tibet".into()) });
    }

    #[test]
    fn command_marks_track_whether_something_is_running() {
        let mut state = State::new(true);
        state.apply(Mark::CommandStart);
        assert!(state.child_running);
        state.apply(Mark::CommandEnd { exit: Some(0) });
        assert!(!state.child_running);
    }

    #[test]
    fn an_uninjected_shell_starts_absent_so_the_shadow_works_immediately() {
        // WSL and ssh get no script, and must not spend 2.5 seconds waiting for a greeting that
        // is never coming.
        let state = State::new(false);
        assert_eq!(state.integration, Integration::Absent);
        assert!(!state.suppresses_shadow());
    }

    #[test]
    fn an_injected_shell_suppresses_the_shadow_while_it_waits() {
        // The greeting is milliseconds away; expanding in the meantime risks doing it twice.
        let state = State::new(true);
        assert!(state.suppresses_shadow());
    }

    #[test]
    fn a_greeting_that_never_arrives_expires_into_absent() {
        // A script that failed to load still leaves the pane Pending, and Pending suppresses the
        // shadow layer. The expiry is what keeps a broken integration from silencing the
        // terminal-side layer for the life of the pane.
        let mut state = State::new(true);
        state.tick();
        assert_eq!(state.integration, Integration::Pending, "2.5 s have not passed yet");

        // The timeout is read from `since`, which `new` set — so a pane that has been waiting
        // since before a test run started is already past it. Rebuilding the state by hand keeps
        // the wait out of the gate; `since` is private, so expire it through a copy that is
        // older than the timeout.
        let mut aged = state.clone();
        aged.since = Instant::now() - HELLO_TIMEOUT;
        aged.tick();
        assert_eq!(aged.integration, Integration::Absent);
        assert!(!aged.suppresses_shadow());

        // Active is not a state the timeout touches: a shell that greeted is a shell that works.
        let mut active = aged;
        active.integration = Integration::Active;
        active.tick();
        assert_eq!(active.integration, Integration::Active);
    }

    #[test]
    fn tokens_differ_between_panes() {
        assert_ne!(new_token(1), new_token(2));
        assert_eq!(new_token(1).len(), 32);
    }
}
