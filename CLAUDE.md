# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Nyx is a Windows-only terminal emulator: one Rust binary crate, ~89 source files, 449 tests. It
renders the grid on the GPU through its own wgpu pipeline and hosts shells through ConPTY. No
async runtime, no webview, no Node. It is built around input latency and idle cost — throughput is
ConPTY-bound at ~1 MB/s for every Windows terminal alike, so that is not the axis anything here
optimises for.

## Commands

```bash
cargo check                    # the fast gate
cargo test                     # 449 tests, ~3 s warm
cargo test <substring>         # one test, or one module's tests, by name substring
cargo test -- --nocapture      # required to see the loud skips (see below)
cargo clippy --all-targets     # currently 4 mechanical warnings, all pre-existing
cargo build --release          # fat LTO + codegen-units=1; ~12 min cold, and a gate of its own
```

`.\install.ps1` does a per-user install into `%LOCALAPPDATA%\Programs\Nyx` (Start-menu shortcut,
optional PATH and Explorer registration); `.\uninstall.ps1` reverses it and keeps config unless
given `-Purge`.

Runtime switches, all read at startup:

- `NYX_PERF=1` — frame-time overlay (snapshot / shape / text prepare / quads, in µs).
- `NYX_BARE=1` — the same eframe+wgpu window with no terminal in it, for attributing memory to the
  graphics stack rather than to Nyx.
- `WGPU_BACKEND` — overrides the DX12-only default (enumerating every backend costs ~46 MB and a
  failed Vulkan probe per launch).

CLI: `nyx [-d DIR] [--attach | --new-window] [--focus] [--install-context-menu |
--remove-context-menu] [-h]`, plus the internal `--launcher` / `--stop-launcher` /
`--remove-launcher` / `--refresh-launcher` forms. The flags that do registry work exit without
opening a window, so `install.ps1` can call them.

**A release build has no stderr** (`windows_subsystem = "windows"`), so `log::warn!` goes to
`%LOCALAPPDATA%\Nyx\logs\nyx.log`, appended with a ~1 MiB launch-time ceiling. Debug builds keep a
console and log to stderr.

**Tests that shell out to real interpreters** (PowerShell, Clink, bash) have found more real bugs
than every other kind here combined, including two "fixes" that passed review and did nothing.
When the interpreter is missing they skip loudly with an `eprintln!` naming it — which `cargo test`
hides unless you pass `--nocapture`. A green suite is that many tests weaker than the number
suggests: on this machine the 5 bash tests run and the 5 Clink ones do not
(`winget install --id chrisant996.Clink -e` closes the remaining gap).

## Architecture

### Module ownership

These boundaries are real and are the reason the code is navigable. Preserve them.

| Module | Rule |
|---|---|
| `term/` | the PTY side. Nothing in it draws. |
| `render/` | the GPU side. Nothing in it touches a `Term`. |
| `layout/` | tabs, split tree, geometry, focus. Pure: no terminal, no rendering. |
| `theme/` | every colour in the program, resolved from config once. A hex literal anywhere else silently ignores the user's theme. |
| `win/` | the only caller of `windows-sys`. Every failure reports itself and returns `None` — a hotkey, pipe or registry key that will not work must still leave a working terminal. |
| `expand/` | sigil/alias expansion, pure, plus `shellgen/` which generates the shell-side scripts. Switchable off per half by `[expand]`: the menu takes keys from the program, the hint does not. |
| `app/` | the frame: routing, `&mut self` effects, and the wiring between all of the above. |

### The two seams

- **`term::snapshot::GridSnapshot`** is the only thing that crosses from terminal state into
  rendering: a plain owned struct with no lifetimes, built under the `Term` lock and consumed on
  the GPU thread long after the lock is gone. Nothing hands out a `&Term`.
- **`event::AppEvent`** is the only channel from non-UI threads into the frame — titles, child
  exits, bells, clipboard stores, search results, IPC requests, hotkey presses. Shell *output*
  deliberately does not use it: it sets an atomic dirty flag and pokes `RepaintCoalescer`, which
  caps output-driven repaints at 60 Hz. Input-driven repaints are egui's own and are never
  throttled, so typing latency is unaffected by that cap.

### The frame

eframe 0.36 splits a frame into `logic` (no painting; also runs while the window is hidden) and
`ui` (the paint pass), and Nyx uses that split literally. [app.rs:665](src/app.rs:665) `logic`
drains events and runs the whole keyboard; [app.rs:979](src/app.rs:979) `ui` lays out chrome,
solves pane rects, reconciles sizes, snapshots dirty panes and adds one paint callback per pane.
While Quake mode has the window hidden, `logic` returns early after the hotkey check — eframe
replays the last shown frame's input every tick, and going further would retype it into the shell.

### Keyboard ownership

The terminal owns the keyboard; everything else has to be asked first. In order: any open popup →
settings → palette → find bar (each owns the keyboard *completely* while up) → copy/Ctrl+C dedup →
leader chords → app bindings → `handle_sigil_key` → the kitty encoding → the shell. Events are
routed in arrival order, because a `Text` event must update the line model before an `Enter`
inspects it.

**A binding that cannot act must not swallow the key.** This is the rule the defaults are built
on, and breaking it is invisible — the key simply does nothing, in a program that was waiting for
it. `directional_focus` hands Alt+Arrow back when there is no pane that way, and `tab_step` hands
Ctrl+Tab back in a one-tab window. `no_default_binding_claims_a_single_modifier_chord_from_the_shell`
(in `config/load.rs`) enforces it for every default: a single-modifier chord must either fall
through like those two, or write no bytes at all the way `Ctrl+Comma` does. It asks
`input::means_something_to_the_program`, i.e. the real encoder, rather than a second list.

No chrome widget may hold egui focus. `release_chrome_focus` surrenders it every pass, which is
the only thing keeping Tab-then-Enter from pressing a button instead of running the command.

### What the program is told

Four things a terminal owes whatever is running in it, all of which Nyx did not say at all until
they were added — and each one fails silently, which is why they lasted:

- **`term/kitty.rs`** — the kitty keyboard protocol. `TermConfig::kitty_keyboard` is now `true`,
  so `alacritty_terminal`'s negotiation and mode stack are live, and `kitty::encode` is the
  answer. Its `None` return is the load-bearing half: with only `DISAMBIGUATE_ESC_CODES` set,
  ordinary typing and a bare Enter must still take the legacy path, or a pane whose editor exited
  without popping the mode has no usable shell in it.
- **`spawn::base_env`** — `TERM`, `COLORTERM`, `TERM_PROGRAM`. Invisible from a PowerShell pane
  and wrong everywhere else; `TERM` and `COLORTERM` defer to an environment that already set
  them, `TERM_PROGRAM` never does.
- **`input::focus_report`** — `CSI I` / `CSI O` for a program that asked with `CSI ?1004h`. Sent
  to every pane in the window, because that is what the mode reports; pane-to-pane focus is
  deliberately not reported.
- **`term/links.rs`** — `http` and `https` only, opened on Ctrl+click through `win::open_url`,
  which checks the scheme a second time. The text came out of a pipe, so every other scheme is a
  local launcher and a bare click is too easy.

### Expansion: two layers that must never both fire

- **Shell-side** — generated PowerShell, Clink and bash init scripts (`expand/shellgen/`) that work
  inside the shell's own line editor and announce themselves over the *window title*:
  `nyx:<token>:<kind>[:payload]` ([expand/handshake.rs](src/expand/handshake.rs)). The title is the
  transport because `alacritty_terminal` routes OSC 0/2 to `Event::Title` and unknown OSC codes
  nowhere at all. The token is per pane and per launch, so output cannot forge integration.
- **Terminal-side** — `Shadow`, a best-effort model of the input line built from keystrokes on
  their way to the PTY, which rewrites by sending backspaces. It works in every shell including
  WSL and ssh, and none of them especially well. It stays quiet while integration is `Pending` or
  `Active`.
- **The gate is the `may_expand` / `may_hint` pair**
  ([expand/shadow.rs:91](src/expand/shadow.rs:91)). `may_expand` = permission to *rewrite* the
  line: off on the alternate screen, off under integration, off when desynced. `may_hint` =
  permission to merely *describe* it (completion menu, inline hint, whole-line `@sigil` + Enter):
  off on the alternate screen, off when desynced, but **on** under integration. Collapsing the two
  either turns the hint off in the three shells where it is most accurate, or sends a second
  rewrite of a line the shell already rewrote.
- **Three signals say "this is not a prompt"**, and the sigil layer needs all three because each
  covers what the others cannot: `child_running` (exact, but only under shell integration), the
  alternate screen (`may_hint`), and `sigil::driven` — a pane in a kitty keyboard mode, since a
  prompt never turns that on and only a program wanting every keystroke does. The third is what
  covers an inline program in a plain cmd.exe, WSL or ssh pane, where nothing else can tell.
  `[expand] completion = false` is the switch for the cases even that cannot see.

### Config and persistence

`%APPDATA%\Nyx\config.toml`, written fully commented on first launch, watched with a 150 ms
debounce plus a content fingerprint. A parse failure keeps the *running* config and reports the
key by name; only the first load at startup is allowed to fall back to defaults, because there is
no running config to keep. Saves go through `toml_edit` and merge into the existing text, so
comments, key order, absent defaults and line endings all survive. `render.scrollback` applies only
to newly constructed terminals and `window.*` only at startup — the app says so rather than
silently ignoring the key.

Generated scripts and TSV tables live in `%LOCALAPPDATA%\Nyx\shell`; shortcut images and sound
packs resolve beside the executable (`win/assets.rs`).

## Invariants that are not visible from any one file

- **Output-driven repaints go through `RepaintCoalescer::poke`.** The handful of direct
  `request_repaint` calls (focus loss, applied settings draft, copy feedback, atlas rebuild) are
  each justified at their site and none is on the PTY-output path. Adding one there is the
  regression this rule exists to prevent.
- **Snapshot capture never blocks on the `Term` lock**: it uses `try_lock_unfair` and reuses the
  last snapshot if the lock is held, so a whole-history search scan — which holds the lock for its
  entire duration — cannot stall the frame. `app/copy_blocks.rs` takes the same exception for the
  pane under the pointer. Everything else takes the lock normally, and should keep doing so.
- **Never add `cosmic-text` as a direct dependency.** glyphon re-exports it; a second copy resolves
  to a different patch and you get two `FontSystem` types that do not unify, behind a hundred-line
  error.
- **`panic = "abort"` is deliberately absent** from the release profile: a panicking PTY thread must
  take down one pane, not a window with five other live shells in it.
- **Float config values are compared at `f32`, not `f64`.** TOML parses `0.45` as `f64`, the schema
  narrows it to `f32`, and widening it back gives `0.44999998807907104`. Comparing at `f64` made
  every save report "changed" and write seventeen digits over the two in the file.

## Conventions

- **Every fix gets a test that fails before the change.** There is no git history in this tree, so
  that test is the record of the bug. Verify a fix by mutation — break it deliberately and watch
  the new test fail — not just by a green run.
- **Comments name the bug the line prevents**, not what the line does. That is why this code reads
  the way it does; match it rather than trimming it.
- **`NyxApp` cannot be constructed in a test** — it needs a `CreationContext` with a live wgpu
  render state. So decision rules get extracted as pure free functions with their tests beside
  them: `sigil::decide` ([app/sigil.rs:324](src/app/sigil.rs:324)) and
  `chrome::navigation_refusal` ([app/chrome.rs:633](src/app/chrome.rs:633)) are the precedents.
  Methods keep the `&mut self` effects; only the ordering moves out.
- **`term::testing::headless_term`** gives a real `Term` — grid and parser, no PTY, no GPU — under
  `cargo test`. Snapshots, search and selection all take `&mut Term` and nothing else, so most
  terminal-behaviour tests belong here rather than behind a mock.
- **Moving code between modules is done verbatim.** A move that also edits is what makes a
  refactor untrustworthy; edit in a separate step.
- **A test that cannot run must skip loudly**, naming the missing dependency (the Clink harness
  even prints the winget line). A silent `return` is the same defect as a feature that was never
  run.

## Traps in this working tree

- **`review-harness/` is a gitignored older copy of every source file**, including a pre-split
  `app.rs` of 2,172 lines. Any repo-wide grep, find or IDE search matches it and returns confidently
  wrong answers about the current code — exclude it when searching, and never edit it. It builds
  only from inside its own directory, where its `.cargo/config.toml` gives it its own target dir.
- **`STATUS.md` and `ANALYSIS.md` (both 16 September) are snapshots with stale numbers and claims**
  — 290 and 386 tests against today's 449, "clippy clean" against today's 4 warnings, and modules
  such as `ui/rail.rs` that no longer exist. `review/` (19 September) is the next newest and
  reconciles them, but it predates the input-fidelity pass below and so is stale in its turn.
- **An input-fidelity pass (20 September) closed eight findings** and the older records do not
  know about them: the pass-through rule for bindings, `[expand]`, the kitty keyboard protocol,
  `spawn::base_env`, focus reporting, Shift+Insert, clickable links, and Git Bash detection by
  `PATH`. Two bugs found on the way are worth not reintroducing — the palette's close chord was
  hardcoded to `Ctrl+P` while the binding was configurable, and `run_chord`'s tab arms were
  guarded in a way that made the match's catch-all swallow a new variant silently.
- **This is not a git repository.** Everything after the "phases 1–8 complete" baseline exists in
  exactly one place, on one disk.
- The rendering path, ConPTY, the hotkey, Quake, the IPC pipe end to end and the cursor subclass
  are **not** covered by the test gate. The project's own record is emphatic about this: the last
  GUI pass found a defect that eight phases and 258 tests had missed. A change in those areas is
  unverified until the app has been run.
