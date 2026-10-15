# Correctness

Ranked by severity. No Critical or High findings: the crash and data-loss surface has already
been worked over twice and I could not find a way in.

---

### [Medium] The completion menu rewrites the input line under `may_hint`, not `may_expand`

**Where:** `src/app/sigil.rs:230` (`accept_completion`), `src/app/sigil.rs:276`
(`clear_input_line`); gate at `src/app/sigil.rs:145`; `src/expand/shadow.rs:91,107`.

**Trigger:** In a PowerShell, Clink or Git Bash pane (integration `Active`), type `@n`, press Tab
or Enter. Nyx writes `0x7f` × 2 followed by `@nyx` straight to the PTY.

**Detail:** `CLAUDE.md` states the rule plainly:

> **`Shadow::may_expand` is the only thing that decides whether the terminal-side layer touches a
> keystroke.** … A second copy of that condition anywhere is how the two layers end up disagreeing
> about one keystroke.

`accept_completion` and `clear_input_line` both *touch a keystroke* — they emit DEL bytes and
replacement text into the shell's live line editor — and both are reached through
`handle_sigil_key`, which gates on `may_hint` (`!ALT_SCREEN && synced`) with no
`integration_active` term. `sigil.rs`'s own module doc argues the distinction correctly for the
hint and the menu *display*, and then routes two writers through the same gate.

The reasoning that makes `whole_line_sigil` safe under integration is explicit and does not
transfer: "an expansion fires on space or Enter, so a line with neither in it has not been
rewritten by anybody." `accept_completion` fires on **Tab**, mid-line, on a line that may already
contain a space — i.e. on a line the shell's own layer may already have rewritten. The model is
kept in step by `mirror_shell_expansion`, so the byte count is *probably* right, but "probably"
is exactly what `may_expand` exists to remove.

**Impact:** A miscount sends too few DELs and leaves fragments of the old token on the line,
which then runs as part of the command. The damage is bounded by `Shadow` being `synced`, so this
is a narrow window rather than a routine failure — but it is unguarded and, per 05, untested.

**Fix:** Either add the fourth condition to `may_expand` and gate the two *writers* on it (leaving
the hint and the menu on `may_hint`), or state in `shadow.rs` that `may_hint` is also a write gate
and say why the integration term is safe to omit. The current code makes the first claim in a
comment and the second in behaviour.

**Confidence:** Verified (the gate and the two write sites); Probable (that a real desync is
reachable in a live shell — that needs a GUI check).

---

### [Medium] A second "Open Nyx here" can miss the pipe and open a new window

**Where:** `src/win/ipc.rs:165` (`send`), server loop at `src/win/ipc.rs:209-254`.

**Trigger:** Right-click → "Open Nyx here" twice in quick succession, or once while the launcher
hotkey also fires a `focus` request.

**Detail:** The server creates the pipe with `nMaxInstances = 1`, serves one client, then
`DisconnectNamedPipe` + `CloseHandle` and loops back to `CreateNamedPipeW`. Between the close and
the next create **there is no pipe with that name at all**. `send` calls `CreateFileW` once and
treats any `INVALID_HANDLE_VALUE` as "nothing was listening":

```rust
if handle == INVALID_HANDLE_VALUE {
    return false;
}
```

`main.rs:144` then reads that `false` as "no instance is running" and falls through to opening a
whole second window — with its own GPU device, its own ~180 MB, and its own shells. The standard
client pattern for a single-instance named pipe (`WaitNamedPipeW` on `ERROR_PIPE_BUSY`, retry on
`ERROR_FILE_NOT_FOUND`) is absent.

The window is small — microseconds — but it is hit by exactly the input the feature is for: two
clicks in a row. The launcher makes it slightly likelier, because `launcher::run` sends a `focus`
request on every hotkey press and spawns `nyx --focus` when `send` returns false.

**Impact:** The advertised behaviour of `--attach` and `single_instance` fails intermittently, in
the direction that is most visible (an extra window) and least diagnosable (it works nine times
out of ten). The comment at `main.rs:147` treats falling through as a deliberate safety valve for
"an instance that holds the mutex but is not listening"; it cannot distinguish that case from
this one.

**Fix:** Retry `CreateFileW` for a bounded time — a few tries over ~200 ms, calling
`WaitNamedPipeW` on `ERROR_PIPE_BUSY` — before concluding nothing is listening. Alternatively
raise `nMaxInstances` above 1 and create the next instance before serving the current one, which
closes the gap at the source.

**Confidence:** Verified.

---

### [Medium] An externally edited launcher hotkey never reaches the running launcher

**Where:** `src/win/launcher.rs:44` (`ensure`), `src/main.rs:129`,
`src/app/settings_apply.rs:54`.

**Trigger:** Edit `launcher.hotkey` in `config.toml` with an external editor (which is a
first-class supported path — the config is watched by directory precisely so this works). Restart
Nyx, or reboot.

**Detail:** The launcher is a separate process registered in `HKCU\…\Run` with its chord baked
into the command line:

```rust
let command = format!("\"{}\" --launcher {} {}", …, binding.modifiers, binding.vk);
```

Only `configure` rewrites that value, and `configure` is reached from exactly two places:
`settings_apply.rs:54`, which fires only when `config.launcher != self.config.launcher` *while
the app is running* (i.e. a GUI change or a hot reload), and `--refresh-launcher`, which
`install.ps1` runs at install time.

At startup `main.rs` calls `ensure`, which short-circuits the moment it sees a launcher process
alive:

```rust
let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name("stop").as_ptr()) };
if !event.is_null() { … return Ok(()); }
```

It checks *that a launcher is running*, never *which chord it registered*. So after an external
edit the running launcher keeps the old chord; and after a reboot the `Run` value — which still
carries the old chord — starts a launcher with it. There is no path that converges.

A hot reload while Nyx is open *does* fix it, via `settings_apply.rs:54`. The failure is
specifically: edit the file while Nyx is closed, or edit it and reboot.

**Impact:** A configured hotkey silently does nothing and the previous one silently still works.
There is no diagnostic; `launcher-status.txt` still reads "Windows launcher registered", because
it is.

**Fix:** Have `ensure` compare the desired chord against the `Run` value (or against a chord the
launcher writes into `launcher-status.txt`) and call `configure` on a mismatch, rather than
treating liveness as sufficient.

**Confidence:** Verified for the code paths; Probable for the reboot leg, which I could not run.

---

### [Low] `Instance::drop` closes a null handle

**Where:** `src/win/ipc.rs:119-124`, reached from `src/win/ipc.rs:140`.

**Trigger:** `CreateMutexW` fails (out of handles, or a namespace the token cannot reach).

**Detail:** The fallback is deliberate and right — behave as the only instance rather than refuse
to start — but it does it by manufacturing a sentinel:

```rust
return Some(Instance(ptr::null_mut()));
```

and `Drop` unconditionally does `CloseHandle(self.0)`. `CloseHandle(NULL)` returns FALSE
harmlessly in a normal process, but raises `STATUS_INVALID_HANDLE` under a debugger or with the
"Enable close exception" gflag set — which turns a graceful degradation into a crash exactly when
someone is debugging.

**Impact:** None in production. A debugging papercut, and an invalid-handle close that a future
reader will have to re-reason about.

**Fix:** `if !self.0.is_null() { CloseHandle(self.0) }`, or make the field `Option<HANDLE>`.

**Confidence:** Verified.

---

### [Low] One stuck local client wedges "Open Nyx here" for the rest of the session

**Where:** `src/win/ipc.rs:259` (`read_request`), server loop at `:209`.

**Trigger:** Any same-user process connects to `\\.\pipe\Nyx.<user>` and holds the connection
open without writing.

**Detail:** The server is a single sequential loop over a `PIPE_WAIT` pipe with
`nMaxInstances = 1`, and `read_request` loops on blocking `ReadFile` until a zero read. There is
no `SetNamedPipeHandleState` timeout, no overlapped read, and no way to abandon a connection. A
client that connects and then blocks holds the only pipe instance for as long as it lives.

**Impact:** Explorer's context menu, `--attach` and the launcher's focus request all stop working
until Nyx is restarted. Requires a process running as the same user, so this is a robustness
issue rather than a security one — but a crashed or suspended helper reaches it by accident, not
only by malice.

**Fix:** An overlapped read with a wait, or the simpler `nMaxInstances > 1` so one wedged client
does not exclude the next.

**Confidence:** Verified.

---

### [Low] `@shortcut` + Enter always opens the first detected profile

**Where:** `src/app/sigil.rs:254`.

**Trigger:** Focus a WSL, Git Bash or custom `[[profile]]` pane, type `@nyx`, press Enter.

**Detail:**

```rust
let profile = self.profiles.first().cloned();
match self.new_tab(profile.as_ref(), Some(cwd)) { … }
```

The new tab always launches `profiles[0]` — whatever detection happened to rank first, usually
PowerShell — rather than the profile of the pane the user was typing in. `ANALYSIS.md` records
the same defect for splits ("Splits currently launch the first resolved profile, not the source
pane's profile"); this is a second site with the same shape, added since.

It is worse here than for splits, because the shortcut's `path` is a **Windows** path and the
tab it opens may be a shell that cannot use one: `@nyx` in a WSL pane opens a PowerShell tab at
`C:\...`, which is at least coherent, but the user asked to go there *in the shell they were in*.

**Impact:** A surprising shell, silently. No error, because nothing failed.

**Fix:** Carry the focused pane's profile id into `open_shortcut_tab` and prefer it, falling back
to `profiles.first()`.

**Confidence:** Verified.

---

### [Low] `open_shortcut_tab` erases the input line before it knows the tab can open

**Where:** `src/app/sigil.rs:252-258`.

**Trigger:** `@shortcut` + Enter where the directory exists but `new_tab` fails (the shell binary
was moved, ConPTY creation failed, the profile is broken).

**Detail:** The order is `clear_input_line()` → `new_tab()` → toast on error. The clear is
correct and well justified — the Enter never reached the shell, so the line has to come back off
the prompt — but it happens before the operation it is compensating for has succeeded. On the
error path the user is left with a toast, an empty prompt, and their typed `@nyx` gone.

The `!cwd.is_dir()` case is handled correctly (it returns before clearing), which shows the
ordering was considered for one failure and not the other.

**Impact:** A few characters of retyping, once, on a rare path.

**Fix:** Clear after `new_tab` returns `Ok`.

**Confidence:** Verified.

---

### [Low] `apply_quake`'s early return also skips the mouse-wheel refresh — *fixed*

> **Fixed.** `self.wheel = crate::win::wheel_lines()` moved above the early return, so it runs on
> every config reload as its comment always claimed.

**Where:** `src/app/settings_apply.rs:147-176`.

**Detail:** The function opens with

```rust
if settings == self.quake_settings { return; }
```

and ends with

```rust
// The wheel setting is the system's, not ours, and a config reload is the natural moment
// to notice someone changed it in Mouse Properties.
self.wheel = crate::win::wheel_lines();
```

The comment says "a config reload"; the code says "a config reload that also changed a Quake
setting". Every other reload — which is all of them, in practice — leaves `self.wheel` at
whatever it was when the app started.

**Impact:** Changing "lines to scroll per notch" in Mouse Properties is not picked up until Nyx
restarts or a Quake setting happens to change. Small, but the comment asserts otherwise, which is
the part worth fixing either way.

**Fix:** Move the `self.wheel` assignment above the early return, or into `apply_config`.

**Confidence:** Verified.

---

### [Low, needs check] Rebinding the launcher hotkey can race its own stop event

**Where:** `src/win/launcher.rs:66-91` (`configure`), `:132` (`CreateEventW`), `:192-196`.

**Detail:** `configure` calls `stop()` — which signals a **manual-reset** event and never resets
it — and then immediately spawns the replacement launcher. The new process calls `CreateEventW`
with the same name; if the old process has not yet closed its handle, the named object still
exists *and is still signalled*, so the new launcher's first
`MsgWaitForMultipleObjectsEx` returns 0 and it exits at once, leaving no launcher running.

In practice the new process first spins on `CreateMutexW` for up to 3 s waiting for the old one's
instance mutex, and the old one closes the stop handle (`:194`) *before* the mutex handle
(`:195`), so by the time the mutex is free the event should be gone. The ordering is correct but
undocumented and load-bearing — swapping those two `CloseHandle` lines would introduce the bug
with nothing to catch it.

**Impact:** If the race is reachable, the launcher silently stops existing after a hotkey change,
and `launcher-status.txt` still says "Windows launcher registered".

**Fix:** Cheap insurance: `ResetEvent` after the wait breaks, or create the stop event with
`bManualReset = 0`. Either way, a comment at `:194` saying the close order matters.

**Confidence:** Needs check — I could not run two launcher processes to observe it.
