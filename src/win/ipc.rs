//! One instance, and the pipe a second launch talks to it over.
//!
//! This is what makes Explorer's "Open Nyx here" open a *tab* rather than a fifth window. The
//! second process starts, finds the mutex already taken, writes the folder down the pipe and
//! exits — from the user's side the running window simply gains a tab, in about the time it takes
//! for a process to start and stop.
//!
//! The mutex lives in the `Local\` namespace, which is per-session: two people signed in to the
//! same machine get an instance each rather than one of them inheriting the other's window. The
//! pipe name carries the user name for the same reason, because pipes have no such namespace.
//!
//! **Everything arriving on the pipe is data.** The payload names a directory and is used as one:
//! it is checked to be an existing directory and then handed to `ShellSpec.cwd`. It is never a
//! command, never a program name, and never anything a shell expands — a local process that can
//! reach the pipe can open a tab somewhere, and that is the whole of what it can do.

use std::io;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::event::{AppEvent, RepaintCoalescer};
use crate::win::wide;

/// The protocol tag. A first line that is not this is a message from something that is not Nyx,
/// or from a Nyx new enough to have changed the format, and is dropped either way.
const TAG: &str = "nyx1";

/// Nothing legitimate is anywhere near this long, and a pipe is an unauthenticated local channel.
const MAX_MESSAGE: usize = 8 * 1024;

/// What one launch wants an existing instance to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// The folder to open a tab in.
    pub cwd: Option<PathBuf>,
    pub focus: bool,
}

impl Request {
    pub fn is_empty(&self) -> bool {
        self.cwd.is_none()
    }

    /// Serialise. One `key\tvalue` line per field, after the tag.
    pub fn encode(&self) -> String {
        let mut out = format!("{TAG}\n");
        if self.focus { out.push_str("focus\t1\n"); }
        if let Some(cwd) = &self.cwd {
            out.push_str("cwd\t");
            out.push_str(&cwd.to_string_lossy());
            out.push('\n');
        }
        out
    }

    /// Parse. Unknown keys are ignored rather than rejected, so a newer Nyx talking to an older
    /// one degrades to the fields they share instead of failing outright.
    pub fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()?.trim() != TAG {
            return None;
        }
        let mut request = Request::default();
        for line in lines {
            let Some((key, value)) = line.split_once('\t') else { continue };
            if key == "focus" && value == "1" { request.focus = true; }
            if key == "cwd" && !value.is_empty() {
                request.cwd = Some(PathBuf::from(value));
            }
        }
        Some(request)
    }

    /// Drop anything that is not an existing directory.
    ///
    /// The payload comes off an unauthenticated local pipe, and this is the one place that
    /// decides what a path from there is allowed to be. A spawn against a non-existent directory
    /// fails with a message about `CreateProcess` that names nothing useful.
    pub fn sanitized(mut self) -> Self {
        if self.cwd.as_deref().is_some_and(|p| !p.is_dir()) {
            log::warn!("ignoring a pipe request for {:?}: not a directory", self.cwd);
            self.cwd = None;
        }
        self
    }
}

/// What `main` worked out about this launch, handed to the app.
#[derive(Debug, Clone, Default)]
pub struct Startup {
    /// The folder this launch named, if any.
    pub request: Request,
    /// Whether this process holds the single-instance claim, and is therefore the one that
    /// listens on the pipe. A second window running alongside a first one does not.
    pub primary: bool,
}

/// A held single-instance claim. Dropping it releases the mutex.
pub struct Instance(HANDLE);

// SAFETY: a kernel handle is a process-wide token, not a borrow of anything thread-local.
unsafe impl Send for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: a handle from `CreateMutexW` that has not been closed.
        unsafe { CloseHandle(self.0) };
    }
}

/// Try to become *the* instance.
///
/// `None` means another one is already running; the caller should send it a request and exit.
/// Note that the mutex is held, not merely tested: releasing it here would let two launches a
/// microsecond apart both decide they were first.
pub fn claim() -> Option<Instance> {
    let name = wide(format!("Local\\Nyx.{}", user_tag()));
    // SAFETY: a named mutex with default security, owned initially. A name in use returns a
    // handle to the existing object plus ERROR_ALREADY_EXISTS.
    let handle = unsafe { CreateMutexW(ptr::null(), 1, name.as_ptr()) };
    if handle.is_null() {
        // Without a mutex we cannot tell, so behave as the only instance rather than refusing to
        // start. A second window is a much smaller problem than no window.
        log::warn!("could not create the single-instance mutex; continuing as a new instance");
        return Some(Instance(ptr::null_mut()));
    }
    // SAFETY: no arguments.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: closing our own handle to the existing mutex, not the mutex.
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(Instance(handle))
}

fn user_tag() -> String {
    // Only for namespacing, so anything stable per user will do.
    std::env::var("USERNAME")
        .unwrap_or_else(|_| "user".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn pipe_name() -> String {
    format!(r"\\.\pipe\Nyx.{}", user_tag())
}

/// Hand a request to the running instance. Returns false if nothing was listening.
pub fn send(request: &Request) -> bool {
    let name = wide(pipe_name());
    // SAFETY: opening an existing pipe for writing. An absent server returns INVALID_HANDLE_VALUE
    // rather than blocking.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_NONE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return false;
    }

    let bytes = request.encode().into_bytes();
    let mut written = 0u32;
    // SAFETY: `bytes` outlives the call, and the length is its own.
    let ok = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    // SAFETY: our own handle.
    unsafe { CloseHandle(handle) };
    ok != 0 && written as usize == bytes.len()
}

/// Listen for launches that found us already running.
///
/// The thread is detached and lives as long as the process: it blocks in `ConnectNamedPipe`, which
/// costs nothing, and there is no shutdown path that matters because the handle goes with the
/// process. Returning an error here is not fatal — it costs the Explorer integration, not the
/// terminal.
pub fn serve(tx: Sender<AppEvent>, repaint: Arc<RepaintCoalescer>) -> io::Result<()> {
    let name = wide(pipe_name());
    std::thread::Builder::new().name("nyx-ipc".into()).spawn(move || {
        loop {
            // A fresh instance per connection. Reusing one across clients is what produces the
            // classic "the second Explorer menu click does nothing" bug, because a pipe left in a
            // half-disconnected state accepts the connection and then reads zero bytes.
            // SAFETY: inbound byte pipe, default security, one instance at a time.
            let pipe = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    0,
                    MAX_MESSAGE as u32,
                    0,
                    ptr::null(),
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                log::warn!("could not create the IPC pipe; \"Open Nyx here\" will start a new window");
                return;
            }

            // SAFETY: a synchronous pipe with no OVERLAPPED. A client that connected between
            // creation and this call reports ERROR_PIPE_CONNECTED, which is success.
            let connected = unsafe { ConnectNamedPipe(pipe, ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

            if connected {
                if let Some(request) = read_request(pipe) {
                    let request = request.sanitized();
                    if tx.send(AppEvent::OpenHere(request)).is_err() {
                        // SAFETY: our own handle.
                        unsafe { CloseHandle(pipe) };
                        return; // the app is gone
                    }
                    repaint.poke();
                }
            }

            // SAFETY: our own handle, connected or not.
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        }
    })?;
    Ok(())
}

fn read_request(pipe: HANDLE) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let mut read = 0u32;
        // SAFETY: `chunk` is the buffer and its own length.
        let ok = unsafe {
            ReadFile(
                pipe,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break; // the client closed, which is how a message ends
        }
        buffer.extend_from_slice(&chunk[..read as usize]);
        if buffer.len() > MAX_MESSAGE {
            log::warn!("dropping an oversized IPC message");
            return None;
        }
    }
    Request::decode(&String::from_utf8_lossy(&buffer))
}

/// What the command line asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub request: Request,
    /// Hand this to a running instance if there is one. Set by the Explorer context menu.
    pub attach: bool,
    /// Never attach, whatever the config says.
    pub new_window: bool,
    /// `--install-context-menu` / `--remove-context-menu`, for the installer script.
    pub install_menu: bool,
    pub remove_menu: bool,
    pub help: bool,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut out = Args::default();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--cwd" | "-d" => {
                    if let Some(path) = iter.next() {
                        out.request.cwd = Some(PathBuf::from(trim_trailing_slash(&path)));
                    }
                },
                "--focus" => out.request.focus = true,
                "--attach" => out.attach = true,
                "--new-window" => out.new_window = true,
                "--install-context-menu" => out.install_menu = true,
                "--remove-context-menu" => out.remove_menu = true,
                "--help" | "-h" | "/?" => out.help = true,
                // A bare path, so `nyx C:\src` does the obvious thing.
                other if !other.starts_with('-') && out.request.cwd.is_none() => {
                    out.request.cwd = Some(PathBuf::from(trim_trailing_slash(other)));
                },
                _ => {},
            }
        }
        out
    }
}

/// Explorer's `%V` for a drive is `C:\`, and a trailing backslash inside the quotes of a registry
/// command line escapes the closing quote — so the path arrives as `C:\"` and names nothing.
///
/// Stripping it here rather than in the registry string means it is fixed for hand-typed paths
/// too, and `C:\` itself still survives as `C:\`.
fn trim_trailing_slash(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('"');
    let stripped = trimmed.trim_end_matches(['\\', '/']);
    if stripped.is_empty() || stripped.ends_with(':') {
        // `C:` alone is the *current directory on C:*, not the root of it.
        format!("{stripped}\\")
    } else {
        stripped.to_string()
    }
}

/// Deliberately ASCII only. The bytes are written straight to whatever handle this launch has,
/// and a console left on a legacy code page turns an em dash into two characters of noise.
pub const HELP: &str = "\
Nyx - a Windows terminal

    nyx [OPTIONS] [DIRECTORY]

    -d, --cwd <DIR>          open the first tab in this directory
        --attach             hand the directory to a running Nyx instead of
                             opening a new window, if one is running
        --new-window         always a new window, whatever the config says
        --install-context-menu   add \"Open Nyx here\" to Explorer (HKCU only)
        --remove-context-menu    take it out again
    -h, --help               this
";


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_request_round_trips_without_a_directory() {
        let request = Request { focus: true, ..Default::default() };
        assert_eq!(Request::decode(&request.encode()), Some(request));
        assert!(Args::parse(["--focus".into()]).request.focus);
    }

    #[test]
    fn a_request_round_trips() {
        let request = Request { cwd: Some(PathBuf::from(r"C:\Users\Nihil\Desktop\Free Tibet")), ..Default::default() };
        let decoded = Request::decode(&request.encode()).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn an_empty_request_round_trips_as_empty() {
        let decoded = Request::decode(&Request::default().encode()).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn something_that_is_not_nyx_is_dropped() {
        // The pipe is an unauthenticated local channel; anything can connect to it.
        assert!(Request::decode("GET / HTTP/1.1\r\n\r\n").is_none());
        assert!(Request::decode("").is_none());
        assert!(Request::decode("nyx9\ncwd\tC:\\").is_none());
    }

    #[test]
    fn an_unknown_key_is_ignored_rather_than_rejected() {
        // A newer Nyx talking to an older one degrades to the fields they share.
        let decoded = Request::decode("nyx1\nprofile\tpwsh\ncwd\tC:\\Windows\n").unwrap();
        assert_eq!(decoded.cwd, Some(PathBuf::from(r"C:\Windows")));
    }

    #[test]
    fn a_path_that_is_not_a_directory_is_dropped() {
        let request = Request { cwd: Some(PathBuf::from(r"C:\definitely\not\here")), ..Default::default() };
        assert!(request.sanitized().is_empty());
        let real = Request { cwd: Some(PathBuf::from(r"C:\Windows")), ..Default::default() };
        assert!(!real.clone().sanitized().is_empty());
    }

    #[test]
    fn explorers_trailing_backslash_is_trimmed() {
        // `%V` for a drive is `C:\`, and inside a quoted registry command line that backslash
        // escapes the closing quote — so the argument arrives as `C:\"`.
        assert_eq!(trim_trailing_slash(r"C:\src\"), r"C:\src");
        assert_eq!(trim_trailing_slash("C:\\src\\\""), r"C:\src");
        // A drive root still has to be a root; `C:` on its own means something else entirely.
        assert_eq!(trim_trailing_slash(r"C:\"), r"C:\");
        assert_eq!(trim_trailing_slash(r"C:"), r"C:\");
    }

    #[test]
    fn the_command_line_parses() {
        let args = Args::parse(
            ["--cwd", r"C:\Windows", "--attach"].iter().map(|s| s.to_string()),
        );
        assert_eq!(args.request.cwd, Some(PathBuf::from(r"C:\Windows")));
        assert!(args.attach);
        assert!(!args.new_window);

        // A bare path, so `nyx C:\src` works the way every other tool does.
        let bare = Args::parse([r"C:\src".to_string()]);
        assert_eq!(bare.request.cwd, Some(PathBuf::from(r"C:\src")));

        // An unknown flag is ignored rather than fatal: a shortcut written by an older installer
        // must not stop the terminal from opening.
        let odd = Args::parse(["--maximize".to_string(), r"C:\src".to_string()]);
        assert_eq!(odd.request.cwd, Some(PathBuf::from(r"C:\src")));
    }

    #[test]
    fn a_second_claim_while_the_first_is_held_is_refused() {
        // Holding the claim and asking again is exactly what a second launch does, so this is the
        // real question: the second answer has to be "someone else has it".
        let Some(first) = claim() else {
            // A real Nyx is running on this machine — which is itself the property under test.
            return;
        };
        assert!(claim().is_none(), "a second claim must not also succeed");
        drop(first);
        // And released, so the launch after that one becomes the instance.
        assert!(claim().is_some());
    }
}
