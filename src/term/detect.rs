//! Finding the shells that are actually installed.
//!
//! Runs off the UI thread: `wsl.exe -l` alone can take a couple of hundred milliseconds, and
//! startup time is a feature. The results arrive as an `AppEvent` and the profile list updates
//! when they do.

use std::path::{Path, PathBuf};

use crate::term::profile::{Profile, ProfileKind};

#[derive(Debug, Clone, Default)]
pub struct Detected {
    pub cmd: Option<PathBuf>,
    pub powershell: Option<PathBuf>,
    pub pwsh: Option<PathBuf>,
    pub git_bash: Option<PathBuf>,
    pub wsl: Option<PathBuf>,
    pub wsl_distros: Vec<String>,
    /// Clink makes cmd.exe's line editor programmable, which is what Phase 6's shell-side
    /// expansion needs. Absent is not an error — the terminal-side fallback covers it.
    pub clink: Option<PathBuf>,
}

fn env_path(var: &str, tail: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var)?;
    let path = PathBuf::from(base).join(tail);
    path.exists().then_some(path)
}

fn on_path(exe: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|dir| dir.join(exe)).find(|p| p.is_file())
}

/// Probe the filesystem only.
///
/// Sub-millisecond — a handful of `exists()` calls — so it runs synchronously at startup and the
/// first tab gets a correct profile immediately. The WSL enumeration is deliberately *not* here:
/// it shells out and can take a couple of hundred milliseconds, which is a visible fraction of
/// the whole launch. Call [`wsl_distros`] on a background thread for that.
pub fn detect() -> Detected {
    Detected {
        cmd: std::env::var_os("ComSpec").map(PathBuf::from).filter(|p| p.exists()),
        powershell: env_path("SystemRoot", r"System32\WindowsPowerShell\v1.0\powershell.exe"),
        pwsh: on_path("pwsh.exe")
            .or_else(|| env_path("ProgramFiles", r"PowerShell\7\pwsh.exe"))
            .or_else(|| env_path("ProgramFiles(x86)", r"PowerShell\7\pwsh.exe")),
        git_bash: env_path("ProgramFiles", r"Git\bin\bash.exe")
            .or_else(|| env_path("ProgramFiles(x86)", r"Git\bin\bash.exe"))
            .or_else(|| env_path("LOCALAPPDATA", r"Programs\Git\bin\bash.exe"))
            .or_else(|| {
                bash_beside_git(&on_path("git.exe")?, |p: &Path| p.is_file())
            }),
        wsl: env_path("SystemRoot", r"System32\wsl.exe"),
        wsl_distros: Vec::new(),
        clink: find_clink(),
    }
}

/// Git Bash found by way of `git.exe` on `PATH`, rather than by a fixed install directory.
///
/// The three directories probed above are where the official installer puts Git, and they are
/// the only places that were looked — so a Git installed anywhere else (scoop, a portable copy,
/// anything under `%LOCALAPPDATA%` that is not `Programs\Git`) left `git_bash` as `None`, no Git
/// Bash profile in the menu, and the bash half of shell integration permanently unreachable.
/// This is the same defect the Clink comment below records, in the one probe that still had it:
/// `pwsh` and `clink` both ask `PATH` first, and `git_bash` did not.
///
/// `git.exe` rather than `bash.exe` is the thing looked for, because `C:\Windows\System32\bash.exe`
/// is **WSL's** launcher, not Git's. Taking that one would spawn a Linux shell under a profile
/// whose path style says MSYS, and every `@shortcut` in it would expand to a `/c/...` path the
/// distro cannot open.
///
/// Walking ancestors covers both layouts the installer produces — `<root>\cmd\git.exe` and
/// `<root>\mingw64\bin\git.exe` both reach `<root>\bin\bash.exe`. The walk is bounded so that a
/// `git.exe` sitting near the root of a drive cannot reach an unrelated `C:\bin\bash.exe`.
///
/// `exists` is a parameter so the rule can be tested against a layout rather than against
/// whatever happens to be installed on the machine running the suite.
fn bash_beside_git(git: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    git.ancestors().skip(1).take(4).find_map(|root| {
        let bash = root.join(r"bin\bash.exe");
        exists(&bash).then_some(bash)
    })
}

/// Where Clink installs itself, in probe order.
///
/// **The winget package lands in `%ProgramFiles(x86)%\clink` even on a 64-bit machine, and adds
/// nothing to `PATH`.** That is the package the settings window tells the user to install, and
/// it was the one location not probed — so on a stock install `clink` was `None`,
/// `Profile::injects` was false for every cmd.exe pane, and the entire Clink integration was
/// unreachable. It is also why none of the bugs in the script we generate for it had ever been
/// seen: nothing had ever loaded that script. `pwsh` and `git_bash` above already look in both
/// Program Files; this was the one that did not.
const CLINK_DIRS: [(&str, &str); 3] = [
    ("LOCALAPPDATA", r"Programs\clink"),
    ("ProgramFiles", "clink"),
    ("ProgramFiles(x86)", "clink"),
];

fn find_clink() -> Option<PathBuf> {
    on_path("clink.bat").or_else(|| on_path("clink.exe")).or_else(|| {
        CLINK_DIRS.iter().find_map(|(var, dir)| env_path(var, &format!(r"{dir}\clink.bat")))
    })
}

/// Enumerate installed WSL distributions.
///
/// **`wsl.exe` writes UTF-16LE, not UTF-8.** Reading it as UTF-8 yields a string with a NUL
/// between every character, which then either fails to match anything or produces distro names
/// full of invisible junk — the classic trap with this command.
/// Enumerate installed WSL distributions from the Windows registry.
///
/// Querying HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss directly is sub-millisecond,
/// works without spawning child processes, prevents flashing console windows, and avoids
/// triggering antivirus heuristic false positives (Behavior:Win32/DefenseEvasion.A!ml).
pub fn wsl_distros() -> Vec<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
            KEY_READ, REG_SZ,
        };
        use crate::win::wide;

        let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss");
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: standard read-only registry key open
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            )
        };
        if status != 0 {
            return Vec::new();
        }

        let mut distros = Vec::new();
        let mut index = 0;
        let mut name_buf = [0u16; 256];
        let val_name = wide("DistributionName");

        loop {
            let mut name_len = name_buf.len() as u32;
            let res = unsafe {
                RegEnumKeyExW(
                    hkey,
                    index,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if res != 0 {
                break;
            }
            index += 1;

            let mut sub_hkey: HKEY = std::ptr::null_mut();
            if unsafe {
                RegOpenKeyExW(
                    hkey,
                    name_buf.as_ptr(),
                    0,
                    KEY_READ,
                    &mut sub_hkey,
                )
            } == 0 {
                let mut val_type = 0u32;
                let mut data = [0u8; 512];
                let mut data_len = data.len() as u32;
                if unsafe {
                    RegQueryValueExW(
                        sub_hkey,
                        val_name.as_ptr(),
                        std::ptr::null_mut(),
                        &mut val_type,
                        data.as_mut_ptr(),
                        &mut data_len,
                    )
                } == 0 && val_type == REG_SZ {
                    let u16_len = (data_len as usize) / 2;
                    let u16_slice: &[u16] = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const u16, u16_len)
                    };
                    let s = String::from_utf16_lossy(u16_slice)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string();
                    if !s.is_empty() {
                        distros.push(s);
                    }
                }
                unsafe { RegCloseKey(sub_hkey) };
            }
        }

        unsafe { RegCloseKey(hkey) };
        distros
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
}

impl Detected {
    /// The profiles to offer, in the order they should appear.
    ///
    /// Only what is actually installed: a menu entry for a shell that is not there is worse than
    /// no entry, because it fails at spawn time with a message about a missing file.
    pub fn profiles(&self) -> Vec<Profile> {
        let mut out = Vec::new();

        if let Some(path) = &self.powershell {
            out.push(Profile {
                id: "powershell".into(),
                name: "Windows PowerShell".into(),
                program: path.clone(),
                args: vec!["-NoLogo".into()],
                kind: ProfileKind::PowerShell,
                cwd: None,
                integration: true,
            });
        }
        if let Some(path) = &self.pwsh {
            out.push(Profile {
                id: "pwsh".into(),
                name: "PowerShell 7".into(),
                program: path.clone(),
                args: vec!["-NoLogo".into()],
                kind: ProfileKind::PowerShell,
                cwd: None,
                integration: true,
            });
        }
        if let Some(path) = &self.cmd {
            out.push(Profile {
                id: "cmd".into(),
                name: "Command Prompt".into(),
                program: path.clone(),
                args: Vec::new(),
                kind: ProfileKind::Cmd,
                cwd: None,
                integration: true,
            });
        }
        if let Some(path) = &self.git_bash {
            out.push(Profile {
                id: "gitbash".into(),
                name: "Git Bash".into(),
                program: path.clone(),
                // Login shell, so the user's own profile and PATH apply.
                args: vec!["-i".into(), "-l".into()],
                kind: ProfileKind::Bash,
                cwd: None,
                integration: true,
            });
        }
        if let Some(path) = &self.wsl {
            for distro in &self.wsl_distros {
                out.push(Profile {
                    id: format!("wsl-{distro}"),
                    name: format!("{distro} (WSL)"),
                    program: path.clone(),
                    args: vec!["-d".into(), distro.clone()],
                    kind: ProfileKind::Wsl,
                    cwd: None,
                    integration: true,
                });
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16le_from_wsl_decodes_to_real_names() {
        // What `wsl.exe -l -q` actually writes: UTF-16LE, CRLF separated.
        let mut bytes = Vec::new();
        for ch in "Ubuntu\r\nDebian\r\n".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        let decoded = decode_utf16le(&bytes);
        let names: Vec<_> = decoded.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        assert_eq!(names, vec!["Ubuntu", "Debian"]);
    }

    #[test]
    fn reading_wsl_output_as_utf8_would_have_been_wrong() {
        // Guards the specific bug: as UTF-8 the same bytes are "U\0b\0u\0..." and every name
        // comes out full of NULs.
        let mut bytes = Vec::new();
        for ch in "Ubuntu".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        let wrong = String::from_utf8_lossy(&bytes);
        assert_ne!(wrong.trim_end_matches('\0'), "Ubuntu");
        assert_eq!(decode_utf16le(&bytes), "Ubuntu");
    }

    #[test]
    fn odd_length_output_does_not_panic() {
        // chunks_exact drops the trailing byte rather than indexing past the end.
        assert_eq!(decode_utf16le(&[0x55, 0x00, 0x42]), "U");
    }

    #[test]
    fn profiles_only_lists_what_was_found() {
        let none = Detected::default();
        assert!(none.profiles().is_empty());

        let some = Detected {
            cmd: Some(PathBuf::from("cmd.exe")),
            wsl: Some(PathBuf::from("wsl.exe")),
            wsl_distros: vec!["Ubuntu".into()],
            ..Default::default()
        };
        let ids: Vec<_> = some.profiles().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["cmd", "wsl-Ubuntu"]);
    }

    #[test]
    fn wsl_without_distros_offers_nothing() {
        // wsl.exe exists on every Windows 11 install even with no distro installed, which is the
        // case on this machine. Listing "WSL" there would be a menu entry that cannot work.
        let d = Detected {
            wsl: Some(PathBuf::from("wsl.exe")),
            wsl_distros: Vec::new(),
            ..Default::default()
        };
        assert!(d.profiles().is_empty());
    }

    #[test]
    fn clink_is_looked_for_where_winget_actually_puts_it() {
        // The 32-bit Program Files, on a 64-bit machine, because that is where the package the
        // settings window recommends installs itself. Leaving it out made every other Clink
        // code path dead without anything being able to report it.
        assert!(CLINK_DIRS.iter().any(|(var, _)| *var == "ProgramFiles(x86)"));
        assert!(CLINK_DIRS.iter().any(|(var, _)| *var == "ProgramFiles"));
        assert!(CLINK_DIRS.iter().any(|(var, _)| *var == "LOCALAPPDATA"));
    }

    #[test]
    fn a_clink_that_is_installed_is_found() {
        // Spelled out here rather than taken from CLINK_DIRS, so this is the filesystem and the
        // probe table agreeing rather than the table agreeing with itself.
        let winget = std::env::var_os("ProgramFiles(x86)")
            .map(|b| PathBuf::from(b).join(r"clink\clink.bat"))
            .filter(|p| p.exists());
        let Some(winget) = winget else {
            eprintln!("SKIPPED: Clink is not installed in Program Files (x86)");
            return;
        };
        // Not asserted to be *that* copy: a machine with clink on PATH would legitimately find
        // that one first. What must not happen is finding none at all.
        assert!(find_clink().is_some(), "{} exists but detection missed it", winget.display());
    }
}

#[cfg(test)]
mod git_bash_tests {
    use super::*;

    /// Both layouts the Git installer produces reach the same `bin\bash.exe`.
    #[test]
    fn git_on_path_finds_bash_from_either_of_its_two_directories() {
        let root = Path::new(r"C:\Users\someone\AppData\Local\hermes\git");
        let bash = root.join(r"bin\bash.exe");
        let exists = |p: &Path| p == bash;

        for git in [root.join(r"cmd\git.exe"), root.join(r"mingw64\bin\git.exe")] {
            assert_eq!(
                bash_beside_git(&git, exists),
                Some(bash.clone()),
                "{} did not lead to Git Bash",
                git.display()
            );
        }
    }

    /// The walk is bounded, or a `git.exe` high up a drive reaches an unrelated `C:\bin\bash.exe`.
    #[test]
    fn a_distant_bash_is_not_claimed_as_gits() {
        let stray = Path::new(r"C:\bin\bash.exe");
        let git = Path::new(r"C:\a\b\c\d\e\git.exe");
        assert_eq!(bash_beside_git(git, |p: &Path| p == stray), None);
    }

    #[test]
    fn no_bash_beside_git_is_not_a_guess() {
        assert_eq!(
            bash_beside_git(Path::new(r"C:\tools\git\cmd\git.exe"), |_: &Path| false),
            None
        );
    }
}
