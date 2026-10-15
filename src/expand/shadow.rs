//! A best-effort model of what the user has typed since the prompt.
//!
//! The shell owns the real line editor, not us. This model is fed from keystrokes on their way to
//! the PTY and is only ever *approximately* right — so the rule is that it refuses to act the
//! moment it might be wrong, rather than guessing. An expansion that fires against a stale model
//! erases the wrong characters, and the user watches their prompt get eaten.
//!
//! Phase 6 adds shell-side integration, which is both more accurate and able to show the
//! expansion before Enter. This layer then steps aside for shells that have it, and remains the
//! only thing that works for WSL and ssh, which we do not write into.

use alacritty_terminal::term::TermMode;

use crate::expand::line::{self, Expansion};
use crate::expand::table::Tables;
use crate::term::profile::PathStyle;

/// What a keystroke did to the input line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Char(char),
    Backspace,
    /// Enter: the line is gone and a new prompt is coming.
    Submit,
    /// Anything the model cannot follow — history recall, tab completion, a cursor move. The
    /// line is no longer knowable until the next prompt.
    Desync,
}

#[derive(Debug, Default)]
pub struct Shadow {
    line: String,
    synced: bool,
}

impl Shadow {
    pub fn new() -> Self {
        // Starts *synced*, because a freshly spawned shell has an empty input line and the model
        // says exactly that. Starting unsynced was the conservative-looking choice and it was
        // wrong: it meant the first command typed in every pane never expanded, which is the one
        // a user is most likely to try.
        //
        // The shell's own banner does not change this — it is output, and output never puts
        // characters on the input line.
        Self {
            line: String::new(),
            synced: true,
        }
    }

    pub fn feed(&mut self, feed: Feed) {
        match feed {
            Feed::Char(c) if self.synced => self.line.push(c),
            Feed::Backspace if self.synced => {
                self.line.pop();
            }
            Feed::Submit => {
                // A fresh prompt is the one moment the line is known to be empty, which is what
                // makes this model usable at all.
                self.line.clear();
                self.synced = true;
            }
            Feed::Desync => {
                self.line.clear();
                self.synced = false;
            }
            _ => {}
        }
    }

    pub fn line(&self) -> Option<&str> {
        self.synced.then_some(self.line.as_str())
    }

    /// Apply a rewrite to the model, so it stays in step with what the shell now shows.
    pub fn apply(&mut self, expansion: &Expansion) {
        if !self.synced {
            return;
        }
        for _ in 0..expansion.backspaces {
            self.line.pop();
        }
        self.line.push_str(&expansion.text);
    }

    /// Whether the terminal-side layer may touch this keystroke at all.
    ///
    /// The single place this is decided. If a fifth condition is ever needed it goes here — a
    /// second copy of the reasoning is how the two expansion layers end up disagreeing about one
    /// keystroke and producing `cd "C:\...""C:\..."`.
    pub fn may_expand(&self, mode: TermMode, integration_active: bool) -> bool {
        // Inside vim, less, fzf or htop there is no command line; "expanding" corrupts a buffer.
        !mode.contains(TermMode::ALT_SCREEN)
            // Phase 6: the shell does it properly and better.
            && !integration_active
            && self.synced
    }

    /// Whether the model is trustworthy enough to *describe* the line, as opposed to rewrite it.
    ///
    /// Deliberately not `may_expand`, and the difference is the whole reason there are two:
    /// shell-side integration is a reason not to send a second rewrite, and no reason at all not
    /// to say what the line already means. Both layers agree about what `@nyx` is; only one of
    /// them may act on it. Gating the hint and the completion menu on `may_expand` would have
    /// turned them off in PowerShell, Clink and bash — every shell we integrate with, which is to
    /// say everywhere they are most accurate.
    pub fn may_hint(&self, mode: TermMode) -> bool {
        // Inside vim, less or htop there is no command line to describe, and a box drawn over the
        // buffer is describing text the user did not type.
        !mode.contains(TermMode::ALT_SCREEN) && self.synced
    }

    /// The shortcut this line names, when it names nothing else at all.
    ///
    /// Returns an index into `Tables::shortcuts`. Only an exact whole-line match counts: `cd @nyx`
    /// is a command that happens to mention a shortcut and has to keep running as one, and the
    /// difference between the two is the only thing standing between "Enter opens a tab" and
    /// "Enter stopped working".
    ///
    /// Safe under integration for a reason worth writing down: an expansion fires on space or
    /// Enter, so a line with neither in it has not been rewritten by anybody, and the model of it
    /// is exactly what was typed.
    pub fn whole_line_sigil(&self, tables: &Tables, mode: TermMode) -> Option<usize> {
        if !self.may_hint(mode) {
            return None;
        }
        let line = self.line()?.trim();
        tables
            .shortcuts
            .iter()
            .position(|s| s.sigil.eq_ignore_ascii_case(line))
    }

    /// The rewrite the *shell's own* integration is about to make, so the model can follow it.
    ///
    /// The model is fed keystrokes whether or not this layer is allowed to expand, and under
    /// integration it was therefore wrong from the first space after a sigil: the shell replaced
    /// `@nyx` with a path, the model still held `@nyx`, and everything reading the model — the
    /// hint, the menu — described a line that was no longer on screen. Nothing read it before, so
    /// nothing reported it.
    ///
    /// The generated scripts expand the token before the cursor by the same rules as
    /// `line::on_space`, from the same table, and the shellgen tests are what hold the two
    /// together. This applies that rewrite to the model and emits nothing: the shell has already
    /// done the visible half.
    pub fn mirror_shell_expansion(&mut self, tables: &Tables, style: PathStyle, mode: TermMode) {
        if mode.contains(TermMode::ALT_SCREEN) || !self.synced {
            return;
        }
        if let Some(expansion) = self.line().and_then(|l| line::on_space(l, tables, style)) {
            self.apply(&expansion);
        }
    }

    pub fn on_space(
        &self,
        tables: &Tables,
        style: PathStyle,
        mode: TermMode,
        integration_active: bool,
    ) -> Option<Expansion> {
        if !self.may_expand(mode, integration_active) {
            return None;
        }
        line::on_space(self.line()?, tables, style)
    }

    pub fn on_submit(
        &self,
        tables: &Tables,
        style: PathStyle,
        mode: TermMode,
        integration_active: bool,
    ) -> Option<Expansion> {
        if !self.may_expand(mode, integration_active) {
            return None;
        }
        line::on_submit(self.line()?, tables, style)
    }
}

/// Bytes that apply an expansion: erase what was typed, then write the replacement.
pub fn bytes_for(expansion: &Expansion) -> Vec<u8> {
    let mut out = Vec::with_capacity(expansion.backspaces + expansion.text.len());
    // DEL, matching what `input.rs` sends for Backspace — a line editor configured for one and
    // fed the other will beep instead of erasing.
    out.extend(std::iter::repeat_n(0x7f, expansion.backspaces));
    out.extend_from_slice(expansion.text.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{AliasMap, ShortcutConfig};

    fn tables() -> Tables {
        let shortcuts = vec![ShortcutConfig {
            id: "tibet".into(),
            label: "Free Tibet".into(),
            sigil: "@tibet".into(),
            path: r"C:\Free Tibet".into(),
            icon: String::new(),
            color: None,
            order: 0,
        }];
        let mut aliases = AliasMap::new();
        aliases.insert("gs".into(), "git status".into());
        Tables::from_config(&shortcuts, &aliases).0
    }

    fn typed(s: &str) -> Shadow {
        let mut shadow = Shadow::new();
        shadow.feed(Feed::Submit); // reach a known prompt
        for c in s.chars() {
            shadow.feed(Feed::Char(c));
        }
        shadow
    }

    const WIN: PathStyle = PathStyle::Windows;

    #[test]
    fn fully_typed_sigils_work_in_any_case_without_a_menu() {
        for token in ["@tibet", "@TIBET", "@TiBeT"] {
            let shadow = typed(token);
            assert!(crate::expand::complete::menu(token, &tables()).is_none());
            assert_eq!(
                shadow.whole_line_sigil(&tables(), TermMode::empty()),
                Some(0)
            );
        }
    }

    #[test]
    fn the_very_first_command_in_a_pane_expands() {
        // A fresh shell has an empty input line, so the model is correct immediately. This was
        // a real bug: starting unsynced meant the first thing anyone typed never expanded.
        let mut shadow = Shadow::new();
        for c in "cd @tibet".chars() {
            shadow.feed(Feed::Char(c));
        }
        assert_eq!(shadow.line(), Some("cd @tibet"));
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .is_some());
    }

    #[test]
    fn a_sigil_typed_after_a_prompt_expands() {
        let shadow = typed("cd @tibet");
        let e = shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .unwrap();
        assert_eq!(e.backspaces, 6);
        assert_eq!(e.text, "\"C:\\Free Tibet\"");
    }

    #[test]
    fn backspace_is_tracked() {
        let mut shadow = typed("cd @tibetX");
        shadow.feed(Feed::Backspace);
        assert_eq!(shadow.line(), Some("cd @tibet"));
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .is_some());
    }

    #[test]
    fn the_alternate_screen_stops_everything() {
        // Typing "@tibet" inside vim must insert those six characters, not a path.
        let shadow = typed("cd @tibet");
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::ALT_SCREEN, false)
            .is_none());
    }

    #[test]
    fn shell_side_integration_takes_priority() {
        // Both layers firing produces `cd "C:\Free Tibet""C:\Free Tibet"`.
        let shadow = typed("cd @tibet");
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), true)
            .is_none());
    }

    #[test]
    fn a_desync_disables_expansion_until_the_next_prompt() {
        // Pressing Up recalls history; the model has no idea what is on the line now.
        let mut shadow = typed("cd @tibet");
        shadow.feed(Feed::Desync);
        assert!(shadow.line().is_none());
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .is_none());

        // ...and Enter restores confidence.
        shadow.feed(Feed::Submit);
        for c in "cd @tibet".chars() {
            shadow.feed(Feed::Char(c));
        }
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .is_some());
    }

    #[test]
    fn applying_an_expansion_keeps_the_model_in_step() {
        let mut shadow = typed("cd @tibet");
        let e = shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .unwrap();
        shadow.apply(&e);
        assert_eq!(shadow.line(), Some("cd \"C:\\Free Tibet\""));
        // And it does not fire a second time on the now-expanded text.
        assert!(shadow
            .on_space(&tables(), WIN, TermMode::empty(), false)
            .is_none());
    }

    #[test]
    fn a_line_that_is_nothing_but_a_sigil_names_a_shortcut() {
        assert_eq!(
            typed("@tibet").whole_line_sigil(&tables(), TermMode::empty()),
            Some(0)
        );
        // Whitespace around it is still nothing but a sigil.
        assert_eq!(
            typed("  @tibet ").whole_line_sigil(&tables(), TermMode::empty()),
            Some(0)
        );
    }

    #[test]
    fn a_sigil_used_as_an_argument_is_not_a_line_that_names_a_shortcut() {
        // The line that must keep running as a `cd`. Getting this wrong does not produce a
        // misplaced tab, it produces a shell where Enter no longer runs commands.
        assert_eq!(
            typed("cd @tibet").whole_line_sigil(&tables(), TermMode::empty()),
            None
        );
        assert_eq!(
            typed("@tibet ls").whole_line_sigil(&tables(), TermMode::empty()),
            None
        );
    }

    #[test]
    fn the_alternate_screen_and_a_desync_both_withhold_the_line() {
        assert_eq!(
            typed("@tibet").whole_line_sigil(&tables(), TermMode::ALT_SCREEN),
            None
        );
        let mut shadow = typed("@tibet");
        shadow.feed(Feed::Desync);
        assert_eq!(shadow.whole_line_sigil(&tables(), TermMode::empty()), None);
    }

    #[test]
    fn hinting_survives_integration_where_expanding_does_not() {
        // The two gates exist to disagree here: under PowerShell or Clink the shell does the
        // rewriting, and the description is still ours to draw.
        let shadow = typed("cd @tibet");
        assert!(shadow.may_hint(TermMode::empty()));
        assert!(!shadow.may_expand(TermMode::empty(), true));
    }

    #[test]
    fn the_model_follows_an_expansion_the_shell_made_for_itself() {
        // Under integration nothing here emits bytes, so the model used to keep `@tibet` while
        // the shell's line editor showed the path — and every reader of the model was then
        // describing a line that was not on screen.
        let mut shadow = typed("cd @tibet");
        shadow.mirror_shell_expansion(&tables(), WIN, TermMode::empty());
        assert_eq!(shadow.line(), Some("cd \"C:\\Free Tibet\""));
    }

    #[test]
    fn mirroring_a_line_the_shell_would_not_have_touched_changes_nothing() {
        let mut shadow = typed("cd somewhere");
        shadow.mirror_shell_expansion(&tables(), WIN, TermMode::empty());
        assert_eq!(shadow.line(), Some("cd somewhere"));
    }

    #[test]
    fn the_bytes_erase_exactly_what_was_typed() {
        let e = Expansion {
            backspaces: 3,
            text: "abc".into(),
        };
        assert_eq!(bytes_for(&e), vec![0x7f, 0x7f, 0x7f, b'a', b'b', b'c']);
    }
}
