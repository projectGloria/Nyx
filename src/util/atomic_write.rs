//! Write a file without ever leaving a half-written one on disk.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` via a temporary file in the same directory, then rename over it.
///
/// The temp file must be a sibling, not in the system temp directory: `rename` is only atomic
/// within a volume, and `%TEMP%` is frequently on a different one. The flush-then-sync before the
/// rename is what stops a crash mid-write leaving a zero-length config where a valid one was —
/// the rename itself is atomic, but the data behind it is not until it has reached the disk.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    fs::create_dir_all(dir)?;

    let count = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let tmp = path.with_extension(format!(
        "tmp{}-{}-{}",
        std::process::id(),
        nanos,
        count
    ));

    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
    }

    // `fs::rename` replaces an existing file on Windows as well as on POSIX — it goes through
    // `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`, which the test below records rather than
    // assumes. So a failure here is never "the destination is in the way"; it is a lock, a
    // permission, a read-only attribute or a scanner holding a handle. Deleting the destination
    // and retrying is useless against all four, and it is the one move that can turn a save that
    // merely failed into a file that is gone.
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_overwrites_without_leaving_temp_files() {
        let dir = std::env::temp_dir().join(format!("nyx-atomic-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("thing.toml");

        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");

        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "thing.toml")
            .collect();
        assert!(leftovers.is_empty(), "left temp files behind: {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_replaces_an_existing_file_on_this_platform() {
        // The fallback below existed because "Windows `rename` refuses to clobber an existing
        // file, unlike POSIX". That is not true of `std::fs::rename`, which goes through
        // MoveFileExW with MOVEFILE_REPLACE_EXISTING. Recorded as a test rather than trusted from
        // memory, because the argument for having no fallback at all rests entirely on it.
        let dir = std::env::temp_dir().join(format!("nyx-atomic-clobber-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (from, onto) = (dir.join("from"), dir.join("onto"));

        fs::write(&from, b"new").unwrap();
        fs::write(&onto, b"old").unwrap();
        fs::rename(&from, &onto).expect("rename over an existing file");

        assert_eq!(fs::read(&onto).unwrap(), b"new");
        assert!(!from.exists(), "the source survived the rename");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    // Clearing the read-only bit afterwards is this test's cleanup, not the pattern the lint
    // exists to catch — the point of the test is what happens while the file *is* read-only.
    #[allow(clippy::permissions_set_readonly_false)]
    fn a_rename_that_fails_keeps_the_original_and_leaves_no_temp_file() {
        // The old fallback read *any* rename failure as "the destination is in the way": it
        // deleted the destination and retried. Two things are wrong with that, and only the
        // first is visible from a test.
        //
        // Visible here: it defeats the read-only attribute. The rename is refused, the file is
        // deleted anyway, the retry succeeds, and a file the user marked read-only is replaced
        // without a word.
        //
        // Not reproducible here, and the reason the fallback is removed rather than narrowed: if
        // the failure has a cause deleting does not clear -- a lock, a scanner holding a handle
        // -- the retry fails too, and by then the destination is already gone. That is a failed
        // save that took the file with it.
        let dir = std::env::temp_dir().join(format!("nyx-atomic-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");

        write_atomic(&path, b"the good one").unwrap();

        // Read-only is the cheapest stand-in for "this rename will not succeed"; a lock or a
        // permission would arrive here the same way.
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();

        let result = write_atomic(&path, b"the replacement");

        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(&path, perms).unwrap();

        // Whatever went wrong, what was on disk is still on disk and still says what it said.
        assert_eq!(fs::read(&path).unwrap(), b"the good one", "the original was destroyed");

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "config.toml")
            .collect();
        assert!(leftovers.is_empty(), "left temp files behind: {leftovers:?}");

        // And it said so, rather than reporting a write that did not happen as a success.
        assert!(result.is_err(), "a write that did not happen reported success");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_missing_directories() {
        let dir = std::env::temp_dir().join(format!("nyx-atomic-deep-{}", std::process::id()));
        let path = dir.join("a").join("b").join("c.toml");
        write_atomic(&path, b"x").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"x");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writes_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("nyx-atomic-concurrent-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = dir.join(format!("file_{i}.txt"));
                std::thread::spawn(move || {
                    let content = format!("content_{i}");
                    write_atomic(&p, content.as_bytes()).unwrap();
                    assert_eq!(fs::read(&p).unwrap(), content.as_bytes());
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
