//! Starting a shell under ConPTY and wiring it to a `Term`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Osc52, Term};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};

use crate::term::proxy::EventProxy;
use crate::term::{GridSize, PaneId};

/// What a shell needs to start. Phase 4 replaces this with the configured profile type.
#[derive(Clone, Debug)]
pub struct ShellSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

impl ShellSpec {
    /// `cmd.exe` is the Phase 1 default because it is the one shell guaranteed present on every
    /// Windows install — pwsh, Git Bash and WSL are all detected in Phase 4 and none of them is
    /// installed on every machine.
    pub fn cmd() -> Self {
        Self {
            program: std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()),
            args: Vec::new(),
            cwd: dirs_home(),
            env: HashMap::new(),
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

pub struct Spawned {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub sender: EventLoopSender,
}

/// What every shell is told about the terminal it is running in.
///
/// Nyx set **none** of this, which is invisible from a cmd.exe or PowerShell pane — those talk to
/// ConPTY through the console API and never read it — and wrong everywhere else. Git Bash, WSL,
/// anything reached over ssh, and every Node or Rust CLI that asks `supports-color` all read these
/// variables, and an unset `TERM` reads as a terminal that cannot do colour or cursor addressing.
/// The grid has been drawing 24-bit colour since Phase 1; it just never said so.
///
/// `xterm-256color` rather than something claiming direct colour, because that is the terminfo
/// entry that exists everywhere, and `COLORTERM=truecolor` is the out-of-band channel every
/// program actually uses to find the other 16 million.
///
/// `TERM` and `COLORTERM` describe the terminal and defer to an environment that already set them
/// — someone who exports `TERM=dumb` on purpose means it. `TERM_PROGRAM` *identifies* it, so it is
/// always overwritten: launching Nyx from inside another terminal would otherwise leave that
/// terminal's name in the variable, and a program asking "what am I running in" would act on the
/// answer for a window that is not the one it is drawing to.
///
/// `defined` is a parameter so the rule can be tested against a stated environment rather than
/// against whatever the machine running the suite happens to export.
pub fn base_env(defined: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("TERM_PROGRAM", "nyx".to_string()),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION").to_string()),
    ];
    if !defined("TERM") {
        out.push(("TERM", "xterm-256color".to_string()));
    }
    if !defined("COLORTERM") {
        out.push(("COLORTERM", "truecolor".to_string()));
    }
    out
}

pub fn spawn(
    id: PaneId,
    spec: &ShellSpec,
    size: GridSize,
    cell_width: u16,
    cell_height: u16,
    scrollback: usize,
    proxy: EventProxy,
) -> anyhow::Result<Spawned> {
    // The profile's own entries win: `NYX_TOKEN` and friends are this pane's identity, and a
    // profile that deliberately states a `TERM` means it.
    let mut env = spec.env.clone();
    for (name, value) in base_env(|name| std::env::var_os(name).is_some()) {
        env.entry(name.to_string()).or_insert(value);
    }

    let pty_options = PtyOptions {
        shell: Some(Shell::new(spec.program.clone(), spec.args.clone())),
        working_directory: spec.cwd.clone(),
        drain_on_exit: true,
        env,
        escape_args: true,
    };

    let window_size = WindowSize {
        num_lines: size.screen_lines as u16,
        num_cols: size.columns as u16,
        cell_width,
        cell_height,
    };

    let pty = tty::new(&pty_options, window_size, id)?;

    // `osc52` is stated rather than inherited. `Osc52::OnlyCopy` happens to be the crate's
    // default, so leaving it out behaved identically — but it is a security posture, and a
    // security posture that holds because of somebody else's default is one upgrade away from
    // not holding. Copy is useful: it is how a program on the far end of an ssh session puts
    // text on the local clipboard. Paste is the dangerous half, because *anything* that can
    // write to the terminal can then ask for the clipboard's contents and read the reply —
    // the last password copied, on the say-so of a `cat`. Alacritty defaults it off for exactly
    // that reason, and `term::proxy` drops the event again in case this line is ever changed.
    //
    // `kitty_keyboard` is likewise stated. The crate's default is `false`, which makes it refuse
    // every `CSI > flags u` a program sends — the negotiation, the mode stack and the query
    // reply are all already implemented in there and were all switched off. With it on, the
    // modes arrive in `TermMode` and `term::kitty` answers them; with it off, the encoder below
    // could never have run, because no flag would ever have been set.
    let term_config = TermConfig {
        scrolling_history: scrollback,
        osc52: Osc52::OnlyCopy,
        kitty_keyboard: true,
        ..Default::default()
    };
    let term = Term::new(term_config, &size, proxy.clone());
    let term = Arc::new(FairMutex::new(term));

    // `EventLoop::new` is what forces `FairMutex` on us, and that is a feature: under heavy
    // output an unfair mutex is re-taken by this thread the instant it releases and the UI
    // thread never gets a turn.
    let event_loop = EventLoop::new(term.clone(), proxy, pty, pty_options.drain_on_exit, false)?;
    let sender = event_loop.channel();
    event_loop.spawn();

    Ok(Spawned { term, sender })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_shell_is_told_the_terminal_does_colour() {
        let env = base_env(|_| false);
        assert_eq!(value(&env, "TERM"), Some("xterm-256color"));
        assert_eq!(value(&env, "COLORTERM"), Some("truecolor"));
        assert_eq!(value(&env, "TERM_PROGRAM"), Some("nyx"));
        assert_eq!(value(&env, "TERM_PROGRAM_VERSION"), Some(env!("CARGO_PKG_VERSION")));
    }

    /// Someone who exports `TERM=dumb` before launching means it.
    #[test]
    fn an_environment_that_already_describes_the_terminal_is_left_alone() {
        let env = base_env(|name| matches!(name, "TERM" | "COLORTERM"));
        assert_eq!(value(&env, "TERM"), None);
        assert_eq!(value(&env, "COLORTERM"), None);
    }

    /// Launched from inside another terminal, the inherited name is about the wrong window.
    #[test]
    fn the_terminals_own_name_is_stated_even_when_something_else_already_did() {
        let env = base_env(|_| true);
        assert_eq!(value(&env, "TERM_PROGRAM"), Some("nyx"));
        assert_eq!(value(&env, "TERM_PROGRAM_VERSION"), Some(env!("CARGO_PKG_VERSION")));
    }
}
