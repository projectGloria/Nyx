//! The compiled shortcut and alias tables.

use crate::config::schema::{AliasMap, ShortcutConfig};
use crate::term::profile::PathStyle;
use crate::win::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub id: String,
    pub label: String,
    /// The token typed on the command line, including its sigil: `@tibet`.
    pub sigil: String,
    pub path: String,
    pub icon: String,
    pub color: Option<String>,
    pub order: i32,
}

impl Shortcut {
    /// The replacement text for this shortcut in a given shell.
    pub fn expansion(&self, style: PathStyle) -> String {
        paths::for_shell(&self.path, style)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tables {
    pub shortcuts: Vec<Shortcut>,
    /// First-word aliases: `gs` -> `git status`.
    pub aliases: Vec<(String, String)>,
}

impl Tables {
    pub fn from_config(shortcuts: &[ShortcutConfig], aliases: &AliasMap) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut out = Vec::new();

        for (i, cfg) in shortcuts.iter().enumerate() {
            if cfg.path.trim().is_empty() {
                problems.push(format!("shortcut #{} has no path", i + 1));
                continue;
            }
            let id = if cfg.id.trim().is_empty() {
                problems.push(format!("shortcut #{} has no id", i + 1));
                continue;
            } else {
                cfg.id.trim().to_string()
            };

            // A sigil is a token typed at a command line, so it has to be one word and it has to
            // be ASCII: expansion works by sending backspaces, and counting those correctly for
            // multi-byte input is not something a shell's echo makes possible to verify.
            let sigil = if cfg.sigil.trim().is_empty() {
                format!("@{id}")
            } else {
                cfg.sigil.trim().to_string()
            };
            if sigil.contains(char::is_whitespace) || !sigil.is_ascii() {
                problems.push(format!(
                    "shortcut \"{id}\": sigil \"{sigil}\" must be one ASCII word"
                ));
                continue;
            }

            out.push(Shortcut {
                label: if cfg.label.trim().is_empty() {
                    id.clone()
                } else {
                    cfg.label.clone()
                },
                id,
                sigil,
                path: cfg.path.clone(),
                icon: if cfg.icon.trim().is_empty() {
                    "builtin:folder".into()
                } else {
                    cfg.icon.clone()
                },
                color: cfg.color.clone().filter(|c| !c.trim().is_empty()),
                order: cfg.order,
            });
        }

        // The order the user gave, then by label. Lookup is by exact match so nothing here can
        // shadow anything — but this *is* the order the completion menu offers, and a list that
        // reordered itself as it narrowed would move the row under the highlight.
        out.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });

        let mut alias_list: Vec<(String, String)> = aliases
            .iter()
            .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        alias_list.sort_by(|a, b| a.0.cmp(&b.0));

        (
            Tables {
                shortcuts: out,
                aliases: alias_list,
            },
            problems,
        )
    }

    pub fn find_sigil(&self, token: &str) -> Option<&Shortcut> {
        self.shortcuts
            .iter()
            .find(|s| s.sigil.eq_ignore_ascii_case(token))
    }

    pub fn find_alias(&self, word: &str) -> Option<&str> {
        self.aliases
            .iter()
            .find(|(k, _)| k == word)
            .map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, path: &str) -> ShortcutConfig {
        ShortcutConfig {
            id: id.into(),
            label: String::new(),
            sigil: String::new(),
            path: path.into(),
            icon: String::new(),
            color: None,
            order: 0,
        }
    }

    #[test]
    fn exact_sigil_lookup_ignores_ascii_case() {
        let mut shortcut = cfg("nyx", r"C:\Nyx");
        shortcut.sigil = "@NyX".into();
        let (tables, _) = Tables::from_config(&[shortcut], &Default::default());
        for token in ["@nyx", "@NYX", "@NyX"] {
            assert_eq!(tables.find_sigil(token).unwrap().id, "nyx");
        }
    }

    #[test]
    fn a_missing_sigil_defaults_to_the_id() {
        let (t, problems) = Tables::from_config(&[cfg("tibet", r"C:\x")], &Default::default());
        assert!(problems.is_empty());
        assert_eq!(t.shortcuts[0].sigil, "@tibet");
        assert_eq!(t.shortcuts[0].label, "tibet");
        assert_eq!(t.shortcuts[0].icon, "builtin:folder");
    }

    #[test]
    fn a_shortcut_without_a_path_is_reported_not_silently_dropped() {
        let (t, problems) = Tables::from_config(&[cfg("broken", "")], &Default::default());
        assert!(t.shortcuts.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("path"));
    }

    #[test]
    fn a_sigil_with_a_space_is_refused() {
        // Expansion replaces the token before the cursor, and a token cannot contain a space.
        let mut c = cfg("x", r"C:\x");
        c.sigil = "@my folder".into();
        let (t, problems) = Tables::from_config(&[c], &Default::default());
        assert!(t.shortcuts.is_empty());
        assert!(problems[0].contains("one ASCII word"));
    }

    #[test]
    fn a_non_ascii_sigil_is_refused() {
        // Backspace counting against a shell's echo is only sound for ASCII.
        let mut c = cfg("x", r"C:\x");
        c.sigil = "@café".into();
        let (t, problems) = Tables::from_config(&[c], &Default::default());
        assert!(t.shortcuts.is_empty());
        assert!(problems[0].contains("one ASCII word"));
    }

    #[test]
    fn expansion_is_per_shell() {
        let (t, _) = Tables::from_config(
            &[cfg("tibet", r"C:\Users\Nihil\Desktop\Free Tibet")],
            &Default::default(),
        );
        let s = &t.shortcuts[0];
        assert_eq!(
            s.expansion(PathStyle::Windows),
            "\"C:\\Users\\Nihil\\Desktop\\Free Tibet\""
        );
        assert_eq!(
            s.expansion(PathStyle::Msys),
            "'/c/Users/Nihil/Desktop/Free Tibet'"
        );
        assert_eq!(
            s.expansion(PathStyle::Wsl),
            "\"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet')\""
        );
    }

    #[test]
    fn ordering_follows_order_then_label() {
        let mut a = cfg("a", r"C:\a");
        a.order = 20;
        a.label = "Zebra".into();
        let mut b = cfg("b", r"C:\b");
        b.order = 10;
        b.label = "Apple".into();
        let (t, _) = Tables::from_config(&[a, b], &Default::default());
        assert_eq!(t.shortcuts[0].label, "Apple");
        assert_eq!(t.shortcuts[1].label, "Zebra");
    }

    #[test]
    fn aliases_skip_blank_entries() {
        let mut map = AliasMap::new();
        map.insert("gs".into(), "git status".into());
        map.insert("empty".into(), "  ".into());
        let (t, _) = Tables::from_config(&[], &map);
        assert_eq!(t.find_alias("gs"), Some("git status"));
        assert_eq!(t.find_alias("empty"), None);
    }
}
