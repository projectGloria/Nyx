//! Which sigils are still reachable from a half-typed token.
//!
//! Pure, and deliberately only that: whether the line is knowable at all is `Shadow`'s question,
//! which row is highlighted is `ui::complete`'s, and what a match *means* is `hint`'s. Keeping
//! the matching rule on its own is what lets it be tested without a terminal, a font or a shell.

use crate::expand::line::Expansion;
use crate::expand::table::Tables;

/// The sigils a partially typed token could still become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// What has been typed so far, `@n`.
    pub token: String,
    /// Indices into `Tables::shortcuts`, in the table's own order so the list does not reorder
    /// itself under the user as they type.
    pub hits: Vec<usize>,
}

/// The token the cursor is sitting at the end of, or `None` when the line ends in whitespace.
///
/// A trailing space means the token is finished and `line::on_space` has already had its turn at
/// it; there is nothing left to complete and a menu would be offering to re-type what is already
/// there.
pub fn trailing_token(line: &str) -> Option<&str> {
    let token = line.rsplit(char::is_whitespace).next()?;
    (!token.is_empty()).then_some(token)
}

/// What to offer for the token at the end of `line`.
pub fn menu(line: &str, tables: &Tables) -> Option<Menu> {
    let token = trailing_token(line)?;

    // The menu opens on punctuation only. A sigil is allowed to be any ASCII word, so a user who
    // spells one `work` would otherwise get a popup over the terminal every time they typed a `w`
    // at a prompt — one keystroke into a word that is far more likely to be `whoami`. The letters
    // *after* that first character are matched normally, which is what makes `@n` a search.
    if !token.starts_with(|c: char| c.is_ascii_punctuation()) {
        return None;
    }

    let lower = token.to_ascii_lowercase();
    let hits: Vec<usize> = tables
        .shortcuts
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sigil.to_ascii_lowercase().starts_with(&lower))
        .map(|(i, _)| i)
        .collect();

    if hits.is_empty() {
        return None;
    }
    // One exact match needs no menu: `hint` already says what that sigil will do, and a one-row
    // list under a finished token is a box that appears only to shut again on the next keystroke.
    if hits.len() == 1 && tables.shortcuts[hits[0]].sigil.eq_ignore_ascii_case(token) {
        return None;
    }
    Some(Menu { token: token.to_string(), hits })
}

/// The rewrite that turns the partial token into a whole sigil.
///
/// An `Expansion` rather than a string because accepting a completion and expanding a sigil are
/// the same operation to everything downstream — the same backspaces, the same bytes, the same
/// `Shadow::apply`. A second way to edit the input line is a second way to get the count wrong.
pub fn accept(menu: &Menu, tables: &Tables, row: usize) -> Option<Expansion> {
    let shortcut = tables.shortcuts.get(*menu.hits.get(row)?)?;
    Some(Expansion {
        backspaces: menu.token.chars().count(),
        text: shortcut.sigil.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{AliasMap, ShortcutConfig};

    fn cfg(id: &str, sigil: &str) -> ShortcutConfig {
        ShortcutConfig {
            id: id.into(),
            label: id.into(),
            sigil: sigil.into(),
            path: format!(r"C:\{id}"),
            icon: String::new(),
            color: None,
            order: 0,
        }
    }

    fn tables() -> Tables {
        Tables::from_config(
            &[cfg("nihil", "@nihil"), cfg("nyx", "@nyx"), cfg("tibet", "@tibet")],
            &AliasMap::new(),
        )
        .0
    }

    fn sigils(line: &str) -> Vec<String> {
        let t = tables();
        match menu(line, &t) {
            Some(m) => m.hits.iter().map(|&i| t.shortcuts[i].sigil.clone()).collect(),
            None => Vec::new(),
        }
    }

    #[test]
    fn a_bare_sigil_character_offers_everything() {
        assert_eq!(sigils("@"), vec!["@nihil", "@nyx", "@tibet"]);
    }

    #[test]
    fn a_letter_narrows_it() {
        assert_eq!(sigils("@n"), vec!["@nihil", "@nyx"]);
        assert_eq!(sigils("@ny"), vec!["@nyx"]);
    }

    #[test]
    fn narrowing_is_case_insensitive_because_the_sigils_are_not_shouted() {
        // The whole point of "@N" is that it is a search, and nobody holds shift on purpose.
        assert_eq!(sigils("@N"), vec!["@nihil", "@nyx"]);
    }

    #[test]
    fn it_is_the_last_token_that_is_being_completed() {
        assert_eq!(sigils("cd @n"), vec!["@nihil", "@nyx"]);
    }

    #[test]
    fn a_finished_token_offers_nothing() {
        // The space has already been typed, so `on_space` has expanded it or decided not to.
        assert_eq!(sigils("cd @nyx "), Vec::<String>::new());
        assert_eq!(trailing_token("cd @nyx "), None);
    }

    #[test]
    fn an_exact_single_match_closes_the_menu_rather_than_showing_one_row() {
        assert_eq!(sigils("@tibet"), Vec::<String>::new());
        // ...but an exact match that is still a prefix of others keeps the list, because the
        // longer ones are still reachable.
        assert_eq!(sigils("@ny"), vec!["@nyx"]);
    }

    #[test]
    fn an_ordinary_word_never_opens_a_menu() {
        // The defect this prevents: a sigil spelled without punctuation would put a popup over
        // the terminal on the first letter of every command.
        let t = Tables::from_config(&[cfg("work", "work")], &AliasMap::new()).0;
        assert_eq!(menu("w", &t), None);
        assert_eq!(menu("whoami", &t), None);
    }

    #[test]
    fn a_token_matching_nothing_offers_nothing() {
        assert_eq!(sigils("@zz"), Vec::<String>::new());
        assert_eq!(sigils("./configure"), Vec::<String>::new());
    }

    #[test]
    fn accepting_erases_exactly_what_was_typed() {
        let t = tables();
        let m = menu("cd @n", &t).unwrap();
        let e = accept(&m, &t, 1).unwrap();
        assert_eq!(e.backspaces, 2, "@n is two characters");
        assert_eq!(e.text, "@nyx");
    }

    #[test]
    fn accepting_a_row_that_is_not_there_changes_nothing() {
        let t = tables();
        let m = menu("@n", &t).unwrap();
        assert_eq!(accept(&m, &t, 99), None);
    }
}
