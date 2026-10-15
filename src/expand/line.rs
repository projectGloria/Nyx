//! Rewriting a command line: `@tibet` to a real path, `gs` to `git status`.
//!
//! Pure. The stateful "what has the user typed so far" model lives in `shadow.rs`; everything
//! here is a function of (line, tables, shell).
//!
//! Merged from the planned `sigil.rs` and `alias.rs`: both are the same walk over the same
//! tokens, and splitting them would have meant two tokenisers that could disagree about what a
//! word is.

use crate::expand::table::Tables;
use crate::term::profile::PathStyle;

/// A rewrite to apply to what the user has already typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// How many characters to erase, counted in characters rather than bytes because that is
    /// what the shell echoed and therefore what a backspace removes.
    pub backspaces: usize,
    pub text: String,
}

/// Byte ranges of whitespace-separated tokens.
fn tokens(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((s, line.len()));
    }
    out
}

/// Rewrite every expandable token, preserving the original spacing exactly.
pub fn expand_line(line: &str, tables: &Tables, style: PathStyle) -> String {
    let spans = tokens(line);
    if spans.is_empty() {
        return line.to_string();
    }

    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;

    for (idx, (start, end)) in spans.iter().copied().enumerate() {
        out.push_str(&line[cursor..start]);
        let token = &line[start..end];

        let replacement = if let Some(shortcut) = tables.find_sigil(token) {
            Some(shortcut.expansion(style))
        } else if idx == 0 {
            // Aliases are a *command* concept: only the first word is one. Otherwise `echo gs`
            // would print "git status".
            tables.find_alias(token).map(|s| s.to_string())
        } else {
            None
        };

        out.push_str(replacement.as_deref().unwrap_or(token));
        cursor = end;
    }

    out.push_str(&line[cursor..]);
    out
}

/// The rewrite to apply when the user has just typed a space.
///
/// Only the token that was completed is considered, so typing continues to feel local — nothing
/// earlier in the line moves under the cursor.
pub fn on_space(line: &str, tables: &Tables, style: PathStyle) -> Option<Expansion> {
    let spans = tokens(line);
    let (start, end) = *spans.last()?;
    // The token must actually end at the cursor; a trailing space means there is nothing to
    // complete.
    if end != line.len() {
        return None;
    }
    let token = &line[start..end];
    let is_first = spans.len() == 1;

    let replacement = if let Some(shortcut) = tables.find_sigil(token) {
        shortcut.expansion(style)
    } else if is_first {
        tables.find_alias(token)?.to_string()
    } else {
        return None;
    };

    Some(Expansion { backspaces: token.chars().count(), text: replacement })
}

/// The rewrite to apply when the user presses Enter.
///
/// The whole line is rewritten and re-sent, rather than editing in place: an alias at the start
/// of a long line would otherwise need the cursor moved back to it, and there is no portable way
/// to do that across cmd.exe, PSReadLine and readline.
pub fn on_submit(line: &str, tables: &Tables, style: PathStyle) -> Option<Expansion> {
    let expanded = expand_line(line, tables, style);
    if expanded == line {
        return None;
    }
    Some(Expansion { backspaces: line.chars().count(), text: expanded })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{AliasMap, ShortcutConfig};

    fn tables() -> Tables {
        let shortcuts = vec![
            ShortcutConfig {
                id: "tibet".into(),
                label: "Free Tibet".into(),
                sigil: "@tibet".into(),
                path: r"C:\Users\Nihil\Desktop\Free Tibet".into(),
                icon: String::new(),
                color: None,
                order: 0,
            },
            ShortcutConfig {
                id: "nyx".into(),
                label: "Nyx".into(),
                sigil: "@nyx".into(),
                path: r"C:\Users\Nihil\Desktop\Free Tibet\Nyx".into(),
                icon: String::new(),
                color: None,
                order: 1,
            },
        ];
        let mut aliases = AliasMap::new();
        aliases.insert("gs".into(), "git status".into());
        aliases.insert("ll".into(), "ls -la".into());
        Tables::from_config(&shortcuts, &aliases).0
    }

    const WIN: PathStyle = PathStyle::Windows;
    const WSL: PathStyle = PathStyle::Wsl;

    #[test]
    fn a_sigil_expands_to_a_quoted_path() {
        let e = on_space("cd @tibet", &tables(), WIN).unwrap();
        assert_eq!(e.backspaces, 6);
        assert_eq!(e.text, "\"C:\\Users\\Nihil\\Desktop\\Free Tibet\"");
    }

    #[test]
    fn the_same_sigil_spells_itself_differently_per_shell() {
        let e = on_space("cd @tibet", &tables(), WSL).unwrap();
        assert_eq!(e.text, "\"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet')\"");
    }

    #[test]
    fn an_alias_expands_only_as_the_first_word() {
        let t = tables();
        assert_eq!(on_space("gs", &t, WIN).unwrap().text, "git status");
        // Otherwise `echo gs` would print "git status".
        assert!(on_space("echo gs", &t, WIN).is_none());
    }

    #[test]
    fn a_sigil_expands_anywhere_in_the_line() {
        // Unlike an alias, a path is a path wherever it appears.
        let t = tables();
        assert!(on_space("cp @nyx", &t, WIN).is_some());
        assert_eq!(expand_line("cp @nyx @tibet", &t, WIN).matches('"').count(), 4);
    }

    #[test]
    fn an_unknown_token_expands_to_nothing() {
        let t = tables();
        assert!(on_space("cd @nowhere", &t, WIN).is_none());
        assert!(on_space("cd", &t, WIN).is_none());
        assert!(on_space("", &t, WIN).is_none());
    }

    #[test]
    fn a_trailing_space_means_there_is_nothing_to_complete() {
        // Otherwise typing two spaces would expand the same token twice.
        assert!(on_space("cd @tibet ", &tables(), WIN).is_none());
    }

    #[test]
    fn submit_rewrites_the_whole_line_and_preserves_spacing() {
        let t = tables();
        let e = on_submit("gs   --short", &t, WIN).unwrap();
        assert_eq!(e.backspaces, "gs   --short".chars().count());
        // The three spaces the user typed are still three spaces.
        assert_eq!(e.text, "git status   --short");
    }

    #[test]
    fn submit_on_a_line_with_nothing_to_do_returns_none() {
        // Otherwise every Enter would erase and retype the line for no reason.
        assert!(on_submit("echo hello", &tables(), WIN).is_none());
    }

    #[test]
    fn submit_expands_an_alias_and_a_sigil_in_one_pass() {
        let t = tables();
        let e = on_submit("ll @tibet", &t, WSL).unwrap();
        assert_eq!(e.text, "ls -la \"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet')\"");
    }

    #[test]
    fn backspaces_are_counted_in_characters_not_bytes() {
        // The shell echoed characters, so that is what a backspace removes. A byte count would
        // over-erase and eat the prompt.
        let t = tables();
        let e = on_submit("échò @tibet", &t, WIN).unwrap();
        assert_eq!(e.backspaces, 11);
        assert_ne!(e.backspaces, "échò @tibet".len());
    }
}
