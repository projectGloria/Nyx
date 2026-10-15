//! Writing `config.toml` back without erasing what the user wrote around it.
//!
//! `config.toml` is the primary interface — hand-authored, commented, and in the user's own
//! order. The settings GUI is a second way to edit the same file, so saving from it has to be a
//! *merge*, not a serialisation: parsing into a value tree and re-emitting would throw away every
//! comment in the file the first time anyone touched a colour.
//!
//! Three rules make that work, and each has a test below.
//!
//! **Only keys whose value actually changed are written.** A colour-picker drag rewrites one line
//! and leaves the other two hundred byte-identical, so formatting, key order and alignment survive
//! by never being touched.
//!
//! **A key that is absent and still at its default stays absent.** Otherwise the first save would
//! inflate a deliberately minimal file into sixty keys nobody asked for, and every shortcut would
//! sprout `order = 0` and `sigil = ""`.
//!
//! **Array-of-tables entries are edited in place.** `[[shortcut]]` #2 keeps the comment above it
//! when #2 is what you renamed; only adding, removing or reordering disturbs anything.

use std::collections::BTreeMap;

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::config::schema::{Config, ProfileConfig, ShortcutConfig};
use crate::util::write_atomic;

/// Save the config to `%APPDATA%\Nyx\config.toml`.
///
/// Returns the exact bytes written. The caller keeps them so it can recognise its own write
/// arriving back through the file watcher — reloading there would fight the cursor of whoever is
/// still dragging a slider.
pub fn save(config: &Config) -> std::io::Result<Vec<u8>> {
    let path = crate::config::paths::config_file();
    save_to(&path, config)
}

fn save_to(path: &std::path::Path, config: &Config) -> std::io::Result<Vec<u8>> {
    let existing = base_document(path)?;
    // A BOM belongs to the file, not to the document: parsing with it in place makes the first
    // table header part of a key name.
    let existing = existing
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing)
        .to_string();

    let text = match render(&existing, config) {
        Ok(text) => text,
        Err(err) => {
            // The file on disk is broken, which is very likely *why* someone opened the settings
            // window. Refusing to save would be a dead end, so keep their file beside the new one
            // rather than either overwriting it silently or giving up.
            let backup = backup_original(path)?;
            log::warn!(
                "config.toml did not parse ({err}); kept as {}",
                backup.display()
            );
            render(super::load::DEFAULT_CONFIG, config).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
            })?
        }
    };

    let bytes = text.into_bytes();
    write_atomic(path, &bytes)?;
    Ok(bytes)
}

fn backup_original(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    let bytes = std::fs::read(path)?;
    for suffix in 0..1024 {
        let extension = if suffix == 0 { "toml.bak".to_string() } else { format!("toml.bak.{suffix}") };
        let backup = path.with_extension(extension);
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && backup.is_file() => continue,
            Err(err) => return Err(err),
        };
        // Never overwrite the document unless an exact, durable backup was written.
        if let Err(err) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&backup);
            return Err(err);
        }
        return Ok(backup);
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "too many config backups"))
}

/// The document the merge is based on: the file on disk, or the documented default when there is
/// no file yet.
///
/// A file that exists but cannot be read is an error, not a reason to start from the default.
/// Rendering `DEFAULT_CONFIG` into the merge would have written a comment-less config straight
/// over one that was merely locked or not valid UTF-8 — a save that *succeeded* taking the user's
/// comments with it. Refusing costs a toast; overwriting cost the file's own text.
fn base_document(path: &std::path::Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(super::load::DEFAULT_CONFIG.to_string())
        },
        Err(err) => Err(std::io::Error::new(
            err.kind(),
            format!("config.toml exists but could not be read: {err}"),
        )),
    }
}

/// Merge `config` into the text of an existing file.
pub fn render(existing: &str, config: &Config) -> Result<String, toml_edit::TomlError> {
    let before: DocumentMut = existing.parse()?;
    let mut doc: DocumentMut = existing.parse()?;
    apply(doc.as_table_mut(), config);
    prune_empty(doc.as_table_mut(), Some(before.as_table()));
    Ok(match_line_endings(doc.to_string(), existing))
}

/// Remove sections we created and then wrote nothing into.
///
/// `ensure_table` has to make the table before the setters can decide whether they have anything
/// to put in it, so a two-line config would otherwise come back with seven empty `[section]`
/// headers appended — which reads as the settings window having vandalised the file. A section the
/// user wrote and deliberately left empty is theirs, and `before` is how it is told apart.
fn prune_empty(table: &mut Table, before: Option<&Table>) {
    // Depth first: a parent is only empty once its own empty children are gone.
    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();
    for key in keys {
        let before_child = before.and_then(|b| b.get(&key)).and_then(Item::as_table);
        if let Some(Item::Table(child)) = table.get_mut(&key) {
            prune_empty(child, before_child);
        }
    }

    table.retain(|key, item| match item {
        Item::Table(t) => !t.is_empty() || before.is_some_and(|b| b.contains_table(key)),
        Item::ArrayOfTables(a) => {
            !a.is_empty() || before.is_some_and(|b| b.contains_array_of_tables(key))
        }
        _ => true,
    });
}

/// toml_edit emits `\n` whatever it read, and every editor on Windows writes `\r\n`.
///
/// Without this a save that changed one colour rewrites every line ending in the file, which is a
/// whole-file diff in git and "the settings window rewrote my config" to anyone looking at it.
fn match_line_endings(rendered: String, original: &str) -> String {
    let crlf = original.matches("\r\n").count();
    let lf = original.matches('\n').count();
    if crlf * 2 <= lf {
        return rendered;
    }
    // Normalise first, so a line that already ends `\r\n` does not become `\r\r\n`.
    rendered.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn apply(root: &mut Table, config: &Config) {
    let d = Config::default();
    let sounds = ensure_table(root, "sounds");
    set_str(sounds, "pack", &config.sounds.pack, &d.sounds.pack);
    set_float(sounds, "volume", config.sounds.volume, d.sounds.volume);
    let launcher = ensure_table(root, "launcher");
    set_bool(
        launcher,
        "enabled",
        config.launcher.enabled,
        d.launcher.enabled,
    );
    set_str(
        launcher,
        "hotkey",
        &config.launcher.hotkey,
        &d.launcher.hotkey,
    );

    let expand = ensure_table(root, "expand");
    set_bool(
        expand,
        "completion",
        config.expand.completion,
        d.expand.completion,
    );
    set_bool(expand, "hint", config.expand.hint, d.expand.hint);

    let t = &config.theme;
    let dt = &d.theme;
    let theme = ensure_table(root, "theme");
    set_str(theme, "name", &t.name, &dt.name);
    set_str(theme, "accent", &t.accent, &dt.accent);
    set_str(theme, "surface", &t.surface, &dt.surface);
    set_str(theme, "background", &t.background, &dt.background);
    set_str(theme, "foreground", &t.foreground, &dt.foreground);
    set_str(theme, "cursor", &t.cursor, &dt.cursor);
    set_str(theme, "cursor_text", &t.cursor_text, &dt.cursor_text);
    set_str(theme, "selection", &t.selection, &dt.selection);
    set_opt_str(theme, "selection_text", t.selection_text.as_deref());
    set_str(theme, "search", &t.search, &dt.search);
    set_str(theme, "search_text", &t.search_text, &dt.search_text);
    set_str(
        theme,
        "search_current",
        &t.search_current,
        &dt.search_current,
    );
    set_str(
        theme,
        "search_current_text",
        &t.search_current_text,
        &dt.search_current_text,
    );
    set_bool(theme, "bold_is_bright", t.bold_is_bright, dt.bold_is_bright);

    const ANSI: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for (name, group, default) in [
        ("normal", &t.normal, &dt.normal),
        ("bright", &t.bright, &dt.bright),
    ] {
        let table = ensure_table(ensure_table(root, "theme"), name);
        for ((key, v), dv) in ANSI.iter().zip(group.as_array()).zip(default.as_array()) {
            set_str(table, key, v, dv);
        }
    }

    let font = ensure_table(root, "font");
    set_str(font, "family", &config.font.family, &d.font.family);
    set_float(font, "size", config.font.size, d.font.size);
    set_float(
        font,
        "line_height",
        config.font.line_height,
        d.font.line_height,
    );
    set_bool(font, "ligatures", config.font.ligatures, d.font.ligatures);

    let r = &config.render;
    let dr = &d.render;
    let render = ensure_table(root, "render");
    set_str(
        render,
        "cursor_style",
        r.cursor_style.name(),
        dr.cursor_style.name(),
    );
    set_str(render, "cursor_icon", &r.cursor_icon, &dr.cursor_icon);
    set_float(render, "padding", r.padding, dr.padding);
    set_int(
        render,
        "scrollback",
        r.scrollback as i64,
        dr.scrollback as i64,
    );
    set_bool(render, "cursor_blink", r.cursor_blink, dr.cursor_blink);
    set_int(
        render,
        "cursor_blink_ms",
        r.cursor_blink_ms as i64,
        dr.cursor_blink_ms as i64,
    );

    let window = ensure_table(root, "window");
    set_float(window, "width", config.window.width, d.window.width);
    set_float(window, "height", config.window.height, d.window.height);
    set_bool(
        window,
        "single_instance",
        config.window.single_instance,
        d.window.single_instance,
    );

    let q = &config.quake;
    let dq = &d.quake;
    let quake = ensure_table(root, "quake");
    set_bool(quake, "enabled", q.enabled, dq.enabled);
    set_str(quake, "hotkey", &q.hotkey, &dq.hotkey);
    set_bool(quake, "dropdown", q.dropdown, dq.dropdown);
    set_float(quake, "width", q.width, dq.width);
    set_float(quake, "height", q.height, dq.height);
    set_str(quake, "align", &q.align, &dq.align);
    set_str(quake, "monitor", &q.monitor, &dq.monitor);
    set_int(
        quake,
        "animate_ms",
        q.animate_ms as i64,
        dq.animate_ms as i64,
    );
    set_bool(
        quake,
        "hide_on_focus_loss",
        q.hide_on_focus_loss,
        dq.hide_on_focus_loss,
    );
    set_bool(quake, "start_hidden", q.start_hidden, dq.start_hidden);

    let k = &config.keys;
    let dk = &d.keys;
    let keys = ensure_table(root, "keys");
    for (name, spec, default) in [
        ("new_tab", &k.new_tab, &dk.new_tab),
        ("close_pane", &k.close_pane, &dk.close_pane),
        ("next_tab", &k.next_tab, &dk.next_tab),
        ("prev_tab", &k.prev_tab, &dk.prev_tab),
        ("split_right", &k.split_right, &dk.split_right),
        ("split_down", &k.split_down, &dk.split_down),
        ("focus_left", &k.focus_left, &dk.focus_left),
        ("focus_right", &k.focus_right, &dk.focus_right),
        ("focus_up", &k.focus_up, &dk.focus_up),
        ("focus_down", &k.focus_down, &dk.focus_down),
        ("zoom_pane", &k.zoom_pane, &dk.zoom_pane),
        ("palette", &k.palette, &dk.palette),
        ("find", &k.find, &dk.find),
        ("settings", &k.settings, &dk.settings),
    ] {
        set_str(keys, name, spec, default);
    }

    let leader = ensure_table(root, "leader");
    set_str(leader, "key", &config.leader.key, &d.leader.key);
    set_int(
        leader,
        "timeout_ms",
        config.leader.timeout_ms as i64,
        d.leader.timeout_ms as i64,
    );
    if !config.leader.chords.is_empty() || root_has(root, "leader", "chords") {
        let chords = ensure_table(ensure_table(root, "leader"), "chords");
        sync_map(chords, &config.leader.chords);
    }

    if !config.aliases.is_empty() || root.contains_key("alias") {
        sync_map(ensure_table(root, "alias"), &config.aliases);
    }

    shortcuts(root, &config.shortcuts);
    profiles(root, &config.profiles);
}

fn root_has(root: &Table, table: &str, key: &str) -> bool {
    root.get(table)
        .and_then(Item::as_table)
        .is_some_and(|t| t.contains_key(key))
}

// ---- array-of-tables -------------------------------------------------------------------------

fn shortcuts(root: &mut Table, list: &[ShortcutConfig]) {
    if list.is_empty() && !root.contains_key("shortcut") {
        return;
    }
    let d = ShortcutConfig::default();
    let aot = ensure_aot(root, "shortcut");
    for (i, s) in list.iter().enumerate() {
        if aot.len() <= i {
            aot.push(Table::new());
        }
        let Some(t) = aot.get_mut(i) else { continue };
        set_str(t, "id", &s.id, &d.id);
        set_str(t, "label", &s.label, &d.label);
        set_str(t, "sigil", &s.sigil, &d.sigil);
        set_str(t, "path", &s.path, &d.path);
        set_str(t, "icon", &s.icon, &d.icon);
        set_opt_str(t, "color", s.color.as_deref());
        set_int(t, "order", s.order as i64, d.order as i64);
    }
    truncate(aot, list.len());
}

fn profiles(root: &mut Table, list: &[ProfileConfig]) {
    if list.is_empty() && !root.contains_key("profile") {
        return;
    }
    let d = ProfileConfig::default();
    let aot = ensure_aot(root, "profile");
    for (i, p) in list.iter().enumerate() {
        if aot.len() <= i {
            aot.push(Table::new());
        }
        let Some(t) = aot.get_mut(i) else { continue };
        set_str(t, "id", &p.id, &d.id);
        // An empty override field is written as *absent*, not as `""`. The two mean different
        // things here — absent inherits from detection, `""` would be a literal empty program —
        // and writing the wrong one turns "use whatever was found" into "launch nothing".
        set_opt_str(t, "name", non_empty(&p.name));
        set_opt_str(t, "program", non_empty(&p.program));
        set_string_array(t, "args", p.args.as_deref());
        set_opt_str(t, "kind", non_empty(&p.kind));
        set_opt_str(t, "cwd", non_empty(&p.cwd));
        set_str(t, "integration", &p.integration, &d.integration);
        set_bool(t, "enabled", p.enabled, d.enabled);
    }
    truncate(aot, list.len());
}

fn non_empty(s: &str) -> Option<&str> {
    (!s.trim().is_empty()).then_some(s)
}

fn truncate(aot: &mut ArrayOfTables, len: usize) {
    while aot.len() > len {
        aot.remove(len);
    }
}

fn ensure_aot<'a>(parent: &'a mut Table, key: &str) -> &'a mut ArrayOfTables {
    if !parent.contains_array_of_tables(key) {
        parent.insert(key, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_array_of_tables_mut)
        .expect("just ensured an array of tables")
}

/// A real `[section]`, not an inline one.
///
/// `parent[key] = …` on a missing key produces `theme = { background = "#000" }` — valid TOML,
/// but not what the rest of the file looks like, and a user who then wants to comment one colour
/// cannot.
fn ensure_table<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
    if !parent.contains_table(key) {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .expect("just ensured a table")
}

// ---- setters ---------------------------------------------------------------------------------
//
// Each takes the schema default as well as the value: a key that is absent from the file and
// still at its default is left absent.

fn set_str(t: &mut Table, key: &str, v: &str, default: &str) {
    if !t.contains_key(key) && v == default {
        return;
    }
    if t.get(key).and_then(Item::as_str) == Some(v) {
        return;
    }
    t[key] = value(v);
}

fn set_opt_str(t: &mut Table, key: &str, v: Option<&str>) {
    match v {
        Some(v) => {
            if t.get(key).and_then(Item::as_str) != Some(v) {
                t[key] = value(v);
            }
        }
        None => {
            t.remove(key);
        }
    }
}

fn set_bool(t: &mut Table, key: &str, v: bool, default: bool) {
    if !t.contains_key(key) && v == default {
        return;
    }
    if t.get(key).and_then(Item::as_bool) == Some(v) {
        return;
    }
    t[key] = value(v);
}

fn set_int(t: &mut Table, key: &str, v: i64, default: i64) {
    if !t.contains_key(key) && v == default {
        return;
    }
    if t.get(key).and_then(Item::as_integer) == Some(v) {
        return;
    }
    t[key] = value(v);
}

/// TOML distinguishes `13` from `13.0`; Rust's `f32` does not.
///
/// A user who wrote `size = 13` must not have it rewritten to `13.0` by a save that changed a
/// colour, so an integer counts as equal to the float it denotes.
///
/// Both halves are done at `f32`, because that is the precision the schema actually holds. TOML
/// parses `0.45` as an `f64`; the schema narrows it to `0.45f32`; widening that back gives
/// `0.44999998807907104`, which is not the number in the file. Comparing at `f64` therefore says
/// "changed" on every single save, and writing at `f64` puts seventeen digits in the file where
/// the user wrote two — for a value nobody touched.
fn set_float(t: &mut Table, key: &str, v: f32, default: f32) {
    if !t.contains_key(key) && v == default {
        return;
    }
    let current = t
        .get(key)
        .and_then(|i| i.as_float().or_else(|| i.as_integer().map(|n| n as f64)));
    if current.map(|c| c as f32) == Some(v) {
        return;
    }
    // `f32::to_string` is the shortest decimal that reads back as this exact float, which is what
    // keeps the written form as short as the one a person would have typed.
    t[key] = value(v.to_string().parse::<f64>().unwrap_or(v as f64));
}

fn set_string_array(t: &mut Table, key: &str, v: Option<&[String]>) {
    let Some(items) = v else {
        t.remove(key);
        return;
    };
    let current: Option<Vec<String>> = t.get(key).and_then(Item::as_array).map(|a| {
        a.iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect()
    });
    if current.as_deref() == Some(items) {
        return;
    }
    let mut array = Array::new();
    for s in items {
        array.push(s.as_str());
    }
    t[key] = Item::Value(Value::Array(array));
}

/// Make an open-ended table match a map: add what is new, update what changed, remove what went.
fn sync_map(t: &mut Table, wanted: &BTreeMap<String, String>) {
    t.retain(|key, _| wanted.contains_key(key));
    for (key, v) in wanted {
        if t.get(key.as_str()).and_then(Item::as_str) != Some(v.as_str()) {
            t[key.as_str()] = value(v.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_config_is_preserved_and_a_failed_backup_refuses_overwrite() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("nyx-config-backup-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let backup = path.with_extension("toml.bak");
        let original = "\u{feff}[broken\r\n# keep my comments\r\n";
        std::fs::write(&path, original).unwrap();
        std::fs::create_dir(&backup).unwrap();
        assert!(save_to(&path, &Config::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir(&backup).unwrap();
        save_to(&path, &Config::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        std::fs::write(&path, "[a second broken document").unwrap();
        save_to(&path, &Config::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(path.with_extension("toml.bak.1")).unwrap(), "[a second broken document");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(backup).unwrap();
        std::fs::remove_file(dir.join("config.toml.bak.1")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    use crate::config::load;

    fn round_trip(config: &Config) -> Config {
        let text = render(load::DEFAULT_CONFIG, config).unwrap();
        load::parse(&text).expect_parsed()
    }

    #[test]
    fn launcher_and_sound_preferences_round_trip() {
        let mut config = Config::default();
        config.sounds.pack = "IBM Model M".into();
        config.sounds.volume = 0.75;
        config.launcher.enabled = true;
        config.launcher.hotkey = "Ctrl+Shift+Space".into();
        let restored = round_trip(&config);
        assert_eq!(restored.sounds, config.sounds);
        assert_eq!(restored.launcher, config.launcher);
    }

    #[test]
    fn saving_an_unchanged_config_does_not_touch_the_file() {
        // The strongest form of "comments survive": the bytes come back identical, so there is
        // nothing for a formatter to have quietly reflowed.
        let parsed = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        let out = render(load::DEFAULT_CONFIG, &parsed).unwrap();
        assert_eq!(out, load::DEFAULT_CONFIG);
    }

    #[test]
    fn changing_one_colour_changes_exactly_one_line() {
        let mut config = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        config.theme.background = "#000000".into();
        let out = render(load::DEFAULT_CONFIG, &config).unwrap();

        assert!(out.contains("# Nyx configuration."));
        assert!(out.contains("the character under a block cursor"));
        assert!(out.contains("background = \"#000000\""));

        let before: Vec<&str> = load::DEFAULT_CONFIG.lines().collect();
        let after: Vec<&str> = out.lines().collect();
        assert_eq!(before.len(), after.len(), "the line count moved");
        let differing: Vec<&&str> = after
            .iter()
            .zip(&before)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a)
            .collect();
        assert_eq!(
            differing.len(),
            1,
            "changed more than one line: {differing:?}"
        );
    }

    #[test]
    fn windows_line_endings_survive_a_save() {
        // Every editor on this platform writes CRLF, and a save that silently converted the whole
        // file to LF would show up as "the settings window rewrote my entire config".
        let source =
            "# a comment\r\n[theme]\r\nbackground = \"#111111\"\r\n\r\n[font]\r\nsize = 13.0\r\n";
        let mut config = load::parse(source).expect_parsed();
        config.theme.background = "#222222".into();
        let out = render(source, &config).unwrap();
        assert_eq!(
            out.matches("\r\n").count(),
            source.matches("\r\n").count(),
            "{out:?}"
        );
        assert!(out.contains("background = \"#222222\"\r\n"), "{out:?}");
    }

    #[test]
    fn a_default_key_that_was_absent_stays_absent() {
        // Otherwise one save turns a deliberately small file into the whole schema.
        let source = "[font]\nsize = 20.0\n";
        let config = load::parse(source).expect_parsed();
        let out = render(source, &config).unwrap();
        assert!(!out.contains("family"), "{out}");
        assert!(!out.contains("ligatures"), "{out}");
        assert!(!out.contains("new_tab"), "{out}");
        assert_eq!(load::parse(&out).expect_parsed().font.size, 20.0);
    }

    #[test]
    fn a_small_file_does_not_sprout_empty_sections() {
        // The sections have to be created before the setters can decide they have nothing to put
        // in them. Seven empty `[section]` headers appended to a two-line file reads as the
        // settings window having vandalised it.
        let source = "[font]\nsize = 20.0\n";
        let config = load::parse(source).expect_parsed();
        let out = render(source, &config).unwrap();
        assert_eq!(out, source, "{out:?}");
    }

    #[test]
    fn a_section_the_user_left_empty_is_left_alone() {
        // Theirs, not ours: they may be about to type into it.
        let source = "[keys]\n\n[alias]\n";
        let config = load::parse(source).expect_parsed();
        let out = render(source, &config).unwrap();
        assert!(out.contains("[keys]"), "{out}");
        assert!(out.contains("[alias]"), "{out}");
    }

    #[test]
    fn a_float_the_schema_narrows_is_not_rewritten_on_every_save() {
        // `0.45` is an f64 in the file, an f32 in the schema, and 0.44999998807907104 if you
        // widen it back — so a comparison done at f64 says "changed" on every save and writes
        // seventeen digits over the two the user typed. Found by the default config failing to
        // round-trip once `quake.height = 0.45` existed.
        let source = "[quake]\nheight = 0.45\n[theme]\nbackground = \"#111111\"\n";
        let mut config = load::parse(source).expect_parsed();
        assert_eq!(config.quake.height, 0.45);
        config.theme.background = "#000000".into();

        let out = render(source, &config).unwrap();
        assert!(out.contains("height = 0.45\n"), "{out}");
        assert!(!out.contains("0.4499"), "{out}");

        // And a value that really did change is written in its shortest form, not its widest.
        config.quake.height = 0.7;
        let out = render(source, &config).unwrap();
        assert!(out.contains("height = 0.7\n"), "{out}");
    }

    #[test]
    fn an_integer_where_a_float_was_expected_is_not_rewritten() {
        // `size = 13` is valid and means 13.0; rewriting it as `13.0` on an unrelated save shows
        // the user a diff for something they did not change.
        let source = "[font]\nsize = 13\n[theme]\nbackground = \"#111111\"\n";
        let mut config = load::parse(source).expect_parsed();
        config.theme.background = "#000000".into();
        let out = render(source, &config).unwrap();
        assert!(out.contains("size = 13\n"), "{out}");
    }

    #[test]
    fn a_missing_table_is_added_as_a_section_not_an_inline_table() {
        let mut config = Config::default();
        config.leader.key = "Ctrl+A".into();
        config.leader.chords.insert("c".into(), "tab.new".into());
        let out = render("", &config).unwrap();
        assert!(out.contains("[leader]"), "{out}");
        assert!(!out.contains("leader = {"), "{out}");
        assert!(out.contains("[leader.chords]"), "{out}");
        assert_eq!(load::parse(&out).expect_parsed().leader.key, "Ctrl+A");
    }

    #[test]
    fn removing_a_shortcut_removes_its_table_and_keeps_the_others() {
        let mut config = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        assert_eq!(config.shortcuts.len(), 3);
        config.shortcuts.remove(1); // "tibet"

        let out = render(load::DEFAULT_CONFIG, &config).unwrap();
        let reloaded = load::parse(&out).expect_parsed();
        let ids: Vec<&str> = reloaded.shortcuts.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["home", "nyx"]);
        assert_eq!(out.matches("[[shortcut]]").count(), 2);
    }

    #[test]
    fn a_renamed_shortcut_keeps_the_comment_block_above_it() {
        // In-place editing is the whole reason the array is not simply rebuilt.
        let source = "# the good one\n[[shortcut]]\nid = \"a\"\npath = 'C:\\a'\n";
        let mut config = load::parse(source).expect_parsed();
        config.shortcuts[0].label = "Renamed".into();
        let out = render(source, &config).unwrap();
        assert!(out.contains("# the good one"), "{out}");
        assert!(out.contains("label = \"Renamed\""), "{out}");
    }

    #[test]
    fn adding_a_shortcut_appends_a_table_without_filler_keys() {
        let mut config = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        config.shortcuts.push(ShortcutConfig {
            id: "docs".into(),
            label: "Docs".into(),
            sigil: "@docs".into(),
            path: r"C:\Users\Nihil\Documents".into(),
            icon: "builtin:folder".into(),
            ..Default::default()
        });
        let out = render(load::DEFAULT_CONFIG, &config).unwrap();
        let reloaded = load::parse(&out).expect_parsed();
        assert_eq!(reloaded.shortcuts.len(), 4);
        assert_eq!(reloaded.shortcuts[3].path, r"C:\Users\Nihil\Documents");
        // `order = 0` is the default and was never in the file.
        let tail = &out[out.rfind("[[shortcut]]").unwrap()..];
        assert!(!tail.contains("order"), "{tail}");
    }

    #[test]
    fn a_windows_path_survives_the_round_trip_exactly() {
        // Whatever quoting toml_edit picks, the backslashes have to come back as backslashes —
        // getting this wrong points a shortcut at a directory that does not exist.
        let mut config = Config::default();
        config.shortcuts.push(ShortcutConfig {
            id: "x".into(),
            path: r"C:\Users\Nihil\Desktop\Free Tibet".into(),
            ..Default::default()
        });
        let reloaded = round_trip(&config);
        assert_eq!(
            reloaded.shortcuts[0].path,
            r"C:\Users\Nihil\Desktop\Free Tibet"
        );
    }

    #[test]
    fn removing_an_alias_removes_the_key() {
        let mut config = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        config.aliases.remove("gd");
        config.aliases.insert("gp".into(), "git pull".into());

        let out = render(load::DEFAULT_CONFIG, &config).unwrap();
        assert!(!out.contains("gd ="), "{out}");
        assert!(out.contains("gp = \"git pull\""), "{out}");
        assert!(out.contains("gs = \"git status\""), "{out}");
    }

    #[test]
    fn an_optional_colour_is_removed_rather_than_written_empty() {
        // Against a bare file, not the default one: the default file carries a *commented*
        // `selection_text` line, and a test that searched the whole text would be satisfied by
        // the comment and prove nothing.
        let source = "[theme]\nbackground = \"#1a1b26\"\n";
        let mut config = load::parse(source).expect_parsed();

        config.theme.selection_text = Some("#ffffff".into());
        let with = render(source, &config).unwrap();
        assert!(with.contains("selection_text = \"#ffffff\""), "{with}");
        assert_eq!(
            load::parse(&with)
                .expect_parsed()
                .theme
                .selection_text
                .as_deref(),
            Some("#ffffff")
        );

        config.theme.selection_text = None;
        let without = render(&with, &config).unwrap();
        assert!(!without.contains("selection_text"), "{without}");
        assert_eq!(
            load::parse(&without).expect_parsed().theme.selection_text,
            None
        );
    }

    #[test]
    fn a_profile_override_writes_only_the_fields_it_has() {
        let mut config = Config::default();
        config.profiles.push(ProfileConfig {
            id: "powershell".into(),
            args: Some(vec!["-NoLogo".into(), "-NoProfile".into()]),
            ..Default::default()
        });
        let out = render("", &config).unwrap();
        assert!(out.contains("[[profile]]"), "{out}");
        assert!(!out.contains("program ="), "{out}");
        assert!(!out.contains("enabled ="), "{out}");

        let reloaded = load::parse(&out).expect_parsed();
        assert_eq!(
            reloaded.profiles[0].args,
            Some(vec!["-NoLogo".to_string(), "-NoProfile".to_string()])
        );
    }

    #[test]
    fn an_empty_args_list_round_trips_as_empty_rather_than_absent() {
        // `args = []` is the only way to say "launch it bare", so it has to survive as itself.
        let mut config = Config::default();
        config.profiles.push(ProfileConfig {
            id: "bare".into(),
            program: "x.exe".into(),
            args: Some(Vec::new()),
            ..Default::default()
        });
        let reloaded = round_trip(&config);
        assert_eq!(reloaded.profiles[0].args, Some(Vec::new()));
        assert_eq!(reloaded.profiles[0].program, "x.exe");
    }

    #[test]
    fn a_missing_file_bases_the_merge_on_the_default_but_an_unreadable_one_is_refused() {
        // Only `base_document` is under test — `save` writes the real %APPDATA% path, which the
        // gate must not touch. The distinction is 1.5's again, one level down: *absent* means the
        // defaults, *present but unreadable* means we do not know what is in it, and writing over
        // something we could not read turns a failed save into lost comments.
        let missing = std::env::temp_dir()
            .join(format!("nyx-write-base-{}-missing", std::process::id()))
            .join("config.toml");
        let _ = std::fs::remove_file(&missing);
        let base = base_document(&missing).unwrap();
        assert!(base.contains("# Nyx configuration."));
        let _ = std::fs::remove_dir_all(missing.parent().unwrap());

        // A directory stands in for "present, and not readable as a file".
        let dir = std::env::temp_dir().join(format!("nyx-write-base-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let unreadable = dir.join("config.toml");
        std::fs::create_dir_all(&unreadable).unwrap();
        let err = base_document(&unreadable).expect_err("a directory is not a config file");
        assert!(err.to_string().contains("could not be read"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_round_trip_preserves_every_value() {
        let mut config = load::parse(load::DEFAULT_CONFIG).expect_parsed();
        config.theme.background = "#101010".into();
        config.theme.bright.cyan = "#00ffff".into();
        config.theme.search_current = "#ff0000".into();
        config.font.family = "JetBrains Mono".into();
        config.font.size = 15.5;
        config.render.scrollback = 50_000;
        config.render.cursor_blink = true;
        config.render.cursor_style = crate::config::schema::CursorStyle::Underline;
        config.render.cursor_icon = r"C:\Nyx\cursors\custom.cur".into();
        config.window.width = 1280.0;
        config.window.single_instance = true;
        config.quake.enabled = true;
        config.quake.hotkey = "Ctrl+Shift+Backtick".into();
        config.quake.height = 0.6;
        config.quake.align = "left".into();
        config.quake.animate_ms = 0;
        config.keys.find = "Ctrl+Shift+F".into();
        config.leader.key = "Ctrl+B".into();
        config
            .leader
            .chords
            .insert("s".into(), "pane.split_right".into());

        let reloaded = round_trip(&config);
        assert_eq!(reloaded.theme.background, "#101010");
        assert_eq!(reloaded.theme.bright.cyan, "#00ffff");
        assert_eq!(reloaded.theme.search_current, "#ff0000");
        assert_eq!(reloaded.font.family, "JetBrains Mono");
        assert_eq!(reloaded.font.size, 15.5);
        assert_eq!(reloaded.render.scrollback, 50_000);
        assert!(reloaded.render.cursor_blink);
        assert_eq!(reloaded.render.cursor_style, config.render.cursor_style);
        assert_eq!(reloaded.render.cursor_icon, config.render.cursor_icon);
        assert_eq!(reloaded.window.width, 1280.0);
        assert!(reloaded.window.single_instance);
        assert!(reloaded.quake.enabled);
        assert_eq!(reloaded.quake.hotkey, "Ctrl+Shift+Backtick");
        assert_eq!(reloaded.quake.height, 0.6);
        assert_eq!(reloaded.quake.align, "left");
        // Zero is the "no slide" value and also not the default, so it has to survive as itself
        // rather than being mistaken for an unset key.
        assert_eq!(reloaded.quake.animate_ms, 0);
        assert_eq!(reloaded.keys.find, "Ctrl+Shift+F");
        assert_eq!(reloaded.leader.key, "Ctrl+B");
        assert_eq!(
            reloaded.leader.chords.get("s").map(String::as_str),
            Some("pane.split_right")
        );
    }
}
