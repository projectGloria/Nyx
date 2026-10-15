# Codebase review — Nyx

Reviewed 19 September 2026 against the working tree at `C:\Users\nihil\Desktop\Nyx`
(HEAD `11a68b7`, "Put the paste invariant's consequence before its mechanism", plus a large
uncommitted Phase 9 / sigil working tree).

## Scope and method

**Read in full:** every untracked module, on the principle that the newest code has never been
through a review pass — `app/sigil.rs`, `app/copy_blocks.rs`, `term/blocks.rs`, `term/cursor.rs`,
`expand/complete.rs`, `ui/window.rs`, `ui/icons.rs`, `win/assets.rs`, `win/cursor.rs`,
`win/launcher.rs`, `win/sounds.rs`. Also read in full: `main.rs`, `win/ipc.rs`,
`expand/shadow.rs`, `app/settings_apply.rs`, and the `logic`/`ui`/`on_exit` frame in `app.rs`.

**Skimmed by targeted grep:** `render/`, `config/`, `layout/`, `term/search.rs`, `win/paths.rs`,
`ui/settings/`. These carry the bulk of the existing test suite and two prior review passes;
I aimed at the seams between them and the frame rather than re-reading them.

**Skipped:** `review-harness/` (a gitignored stale copy — see 05), `Cargo.lock`, the generated
shell script bodies (covered by real-interpreter tests).

**Prior work reconciled:** `ROADMAP.md`, `ANALYSIS.md` and `CLAUDE.md` were read first. Findings
those documents already record as known-and-deliberate are not repeated here. Where I contradict
a documented invariant, I say so and cite the line.

**Tools run:** none *at the time of the review*. The findings below were all derived by source
inspection, with no toolchain on the machine. A Rust toolchain and VS Build Tools were installed
afterwards and the gate was then run — see **Gate** and **Resolved in this pass** below. Where a
measurement contradicted a finding, the finding is corrected in place rather than quietly edited.

## Gate

Run 19 September 2026, after the review, on rustc 1.98.1 / VS 2022 BuildTools / SDK 10.0.26100:

| | |
|---|---|
| `cargo check` | clean, 0 warnings |
| `cargo test` | **411 passed, 0 failed** — of which 10 skipped (see below) |
| `cargo clippy --all-targets` | **4 warnings** |
| `cargo build --release` | clean, 0 warnings, 11m 54s — 21.9 MB binary |
| `nyx.exe --help` | runs, prints, exits 0, leaves no process behind |

The release build matters on its own: fat LTO with `codegen-units = 1` is a compile path the
debug profile does not exercise. The `--help` run also confirms `win::console` really does
borrow the launching console from a `windows_subsystem = "windows"` binary, which is otherwise
only assertable by reading.

Two things that changes:

- **The test count in `CLAUDE.md` (386) and `ANALYSIS.md` is stale** — it is 411 now (400 before
  the fixes in this pass).
- **`ANALYSIS.md`'s claim that clippy is "clean — 0 warnings, in the bin and test targets both"
  is false.** Four warnings: `app/find.rs:29`, `app/keys.rs:182`, `term/pane.rs:184`,
  `win/registry.rs:247`. Three are in code written since that was measured. All mechanical;
  `cargo clippy --fix` handles three of them. Left unfixed — they predate this pass.
- **10 of the 411 tests do not run here:** 5 Clink and 5 bash, because neither shell is installed
  on this machine any more. Both skip *loudly*, with a clear message naming the missing
  dependency, and the Clink one prints the winget command. That is the correct behaviour and
  corrects a doubt raised in [05](05-architecture.md); the silent-skip criticism applies only to
  `win::sounds`.

## Resolved in this pass

Three findings fixed, each verified against the gate.

- **[02] The icon cache is cleared on every settings keystroke** — `ui/icons.rs` gained `retain`
  and `app/settings_apply.rs` now retains live icon paths. Two corrections to the finding as
  written surfaced during the fix, and both are recorded in
  [02](02-performance.md#medium-every-settings-keystroke-throws-away-every-icon-texture).
- **[01] `apply_quake`'s early return skipped the wheel refresh** — moved above the return.
- **[05] The sigil key layer has no tests** — the routing decision is now a pure `decide`
  function with eleven tests beside it, following the `navigation_refusal` precedent. Verified
  by mutation, not just by passing: inverting the menu-versus-whole-line precedence makes
  `enter_on_a_bare_sigil_opens_the_highlighted_row_rather_than_the_line_as_typed` fail with
  `OpenTab(0)` against `OpenTab(1)`.

Everything else in these reports is unchanged and unfixed.

## Verdict

This is unusually disciplined code. The invariants are real invariants, the comments name the bug
each line prevents, and the test suite pins behaviour rather than implementation. Two prior review
passes have already burned through the obvious defect classes, and it shows: the panic surface is
almost nil (25 `unwrap`/`expect` sites outside tests, nearly all on fixed-size arrays), every
`unsafe` block carries a real `// SAFETY:` line, and the trust boundaries — the IPC pipe, the
title handshake, bracketed paste, OSC 52 — are each correct for reasons that are written down.

What is left is concentrated in one place: **the newest layer, `app/sigil.rs`, is the only part of
the program that takes keys away from the terminal and writes bytes to the PTY, and it has zero
test coverage.** Every pure helper under it is thoroughly tested; the wiring that decides when to
fire them is not. That is precisely the failure shape this project's own doctrine names — "a
feature whose failure mode is silence does not need a bug to be broken, only never to have been
run" — and it is the single highest-value thing to fix.

Three invariants in `CLAUDE.md` are now false, and `ROADMAP.md`'s section for recording exactly
that is empty.

## Top issues

| Severity | Finding | File | Category |
|---|---|---|---|
| High | The sigil key layer has no tests, and it is the layer that claims Tab/Enter/Escape/arrows | `app/sigil.rs` | [05](05-architecture.md#high-the-one-layer-that-takes-keys-from-the-terminal-has-no-tests) |
| Medium | Three `CLAUDE.md` invariants are currently false | `CLAUDE.md`, `app/copy_blocks.rs` | [05](05-architecture.md#medium-three-claudemd-invariants-are-currently-false) |
| Medium | Completion accept rewrites the PTY line under `may_hint`, bypassing `may_expand` | `app/sigil.rs:230` | [01](01-bugs.md#medium-the-completion-menu-rewrites-the-input-line-under-may_hint-not-may_expand) |
| Medium | `ipc::send` has no retry, so a rapid second "Open Nyx here" opens a window | `win/ipc.rs:165` | [01](01-bugs.md#medium-a-second-open-nyx-here-can-miss-the-pipe-and-open-a-new-window) |
| Medium | The launcher's hotkey never reconciles with an externally edited config | `win/launcher.rs:44` | [01](01-bugs.md#medium-an-externally-edited-launcher-hotkey-never-reaches-the-running-launcher) |
| Medium | The icon cache is cleared on every settings keystroke, needlessly | `app/settings_apply.rs:74` | [02](02-performance.md#medium-every-settings-keystroke-throws-away-every-icon-texture) |
| Medium | `sounds::packs()` scans a directory twice per frame while a settings tab is open | `ui/settings/system.rs:34` | [02](02-performance.md#medium-the-sound-pack-list-is-re-scanned-from-disk-twice-a-frame) |
| Medium | Closing the window kills every shell in every tab with no confirmation | `ui/window.rs:45` | [04](04-ux.md#medium-the-close-button-kills-every-shell-in-every-tab-with-no-confirmation) |
| Medium | The custom title bar gives up Windows 11 snap layouts and the Alt+Space menu | `ui/window.rs:42` | [04](04-ux.md#medium-the-custom-title-bar-drops-snap-layouts-and-the-system-menu) |

## Counts

| Category | Critical | High | Medium | Low |
|---|---|---|---|---|
| Bugs | 0 | 0 | 3 | 6 |
| Performance | 0 | 0 | 2 | 2 |
| Security | 0 | 0 | 0 | 2 |
| UX | 0 | 0 | 2 | 2 |
| Architecture | 0 | 1 | 1 | 3 |

## Blind spots

Large ones. Be sceptical of anything this review implies about them.

- ~~**No build, no tests, no lints.**~~ **Closed.** A toolchain was installed after the review and
  the gate was run; see **Gate** above. The tree compiles, the suite is green, and my doubt about
  `dirs = "7"` / `toml = "1"` was misplaced — both resolve (`dirs 7.0.0`, `toml 1.1.6`). Still
  not run: `cargo audit` / `cargo deny`, which are not installed.
- **10 tests still do not execute here** — the Clink and bash integration tests. Those are the
  two paths that produced five real bugs in Tier 3, so a green suite on this machine is weaker
  evidence than the number suggests. Installing `chrisant996.clink` and `Git.Git` closes it.
- **Nothing was run.** No window was opened, no shell was spawned, no GPU work was exercised. The
  rendering path, the ConPTY path, the hotkey, Quake, the IPC pipe end-to-end, and the custom
  cursor subclass are all judged by reading only. The project's own record is emphatic that this
  is the weakest kind of evidence for this codebase specifically — the last GUI pass found a bug
  that eight phases and 258 tests had missed.
- **Timings are unmeasured.** The two performance findings marked Medium are argued from call
  counts and known costs (`SHGetFileInfoW`, `read_dir` + `stat`, a texture upload), not from a
  profiler. The direction is certain; the magnitude is not.
- **WSL, IME and the mouse protocols** were not exercised, same as in every prior pass.
- **I did not re-verify the two prior reviews' closed findings.** `ROADMAP.md` and `ANALYSIS.md`
  claim each was fixed with a test that failed first. I took that at face value and spent the
  budget on code neither of them had seen.
- **`app.rs` between lines 1 and 660 and 1350 onward** was read only by signature and grep, not
  line by line.
