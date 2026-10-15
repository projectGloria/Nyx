# Performance

Context for the two Medium findings: this project has already paid for exactly this defect class
once. `ROADMAP.md` 4.3 records that shell-script generation cost 26 ms on the keystroke that
triggered it, because "the settings window applies its draft on every keystroke", and moved it to
a worker. Both findings below are the same trigger, on paths that were not moved.

No timings here are measured — there is no toolchain on this machine. The call counts are exact;
the costs are argued from known syscall and upload costs.

---

### [Medium] Every settings keystroke throws away every icon texture — *fixed*

> **Fixed, with two corrections to this finding.** `Cache::clear` is replaced at the app-wide
> call site by `Cache::retain`, which drops only the entries no shortcut or tab asks for.
>
> 1. **"Delete the `clear()` call" — as written, that leaks.** The caches are keyed by path, so
>    typing a path into the icon field inserts a *failed* lookup per prefix, and with no
>    invalidation at all those stay for the life of the process. The clear was incidentally
>    bounding that. `retain` fixes the reload storm without reintroducing the leak.
> 2. **There was a second caller this finding missed.** `ui/settings/mod.rs:116` clears a
>    *separate* cache owned by the settings window, on window open. That one is correct and was
>    kept: it runs once per open, not per keystroke, and opening the window is the one moment at
>    which the bytes behind an unchanged path plausibly changed — which is the invalidation this
>    finding claimed never happens. The claim was true of the app-wide cache and wrong in
>    general. `Cache::clear` therefore still exists, for that caller only.
>
> The compiler caught the second one: removing `clear` failed the build on a call site a grep
> for `icon_cache` could not see, because it is bound under a different name.

**Where:** `src/app/settings_apply.rs:66-75`, `src/ui/icons.rs:60-125`.

**Trigger:** Type one character into any shortcut field — label, sigil, path, icon — or any alias
field, in the settings window.

**Detail:** `apply_draft` → `apply_config` runs per keystroke. `tables_changed` is true for any
edit to `config.shortcuts` or `config.aliases`, and the handler does:

```rust
if tables_changed {
    self.shellgen.request(self.tables.clone());
    // A shortcut may now point its `exe:` icon somewhere else, and the old texture is
    // keyed by the old path.
    self.icon_cache.clear();
}
```

`shellgen::request` is the fixed half — it hands off to a worker that keeps only the newest
request. `icon_cache.clear()` is synchronous and unbounded, and it is **also unnecessary**. Both
maps are keyed by the path string:

```rust
files: FxHashMap<String, Option<egui::TextureHandle>>,
exes:  FxHashMap<String, Option<egui::TextureHandle>>,
```

A shortcut that "now points its `exe:` icon somewhere else" is a *different key*, so it is
already a natural cache miss. The clear only helps if the bytes at an unchanged path changed,
which is not what the comment describes and not what a settings keystroke indicates.

What the clear costs, on the very next frame, for every icon currently drawn (the tab strip, the
footer's "Saved locations" menu, and one preview row per shortcut in the settings window itself):

- each `exe:` icon → a fresh `SHGetFileInfoW`, which opens and reads the PE file, plus the GDI
  bitmap/mask conversion in `win/shellicon.rs`, plus a `ctx.load_texture`;
- each `file:` icon → `fs::read` + a full `image` decode + `ColorImage::from_rgba_unmultiplied`
  + a `ctx.load_texture`;
- each `file:` icon pointing at an `.svg` → all of the above *plus*
  `fontdb.load_system_fonts()` (see the Low finding below).

Every one of those is a GPU texture allocation and upload, and every previous handle is dropped,
so it is also texture churn rather than just work.

**Impact:** Typing a twenty-character path into a shortcut does twenty full icon reloads. This is
the same user-visible symptom 4.3 was raised for — dropped frames while typing in the settings
window — arriving by a different route after that one was closed.

**Fix:** Delete the `clear()` call. The caches are correctly keyed already. If a genuine
content-changed invalidation is ever wanted (an icon file edited in place), key it on that event,
not on the tables moving.

**Confidence:** Verified for the call graph and the cache keys; the per-icon cost is argued, not
measured.

---

### [Medium] The sound-pack list is re-scanned from disk twice a frame

**Where:** `src/ui/settings/system.rs:34` and `:53`, `src/win/sounds.rs:24-34`.

**Trigger:** Open the settings window's System tab and leave it open.

**Detail:** `packs()` is not cached:

```rust
let mut packs: Vec<_> = std::fs::read_dir(super::assets::root().join("sounds"))
    .into_iter().flatten().flatten()
    .filter(|e| e.path().is_dir() && e.path().join("normal.wav").is_file())
    .map(|e| e.file_name().to_string_lossy().into_owned())
    .collect();
packs.sort_by_key(|s| s.to_lowercase());
```

That is a `read_dir` plus, per entry, an `is_dir()` stat and an `is_file()` stat on a path built
by two `PathBuf` joins — then a sort with a `to_lowercase()` allocation per comparison. `system.rs`
calls it twice per frame:

```rust
for pack in crate::win::sounds::packs() { … }          // :34, inside the ComboBox body
…
if !state.draft.sounds.pack.is_empty()
    && !crate::win::sounds::packs().contains(&state.draft.sounds.pack)   // :52-53
```

The second call is outside the ComboBox closure, so it runs whether or not the dropdown is open.
At 60 fps that is ~120 directory enumerations and several hundred stat calls per second, on the
UI thread, for a list that changes when the user copies a folder in.

It is also the third caller: `Sounds::new`'s worker calls `packs()` on every `Configure` to
validate the pack name, which is correct and cheap there because `Configure` is rare.

**Impact:** Sustained filesystem traffic and a measurable idle cost while a settings tab is open.
This does not touch the headline "zero frames when idle" claim — the settings window is active
use — but it is the sort of thing that makes a settings window feel heavier than the terminal.

**Fix:** Scan once when the settings window opens (or when the System tab is first shown) and
keep the list in `SettingsState`, refreshing on an explicit action.

**Confidence:** Verified.

---

### [Low] Every SVG icon decode loads the entire system font set

**Where:** `src/win/assets.rs:24-29`, reached from `src/ui/icons.rs:83`.

**Detail:**

```rust
let mut options = resvg::usvg::Options { … };
options.fontdb_mut().load_system_fonts();
```

`load_system_fonts()` walks the font directories and parses every face. `CLAUDE.md` measures
cosmic-text's equivalent at **31 ms for 231 faces** and records it as "not worth optimising"
because it happens once at startup. Here it happens once per SVG decode, on the UI thread, inside
a frame.

The import path mostly hides this: `assets::import` rasterizes an SVG to a PNG companion and
records `file:icons/<name>.svg.png`, so an imported SVG is never decoded as SVG again. The
exposure is a **hand-written** `icon = "file:C:\...\logo.svg"` in `config.toml`, which is a
documented and reasonable thing to write.

Mitigating: the result is cached (including failure), so this is one stall, not a per-frame cost
— **except** in combination with the icon-cache finding above, where it becomes one stall per
settings keystroke.

**Impact:** A visible hitch the first time a hand-configured SVG icon is drawn.

**Fix:** Skip `load_system_fonts()` when the SVG contains no `<text>` element, or build the
`fontdb` once and share it. Alternatively, treat a hand-written `file:*.svg` the way `import`
does and rasterize it to a cached companion on first use.

**Confidence:** Verified for the call path; the 31 ms figure is `CLAUDE.md`'s measurement of an
analogous operation, not of this one.

---

### [Low] The terminal lock is taken twice per frame for the pane under the pointer

**Where:** `src/app/copy_blocks.rs:24`, `src/app.rs:1055` and `src/app.rs:1124-1129`.

**Detail:** `copy_block_hover` runs at `app.rs:1055`, before the pane loop, and takes the lock:

```rust
let term = state.pane.term.try_lock_unfair()?;
```

The pane loop at `:1124` then takes it again for the same pane:

```rust
let locked = if scanned { state.pane.term.try_lock_unfair() } else { Some(state.pane.term.lock()) };
```

Two acquisitions, and `Blocks::observe_prompt` runs under both — once against a grid that has not
yet had `apply_pending` run on it, once after. `observe_prompt` is idempotent for the signature
table (it checks `contains` first), so this is cost rather than corruption, but it also clears
`pending_prompt` from the earlier, staler view.

The `try_lock_unfair` is the part worth flagging. `CLAUDE.md` is explicit that this is a narrowly
scoped exception:

> The exception is deliberately narrow. `FairMutex` is what keeps the renderer and the PTY thread
> taking turns under `cat`, and reaching around it on every frame would undo exactly that — so
> every other pane, and every frame with no scan running, takes the lock the same way it always
> did.

`copy_block_hover` reaches around it on every frame the pointer is over a pane, scan or no scan.
Under sustained output that is a second lock acquisition per frame competing with the PTY thread,
which is the case the fairness was bought for. (See also 05 — this makes the stated invariant
false.)

`Blocks::at`'s own cost under the lock is modest: it is keyed to `(cursor, history_size,
columns)`, so it re-walks whenever the cursor moves, but `prompt_columns` short-circuits on the
first mismatched character, making a re-walk roughly `MAX_SCAN_DISTANCE × prompts` cheap
comparisons rather than anything expensive.

**Impact:** Small and hard to observe; the concern is the precedent and the now-false invariant
more than the cycles.

**Fix:** Hoist one acquisition — compute the hover block inside the pane loop, where the lock is
already held and `apply_pending` has already run — and pass the result out.

**Confidence:** Verified.

---

## Not findings

Worth recording so the next pass does not re-derive them:

- **`Blocks::at` unbounded history walks.** Guarded by `MAX_SCAN_DISTANCE` (256 rows each way)
  with an explicit "hide the controls rather than scan" bail-out, and by `MAX_PROMPTS`. The
  eviction test even asserts the *newest* signature survives, which is the one a live prompt is
  matched against. This is careful work.
- **`CursorSettle`.** `MAX_HOLD` correctly measures from the first hold rather than resetting per
  new position, so a wandering cursor is bounded. The out-of-grid drop on resize is handled.
- **The atlas-full recovery** (`render/callback.rs:131`) does invalidate the row cache, via
  `rebuild_atlas` → `pane.rows.invalidate()`. The invariant holds.
- **Idle repaints.** Every `request_repaint*` call site is guarded by something that is actually
  animating — blink, slide, toast expiry, copy feedback, resize poll, search debounce. Nothing
  new in Phase 9 asks for an unconditional frame.
