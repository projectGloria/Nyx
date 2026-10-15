//! What to launch, and what kind of shell it is.
//!
//! `ProfileKind` looks redundant next to the program path, but it is what Phase 5 uses to decide
//! how a path shortcut should be spelled (`C:\x`, `/c/x`, `/mnt/c/x`) and what Phase 6 uses to
//! pick a shell-integration script. Deriving that from the executable name later would mean
//! re-deciding it in several places.

use std::path::{Path, PathBuf};

use crate::term::spawn::ShellSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    Cmd,
    PowerShell,
    Bash,
    Wsl,
}

impl ProfileKind {
    /// How a Windows path has to be written for this shell.
    pub fn path_style(self) -> PathStyle {
        match self {
            ProfileKind::Cmd => PathStyle::Windows,
            ProfileKind::PowerShell => PathStyle::PowerShell,
            ProfileKind::Bash => PathStyle::Msys,
            ProfileKind::Wsl => PathStyle::Wsl,
        }
    }

    /// What this kind is called in `config.toml`, and in the settings GUI's dropdown.
    pub fn name(self) -> &'static str {
        match self {
            ProfileKind::Cmd => "cmd",
            ProfileKind::PowerShell => "powershell",
            ProfileKind::Bash => "bash",
            ProfileKind::Wsl => "wsl",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "cmd" | "cmd.exe" => ProfileKind::Cmd,
            "powershell" | "pwsh" | "ps" => ProfileKind::PowerShell,
            "bash" | "msys" | "git-bash" | "gitbash" => ProfileKind::Bash,
            "wsl" | "linux" => ProfileKind::Wsl,
            _ => return None,
        })
    }

    /// Guess from an executable's file name.
    ///
    /// Returns `None` for anything unrecognised rather than falling back, because the caller uses
    /// "we recognised this" as the condition for injecting an init script: guessing `powershell`
    /// for an arbitrary program appends `-NoExit -Command . script.ps1` to something that has
    /// never heard of either flag, and the pane opens onto an error instead of a shell.
    pub fn infer(program: &Path) -> Option<Self> {
        let file = program.file_stem()?.to_string_lossy().to_ascii_lowercase();
        Some(match file.as_str() {
            "cmd" => ProfileKind::Cmd,
            "powershell" | "pwsh" => ProfileKind::PowerShell,
            "bash" | "sh" | "zsh" | "fish" => ProfileKind::Bash,
            "wsl" | "wsl.exe" | "ubuntu" | "debian" => ProfileKind::Wsl,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// `C:\Users\Nihil`
    Windows,
    /// Windows paths with literal PowerShell quoting.
    PowerShell,
    /// `/c/Users/Nihil` — Git Bash.
    Msys,
    /// `/mnt/c/Users/Nihil`
    Wsl,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub kind: ProfileKind,
    /// Where a tab running this profile starts. `None` means the user's home directory.
    pub cwd: Option<PathBuf>,
    /// Whether a generated init script may be injected. False for an unrecognised program, and
    /// for anything the user marked `integration = "none"`.
    pub integration: bool,
}

/// How to start a shell, and whether an init script went with it.
pub struct Launch {
    pub spec: ShellSpec,
    /// True when a generated script was actually handed to the shell. Drives whether the pane
    /// waits for a greeting or falls straight back to the terminal-side layer.
    pub injected: bool,
    pub token: String,
}

impl Profile {
    pub fn to_spec(&self, cwd: Option<PathBuf>) -> ShellSpec {
        ShellSpec {
            program: self.program.to_string_lossy().into_owned(),
            args: self.args.clone(),
            // The profile's own directory wins over the caller's default; the caller's is only a
            // fallback for profiles that did not name one.
            cwd: self.cwd.clone().or(cwd),
            env: Default::default(),
        }
    }

    /// Whether launching this profile will actually inject an init script.
    ///
    /// Deliberately not the same question as the `integration` flag, which only says the user
    /// allowed it. WSL has no script to inject — we do not write into someone else's distro — and
    /// cmd.exe has no scriptable line editor without Clink. The settings window shows *this*,
    /// because a row claiming "shell-side expansion" while the pane quietly uses the fallback is
    /// worse than no indicator at all.
    pub fn injects(&self, clink: Option<&Path>) -> bool {
        self.integration
            && crate::expand::shellgen::available(self.kind).is_some()
            && (self.kind != ProfileKind::Cmd || clink.is_some())
    }

    /// Build the launch, injecting shell integration where there is a script for it.
    ///
    /// `clink` is the path to `clink.bat` when it was detected; without it a cmd.exe profile
    /// simply launches plain and the terminal-side layer covers it.
    pub fn launch(
        &self,
        pane: u64,
        cwd: Option<PathBuf>,
        clink: Option<&Path>,
    ) -> Launch {
        use crate::expand::handshake;
        use crate::expand::shellgen;

        let mut spec = self.to_spec(cwd);
        let token = handshake::new_token(pane);
        let mut injected = false;

        // One condition, shared with what the settings window displays: a pane that says it has
        // shell-side expansion and a pane that gets it must never be able to disagree.
        let script = self.injects(clink).then(|| shellgen::available(self.kind)).flatten();

        match (self.kind, script) {
            (ProfileKind::PowerShell, Some(script)) => {
                // `-Command` swallows everything after it, so it has to come last. `-NoExit`
                // keeps the session alive once the dot-source finishes.
                spec.args.push("-NoExit".into());
                spec.args.push("-Command".into());
                spec.args.push(format!(". {}", shellgen::ps_single_quote(&script)));
                injected = true;
            },
            (ProfileKind::Cmd, Some(_)) => {
                if let Some(clink) = clink {
                    // Clink finds our Lua through CLINK_PATH, set below.
                    spec.args.push("/k".into());
                    spec.args.push(clink.to_string_lossy().into_owned());
                    spec.args.push("inject".into());
                    spec.args.push("--quiet".into());
                    injected = true;
                }
            },
            (ProfileKind::Bash, Some(script)) => {
                // --rcfile replaces ~/.bashrc entirely; the script sources the user's own files
                // first so nothing is lost.
                let mut args = vec!["--rcfile".to_string(), script.to_string_lossy().into_owned()];
                args.append(&mut spec.args);
                spec.args = args;
                injected = true;
            },
            // WSL gets nothing: we do not write into someone else's distro.
            _ => {},
        }

        if injected {
            spec.env.insert("NYX_TOKEN".into(), token.clone());
            spec.env.insert(
                "NYX_SHORTCUTS".into(),
                shellgen::shortcut_file(self.kind.path_style()).to_string_lossy().into_owned(),
            );
            spec.env.insert(
                "NYX_ALIASES".into(),
                shellgen::alias_file().to_string_lossy().into_owned(),
            );
            if self.kind == ProfileKind::Cmd {
                spec.env.insert(
                    "CLINK_PATH".into(),
                    shellgen::shell_dir().join("clink").to_string_lossy().into_owned(),
                );
            }
        }

        Launch { spec, injected, token }
    }
}

/// Layer the user's `[[profile]]` entries over what detection found.
///
/// An entry whose `id` matches a detected shell edits it in place, keeping its position in the
/// list and every field it did not name. An entry with a new id is appended. Entries are
/// overrides rather than replacements because a profile that only wanted to add `-NoLogo` should
/// not stop working when a Windows update moves `powershell.exe`.
pub fn resolve(
    detected: &[Profile],
    configured: &[crate::config::schema::ProfileConfig],
) -> (Vec<Profile>, Vec<String>) {
    let mut problems = Vec::new();
    let mut out: Vec<Profile> = detected.to_vec();
    let mut disabled: Vec<String> = Vec::new();

    for (i, cfg) in configured.iter().enumerate() {
        let id = cfg.id.trim();
        if id.is_empty() {
            problems.push(format!("profile #{} has no id", i + 1));
            continue;
        }

        if !cfg.enabled {
            disabled.push(id.to_string());
            continue;
        }

        let existing = out.iter().position(|p| p.id == id);
        let program = if cfg.program.trim().is_empty() {
            match existing.map(|i| out[i].program.clone()) {
                Some(p) => p,
                None => {
                    problems.push(format!(
                        "profile \"{id}\": no program, and nothing detected under that id"
                    ));
                    continue;
                },
            }
        } else {
            PathBuf::from(cfg.program.trim())
        };

        // An explicit kind means the user vouched for it, so integration is allowed. An inferred
        // one only counts when the file name was actually recognised.
        let (kind, recognised) = match ProfileKind::parse(&cfg.kind) {
            Some(kind) => (kind, true),
            None if !cfg.kind.trim().is_empty() => {
                problems.push(format!(
                    "profile \"{id}\": kind \"{}\" is not cmd, powershell, bash or wsl",
                    cfg.kind
                ));
                (ProfileKind::Cmd, false)
            },
            None => match existing.map(|i| out[i].kind) {
                Some(kind) => (kind, true),
                None => match ProfileKind::infer(&program) {
                    Some(kind) => (kind, true),
                    None => (ProfileKind::Cmd, false),
                },
            },
        };

        let integration = match cfg.integration.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "false" => false,
            "auto" | "" => recognised,
            other => {
                problems.push(format!(
                    "profile \"{id}\": integration \"{other}\" is not auto or none"
                ));
                recognised
            },
        };

        let profile = Profile {
            name: if cfg.name.trim().is_empty() {
                existing.map(|i| out[i].name.clone()).unwrap_or_else(|| id.to_string())
            } else {
                cfg.name.trim().to_string()
            },
            args: match &cfg.args {
                Some(args) => args.clone(),
                None => existing.map(|i| out[i].args.clone()).unwrap_or_default(),
            },
            cwd: if cfg.cwd.trim().is_empty() {
                existing.and_then(|i| out[i].cwd.clone())
            } else {
                Some(PathBuf::from(cfg.cwd.trim()))
            },
            id: id.to_string(),
            program,
            kind,
            integration,
        };

        match existing {
            Some(i) => out[i] = profile,
            None => out.push(profile),
        }
    }

    out.retain(|p| !disabled.iter().any(|id| id == &p.id));
    (out, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::ProfileConfig;

    fn detected() -> Vec<Profile> {
        vec![
            Profile {
                id: "powershell".into(),
                name: "Windows PowerShell".into(),
                program: PathBuf::from(r"C:\Windows\System32\powershell.exe"),
                args: vec!["-NoLogo".into()],
                kind: ProfileKind::PowerShell,
                cwd: None,
                integration: true,
            },
            Profile {
                id: "cmd".into(),
                name: "Command Prompt".into(),
                program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
                args: Vec::new(),
                kind: ProfileKind::Cmd,
                cwd: None,
                integration: true,
            },
        ]
    }

    fn cfg(id: &str) -> ProfileConfig {
        ProfileConfig { id: id.into(), ..Default::default() }
    }

    #[test]
    fn an_override_keeps_the_fields_it_did_not_name() {
        // The whole point: adding an argument must not require restating the path, or a Windows
        // update that moves powershell.exe breaks the profile.
        let mut c = cfg("powershell");
        c.args = Some(vec!["-NoLogo".into(), "-NoProfile".into()]);
        let (profiles, problems) = resolve(&detected(), &[c]);

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].args, vec!["-NoLogo", "-NoProfile"]);
        assert_eq!(profiles[0].program, PathBuf::from(r"C:\Windows\System32\powershell.exe"));
        assert_eq!(profiles[0].name, "Windows PowerShell");
        // And it stays where it was, rather than jumping to the end of the menu.
        assert_eq!(profiles[1].id, "cmd");
    }

    #[test]
    fn an_empty_args_list_means_none_rather_than_inherit() {
        // `args = []` is the only way to say "launch it bare", so it has to be distinguishable
        // from the key being absent.
        let mut c = cfg("powershell");
        c.args = Some(Vec::new());
        let (profiles, _) = resolve(&detected(), &[c]);
        assert!(profiles[0].args.is_empty());
    }

    #[test]
    fn a_new_id_is_appended_and_infers_its_kind() {
        let mut c = cfg("dev");
        c.name = "Dev shell".into();
        c.program = r"C:\Program Files\Git\bin\bash.exe".into();
        let (profiles, problems) = resolve(&detected(), &[c]);

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[2].kind, ProfileKind::Bash);
        assert!(profiles[2].integration);
    }

    #[test]
    fn an_unrecognised_program_never_gets_an_init_script_injected() {
        // `-NoExit -Command . nyx-init.ps1` appended to something that has never heard of either
        // flag opens the pane onto an error instead of a shell.
        let mut c = cfg("python");
        c.program = r"C:\Python\python.exe".into();
        let (profiles, _) = resolve(&detected(), &[c]);
        assert!(!profiles[2].integration);
        assert_eq!(profiles[2].kind, ProfileKind::Cmd, "path style still has to be something");
    }

    #[test]
    fn an_explicit_kind_is_trusted_even_when_the_name_says_nothing() {
        let mut c = cfg("custom");
        c.program = r"C:\tools\myshell.exe".into();
        c.kind = "powershell".into();
        let (profiles, _) = resolve(&detected(), &[c]);
        assert_eq!(profiles[2].kind, ProfileKind::PowerShell);
        assert!(profiles[2].integration);
    }

    #[test]
    fn integration_none_turns_it_off_for_a_shell_that_would_have_had_it() {
        let mut c = cfg("powershell");
        c.integration = "none".into();
        let (profiles, problems) = resolve(&detected(), &[c]);
        assert!(problems.is_empty());
        assert!(!profiles[0].integration);
    }

    #[test]
    fn disabling_a_detected_profile_removes_it_from_the_menu() {
        let mut c = cfg("cmd");
        c.enabled = false;
        let (profiles, _) = resolve(&detected(), &[c]);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, "powershell");
    }

    #[test]
    fn a_profile_with_no_program_and_nothing_detected_is_reported() {
        let (profiles, problems) = resolve(&detected(), &[cfg("nonexistent")]);
        assert_eq!(profiles.len(), 2);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("nonexistent"), "{}", problems[0]);
    }

    #[test]
    fn a_bad_kind_is_named_rather_than_silently_becoming_cmd() {
        let mut c = cfg("x");
        c.program = r"C:\x.exe".into();
        c.kind = "fish-shell".into();
        let (_, problems) = resolve(&detected(), &[c]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("fish-shell"));
    }

    #[test]
    fn a_profile_cwd_wins_over_the_callers_default() {
        let mut p = detected().remove(0);
        p.cwd = Some(PathBuf::from(r"C:\work"));
        let spec = p.to_spec(Some(PathBuf::from(r"C:\Users\Nihil")));
        assert_eq!(spec.cwd, Some(PathBuf::from(r"C:\work")));
    }
}
