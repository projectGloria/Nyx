# Nyx — status report

*16 September 2026. All eight phases complete, and run.*

Nyx is a Windows terminal emulator in pure Rust — a CMDer replacement built for latency rather
than portability. It renders the grid on the GPU through a custom wgpu pipeline, hosts shells
through ConPTY, and exists to be fast enough that the speed is the feature.

**19,118 lines across 74 files · 290 tests · `cargo check` clean · 17.3 MB release binary.**

---

## What you asked for, and what exists

Your original brief was four things. All four are built.

| You asked for | Status | Where it lives |
|---|---|---|
| **Blazing fast** | Done, and falsifiable — see [Measured](#measured-not-estimated) | whole design |
| **Path shortcuts with customizable icons** | Done. `builtin:` vectors, `file:` images, `exe:` shell icons | `expand/table.rs`, `ui/icons.rs`, `ui/rail.rs` |
| **Prefix support** | Done — all four kinds | `expand/` |
| **Custom theming: colours, font, resizable window** | Done, live-editable, comment-preserving | `theme/`, `config/`, `ui/settings/` |

You chose "prefix support" to mean **all four kinds**, and all four work:

| Kind | How it works |
|---|---|
| Path sigil — `cd @tibet␣` | Shell-side in PowerShell's line editor; terminal-side shadow buffer everywhere else |
| Command alias — `gs` → `git status` | Native shell function where integration is live; first-word rewrite on Enter otherwise |
| Leader chord — tmux style | Always Nyx-side; the keystrokes never reach the PTY |
| Wrapper prefix | **`spawn` mode only** (via a profile's `args`) — see [What is left](#what-is-left) |

And the four v1 scope items you picked are all in: **split panes**, **scrollback search**,
**Quake/dropdown mode**, **Explorer "Open Nyx here"**.

---

## Phase by phase

Each phase ended in something launchable. That was the point of the phasing.

### Phase 1 — a window hosting one shell
eframe window, one ConPTY, snapshot → glyphon + background quads, keystrokes, cursor.
Settled the things that are expensive to change later: the `ColorMode` choice, the viewport reset,
the repaint coalescer, and the `NYX_PERF=1` frame-time overlay.

**Also produced the measurements that corrected two wrong assumptions in the plan** — memory is
dominated by the graphics stack, not by us, and cold start is ~3 s rather than the promised
sub-100 ms.

### Phase 2 — a terminal you would actually use
Full key matrix, bracketed paste, mouse reporting, scrollback and wheel, selection → clipboard,
alternate screen, OSC 52 and colour queries, bell, child exit. `vim` works. `git log` works.
10,000 lines of scrollback work.

### Phase 3 — tokens and config
`config.toml` with hot reload, every hardcoded colour deleted, live font changes, system font
enumeration. Edit the file in another editor and the window restyles. A typo costs that one value
and reports itself by name.

### Phase 4 — tabs, splits, profiles
Split tree with draggable gutters, geometric focus (`Alt+Arrow`), zoom, tab strip, and shell
detection: cmd.exe, PowerShell 5.1, pwsh, Git Bash, WSL distros, Clink.

### Phase 5 — shortcuts, rail, palette, shadow expansion
The icon rail down the left edge, the `Ctrl+P` fuzzy palette, and the terminal-side expansion
layer — a best-effort model of the input line that rewrites sigils and aliases by sending
backspaces. `win/paths.rs` (Windows ↔ MSYS ↔ WSL translation) landed here with an exhaustive test
table, written before anything used it.

### Phase 6 — shell-side integration
Generated init scripts for PowerShell, Clink and bash; the `nyx:<token>:` handshake over the
window title; leader chords. Two layers that must never both fire — `Shadow::may_expand` is the
single place that decides.

### Phase 7 — search and the settings GUI
`RegexSearch` across the whole scrollback, highlighting only the visible rows. A full settings
window with a live theme editor, font picker, shortcut/icon manager, keybinding recorder, and
profile editor — all writing back through `toml_edit` so comments, key order and formatting
survive.

### Phase 8 — Windows integration and ship
Global hotkey, Quake dropdown, single instance, named pipe, Explorer context menu, DPI manifest,
`exe:` icons, the real wheel-scroll setting, and per-user install/uninstall scripts.

---

## Measured, not estimated

Taken on this machine (Windows 11, 144 Hz, RTX 3060 laptop).

| | |
|---|---|
| Warm start to window | ~170 ms release; ~3 s on a genuinely cold first run |
| Working set | 179–181 MB, unchanged from Phase 1 to Phase 8 |
| Of which is the terminal | **~10 MB.** A bare eframe+wgpu window with nothing in it is 171 MB |
| Idle CPU (settled) | 0, 15.6, 62.5, 78.1 ms per 30 s — at or below the 15.6 ms timer granularity |
| Idle CPU (first 30 s) | ~280 ms — that is startup, not idle |
| Hotkey + IPC threads | No measurable cost; both block in a kernel wait |
| 67 MB dumped through the grid | 70.9 s, **zero UI stalls** in 268 samples, worst UI response 24 ms |
| Same file, Windows Terminal | 70.8 s — **identical** |

**Throughput is ConPTY-bound at about 1 MB/s, for us and for Windows Terminal alike.** `cmd /c
type` to a pipe manages 13 MB/s, so the bottleneck is the pseudoconsole, not the renderer. That
retires throughput as a design risk and makes input latency and idle cost the things worth
defending — which is what the numbers above actually measure.

---

## What is left

Four different kinds of "left". They are not equally important.

### 1. Written but never run on this machine

The highest-value thing anyone could do next, because it is the only category where something
might simply be **wrong** rather than absent.

| What | Why it is untested | What it costs if broken |
|---|---|---|
| `shellgen/clink.rs` | Clink is not installed here | cmd.exe falls back to terminal-side expansion |
| `shellgen/bash.rs` | Git Bash is not installed here | Git Bash falls back to terminal-side expansion |
| WSL path translation end to end | No WSL distro installed | `@shortcut` may expand to a wrong `/mnt/c/` path |

The Clink hook names (`clink.onfilterinput`, `rl.setbinding`) have moved between Clink versions,
and `bind -x` under MSYS readline is the part most likely to need adjusting. **All three degrade
safely**: a script that fails to load means the handshake never arrives and the shadow layer takes
over. The cost is the *better* expansion, not the feature.

The WSL one is different in kind — it is a correctness question, not a degradation. The
translation table is exhaustively unit-tested, but the tests assert against expectations, not
against a real distro.

### 2. Deliberately not built

Each of these was a decision, not an oversight.

- **Wrapper prefix: `session` and `per_command` modes.** Only `spawn` exists (through a profile's
  `args`). The other two need the prompt-to-prompt marks to be reliable in every shell, and they
  are only reliable in PowerShell today.
- **`file:` icon preview in the settings window** shows a placeholder rather than the image.
  `builtin:` and `exe:` are drawn for real. Mis-drawing someone's PNG at 48 px is worse than
  saying "file".
- **Application keypad mode (DECKPAM).** egui exposes no numeric-keypad keys at all, so this is
  not implementable without patching egui.
- **The kitty keyboard protocol.** Nobody has asked, and it is a large surface.
- **`render.scrollback` does not apply to running shells.** `Term` fixes its history size at
  construction. The app *says so* when you change it rather than silently ignoring the key.

### 3. Blocked on something outside the code

- **No `.ico` resource in the executable.** Explorer, the Start-menu shortcut and the context-menu
  entry all show the generic application icon. Needs `rc.exe` from the Windows SDK *and* artwork
  that does not exist. Cheap to add once there is a design.
- **Windows 11's short context menu.** "Open Nyx here" lands under "Show more options". The
  top-level sheet only accepts an `IExplorerCommand` from a registered package — an MSIX identity,
  signing certificate and packaging pipeline for the privilege of launching a terminal.
- **No code signing.** SmartScreen will warn on first run of a downloaded copy. This is a
  certificate purchase, not a code change.

### 4. Known optimisation, not urgent

**An ASCII fast path in `render/shape.rs`.** Under sustained full-screen output a frame is
`shape 2225 µs · text prepare 327 µs · snapshot 140 µs · quads 27 µs` — shaping dominates
everything else by 7×, at ~65 µs per row. A row that is pure ASCII in the primary font at default
weight could skip cosmic-text's `Shaping::Advanced` entirely.

**This buys battery life, not wall time.** Throughput is ConPTY-bound, so the terminal will not
get faster. Worth doing eventually; worth nobody's afternoon today.

---

## Verification, and its limits

`cargo check` and `cargo test` are the gate. 290 tests, all passing, concentrated in the pure
modules where the subtle bugs live:

| Module | Tests |
|---|---|
| `term/` — input encoding, mouse, search, profiles, detection | 70 |
| `expand/` — sigils, aliases, shadow model, chords, shell script generation | 63 |
| `win/` — path translation, quake geometry, IPC protocol, registry, hotkey chords | 45 |
| `config/` — parsing, key bindings, comment-preserving write-back | 41 |
| `ui/` — palette matching and elision, find bar status, icons | 28 |
| `layout/` — split solving, geometric focus, tab and pane removal | 24 |
| `theme/`, `render/`, `util/`, `app/`, `event` | 19 |

Rendering and PTY behaviour are verified by running the app against real shells, once per phase.

**Two things were verified only indirectly.** Explorer's context menu was confirmed three ways —
the registry keys, the exact command line executed verbatim (including the awkward `C:\` drive-root
case where the trailing backslash escapes the closing quote), and the shell's own verb list
returning `Open Nyx here` — but a literal right-click was never driven, because another
application held the foreground and taking it would have meant interfering with the machine.
And the second monitor case for Quake geometry is unit-tested against negative coordinates but was
only ever run on one screen.

### The GUI pass — done, 16 September

The roadmap called this "the largest untested surface by far", because the gate has no GPU, no
PTY and no window. It has now been walked in a real release build, driven by synthesised input
against a live PowerShell, and every claim below is a thing that was watched happen rather than
reasoned about:

| Checked | Result |
|---|---|
| Grid, colour, ANSI | PSReadLine's own syntax colouring, correct; columns stay a grid |
| Idle repaints | 47–94 ms of CPU over 30 s — three to six timer ticks. It draws nothing when idle |
| Memory | 181.9 MB working set, matching the figure measured at the end of Phase 1 |
| Splits | Three panes; the focused one a solid cursor and a border, both unfocused ones hollow |
| Scrollback search | Match highlighted in the grid, `1 on screen`; `no matches here` in an empty pane |
| Tab then Enter | Completes a path and runs the line. No new tab — `release_chrome_focus` holds |
| Hot reload | An external edit restyled grid and chrome live |
| A bad colour | A toast naming the key, that value alone fell back, the other 263 kept |
| Background bell | `•` on the ringing tab, cleared by looking at it |
| Sigil expansion | `cd @nyx␣` became `cd "C:\Users\Nihil\Desktop\Free Tibet\Nyx"`, quoted for the space |
| Alias expansion | `gs` became `git status`, in colour |
| Quake | Hiding parks the window at y=-464, *above* the work area, then vanishes; showing snaps to 1920x464 — full width, 45% of a 1032 px work area |
| Global hotkey | Registered, and toggles from inside another application's foreground |
| Release logging | The bad-colour warning reached `%LOCALAPPDATA%\Nyx\logs\nyx.log`. That was 4.1's untested half |

**It found one defect, and it was exactly the kind the gate cannot hold.** The Ctrl+P palette drew
each row's detail right-aligned and its label left-aligned, with neither bounded — so a profile
whose program is a full `C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe` was drawn
straight back through its own label. Nothing overflowed and nothing was clipped; two strings
simply occupied the same pixels, which is unreadable on screen and invisible to every test in the
repo. `palette::elide_front` now cuts the detail to the room the label actually left.

It elides from the *front*, which is the opposite of `app::trim_title`: a window title's subject
comes first, but a path's tail is the half that tells two of them apart — every stock shell lives
under `System32`, so a head-elided detail would render cmd and PowerShell identically and the
palette would stop answering the question it exists to answer.

**The test that fix first shipped with was worthless, and that is the more useful finding.** It
asked a `Ui` from egui's test harness to measure the strings. That harness loads no font data and
reports every string as **zero pixels wide** — so every input "fit", nothing was ever elided, and
three of its four assertions passed without executing a line of the rule. `elide_front` now takes
its measurer as an argument: the app passes the painter, the tests pass seven pixels per
character. A test that cannot fail is the same defect as a feature that was never run, one level
up.

---

## Two bugs worth remembering

Both were found by running the thing, not by reading it.

**The `Ctrl+ControlLeft` recorder bug (Phase 7).** egui reports Ctrl, Shift and Alt *both* as
modifier flags and as key presses of their own — and the modifier arrives first. The settings
window's keybinding recorder took it at face value and wrote `find = "Ctrl+ControlLeft"` into the
live config. Fixing it also fixed a latent Phase 6 bug: pressing Shift on the way to a capital
letter cancelled an armed leader chord.

**Float comparison at the wrong precision (Phase 8).** TOML parses `0.45` as an `f64`, the schema
narrows it to `f32`, and widening it back gives `0.44999998807907104` — which is not the number in
the file. `config/write.rs` compared at `f64`, so every save would have reported "changed" and
written seventeen digits over the two you typed. It went unnoticed for a whole phase because every
earlier default (`6.0`, `13.0`, `1.25`, `960.0`) happens to be exactly representable.
`quake.height = 0.45` was the first one that was not.

---

## Where things are

```
src/
  main.rs      single instance, the command line, eframe::run_native
  app.rs       NyxApp: the update loop. Owns panes, config, theme, focus, mouse
  event.rs     AppEvent — the only non-UI → UI channel — plus the repaint gate
  config/      schema · load · watch · write · paths · keys
  theme/       resolved once; the only colours in the program
  layout/      tree · geometry · focus · Tab          (pure: no terminal, no rendering)
  expand/      table · line · shadow · handshake · chords · shellgen/
  term/        pane · proxy · spawn · snapshot · input · mouse · select · search
               detect · profile
  render/      metrics · color · shape · quads · callback
  ui/          icons · rail · palette · findbar · settings/
  win/         paths · hotkey · quake · ipc · registry · shellicon · console
build.rs       embeds nyx.manifest (per-monitor-v2 DPI, long paths, UTF-8, asInvoker)
install.ps1    per-user install under %LOCALAPPDATA%\Programs\Nyx; uninstall.ps1 undoes it
CLAUDE.md      architecture and the invariants that are not visible from any one file
AGENTS.md      coding style and commands
```

Config lives at `%APPDATA%\Nyx\config.toml`, written fully commented on first launch — 315 lines
across 13 tables. Generated shell scripts and other derived state go to `%LOCALAPPDATA%\Nyx`.

Dependencies: `eframe`/`egui`/`egui-wgpu` + `wgpu` for the window and chrome, `glyphon` for grid
text, `alacritty_terminal` for VT emulation and ConPTY, `toml_edit` for comment-preserving
write-back, `windows-sys` for the Win32 surface. No async runtime, no web view, no Node.

---

## If you pick this up again

In the order I would actually do them:

1. **Install Clink and run a cmd.exe pane.** It is the single largest untested surface, and the
   settings window already offers the `winget install chrisant996.clink` line.
2. **Install Git Bash and check `bind -x`.** Same reasoning, smaller surface.
3. **Run it on a second monitor**, especially a 100%/150% DPI pair, with Quake mode on.
4. **Draw an icon** and add the `.ico` resource. It is the most visible missing thing and the
   cheapest to fix.
5. Everything else is optional.
