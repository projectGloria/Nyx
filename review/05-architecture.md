# Architecture, testability and maintainability

The structure here is good and I am not going to pad this section by suggesting otherwise. The
module boundaries are real (`term/` never draws, `render/` never holds a `Term`, `win/` is the
only Win32 caller, `GridSnapshot` is the only thing that crosses the thread seam), the `app/`
split was verified byte-for-byte, and the documentation carries reasoning rather than
restatement.

The findings are about the edges of that: one untested layer, and documentation that has drifted
out from under the code.

---

### [High] The one layer that takes keys from the terminal has no tests — *fixed*

> **Fixed.** The obstacle was structural and worth recording: `NyxApp` is never constructed in
> any test and cannot be — it needs a `CreationContext` with a live wgpu render state — so
> `handle_sigil_key`, being a method on it, was untestable as written. That is *why* the layer
> had no tests.
>
> Resolved by the precedent already in this codebase: `navigation_refusal` (`app/chrome.rs:633`),
> a private free function whose doc comment says purity is what makes the rule testable. The
> routing decision is now `fn decide(...) -> Action` at the bottom of `app/sigil.rs`, with
> eleven tests beside it. `handle_sigil_key` keeps every `&mut self` effect — writing bytes,
> opening the tab, syncing `Shadow` — and only the ordering moved out. `whole_line_sigil` is
> still asked of `Shadow` rather than re-derived, so there stays one implementation of what a
> whole-line sigil is; `decide` orders the rules, it does not own them.
>
> Six of the seven cases listed below are covered. The outstanding one is **(5) nothing is
> claimed on the alternate screen, while desynced, or while `child_running`** — those three
> guards live in `handle_sigil_key`'s preamble, above `decide`, and are still reachable only
> through a `NyxApp`. They are each one line and each delegates to an already-tested predicate
> (`may_hint`, `child_running`), but the composition is unpinned.
>
> Verified by mutation rather than by passing: inverting the menu-versus-whole-line precedence
> makes `enter_on_a_bare_sigil_opens_the_highlighted_row_rather_than_the_line_as_typed` fail
> with `OpenTab(0)` against `OpenTab(1)` — the exact defect the rule exists to prevent.
>
> **This edited `handle_sigil_key` rather than moving it verbatim**, which is the thing
> `CLAUDE.md`'s 4.5 rule warns about. The exception taken here: that rule assumes the
> destination is untested, and the edit is what buys the coverage. Worth a line in `CLAUDE.md`
> either way.

**Where:** `src/app/sigil.rs` — `handle_sigil_key` (:132), `accept_completion` (:230),
`open_shortcut_tab` (:239), `clear_input_line` (:276), `sigil_overlay` (:37).

**Detail:** A grep for those five names across `src/` returns their definitions, one call site in
`app.rs:935`, and nothing else. There is no `#[cfg(test)] mod tests` in `app/sigil.rs` and no
test elsewhere that drives them.

What is tested is everything *underneath*: `complete::menu` and `complete::accept` have ten
cases between them, `Shadow::whole_line_sigil` has four, `bytes_for` has one, `hint` has its own.
Each is pure and each is pinned. What is not tested is the decision layer that sits on top of
them and answers, per keystroke:

- whether to claim Tab, Escape, Up or Down from the terminal at all;
- whether Enter answers the menu, opens a tab, or falls through to the shell;
- which of `accept_completion`, `open_shortcut_tab` or nothing runs;
- what bytes go to the PTY, and whether `Shadow` is updated to match.

This is the same gap `ANALYSIS.md` named as the project's most consequential seam — "Helper
correctness does not automatically establish that the frame uses the helper correctly" — and the
same shape as the bug a real session found last time, where `release_chrome_focus` had been
missing for eight phases because nothing that could see it had ever run.

It matters more here than for most app code because of what the layer does. `app.rs` is mostly
routing; `app/sigil.rs` **withholds keystrokes from the shell and injects bytes into the input
line**. Its two failure modes are "a key the user pressed did nothing" and "characters the user
did not type appeared on the line" — the two things a terminal is least allowed to do. And the
Enter path has four branches whose ordering is argued at length in the comments and enforced
nowhere.

The infrastructure already exists. `app.rs`'s own test module drives real egui passes
(`tab_then_enter_presses_a_chrome_button_unless_focus_is_released`,
`the_expansion_path_encodes_modified_text_exactly_as_the_plain_one_does`,
`under_integration_the_model_follows_the_shell_without_writing_anything`), and
`term::testing::headless_term` builds a real `Term`. Nothing about this layer needs a GPU or a
PTY to test; it needs a `Shadow`, a `Tables`, a `TermMode` and an assertion about the returned
bool and the bytes written.

**Impact:** The newest, most keyboard-invasive code in the program is the only part with no
regression net, and the invariants that hold it together live only in comments.

**Fix:** The cases worth pinning first, roughly in order of what breaks worst:

1. Tab, Up, Down and Escape are **not** claimed when no menu is on screen (the history-recall
   and path-completion regression).
2. Enter with `cd @n` and a menu open accepts the completion; Enter with `@nyx` alone opens a
   tab; Enter with `@nyx` and a menu open follows the highlighted row, not the typed text.
3. A chord (`m.any()`) is never claimed.
4. `accept_completion` writes exactly `bytes_for(expansion)` and leaves `Shadow::line()` equal to
   what the shell would now show.
5. Nothing is claimed on the alternate screen, while desynced, or while `child_running`.
6. `clear_input_line` erases exactly `line.chars().count()`, including after a
   `mirror_shell_expansion`.

**Confidence:** Verified (the absence is a grep result, not an inference).

---

### [Medium] Three `CLAUDE.md` invariants are currently false

`ROADMAP.md` carries a section named "Lines in CLAUDE.md that are currently false", and its
doctrine is explicit: *"When an invariant here stops being true, fix the code or move the
sentence to that ROADMAP section — do not leave it standing."* All three entries in that section
are currently struck through as resolved, so the section reads as empty. These three belong in it
(or the code belongs changed).

**1. The `Term` lock is taken once per pane per frame.**

> **The `Term` lock is taken once per pane per frame, in `snapshot.rs`, and released before any
> GPU work.**

`app/copy_blocks.rs:24` takes it a second time, at `app.rs:1055`, before the pane loop at
`app.rs:1128` takes it again. Detail and suggested fix in
[02-performance.md](02-performance.md#low-the-terminal-lock-is-taken-twice-per-frame-for-the-pane-under-the-pointer).

**2. `try_lock_unfair` is reserved for the pane being scanned.**

> The exception is deliberately narrow … every other pane, and every frame with no scan running,
> takes the lock the same way it always did.

`copy_block_hover` uses `try_lock_unfair` on every frame the pointer is over any pane, with no
scan involved. The sentence describes a property the code no longer has.

**3. `request_repaint` is called only through `RepaintCoalescer::poke`.**

> **`request_repaint` is called only through `RepaintCoalescer::poke`.**

There are now four direct `Context::request_repaint()` calls — `app.rs:772`,
`app/settings_apply.rs:186`, `app/copy_blocks.rs:65`, `render/callback.rs:134` — plus eight
`request_repaint_after`. Each one is individually justified (focus loss, draft applied, copy
feedback, atlas rebuild) and none of them is on the high-frequency PTY-output path the coalescer
exists for. The invariant is right in spirit and wrong as written; it should say that
*output-driven* repaints go through the coalescer.

**Why this matters more here than in most codebases:** this project's documentation is load-bearing
in a way that is unusual. It is the specification, the regression record, and — per its own
introduction — the thing a future editor is expected to preserve rather than rediscover. A false
invariant in a file like this is worse than an undocumented one, which `ROADMAP.md` already says
in so many words about finding 1.3.

**Confidence:** Verified.

---

### [Low] `review-harness/` is 8,000 lines of stale source sitting in the working directory

**Where:** `review-harness/` (gitignored at `.gitignore:11`).

**Detail:** It is correctly kept out of git, correctly given its own `--target-dir`, and its
README correctly warns that its findings are leads rather than facts. The reason it ran from
`target/` originally, and the reason it was moved out, are both well recorded. None of that is
the problem.

The problem is that it is an **older revision** of every source file in `src/`, sitting on disk
next to the current one. Any tool that does not read `.gitignore` — most `grep`/`rg` invocations,
IDE project-wide search, and every coding agent given this directory — matches both copies. The
line-count survey I opened this review with returned `review-harness/src/app.rs` at 2,172 lines
as the largest file in the repository, which is the *pre-split* `app.rs` that
`ROADMAP.md` 4.5 records as having been reduced to 1,348. It is an active source of wrong
answers about the current state of the code.

`source-hashes.json` pins which revision each finding came from, which is the right idea — but it
does not stop the files themselves from being read by mistake.

**Impact:** Wasted time and confidently wrong conclusions, for anyone (or anything) reading the
tree rather than the git index.

**Fix:** The harness has done its job — every finding it produced is closed and recorded in
`ROADMAP.md` with the evidence each one actually had. Archive it outside the working directory
(a sibling folder, or a zip) rather than keeping a second live source tree inside it. If it stays,
a `review-harness/README.md` line saying *which* revision it is a copy of would at least make the
staleness discoverable.

**Confidence:** Verified.

---

### [Low] The working tree carries an entire phase against a Phase 1–8 baseline

**Where:** `git status` — 61 modified files, 13 untracked source files, HEAD at `11a68b7`.

**Detail:** `ROADMAP.md` 0.1 records this as a decision, not an oversight:

> **0.1 Version control — *out of scope*.** Not part of this project, by decision. Everything
> below is written to stand without it: each fix gets a test that fails before the change, which
> is the part that does not need history.

That reasoning is sound for *reviewability*, and the project has honoured it — the test-first
discipline really does substitute for a diff. So this is recorded for one narrower reason only:
the repository *does* have seven commits, ending at "Baseline: phases 1-8 complete", and
everything since — Phase 9, the sigil layer, the `app/` split, `theme/presets.rs`, four new
`win/` modules — exists in exactly one place, on one disk. The byte-for-byte verification that
makes the `app/` split trustworthy was performed against a copy that is no longer recoverable.

Not a defect in the code, and not a recommendation about how to work. Noted because a review that
listed the risks and left this one out would be incomplete.

**Confidence:** Verified.

---

### [Low] Documentation drift between five overlapping records

**Where:** `CLAUDE.md`, `ROADMAP.md`, `STATUS.md`, `REVIEW.md`, `ANALYSIS.md`.

**Detail:** The project keeps five prose records, and two of them are known-obsolete by the
others' own admission. `CLAUDE.md` says: *"`STATUS.md` and `REVIEW.md` are older snapshots and
both contain obsolete counts"*. `ANALYSIS.md` is more specific: STATUS.md *"reports 290 tests and
says Clink/Git Bash were not installed or exercised"*, and its claim about file-icon previews
*"disagrees with the implemented preview code"*.

Two documents that are only correct once you have read a third to learn they are wrong are a trap
for exactly the reader they are addressed to. `CLAUDE.md` and `ROADMAP.md` are actively
maintained and genuinely excellent; the other three are sediment.

`ANALYSIS.md` is also dated "16 September 2026" and describes itself as "the current one", which
will stop being true the moment anything lands.

**Impact:** Small and self-limiting, but it costs every new reader the same twenty minutes.

**Fix:** Fold whatever is still true out of `STATUS.md` and `REVIEW.md` into `ROADMAP.md`, and
delete them. `REVIEW.md`'s value is historical (it is the input `ROADMAP.md` was written against)
and `ROADMAP.md` already preserves each of its twenty findings verbatim in the Tier 2 table.

**Confidence:** Verified.

---

## What is genuinely good, and should not be refactored away

Recorded because a later pass with less context might "tidy" one of these.

- **`term::testing::headless_term`.** A real `Term` with no PTY and no GPU is the single
  highest-leverage piece of test infrastructure in the repo, and it is under-used — the sigil
  layer above could be tested through it today.
- **Tests that shell out to real interpreters.** `powershell`, `clink` and `bash` round-trips
  found five bugs that no amount of reading found, including two "fixes" that passed review and
  did nothing. This is expensive and slow and worth every millisecond. The Clink harness's
  loud-skip-on-stderr is the right call; `win/sounds.rs`'s
  `supplied_release_sound_packs_decode_completely` silently `return`s when
  `target/release/sounds` is absent, which is the pattern the Clink harness exists to avoid — a
  one-line `eprintln!` would bring it in line. **Confirmed by running them:** both shell
  harnesses skip loudly and name the missing dependency, and Clink's prints the winget command.
  The criticism applies to `win::sounds` alone.
- **The `app/` split's verbatim-move rule.** "Anything later that moves code here *and* edits it
  gives that up" is the correct constraint and the only reason that refactor is trustworthy.
- **Comments that name the bug.** `term/cursor.rs`'s module doc and `expand/shadow.rs`'s
  `may_expand`/`may_hint` pair are the best examples. They are why this review could go deep in a
  few hours instead of a few days.
