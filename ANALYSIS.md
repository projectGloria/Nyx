# Nyx codebase analysis

Reviewed 16 September 2026 against the current working tree.

Nyx is a Windows terminal emulator focused on input latency and low idle CPU. It is one Rust binary crate, with 75 Rust source files and 19,362 source lines including comments, tests, and blank lines. Its major features are tabs, split panes, scrollback search, live configuration and settings, directory shortcuts, shell aliases, leader chords, Quake mode, and Explorer integration.

This document maps every source module and traces the main application flows. The checks below were executed in this session. Runtime observations in CLAUDE.md, ROADMAP.md, STATUS.md, and REVIEW.md are historical evidence, not measurements repeated here. Source inspection cannot establish correctness for every native interaction or timing interleaving.

## Current validation and repository state

- `cargo check`: passed, without warnings.
- `cargo test`: 386 passed, 0 failed, 0 ignored; test execution takes roughly 5–6 seconds. (An
  earlier revision of this document reported 297; that count was taken against an older tree.)
- `cargo clippy --all-targets`: clean — 0 warnings, in the bin and test targets both. (An earlier
  revision reported 28 warnings; all have been fixed or justified at the site.)
- The working tree already contained extensive modifications and new modules, including `src/app/` and `src/term/cursor.rs`. `AGENTS.md` is deleted in the working tree. Those changes were preserved.
- Native window interaction, GPU profiling, IME entry, mouse protocols against a live application, and WSL sessions were not exercised in this session.

The original review describes an older implementation. Its twenty findings must be reconciled with the current source and ROADMAP.md before being treated as current defects. STATUS.md also contains obsolete counts and statements: it reports 290 tests and says Clink/Git Bash were not installed or exercised, while newer documentation and the current shell tests show subsequent work. Its statement that file-icon previews are placeholders also disagrees with the implemented preview code.

## Technology and ownership

| Layer | Implementation | Responsibility |
|---|---|---|
| Window and chrome | eframe / egui 0.36.2 | Native event loop, input events, settings, tabs, overlays |
| GPU | wgpu 30 / egui-wgpu | Surface, callback integration, instanced rectangle pipeline |
| Terminal text | glyphon 0.12 and its cosmic-text re-export | Font discovery, shaping, glyph rasterization and atlas |
| Terminal engine | alacritty_terminal 0.26 | VT state machine, grid, history, search, selection, ConPTY |
| Configuration | serde, toml, toml_edit, notify, dirs | Defaults, validation, watch/reload, formatting-preserving saves |
| Thread communication | crossbeam-channel, atomics, mutexes | Background events, repaint coalescing, shared terminal state |
| Windows integration | windows-sys 0.61, raw-window-handle | HWND, hotkeys, monitors, registry, named pipes, shell icons |
| Clipboard and image files | arboard, image | OSC 52 stores, copy/paste, PNG/BMP icon decoding |

There is no webview, server, database, Node layer, or async runtime. Windows support is structural: `win/mod.rs` directly uses Windows OS extensions. The target is a native Windows application rather than a portable terminal with a Windows backend.

`NyxApp` owns UI state, configuration, tabs, pane objects, snapshots, settings drafts, search state, shell tables, and Windows services. Each `Pane` owns a shared `Arc<FairMutex<Term<EventProxy>>>`, a PTY event-loop sender, a dirty flag, and resize state. Each `Tab` owns a binary split tree and a focused pane ID; the actual panes live in the app's map.

`GridSnapshot` is the owned boundary between terminal state and rendering. Render callbacks carry snapshots rather than terminal references. `GridResources` owns shared font and atlas resources, and a map of rendering resources for each pane.

## Complete source map

Paths below are relative to the repository root.

| Files | Purpose |
|---|---|
| `src/main.rs` | Logging, command-line dispatch, config startup load, single-instance claim/attach, DX12 default, native window construction, bare-window measurement mode |
| `src/event.rs` | Background-to-UI event enum, clipboard target policy, atomic repaint gate with a roughly 60 Hz output ceiling |
| `src/app.rs` | App and pane state, initialization, expansion/input wrapper, frame logic, rendering pass, cursor blink, DPI changes, save-on-exit, gutter parent reconstruction |
| `src/app/events.rs` | Drain PTY/config/search/IPC/hotkey events; route title handshakes, clipboard writes, replies, pane exits, and bells |
| `src/app/panes.rs` | Spawn tabs/panes, split, close, retire renderer resources, focus and dirty-state helpers, IME cursor rectangle |
| `src/app/keys.rs` | App bindings, leader command execution, palette/settings keyboard routing, chrome focus release, clipboard commands |
| `src/app/pointer.rs` | Raw pointer routing, terminal selection, held mouse button and motion reports, wheel routing, coordinate conversion |
| `src/app/find.rs` | Find-bar keyboard editing, 100 ms search debounce, single-flight requests and worker launch |
| `src/app/settings_apply.rs` | Derive runtime settings, apply theme/font/keys/tables/profiles/Quake changes, schedule and flush saves |
| `src/app/chrome.rs` | Palette entries/actions, shortcut navigation, toasts, tab strip and background bells, performance overlay |
| `src/config/mod.rs`, `schema.rs` | Module exports; serialized configuration types, defaults and numeric sanitization |
| `src/config/load.rs` | Documented initial config text, first-run creation, BOM removal, explicit parsed/failed outcomes |
| `src/config/paths.rs`, `watch.rs` | Roaming config/local data roots; directory watcher, 150 ms debounce and content fingerprints |
| `src/config/keys.rs` | Binding parsing, exact modifier matching, serialization and action/default tables |
| `src/config/write.rs` | Merge into TOML while preserving unchanged keys, comments, absent defaults and line-ending style |
| `src/term/mod.rs` | Terminal exports, grid dimensions, headless real-terminal test helpers |
| `src/term/spawn.rs`, `pane.rs`, `proxy.rs` | Shell specification, ConPTY and event-loop construction, pane operations, Alacritty event translation |
| `src/term/detect.rs`, `profile.rs` | Installed shell/Clink discovery, background WSL enumeration, configured overrides, launch arguments and script injection |
| `src/term/input.rs` | Terminal byte encoding, Ctrl/Alt/text and IME handling, function/navigation keys, copy/paste, bracketed-paste sanitization |
| `src/term/mouse.rs`, `select.rs` | Cell/side hit testing, SGR/legacy mouse protocols, wheel/motion policy, real terminal selections |
| `src/term/search.rs` | Literal/regex compilation, visible highlights, search request sequencing, origin/scan/apply functions |
| `src/term/snapshot.rs`, `cursor.rs` | Owned visible-row snapshots, resolved cell runs and cursor; bounded settling of nonadjacent cursor jumps |
| `src/render/mod.rs` | Shared GPU/font resources, per-pane resource map, atlas rebuild, font generation, aggregate frame timing |
| `src/render/callback.rs` | Prepare row text and quads; paint backgrounds, glyphs and foreground decorations with viewport/scissor handling |
| `src/render/metrics.rs`, `shape.rs` | Measure physical cell dimensions; cache and reshape changed rows using monospace width and rich attributes |
| `src/render/color.rs`, `quads.rs` | Resolve ANSI flags/palette overrides; WGSL rectangle shader, instance uploads, backgrounds, cursors and decorations |
| `src/layout/mod.rs`, `tree.rs` | Tabs and split nodes, pane ownership lookup, idempotent close outcomes, active-index adjustment and bell ownership |
| `src/layout/geometry.rs`, `focus.rs` | Recursively solve pane/gutter rectangles, minimum split ratios, directional and nearest-survivor focus |
| `src/expand/mod.rs`, `table.rs` | Expansion exports; validate/sort shortcuts and aliases, lookup sigils, derive paths for shell styles |
| `src/expand/line.rs`, `shadow.rs` | Whitespace-token rewrite; conservative fallback input-line model and backspace/replacement bytes |
| `src/expand/handshake.rs`, `chords.rs` | Token-addressed title protocol and integration state; leader command parsing and timeout state machine |
| `src/expand/shellgen/mod.rs` | Generate three shortcut TSVs, aliases TSV and three scripts; worker drains obsolete queued table updates |
| `src/expand/shellgen/powershell.rs` | PSReadLine token expansion, generated alias functions, reload and prompt/title marks, UTF-8 path encoding |
| `src/expand/shellgen/bash.rs` | Bash startup sourcing, native aliases, readline space rewrite, builtins-only path encoding and prompt marks |
| `src/expand/shellgen/clink.rs` | Clink Lua table reload, input filtering, buffer rewrite on space, prompt marks and byte encoding |
| `src/theme/mod.rs`, `visuals.rs` | Parse RGB colors, build ANSI/xterm palettes, convert target color space and derive egui visuals |
| `src/ui/mod.rs`, `rail.rs` | UI exports; shortcut rail, vector/file/executable icon drawing and navigation choice |
| `src/ui/icons.rs` | Parse icon specs, draw builtins, cache image and executable textures including failed loads |
| `src/ui/palette.rs`, `findbar.rs` | Fuzzy ranking, palette selection/detail elision; painted search query, status and controls |
| `src/ui/settings/mod.rs` | Seven settings tabs, draft lifetime, binding capture, settings window and problem display |
| `src/ui/settings/theme_tab.rs`, `font.rs` | Color/ANSI editing; font, rendering, startup window dimensions and rail settings |
| `src/ui/settings/shortcuts.rs`, `aliases.rs` | Shortcut fields, previews/reordering; alias name and command editing |
| `src/ui/settings/keys.rs`, `shells.rs` | Binding/leader editing; profiles, integration status, persistent argument editor and quote grouping |
| `src/ui/settings/system.rs`, `widgets.rs` | Quake and Explorer controls; shared text/color/numeric/toggle widgets |
| `src/util/mod.rs`, `atomic_write.rs` | Temporary write, flush/sync and rename replacement; preserve destination if rename fails |
| `src/win/mod.rs`, `console.rs` | UTF-16 strings, HWND wrapper, system wheel setting; attach/write launching console for CLI output |
| `src/win/hotkey.rs`, `quake.rs` | Dedicated registration/message-loop thread; physical-pixel monitor placement and fixed-size Y animation |
| `src/win/ipc.rs` | Command-line parser, instance mutex, named-pipe protocol, size cap and existing-directory validation |
| `src/win/registry.rs`, `shellicon.rs` | Per-user Explorer verbs; shell icon acquisition, GDI bitmap/mask conversion and handle cleanup |

## End-to-end flows

### Startup and shell output

1. Logging starts; help and Explorer registration flags finish without opening a terminal window.
2. Configuration loads before the window so its initial dimensions apply immediately. Defaults are allowed on an initial parse failure because no prior live config exists.
3. A held per-user instance mutex determines the primary process. Attach requests are sent before expensive GPU/font initialization; attach failure falls through to a new window.
4. The app derives colors, bindings, shortcuts, chords, profiles and Quake settings. It creates rendering resources, detects shells, starts the config watcher, generates shell files and starts relevant workers.
5. The first tab launches its shell in ConPTY. Alacritty's event loop reads/parses terminal output behind the pane's fair mutex.
6. Output wakeups set an atomic dirty flag and poke the repaint coalescer rather than allocate one channel message per output chunk. Titles, exits, bells and replies use `AppEvent`.
7. The UI drains events; the paint pass solves layout, reconciles sizes, captures dirty grids, then submits one GPU callback per visible pane.

### Input and expansion

Settings, the palette and the find bar each take keyboard ownership before normal shell input. App bindings and leader chords are consumed before forwarding remaining events. Ordinary terminal input is translated to UTF-8/control/escape bytes and sent to the PTY event loop. Input returns scrollback to the bottom and clears selection.

Expansion has two layers. Generated PowerShell/Bash/Clink scripts work with their shell's line editor and announce themselves through `nyx:<token>:...` window titles. While integration is pending or active, terminal-side rewriting is suppressed. After the greeting timeout, an absent integration enables the fallback `Shadow` model. Arrow navigation, completion and other edits desynchronize that model; Enter resets it. This avoids rewriting a line the fallback no longer knows.

Sigils map to shell-style directory spellings. Aliases expand only as commands at the first token. The tokenization is whitespace-based, not a complete shell grammar. Native shell behavior differs across the three integrations; it should not be assumed that all submit and cursor-edit cases are identical.

Rail/palette directory navigation directly sends `cd <expanded-path>\r` to the focused pane. Splits currently launch the first resolved profile, not the source pane's profile or its reported current directory.

### Rendering and resize

Metrics are physical pixels; layout and pointer positions are egui points converted using current DPI scale. A pane resize waits 60 ms unless forced by relayout, then updates both the terminal grid and ConPTY dimensions.

Snapshot capture walks visible cells, resolves ANSI colors and flags, overlays search/selection/cursor colors, and builds owned row strings with attribute runs. Wide-character spacer cells are skipped and combining characters stay attached. The focused block cursor is baked into the cell's colors so its glyph remains readable.

`RowCache` compares full row snapshots and only reshapes changed rows. Font generations invalidate its cache. Glyphon shares font/raster/atlas resources while text renderers and quad buffers are per pane. Painting orders backgrounds before glyphs and decorations after glyphs. The callback restores a full-screen viewport because egui sets a callback-local one. Atlas trimming is once per frame; an atlas-full preparation rebuilds shared resources.

The cursor-settle layer delays an unproven nonadjacent jump by 30 ms, with a 90 ms maximum hold. This affects drawn cursor position rather than terminal input or VT state.

### Configuration and persistence

External file changes pass through a 150 ms debounce and fingerprint comparison. A syntax/type/read failure carries no replacement config; the app reports it and retains live settings. A parsed config can still receive per-value fallbacks or numeric clamps. Those fallbacks are not necessarily the previous runtime value: invalid theme colors use built-in defaults.

The settings window edits a draft and applies it live. Disk saving is debounced for 400 ms and flushed during orderly exit. TOML saves merge against existing text rather than serialize the entire configuration. Watcher echoes matching the last saved fingerprint are ignored. Shell table changes go to a separate generation worker; running shells reload data at subsequent prompts. Scrollback depth only affects newly constructed terminals; configured window dimensions are startup dimensions.

Config lives in `%APPDATA%\Nyx\config.toml`. Generated integration scripts and TSV data live in `%LOCALAPPDATA%\Nyx\shell`. Release warnings go to `%LOCALAPPDATA%\Nyx\logs\nyx.log`; debug builds use stderr. The release log has an approximately 1 MiB launch-time ceiling, not an active rotation mechanism.

### Search and Windows services

Search compiles literals as escaped regex patterns. Visible highlights run over the visible grid. Whole-history next/previous searches run on a worker, but hold the searched terminal mutex for the entire scan. The UI's snapshot path uses `try_lock_unfair` for the tracked scanned pane and reuses its last snapshot if unavailable. Requests are single-flight, keeping only the newest queued direction; results arrive through sequence-numbered events.

The global hotkey owns its registration on a dedicated Win32 message-loop thread. Rebinding is a thread message. That thread raises a hidden HWND immediately; app logic decides whether the press means show or hide. Quake uses physical monitor work-area pixels and moves Y at fixed size. Hidden app logic returns before normal input forwarding to avoid replaying eframe's retained input. Disabling the only way to reveal a hidden window reveals it first.

IPC accepts an at-most-8 KiB `nyx1` message, validates directories and uses them only as launch working directories. Its server is sequential and uses blocking Win32 pipe calls. Explorer verbs cover directories, directory backgrounds and drives under HKCU.

## Current issues and limits to carry forward

These observations are from current source inspection. They are not new regression probes or native reproductions performed in this session.

| Observation | Evidence and consequence |
|---|---|
| App expansion wrapper bypasses printable Meta handling | `app.rs:516` handles every `Text` itself and appends plain UTF-8. `term/input.rs` prefixes printable Alt text with ESC, but only non-Text events reach that translator through the wrapper. The input-unit test therefore does not establish correct Alt+B/Alt+F behavior through the app. |
| Query edits do not immediately invalidate a running search result | `Search::set_query/recompile` resets the pattern/current match but does not change `running` or its sequence. The find handler waits 100 ms before requesting the replacement scan. If the old scan lands first, `land` permits applying its old answer to the new query. Empty or invalid replacement queries have the same gap. The existing supersession test requests the second scan immediately, unlike the app debounce. |
| Search origin can become stale between its two locks | `app/find.rs:123` computes a clamped origin under a short lock, then releases it. `search::scan` later takes a separate lock and forwards that origin without another clamp. Resize/history movement can occur between them. Installed Alacritty `search_next` immediately calls `expand_wide`, which indexes the grid at that point. The existing resize tests resize before origin computation and do not cover this interleaving. |
| Search responsiveness protection is partial | The scan blocks the pane's PTY parser. Snapshot capture declines the lock, but resize (`pane.rs:130`), pointer selection, clipboard/scroll operations, and ordinary typing after closing the bar (`app.rs:889`) still take blocking terminal locks. Closing search abandons tracking without stopping the worker, so ordinary snapshot capture may also block on a scan still finishing. |
| Search worker failure has no completion recovery | A worker panic sends no `SearchFinished`; `running` can stay set until the bar is closed. This is already acknowledged in ROADMAP.md. |
| Pointer state is global rather than tied to a reporting pane | `MouseState` tracks a button and last reported `Point`, without the originating pane. Equal cell coordinates across panes can suppress a first motion report, and a release in another pane/outside the grid need not clear/report the original button correctly. Native drag tests are needed. |
| Argument editor round-trip is incomplete for literal quotes | `join_args` wraps whitespace-containing arguments with double quotes without escaping embedded quotes, and does not quote other arguments containing literal quote characters. `split_args` consumes grouping quotes, so some stored values cannot survive a subsequent edit. Current round-trip tests omit those cases. |
| Shell quoting combines cmd and PowerShell under one Windows style | `win/paths.rs` uses double quotes for Windows paths; PowerShell still interprets `$` and backticks in those quotes, and cmd percent expansion remains a separate concern. A path's style alone does not identify its shell's quoting grammar. |
| WSL translation assumes a conventional mount and same distro | Drive paths use `/mnt/<drive>`. WSL UNC conversion strips the distro identity. Custom automount roots and cross-distro paths need real distro-aware validation. Both limits are recorded in newer documentation. |
| Direct shortcut navigation assumes a command prompt | `app/chrome.rs:52` appends `cd ...` and Enter to the current input recipient without checking partial input, alternate-screen applications or the integration's child-running state. Navigation semantics need a defined policy in those situations. |

Other explicit scope limits: no kitty keyboard protocol, no application-keypad support, no inline IME preedit drawing, no native WSL init-script injection, and no wrapper-prefix session/per-command modes. File icons decode PNG/BMP rather than SVG. There is no embedded application icon artwork/resource or signing pipeline in the project.

## Performance and test interpretation

The design avoids continual idle frames, throttles shell-output repaints separately from input, snapshots under a short lock, caches shaped rows, shares glyph resources and debounces filesystem work. Closing panes now queues GPU resource retirement; pane ownership lookup and close outcomes make duplicate exit notifications harmless to tab structure.

Historical measurements in CLAUDE.md put sustained ConPTY throughput near 1 MiB/s and identify row shaping as the largest rendering CPU cost. Those figures were not repeated here. An ASCII shaping fast path remains a documented possible optimization, with visual correctness requiring GPU verification.

Tests are embedded in source modules. They exercise key/paste/mouse encoding, real headless terminal search and snapshots, layout, TOML preservation, shell paths, icon handling, and Windows helpers. Some tests execute PowerShell, Git Bash or Clink; optional interpreter tests can return early with a skip message even though Rust's summary says zero ignored. Passing the suite is therefore strong helper-level evidence, not complete native end-to-end coverage.

The review harness is an ignored copy of older source with extra probes and a distinct package name. Its local `.cargo/config.toml` selects its own target directory when run inside that directory. Its historical failing probes are expected and are not the current production test gate. The old commands printed in root REVIEW.md are superseded by `review-harness/README.md`.

The build script embeds the DPI/UTF-8/long-path manifest using MSVC linker flags. The release profile enables optimization, fat LTO, one codegen unit and stripping while retaining unwinding. `install.ps1` copies the executable, creates a Start-menu shortcut and optionally edits user PATH/Explorer registration. `uninstall.ps1` reverses that and preserves config unless `-Purge` is supplied. Neither script was executed during analysis. No tracked CI workflow or external integration-test directory was found.

For follow-up changes, the most consequential seam is the app around otherwise well-tested helpers: event ordering, keyboard ownership, search worker lifetime, terminal lock acquisition and per-pane pointer state. Helper correctness does not automatically establish that the frame uses the helper correctly.

## Resolution of the observations above, 16 September

Each row of the issues table was re-derived from source and then either fixed or confirmed as recorded. Every fix carries a test that failed before the change, except where noted.

| Observation | Outcome |
|---|---|
| App expansion wrapper bypasses printable Meta handling | **Not present in the current tree.** `app::translate_with_expansion` already applies `input::text_rule` and has the meta/suppress arms, with `the_expansion_path_encodes_modified_text_exactly_as_the_plain_one_does` pinning the two paths together. |
| Query edits do not immediately invalidate a running search result | **Fixed.** `Search` now records the pattern a running scan was started for, and `land` refuses to apply a result whose pattern has moved on — the view no longer jumps to a match of a query the user replaced while the debounce held the replacement request back. |
| Search origin can become stale between its two locks | **Fixed.** `search::scan` re-clamps the origin against the grid it actually locked, closing the `origin_for` → `scan` resize gap that the stored-`Point` panic used. |
| Search responsiveness protection is partial | **Recorded, unchanged.** As the roadmap documents, the scan's indivisibility and the remaining blocking sites are deliberate: the narrow `try_lock_unfair` exception exists precisely so everything else keeps taking the lock normally. |
| Search worker failure has no completion recovery | **Recorded, unchanged.** A panicking scan thread still leaves the bar stale until closed; the roadmap documents why this is quieter, not worse, than the pre-thread behaviour. |
| Pointer state is global rather than tied to a reporting pane | **Fixed for the reported half.** `MouseState::reported_cell` is now keyed by pane as well as cell, so two panes holding the same coordinate can no longer suppress each other's motion reports. `selecting` already carried the pane. |
| Argument editor round-trip is incomplete for literal quotes | **Fixed.** `join_args` quotes any argument carrying a quote character — which `split_args` would otherwise have mangled even without whitespace — choosing the quote kind the argument does not contain; both-kinds arguments remain inexpressible and are documented as best-effort. New round-trip cases cover `it's`, `say "hi"`, and `-Command "Get-ChildItem"`. |
| Shell quoting combines cmd and PowerShell under one Windows style | **Recorded, unchanged.** A path's style genuinely does not identify its shell's quoting grammar, and splitting the TSV per shell is a design change, not a fix. |
| Direct shortcut navigation assumes a command prompt | **Fixed for the knowable cases.** `go_to_shortcut` now refuses with a toast when the focused pane is on the alternate screen or (under shell integration) has a command running; `navigation_refusal` is a pure, tested rule shared by every route in. The unintegrated-shell case remains undetectable by design. |

Additional fixes from the same pass, outside the table:

- **egui's default Ctrl+Q quit shortcut** was answered with `ViewportCommand::Close`, so the app filtered Ctrl+Q out of the event list every frame — which also meant the chord could never reach the shell. The shortcut is now cleared once from `Context::options` (`Options::quit_shortcuts`) and Ctrl+Q passes through to the terminal.
- **`handshake::State::tick` ran per keystroke on the focused pane only**, so a pane whose injected script never greeted stayed `Pending` — shadow expansion suppressed — until someone typed into it. It now runs once per frame for every pane, with a test for the expiry.
- **`config::write::save` treated a file that exists but could not be read as if it were absent**, basing the merge on `DEFAULT_CONFIG` and overwriting an unreadable file's comments with a successful write. Absent still means the default document; present-but-unreadable is now an error the caller reports.
- **Clippy**: all warnings across the bin and test targets resolved — `Box<Config>` in `Loaded::Parsed` for the enum-variant size, `as_chunks` conversions, `Search`'s derived `Default`, iterator-based palette loops, and the remainder mechanical. `permissions_set_readonly_false` is allowed on the one test whose cleanup legitimately clears the bit.
