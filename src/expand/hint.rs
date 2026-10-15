//! One dim sentence saying what the line you are typing is about to do.
//!
//! Pure, and separate from `complete` on purpose: the menu answers "which sigils are still
//! reachable", this answers "what does the thing you have already typed mean". They are shown
//! together and they are two different questions — a finished `@nyx` has no menu and a hint,
//! a half-typed `@n` has a menu and no hint.

use crate::expand::complete;
use crate::expand::table::Tables;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// The line is *nothing but* a sigil, so Enter opens it rather than running it.
    OpensTab { label: String, path: String },
    /// A finished sigil somewhere in the line: this is the path it stands for.
    Path { label: String, path: String },
    Alias { name: String, expansion: String },
    Builtin { name: String, text: &'static str },
}

impl Hint {
    /// The sentence, as drawn.
    pub fn text(&self) -> String {
        match self {
            Hint::OpensTab { label, path } => format!("↵ opens {label} in a new tab — {path}"),
            Hint::Path { label, path } => format!("{label} — {path}"),
            Hint::Alias { name, expansion } => format!("{name} → {expansion}"),
            Hint::Builtin { name, text } => format!("{name} — {text}"),
        }
    }
}

/// What to say about `line`, if anything.
///
/// The order matters and is the whole design: the most specific claim wins, and the most specific
/// claim is also the one with a consequence attached. "Enter opens a tab" outranks "this is a
/// path" because it is the thing about to happen, not the thing on screen.
pub fn for_line(line: &str, tables: &Tables) -> Option<Hint> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(shortcut) = tables.find_sigil(trimmed) {
        return Some(Hint::OpensTab {
            label: shortcut.label.clone(),
            path: shortcut.path.clone(),
        });
    }

    // The token under the cursor rather than any sigil in the line: the one being typed is the
    // one being asked about, and describing an argument three words back is noise.
    if let Some(shortcut) = complete::trailing_token(line).and_then(|t| tables.find_sigil(t)) {
        return Some(Hint::Path { label: shortcut.label.clone(), path: shortcut.path.clone() });
    }

    // Past the first word we are looking at arguments, and this module has nothing true to say
    // about those — `git status --short` is not explained by explaining `git`.
    let mut words = trimmed.split_whitespace();
    let word = words.next()?;
    if words.next().is_some() {
        return None;
    }

    if let Some(expansion) = tables.find_alias(word) {
        return Some(Hint::Alias { name: word.to_string(), expansion: expansion.to_string() });
    }
    builtin(word).map(|text| Hint::Builtin { name: word.to_string(), text })
}

/// What a command is, for the handful of commands where the name does not say.
///
/// Curated, and meant to stay that way. The rule for adding a row: it is a shell builtin or a
/// stock Windows command, its name does not already explain it, and the description fits on one
/// line beside a prompt. A table that tries to be a manual page is one nobody reads, and a table
/// that grows to a thousand rows is a dependency wearing a `&[(&str, &str)]` costume.
///
/// Sorted, because the lookup binary-searches it and a test asserts the ordering — an entry
/// appended in the wrong place would otherwise be findable or not depending on where it landed.
pub fn builtin(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    TABLE.binary_search_by(|(k, _)| k.cmp(&lower.as_str())).ok().map(|i| TABLE[i].1)
}

#[rustfmt::skip]
const TABLE: &[(&str, &str)] = &[
    ("attrib",   "show or change a file's read-only, hidden and system attributes"),
    ("cat",      "print a file to the screen"),
    ("cd",       "change the working directory"),
    ("chdir",    "change the working directory"),
    ("chmod",    "change a file's permission bits"),
    ("clear",    "clear the screen"),
    ("cls",      "built-in: clear the screen"),
    ("copy",     "copy files"),
    ("curl",     "fetch a URL"),
    ("del",      "delete files, without sending them to the Recycle Bin"),
    ("dir",      "built-in: list the directory's contents"),
    ("echo",     "print its arguments"),
    ("erase",    "delete files, without sending them to the Recycle Bin"),
    ("exit",     "end this shell, closing the pane"),
    ("find",     "MSYS: walk a directory tree. cmd: search files for a literal string"),
    ("findstr",  "search files for a pattern"),
    ("gc",       "Get-Content: print a file to the screen"),
    ("gci",      "Get-ChildItem: list the directory's contents"),
    ("gcm",      "Get-Command: find what a name resolves to"),
    ("gl",       "Get-Location: print the working directory"),
    ("grep",     "search input for a pattern"),
    ("head",     "print the first lines of a file"),
    ("kill",     "signal a process to stop"),
    ("less",     "page through a file"),
    ("ls",       "list the directory's contents"),
    ("md",       "create a directory"),
    ("mkdir",    "create a directory"),
    ("more",     "page through a file"),
    ("move",     "move or rename files"),
    ("mv",       "move or rename files"),
    ("ping",     "check whether a host answers"),
    ("popd",     "return to the directory the last pushd came from"),
    ("pushd",    "change directory, remembering the one you left"),
    ("pwd",      "print the working directory"),
    ("rd",       "remove a directory"),
    ("ren",      "rename a file"),
    ("rename",   "rename a file"),
    ("rm",       "delete files"),
    ("rmdir",    "remove a directory"),
    ("robocopy", "copy directory trees, resumably"),
    ("set",      "built-in: show or set environment variables"),
    ("sl",       "Set-Location: change the working directory"),
    ("sls",      "Select-String: search input for a pattern"),
    ("start",    "open a file or URL with whatever handles it"),
    ("tail",     "print the last lines of a file"),
    ("taskkill", "end a running process"),
    ("tasklist", "list running processes"),
    ("touch",    "create an empty file, or update its timestamp"),
    ("tree",     "draw the directory tree"),
    ("type",     "cmd: print a file to the screen. bash: say what a name resolves to"),
    ("ver",      "print the Windows version"),
    ("where",    "find which file a command name resolves to"),
    ("which",    "find which file a command name resolves to"),
    ("whoami",   "print the current user"),
    ("xcopy",    "copy files and directory trees"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{AliasMap, ShortcutConfig};

    fn tables() -> Tables {
        let shortcuts = vec![ShortcutConfig {
            id: "nyx".into(),
            label: "Nyx".into(),
            sigil: "@nyx".into(),
            path: r"C:\Users\Nihil\Desktop\Free Tibet\Nyx".into(),
            icon: String::new(),
            color: None,
            order: 0,
        }];
        let mut aliases = AliasMap::new();
        aliases.insert("gs".into(), "git status".into());
        Tables::from_config(&shortcuts, &aliases).0
    }

    #[test]
    fn the_table_is_sorted_so_the_binary_search_can_find_every_row() {
        // Without this an entry added in the wrong place is findable or not depending on where
        // it happened to land, which reads as "the hint works for some commands".
        let mut sorted = TABLE.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(TABLE, sorted.as_slice());
        for (name, _) in TABLE {
            assert_eq!(builtin(name), Some(TABLE.iter().find(|(k, _)| k == name).unwrap().1));
        }
    }

    #[test]
    fn a_command_is_looked_up_however_it_is_capitalised() {
        assert!(builtin("CLS").is_some());
        assert_eq!(builtin("Cls"), builtin("cls"));
    }

    #[test]
    fn an_unknown_command_says_nothing_rather_than_guessing() {
        assert_eq!(builtin("cargo"), None);
        assert_eq!(for_line("cargo", &tables()), None);
    }

    #[test]
    fn a_whole_line_sigil_promises_a_tab() {
        let hint = for_line("@nyx", &tables()).unwrap();
        assert!(matches!(hint, Hint::OpensTab { .. }));
        assert!(hint.text().contains("Nyx"), "{}", hint.text());
    }

    #[test]
    fn the_same_sigil_in_a_command_promises_only_a_path() {
        // The consequence is different, so the sentence has to be: Enter here runs a cd.
        let hint = for_line("cd @nyx", &tables()).unwrap();
        assert!(matches!(hint, Hint::Path { .. }), "{hint:?}");
    }

    #[test]
    fn surrounding_whitespace_does_not_change_what_a_line_is() {
        assert_eq!(for_line("  @nyx  ", &tables()), for_line("@nyx", &tables()));
    }

    #[test]
    fn an_alias_explains_itself_from_the_config() {
        let hint = for_line("gs", &tables()).unwrap();
        assert_eq!(hint, Hint::Alias { name: "gs".into(), expansion: "git status".into() });
    }

    #[test]
    fn a_builtin_is_explained_from_the_table() {
        assert!(matches!(for_line("cls", &tables()), Some(Hint::Builtin { .. })));
    }

    #[test]
    fn arguments_are_not_explained_by_explaining_the_command() {
        // "dir" is in the table; "dir /s C:\" is a different thing and we have nothing true to
        // say about it, so saying what `dir` alone means would be a claim about the wrong line.
        assert_eq!(for_line("dir /s", &tables()), None);
    }

    #[test]
    fn an_empty_line_says_nothing() {
        assert_eq!(for_line("", &tables()), None);
        assert_eq!(for_line("   ", &tables()), None);
    }
}
