# AGENTS.md — Nyx

Nyx is a Windows terminal emulator built in Rust. ConPTY hosts shells and eframe, wgpu and glyphon render the terminal grid.

## Commands
- Launch locally with `cargo run`. Use `cargo check` for the fast compile gate.
- Run the suite with `cargo test -- --nocapture` to see which interpreter tests skipped.
- Check keyboard binding fallthrough with `cargo test no_default_binding_claims_a_single_modifier_chord_from_the_shell`.
- Lint with `cargo clippy --all-targets`. Reproduce the release workflow build with `cargo build --release --locked`.
- Use `.\install.ps1` for a per-user install. `.\uninstall.ps1` preserves configuration unless invoked with `-Purge`.

## Architecture
- `src/main.rs` handles startup and CLI dispatch. Registry and launcher maintenance commands can exit before opening a window.
- `src/app.rs` and `src/app/` own UI-thread effects. The eframe logic pass drains events and routes input. The UI pass computes pane geometry and submits paint callbacks.
- `src/term/` owns PTYs and terminal state. Only an owned `GridSnapshot` crosses into `src/render/`. Renderers never borrow a live terminal.
- `src/event.rs` carries worker notifications into the frame through `AppEvent`. PTY output uses a dirty flag and repaint coalescer.
- `src/layout/` owns pure tab, split and focus geometry. `src/theme/` resolves UI colours. `src/win/` owns Windows integration and degrades gracefully on optional feature failures.
- `src/expand/` owns alias and sigil expansion. `src/expand/shellgen/` emits PowerShell, Clink and bash integration scripts.
- `src/config/` loads, watches and writes TOML. Generated shell scripts live under `%LOCALAPPDATA%\Nyx\shell`.
- **Version files:** Keep `Cargo.toml`, the nyx package in `Cargo.lock`, and `gloria.toml` aligned. Keep the four-part assembly identity in `nyx.manifest` aligned with the app release.

## Personal Rules (Brain)
- **Commit Format:** Write one plain sentence explaining the change and its reason. Omit conventional-commit prefixes.
- **Commit & Push Policy:** Commit, tag or push only on explicit request. For “commit and push”, verify build and tests, commit on main and push to origin main. Never force-push main.
- **Releases:** Use only /monjita-github-release to bump, dispatch and monitor publication. Never create release tags or GitHub Releases by hand.
- **Path Privacy:** Use `C:\Users\you` for example paths in documentation, comments and examples.
- **Release Workflow:** Follow the GloriaSetup release template with shared check and release jobs on main and a project-specific build job. Keep GloriaSetup unpinned.
- **Window Chrome:** Use a frameless titlebar with a drag region, double-click maximize and custom minimize, maximize and close controls.
- **Desktop Shell Lock:** Fit the shell to the viewport, scroll within panes and disable text selection on chrome.
- **Scrollbars:** Use themed 4–6 px scrollbars with transparent tracks and no layout shift.
- **Interactive Controls:** Style hover, active, keyboard-focus and disabled states with 120–180 ms transitions.
- **Git Branching:** Work directly on main. Create branches or worktrees only on explicit request.
- **Versioning:** Leave versions alone during normal work. The user chooses the release version and /monjita-github-release updates all version files together.

## Playbooks (Read Before Task)
- Read `.brain/building-the-project.md` before compiling release binaries, packaging or building an installer.
- Read `.brain/github-actions.md` before editing workflows, setting up releases or debugging a failed release.

## Repo Guardrails & Gotchas
- **Repaints:** Route PTY output through RepaintCoalescer::poke to cap output-driven paints at 60 Hz. Keep input-driven repaints immediate.
- **Snapshots:** Use try_lock_unfair on the frame path and reuse the last snapshot on contention. A history search can hold the terminal lock across its scan.
- **Hidden windows:** In Quake mode, stop the hidden frame after the hotkey check so replayed egui input cannot reach the shell.
- **Keyboard routing:** Preserve arrival order through popups, settings, palette, find, copy deduplication, leader chords, bindings, sigils, kitty encoding and shell input.
- **Binding fallthrough:** Return an unusable chord to the shell. Check default bindings against the real encoder, respect the configured palette chord, and keep chord matches exhaustive.
- **Chrome focus:** Release chrome focus every pass so Tab followed by Enter reaches the shell.
- **Terminal protocols:** Preserve legacy typing and bare Enter under kitty disambiguation. Honour inherited TERM and COLORTERM, set TERM_PROGRAM, and send requested focus reports to every pane on window focus changes.
- **Links:** Require Ctrl+click and an HTTP or HTTPS URL. Recheck the scheme in the Windows opener.
- **Expansion:** Authenticate shell integration via the per-pane launch token in the window title. Disable shadow rewrites while integration is pending or active. Keep may_hint separate from may_expand.
- **Sigils:** Gate prompt actions on child_running, alternate-screen or desynced state, and kitty-driven programs. Preserve separate settings for completion and hints.
- **Config:** On reload errors retain the running config and name the bad key. Merge saves with toml_edit to preserve comments, order, omitted defaults and line endings. Compare schema floats at f32 precision.
- **Config lifetime:** Apply scrollback changes to new terminals and startup-only window settings on the next launch. Surface those limits in the UI.
- **Dependencies:** Use glyphon’s cosmic-text re-export so rendering shares one FontSystem type.
- **Panic handling:** Preserve release unwinding so a panicking PTY thread costs one pane.
- **Regression tests:** Add a test for every fix and verify it fails when the fix is reverted. Use term::testing::headless_term for grid and parser behaviour.
- **UI tests:** Extract pure decisions for tests and keep effects on NyxApp, whose construction needs a live GPU context.
- **Interpreter tests:** Print the missing dependency when skipping a shell test. Exercise the real interpreter when available.
- **Refactors:** Move code verbatim before editing it. Preserve comments that identify the bug a line prevents.
- **Review history:** Treat STATUS.md, ANALYSIS.md and review/ as dated snapshots. If the ignored review-harness/ exists, exclude it from searches and edits.
- **Diagnostics:** NYX_PERF enables frame timings, NYX_BARE isolates window/GPU cost, and WGPU_BACKEND overrides the DX12 default. Release warnings go to `%LOCALAPPDATA%\Nyx\logs\nyx.log`.

## Done Criteria
- Run the compile, test and lint gates above. For release work, run the locked release build and check all four version files.
- Exercise changed rendering, ConPTY, global hotkeys, Quake, end-to-end IPC and cursor subclassing in a running Nyx window.

<!-- brain-sync: cb66c48eed22 -->
