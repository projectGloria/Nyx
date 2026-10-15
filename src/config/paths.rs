//! The only file that names a location on disk.
//!
//! Never `exe_dir()`. An installed Nyx under `Program Files` cannot write beside itself, and the
//! failure is not loud — it is a silent loss of every setting on every launch.

use std::path::PathBuf;

/// `%APPDATA%\Nyx` — user-authored, roams with the Windows profile.
pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Nyx")
}

/// `%LOCALAPPDATA%\Nyx` — derived and rebuildable, deliberately machine-local.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("Nyx")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `%LOCALAPPDATA%\Nyx\logs` — release builds only, and the `cfg` says so rather than a comment.
///
/// A debug build has a console and logs to stderr, where the output is immediate and nobody has
/// to go and find a file. A release build has no console at all, which is why this exists; see
/// `main::init_logging`. Compiling it out of debug is what keeps "never used" meaning something
/// here instead of being a warning everyone has learned to scroll past.
#[cfg(not(debug_assertions))]
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_data_are_different_roots() {
        // On Windows these are Roaming and Local respectively. Collapsing them would make the
        // generated shell scripts and icon cache roam between machines, which is exactly what
        // the split exists to prevent.
        assert_ne!(config_dir(), data_dir());
        assert!(config_file().starts_with(config_dir()));
    }
}
