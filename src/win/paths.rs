//! Windows paths as each shell wants to see them, and how to quote them.
//!
//! This is the module most likely to be subtly wrong in a way nobody notices until a shortcut
//! silently points at the wrong directory, so it is pure, heavily tested, and the **only**
//! implementation. Generated shell scripts in Phase 6 read pre-translated data produced here
//! rather than doing their own conversion — three inline implementations would mean two of them
//! being quietly different.
//!
//! WSL display previews use the conventional mount spelling. Executable expansions resolve
//! drive mounts with `wslpath` in the target shell and check its distribution before unwrapping
//! a WSL UNC path, since either assumption can otherwise select a different directory.

use std::borrow::Cow;

use crate::term::profile::PathStyle;

/// Render a Windows path the way `style` expects to receive it.
pub fn translate(path: &str, style: PathStyle) -> String {
    let path = strip_long_prefix(path);
    let path = path.as_ref();

    match style {
        PathStyle::Windows | PathStyle::PowerShell => path.replace('/', "\\"),
        PathStyle::Msys | PathStyle::Wsl => to_posix(path, style),
    }
}

/// `\\?\C:\x` and `\\?\UNC\server\share` are the long-path forms; every shell wants the plain
/// spelling, and leaving the prefix on produces a path that exists but that nothing can `cd` to.
fn strip_long_prefix(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        // The two leading slashes belong to the path, not to the prefix: `\\?\UNC\server\share`
        // *is* `\\server\share`, so they have to be put back here. There is no "caller's own UNC
        // handling" to do it — returning `server\share` produced a relative path plausible enough
        // to resolve somewhere real and wrong.
        return Cow::Owned(format!(r"\\{rest}"));
    }
    Cow::Borrowed(path.strip_prefix(r"\\?\").unwrap_or(path))
}

fn to_posix(path: &str, style: PathStyle) -> String {
    let unified = path.replace('\\', "/");
    let mount_root = if style == PathStyle::Wsl { "/mnt" } else { "" };

    // `\\wsl$\Ubuntu\home\x` and `\\wsl.localhost\Ubuntu\home\x` address a distro's own
    // filesystem. *Inside WSL* that path is simply `/home/x`: it is not a mounted Windows drive,
    // so there is no `/mnt` prefix, and adding one points at a directory that does not exist.
    //
    // Everywhere else it is an ordinary UNC path and has to stay one. Unwrapping it regardless of
    // the target shell handed Git Bash the distro's path spelled as its own: `/etc/hosts` in Git
    // Bash is `C:\Program Files\Git\etc\hosts` — a real file that is not the distro's — and
    // `/usr` and `/tmp` are equally real there. That is this module's stated failure mode, a
    // shortcut silently pointing somewhere else, rather than a shortcut that fails.
    if style == PathStyle::Wsl {
        for prefix in ["//wsl$/", "//wsl.localhost/"] {
            if let Some(rest) = unified.strip_prefix(prefix) {
                // The distro name is dropped rather than checked: `PathStyle` does not carry
                // which distro this pane runs, so `\\wsl$\Debian\home\x` in an Ubuntu pane still
                // becomes Ubuntu's `/home/x`. Recorded in the roadmap; it needs the distro
                // plumbed through expansion, which is more than a translation table can decide.
                let mut parts = rest.splitn(2, '/');
                let _distro = parts.next();
                let tail = parts.next().unwrap_or("");
                return format!("/{tail}");
            }
        }
    }

    // A plain UNC share has no drive letter to map, and no mount point to invent. MSYS resolves
    // `//server/share` through Windows' own redirector, so it passes through and works. WSL does
    // *not*: a share has to be mounted explicitly with `mount -t drvfs`, and Linux collapses the
    // leading `//`, so the path arrives as `/server/share` and fails to exist. Passing it through
    // is still the best of the available answers — there is no path that would work, and `cd`
    // reporting a directory that is not there beats silently landing in one that is.
    if unified.starts_with("//") {
        return unified;
    }

    // `C:/x` -> `<mount_root>/c/x`
    let bytes = unified.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && (bytes[0] as char).is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = &unified[2..];
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return if rest.is_empty() {
            format!("{mount_root}/{drive}")
        } else {
            format!("{mount_root}/{drive}/{rest}")
        };
    }

    // Already relative or already POSIX; leave it alone.
    unified
}

/// Quote a path so the shell receives it as one argument.
pub fn quote(path: &str, style: PathStyle) -> String {
    match style {
        // cmd.exe and PowerShell have different literal-string syntax.
        PathStyle::Windows => {
            if needs_quoting(path) {
                format!("\"{}\"", path.replace('"', "\"\""))
            } else {
                path.to_string()
            }
        },
        PathStyle::PowerShell => format!("'{}'", path.replace('\'', "''")),
        // POSIX single quotes are literal, so only `'` itself needs the close-escape-reopen
        // dance. This is safe against spaces, `$`, backticks and everything else.
        PathStyle::Msys | PathStyle::Wsl => {
            if needs_quoting(path) {
                format!("'{}'", path.replace('\'', r"'\''"))
            } else {
                path.to_string()
            }
        },
    }
}

fn needs_quoting(s: &str) -> bool {
    s.is_empty()
        || s.chars().any(|c| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '&' | '|' | '<' | '>' | '^' | '(' | ')' | '$' | '`' | ';')
        })
}

/// Translate and quote in one step — what a shortcut actually needs.
pub fn for_shell(path: &str, style: PathStyle) -> String {
    if style == PathStyle::Wsl {
        let stripped = strip_long_prefix(path);
        let unified = stripped.replace('\\', "/");
        for prefix in ["//wsl$/", "//wsl.localhost/"] {
            if unified.to_ascii_lowercase().starts_with(prefix) {
                let rest = &unified[prefix.len()..];
                let (distro, tail) = rest.split_once('/').unwrap_or((rest, ""));
                return format!(
                    "\"$(if [ \"$WSL_DISTRO_NAME\" = {} ]; then printf '%s' {}; else printf '%s\\n' 'Nyx: shortcut belongs to another WSL distribution' >&2; fi)\"",
                    posix_literal(distro), posix_literal(&format!("/{tail}")));
            }
        }
        let bytes = unified.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return format!("\"$(wslpath -u {})\"", posix_literal(stripped.as_ref()));
        }
    }
    quote(&translate(path, style), style)
}

fn posix_literal(path: &str) -> String { format!("'{}'", path.replace('\'', r"'\''")) }

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: PathStyle = PathStyle::Windows;
    const MSYS: PathStyle = PathStyle::Msys;
    const WSL: PathStyle = PathStyle::Wsl;

    #[test]
    fn powershell_paths_quote_variables_backticks_and_apostrophes_literally() {
        assert_eq!(for_shell(r"C:\$HOME\a`b\it's here", PathStyle::PowerShell),
            r"'C:\$HOME\a`b\it''s here'");
    }

    #[test]
    fn wsl_expansions_resolve_mounts_and_guard_the_distribution() {
        let drive = for_shell(r"Z:\a'b\$HOME", WSL);
        assert!(drive.contains("wslpath -u 'Z:\\a'\\''b\\$HOME'"));
        assert!(!drive.contains("/mnt/"));
        let unc = for_shell(r"\\wsl.localhost\Debian\home\x", WSL);
        assert!(unc.contains("[ \"$WSL_DISTRO_NAME\" = 'Debian' ]"));
        assert!(unc.contains("printf '%s' '/home/x'"));
        assert!(unc.contains("another WSL distribution"));
    }

    #[test]
    fn drive_letters_map_to_their_mount_points() {
        let p = r"C:\Users\Nihil\Desktop";
        assert_eq!(translate(p, WIN), r"C:\Users\Nihil\Desktop");
        assert_eq!(translate(p, MSYS), "/c/Users/Nihil/Desktop");
        assert_eq!(translate(p, WSL), "/mnt/c/Users/Nihil/Desktop");
    }

    #[test]
    fn drives_other_than_c_work_and_are_lowercased() {
        assert_eq!(translate(r"D:\data", WIN), r"D:\data");
        assert_eq!(translate(r"D:\data", MSYS), "/d/data");
        assert_eq!(translate(r"Z:\data", WSL), "/mnt/z/data");
        // A bare drive root must not produce a trailing-slash oddity.
        assert_eq!(translate(r"C:\", MSYS), "/c");
        assert_eq!(translate("C:", WSL), "/mnt/c");
    }

    #[test]
    fn long_path_prefixes_are_stripped() {
        // `\\?\C:\x` exists but nothing can cd to it.
        //
        // All three styles, deliberately. This table has two axes — the path shape and the shell
        // — and asserting the shapes against one column is what let the `\\wsl$\` rule be wrong
        // for Git Bash for as long as it existed. It reads exactly like coverage and is not.
        for style in [WIN, MSYS, WSL] {
            assert_eq!(
                translate(r"\\?\C:\Users\Nihil", style),
                translate(r"C:\Users\Nihil", style),
                "the long form differed from the short one as {style:?}"
            );
        }
        assert_eq!(translate(r"\\?\C:\Users\Nihil", WIN), r"C:\Users\Nihil");
        assert_eq!(translate(r"\\?\C:\Users\Nihil", MSYS), "/c/Users/Nihil");
        assert_eq!(translate(r"\\?\C:\Users\Nihil", WSL), "/mnt/c/Users/Nihil");
    }

    #[test]
    fn the_long_unc_form_is_the_same_share_as_the_short_one() {
        // `\\?\UNC\server\share` *is* `\\server\share`: the two leading slashes belong to the
        // path, not to the prefix. Dropping them left `server\share`, a relative path plausible
        // enough to resolve somewhere real and wrong.
        //
        // This was covered before and the expectation was the bug — the test asserted
        // `server\share`. Stated as an equivalence with the short form rather than a literal, so
        // it cannot drift away from `unc_shares_pass_through_rather_than_being_invented_a_mount`.
        for (long, short) in [
            (r"\\?\UNC\server\share", r"\\server\share"),
            (r"\\?\UNC\server\share\dir\file.txt", r"\\server\share\dir\file.txt"),
        ] {
            for style in [WIN, MSYS, WSL] {
                assert_eq!(translate(long, style), translate(short, style), "{long:?} as {style:?}");
            }
        }
    }

    #[test]
    fn wsl_internal_paths_do_not_get_a_mnt_prefix() {
        // `\\wsl$\Ubuntu\home\nihil` is `/home/nihil` *inside* Ubuntu. Prefixing /mnt points at
        // a directory that does not exist.
        assert_eq!(translate(r"\\wsl$\Ubuntu\home\nihil", WSL), "/home/nihil");
        assert_eq!(translate(r"\\wsl.localhost\Debian\etc\hosts", WSL), "/etc/hosts");
        assert_eq!(translate(r"\\wsl$\Ubuntu", WSL), "/");
    }

    #[test]
    fn a_wsl_distro_path_is_only_unwrapped_for_wsl_itself() {
        // `\\wsl$\Ubuntu\etc\hosts` is a UNC path. Only *inside* a WSL distro does it mean
        // `/etc/hosts`; to every other shell it is a network share like any other.
        //
        // Git Bash is the one that makes this expensive. `/etc/hosts` there is
        // `C:\Program Files\Git\etc\hosts` — a real file, and not the distro's. So a shortcut to
        // a distro's `/etc`, `/usr` or `/tmp` opened in a Git Bash pane silently landed in Git's
        // own copy of each: this module's stated failure mode, not a missing feature.
        assert_eq!(translate(r"\\wsl$\Ubuntu\etc\hosts", MSYS), "//wsl$/Ubuntu/etc/hosts");
        assert_eq!(
            translate(r"\\wsl.localhost\Debian\home\nihil", MSYS),
            "//wsl.localhost/Debian/home/nihil"
        );
        // Windows addresses it that way to begin with, so it comes back unchanged.
        assert_eq!(translate(r"\\wsl$\Ubuntu\etc\hosts", WIN), r"\\wsl$\Ubuntu\etc\hosts");
        // And a UNC path is what the neighbouring rule already does with every other share, so
        // this is one rule rather than a special case — see the test below.
        assert_eq!(translate(r"\\wsl$\Ubuntu\etc\hosts", WSL), "/etc/hosts");
    }

    #[test]
    fn unc_shares_pass_through_rather_than_being_invented_a_mount() {
        assert_eq!(translate(r"\\server\share\dir", MSYS), "//server/share/dir");
        assert_eq!(translate(r"\\server\share\dir", WIN), r"\\server\share\dir");
        // The WSL row is passthrough for want of anything better, **not** because WSL accepts it.
        // MSYS resolves `//server/share` through Windows' redirector; WSL has no SMB client in
        // its VFS and Microsoft's own documentation says a share must be mounted by hand with
        // `mount -t drvfs '\\server\share' /mnt/...`. Linux also collapses the leading `//`, so
        // what arrives is `/server/share/dir` and `cd` says it does not exist. This test records
        // that the answer is a loud failure rather than a working path — the comment here used to
        // claim WSL accepted the spelling, which is the kind of belief Tier 3 exists to check.
        assert_eq!(translate(r"\\server\share\dir", WSL), "//server/share/dir");
    }

    #[test]
    fn forward_slashes_are_normalised_for_windows() {
        assert_eq!(translate("C:/Users/Nihil", WIN), r"C:\Users\Nihil");
    }

    #[test]
    fn spaces_are_quoted_per_shell() {
        let p = r"C:\Users\Nihil\Desktop\Free Tibet";
        assert_eq!(for_shell(p, WIN), "\"C:\\Users\\Nihil\\Desktop\\Free Tibet\"");
        assert_eq!(for_shell(p, MSYS), "'/c/Users/Nihil/Desktop/Free Tibet'");
        assert_eq!(for_shell(p, WSL), "\"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet')\"");
    }

    #[test]
    fn a_path_without_anything_special_is_left_unquoted() {
        assert_eq!(for_shell(r"C:\Windows", WIN), r"C:\Windows");
        assert_eq!(for_shell(r"C:\Windows", MSYS), "/c/Windows");
    }

    #[test]
    fn single_quotes_inside_a_posix_path_are_escaped() {
        // The close-escape-reopen dance; getting this wrong ends the quoted string early and
        // hands the rest of the path to the shell as code.
        let out = quote("/c/it's here", MSYS);
        assert_eq!(out, r"'/c/it'\''s here'");
    }

    #[test]
    fn double_quotes_inside_a_windows_path_are_doubled() {
        let out = quote(r#"C:\a"b"#, WIN);
        assert_eq!(out, r#""C:\a""b""#);
    }

    #[test]
    fn shell_metacharacters_force_quoting_even_without_spaces() {
        // `C:\a&b` unquoted in cmd runs `b` as a second command.
        assert!(for_shell(r"C:\a&b", WIN).starts_with('"'));
        assert!(for_shell("/c/a$b", MSYS).starts_with('\''));
        assert!(for_shell("/c/a;b", WSL).starts_with('\''));
    }

    #[test]
    fn round_trip_of_a_realistic_project_path() {
        let p = r"C:\Users\Nihil\Desktop\Free Tibet\Nyx";
        assert_eq!(translate(p, MSYS), "/c/Users/Nihil/Desktop/Free Tibet/Nyx");
        assert_eq!(translate(p, WSL), "/mnt/c/Users/Nihil/Desktop/Free Tibet/Nyx");
        assert_eq!(for_shell(p, WSL), "\"$(wslpath -u 'C:\\Users\\Nihil\\Desktop\\Free Tibet\\Nyx')\"");
    }
}
