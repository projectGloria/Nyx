# Security

No Critical, High or Medium findings. The trust boundaries in this codebase are handled well
enough that the useful output of this section is mostly the list of things I checked and found
correct, so a later pass does not spend its budget re-deriving them.

Threat model, as the code itself frames it: a single-user desktop application. The adversaries
that matter are (a) hostile bytes arriving from a program running inside the terminal, and (b)
another process running as the same user on the same machine. Nyx is not network-facing, has no
privileged component, and runs `asInvoker`.

---

### [Low] The IPC pipe's `focus` field is unconstrained

**Where:** `src/win/ipc.rs:73-100`, `src/win/launcher.rs:167-172`.

**Trigger:** Any same-user process writes `nyx1\nfocus\t1\n` to `\\.\pipe\Nyx.<user>`.

**Detail:** The module doc makes a precise claim and keeps it for `cwd`:

> **Everything arriving on the pipe is data.** The payload names a directory and is used as one:
> it is checked to be an existing directory … a local process that can reach the pipe can open a
> tab somewhere, and that is the whole of what it can do.

`focus` was added later and is not quite covered by that sentence: it un-hides and raises the
window. `sanitized()` validates `cwd` and does not look at `focus`, and there is no rate limit,
so a same-user process can raise and re-raise the Nyx window in a loop.

Default pipe security is correct for this: with a NULL security descriptor the default DACL
grants *read* to Everyone and full control to the creator — and this is `PIPE_ACCESS_INBOUND`, so
a client needs `GENERIC_WRITE`, which other users do not get. The exposure really is same-user
only.

**Impact:** A focus-stealing nuisance available to code that, running as the same user, could
already read the config, write the generated shell scripts, or simply call `SetForegroundWindow`.
Not a privilege boundary.

**Fix:** Optional. If you want the doc comment to stay exactly true, extend it to name `focus`
and what it can do, rather than changing the code.

**Confidence:** Verified.

---

### [Low] The launcher's chord is taken from an HKCU value and parsed without bounds

**Where:** `src/main.rs:86-93`, `src/win/launcher.rs:114,139`.

**Detail:**

```rust
let modifiers = raw.get(1).and_then(|a| a.parse().ok()).unwrap_or(0x4006);
let vk        = raw.get(2).and_then(|a| a.parse().ok()).unwrap_or(0x20);
…
RegisterHotKey(ptr::null_mut(), 7, modifiers, vk)
```

Those two `u32`s come from the `Software\Microsoft\Windows\CurrentVersion\Run` value that
`configure` wrote, and are passed straight to `RegisterHotKey` with no range check. An attacker
who can write HKCU could point the `Run` value at a different binary entirely, so this is not a
new capability — and `RegisterHotKey` validates its own arguments and returns 0 rather than
misbehaving.

**Impact:** None beyond what HKCU write access already grants. Recorded because it is an
unvalidated path from persisted state into a Win32 call, which is a shape worth knowing about if
the launcher ever gains more arguments.

**Fix:** None needed. If the launcher's argument list grows, validate there.

**Confidence:** Verified.

---

## Checked and correct

Each of these is a place where a plausible vulnerability would live, and each is handled — with
the reasoning written down at the site, which is why they took minutes rather than hours to
clear.

- **Bracketed paste terminator injection** (`term/input.rs`). The rescanning strip is the right
  fix and the invariant now states *why* one pass is insufficient. This is the highest-severity
  thing in the file and it is closed.
- **OSC 52 read** (`term/spawn.rs`, `term/proxy.rs`). Refused in two places, deliberately
  redundant, with the reasoning ("a security property that holds because of somebody else's
  default is one upgrade away from not holding") recorded. Correct, and correctly paranoid.
- **OSC 52 primary-selection write** (`event::reaches_system_clipboard`). Dropped rather than
  redirected. The destructive-mirroring argument is right.
- **The title-channel handshake** (`expand/handshake.rs`). The token stops impersonation; the
  payload is explicitly untrusted, `decode_percent` works on bytes end to end, and the
  `%+1`/`u8::from_str_radix` sign bug is closed. Parsing runs on the UI thread and cannot panic
  on attacker-chosen input, which is the property that matters.
- **IPC payload validation** (`Request::sanitized`). A path from the pipe is checked to be an
  existing directory and used only as `ShellSpec.cwd` — never as a program, an argument, or
  anything a shell expands. `MAX_MESSAGE` caps the read. The tag check rejects non-Nyx traffic.
- **Registry writes** (`win/registry.rs`) are HKCU-only, as the module and `--help` both state.
  No HKLM path exists, so the context menu never needs elevation.
- **`unsafe`** — every block carries a `// SAFETY:` line, and the ones I read
  (`win/cursor.rs`, `win/ipc.rs`, `win/launcher.rs`, `win/assets.rs`) are sound: the subclass
  reference data points into a stable `Box`, `WM_NCDESTROY` clears the handle, dialog buffers
  outlive their modal calls, and the `GetLastError` reads are immediate.
- **Log file** (`main.rs:54`) is under `%LOCALAPPDATA%` with a size ceiling and an append open,
  not a world-writable path.
- **`write_atomic`** never deletes the destination, per the invariant and the test.

## Not assessed

- No `cargo audit` / `cargo deny` was run — no toolchain. `Cargo.lock` exists but I did not
  cross-reference advisories. The dependency set is conservative (no network stack, no
  serialization of untrusted input beyond TOML and the two small line protocols), but
  `resvg`/`usvg`/`tiny-skia` and `image` are real parsers being handed user-chosen files, and an
  advisory sweep is worth running when a toolchain is available.
- `win/shellicon.rs`'s GDI buffer arithmetic was read only in outline. It converts an
  attacker-irrelevant input (a local `.exe`'s own icon) but does do size math against a header,
  which is the classic place for an off-by-one. Worth a dedicated read.
