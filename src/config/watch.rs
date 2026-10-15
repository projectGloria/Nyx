//! Hot reload.
//!
//! **The watch is on the directory, not the file.** Every competent editor saves by writing a
//! temporary file and renaming it over the target, which destroys the inode the watch was
//! attached to. Watching the file directly works exactly once and then silently stops — and the
//! bug report says "it works sometimes".

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::load;
use crate::event::{AppEvent, RepaintCoalescer};

/// Coalesce the burst of events a single save produces (a create, a rename, and one or more
/// writes) into one reload.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps the watcher alive. Dropping this stops hot reload.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

/// Watch the config directory and send a reload whenever the file changes on disk.
pub fn spawn(
    tx: crossbeam_channel::Sender<AppEvent>,
    repaint: Arc<RepaintCoalescer>,
) -> Option<ConfigWatcher> {
    let path = crate::config::paths::config_file();
    let dir = path.parent()?.to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("cannot watch config dir {}: {err}", dir.display());
        return None;
    }

    let (raw_tx, raw_rx) = crossbeam_channel::unbounded::<()>();

    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // The event's own paths are unreliable across the rename dance, so any change in the
        // directory just means "look again". Re-reading one small file is cheaper than being
        // clever about which event was the real one.
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    }) {
        Ok(w) => w,
        Err(err) => {
            log::warn!("config watcher unavailable: {err}");
            return None;
        },
    };

    if let Err(err) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        log::warn!("cannot watch {}: {err}", dir.display());
        return None;
    }

    std::thread::Builder::new()
        .name("nyx-config-watch".into())
        .spawn(move || debounce_loop(raw_rx, path, tx, repaint))
        .ok()?;

    Some(ConfigWatcher { _watcher: watcher })
}

fn debounce_loop(
    raw_rx: crossbeam_channel::Receiver<()>,
    path: PathBuf,
    tx: crossbeam_channel::Sender<AppEvent>,
    repaint: Arc<RepaintCoalescer>,
) {
    let mut last_hash = current_hash(&path);

    while raw_rx.recv().is_ok() {
        // Drain the rest of the burst.
        while raw_rx.recv_timeout(DEBOUNCE).is_ok() {}

        let hash = current_hash(&path);
        if hash == last_hash {
            // Either a file we do not care about changed, or this is the settings GUI's own
            // write coming back at us. Reloading here would fight the user's cursor.
            continue;
        }
        last_hash = hash;

        // A file that did not parse produces no config, so there is nothing to apply and the
        // running one is left alone. Forwarding `Config::default()` here is what used to make a
        // half-saved file revert the theme, font, keys, aliases, shortcuts and Quake settings.
        let event = match load::load_from(&path) {
            load::Loaded::Parsed { config, problems } => AppEvent::ConfigReloaded {
                config: Arc::new(*config),
                problems,
                // Carried so the app can recognise its *own* save arriving back. The settings GUI
                // writes this file, and applying the echo would reset the draft under the cursor
                // of whoever is still dragging a colour picker.
                hash,
            },
            load::Loaded::Failed { problems } => AppEvent::ConfigInvalid { problems },
        };
        if tx.send(event).is_err() {
            break;
        }
        repaint.poke();
    }
}

/// Cheap content fingerprint, used only to notice "nothing actually changed".
fn current_hash(path: &std::path::Path) -> u64 {
    match std::fs::read(path) {
        Ok(bytes) => hash_bytes(&bytes),
        Err(_) => hash_bytes(&[]),
    }
}

/// The same fingerprint, over bytes we are about to write.
///
/// Both sides have to use this one function: the app compares the hash it computed over what it
/// saved against the hash the watcher computed over what landed on disk, and two hashers that
/// merely look equivalent would make every save look like an external edit.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

