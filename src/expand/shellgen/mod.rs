//! Generating the init scripts that give a shell native expansion.
//!
//! **The shortcut table reaches the scripts as pre-translated data, never as logic.** Path
//! translation and quoting happen once, in `win::paths`, and each script reads a two-column TSV.
//! A Lua, a PowerShell and a bash implementation of the same conversion would mean two of them
//! being quietly different, and the symptom is a shortcut that points somewhere else in one
//! shell only.
//!
//! Everything is written atomically under `%LOCALAPPDATA%\Nyx\shell\`, never beside the exe.

use std::path::{Path, PathBuf};

use crate::expand::Tables;
use crate::term::profile::{PathStyle, ProfileKind};
use crate::util::write_atomic;

mod bash;
mod clink;
mod powershell;

/// Where the generated files live.
pub fn shell_dir() -> PathBuf {
    crate::config::paths::data_dir().join("shell")
}

fn data_dir() -> PathBuf {
    shell_dir().join("data")
}

pub fn shortcut_file(style: PathStyle) -> PathBuf {
    let name = match style {
        PathStyle::Windows => "shortcuts.win.tsv",
        PathStyle::PowerShell => "shortcuts.ps.tsv",
        PathStyle::Msys => "shortcuts.msys.tsv",
        PathStyle::Wsl => "shortcuts.wsl.tsv",
    };
    data_dir().join(name)
}

pub fn alias_file() -> PathBuf {
    data_dir().join("aliases.tsv")
}

pub fn init_script(kind: ProfileKind) -> Option<PathBuf> {
    match kind {
        ProfileKind::PowerShell => Some(shell_dir().join("nyx-init.ps1")),
        ProfileKind::Cmd => Some(shell_dir().join("clink").join("nyx.lua")),
        ProfileKind::Bash => Some(shell_dir().join("nyx.bash")),
        // We do not write into someone's WSL distro or someone's remote host. That is precisely
        // why the terminal-side shadow layer exists.
        ProfileKind::Wsl => None,
    }
}

/// The newest item on the channel, discarding everything it superseded.
///
/// Every message here is a *complete* set of tables, not a delta, so an older one has nothing to
/// contribute once a newer one exists — writing it would be seven files of disk traffic to
/// produce a state that is already wrong. Typing a path into the settings window sends one of
/// these per keystroke.
fn newest<T>(first: T, rx: &crossbeam_channel::Receiver<T>) -> T {
    let mut latest = first;
    while let Ok(newer) = rx.try_recv() {
        latest = newer;
    }
    latest
}

/// Rewrites the generated files off the UI thread.
///
/// **Seven atomic writes cost 26 ms on this machine** — measured, best of five, and each one is
/// a create, a `sync_all` and a rename because a half-written script is worse than none. That is
/// more than a whole frame at 60 Hz, and it used to happen inline on every keystroke that changed
/// the tables: typing a path into a shortcut's field dropped frames for as long as the path was.
///
/// One worker for the life of the process rather than a thread per request, because that is also
/// what keeps the writes ordered. Two threads racing to rewrite the same seven files can land in
/// either order, and the loser's state is the one that stays on disk.
pub struct Writer {
    /// `None` when the thread could not be started. Then requests are written inline, which is
    /// slow but correct — the same trade `win/` makes everywhere: degrade, never fail to run.
    tx: Option<crossbeam_channel::Sender<Tables>>,
}

impl Writer {
    pub fn spawn() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<Tables>();
        let started = std::thread::Builder::new()
            .name("nyx-shellgen".into())
            .spawn(move || {
                while let Ok(tables) = rx.recv() {
                    let tables = newest(tables, &rx);
                    if let Err(err) = regenerate(&tables) {
                        log::warn!("could not rewrite shell integration scripts: {err}");
                    }
                }
            })
            .is_ok();

        if !started {
            log::warn!("no shellgen thread; shell integration files will be written inline");
        }
        Self { tx: started.then_some(tx) }
    }

    /// Ask for a rewrite. Returns immediately unless there is no worker.
    pub fn request(&self, tables: Tables) {
        match &self.tx {
            Some(tx) => {
                let _ = tx.send(tables);
            },
            None => {
                if let Err(err) = regenerate(&tables) {
                    log::warn!("could not rewrite shell integration scripts: {err}");
                }
            },
        }
    }
}

/// Rewrite every generated file.
///
/// Called directly only at startup, where it *must* block: a pane spawned against a script that
/// is not on disk yet is told it has integration and then waits 2.5 s for a greeting that cannot
/// arrive. Every later rewrite goes through `Writer`.
pub fn regenerate(tables: &Tables) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(shell_dir().join("clink"))?;

    for style in [PathStyle::Windows, PathStyle::PowerShell, PathStyle::Msys, PathStyle::Wsl] {
        write_atomic(&shortcut_file(style), shortcut_tsv(tables, style).as_bytes())?;
    }
    write_atomic(&alias_file(), alias_tsv(tables).as_bytes())?;

    write_atomic(&shell_dir().join("nyx-init.ps1"), powershell::SCRIPT.as_bytes())?;
    write_atomic(&shell_dir().join("clink").join("nyx.lua"), clink::SCRIPT.as_bytes())?;
    write_atomic(&shell_dir().join("nyx.bash"), bash::SCRIPT.as_bytes())?;

    Ok(())
}

/// `<sigil>\t<already-quoted expansion>` per line.
///
/// A tab is the separator because no shortcut sigil can contain one and every one of Lua,
/// PowerShell and bash splits on it in a single line of code with no dependency.
fn shortcut_tsv(tables: &Tables, style: PathStyle) -> String {
    let mut out = String::new();
    for shortcut in &tables.shortcuts {
        // A path containing a newline would corrupt the file; skip rather than emit a half line.
        if shortcut.path.contains(['\n', '\r', '\t']) {
            log::warn!("shortcut \"{}\" has a path with a tab or newline; skipped", shortcut.id);
            continue;
        }
        out.push_str(&shortcut.sigil);
        out.push('\t');
        out.push_str(&shortcut.expansion(style));
        out.push('\n');
    }
    out
}

fn alias_tsv(tables: &Tables) -> String {
    let mut out = String::new();
    for (name, value) in &tables.aliases {
        if name.contains(['\n', '\r', '\t']) || value.contains(['\n', '\r', '\t']) {
            continue;
        }
        out.push_str(name);
        out.push('\t');
        out.push_str(value);
        out.push('\n');
    }
    out
}

/// Whether a generated script exists and can be injected for this shell.
pub fn available(kind: ProfileKind) -> Option<PathBuf> {
    let path = init_script(kind)?;
    path.exists().then_some(path)
}

/// Quote a path for a PowerShell single-quoted string.
pub fn ps_single_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

/// Shared by the real-interpreter tests in `powershell`, `clink` and `bash`.
#[cfg(test)]
pub mod testing {
    /// The same text with the title escapes taken out.
    ///
    /// A mark ends in BEL, not a newline, so the greeting a script sends while it loads is glued
    /// to the front of whatever the probe prints first. Reading a probe's output by line means
    /// taking the marks out of it first.
    pub fn without_titles(stdout: &str) -> String {
        let mut out = String::with_capacity(stdout.len());
        let mut rest = stdout;
        while let Some(i) = rest.find("\u{1b}]2;") {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            match rest.find('\u{7}') {
                Some(end) => rest = &rest[end + 1..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    /// Every `nyx:` title a script wrote, in order.
    ///
    /// The marks go out as OSC 2 — a real escape sequence in the process's stdout, not something
    /// the shell prints for a person to read — so picking them back out is how a test sees the
    /// transport rather than only the string that was going to be sent.
    pub fn titles(stdout: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = stdout;
        while let Some(i) = rest.find("\u{1b}]2;") {
            rest = &rest[i + 4..];
            let Some(end) = rest.find('\u{7}') else { break };
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        out
    }
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
            path: r"C:\Users\Nihil\Desktop\Free Tibet".into(),
            icon: String::new(),
            color: None,
            order: 0,
        }];
        let mut aliases = AliasMap::new();
        aliases.insert("gs".into(), "git status".into());
        Tables::from_config(&shortcuts, &aliases).0
    }

    #[test]
    fn the_tsv_carries_a_ready_quoted_path_per_shell() {
        let t = tables();
        assert_eq!(
            shortcut_tsv(&t, PathStyle::Windows),
            "@tibet\t\"C:\\Users\\Nihil\\Desktop\\Free Tibet\"\n"
        );
        assert_eq!(
            shortcut_tsv(&t, PathStyle::Wsl),
            "@tibet\t\"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet')\"\n"
        );
        assert_eq!(alias_tsv(&t), "gs\tgit status\n");
    }

    #[test]
    fn a_path_containing_a_tab_is_skipped_rather_than_corrupting_the_file() {
        let mut cfg = ShortcutConfig { id: "bad".into(), ..Default::default() };
        cfg.path = "C:\\a\tb".into();
        let (t, _) = Tables::from_config(&[cfg], &Default::default());
        assert_eq!(shortcut_tsv(&t, PathStyle::Windows), "");
    }


    #[test]
    fn a_rewrite_request_supersedes_every_one_still_waiting() {
        // The settings window applies its draft on every keystroke, so typing a twenty-character
        // path queues twenty rewrites of seven files each. Nineteen of them describe a path the
        // user was halfway through typing; only the last is worth 26 ms of disk.
        let (tx, rx) = crossbeam_channel::unbounded();
        for n in 1..=20 {
            tx.send(n).unwrap();
        }

        let first = rx.recv().unwrap();
        assert_eq!(newest(first, &rx), 20, "an older set of tables would have been written");
        assert!(rx.try_recv().is_err(), "the queue still holds work that is already stale");
    }

    #[test]
    fn nothing_waiting_leaves_the_request_it_was_handed() {
        // The ordinary case: one change, one rewrite. `newest` must not block waiting for a
        // second message that is never coming.
        let (tx, rx) = crossbeam_channel::unbounded::<u32>();
        drop(tx);
        assert_eq!(newest(7, &rx), 7);
    }

    #[test]
    fn wsl_gets_no_init_script() {
        // Writing into someone's distro is not ours to do.
        assert!(init_script(ProfileKind::Wsl).is_none());
        assert!(init_script(ProfileKind::PowerShell).is_some());
    }

    #[test]
    fn powershell_quoting_doubles_embedded_quotes() {
        assert_eq!(ps_single_quote(Path::new(r"C:\it's")), "'C:\\it''s'");
    }
}
