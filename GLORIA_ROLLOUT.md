# Gloria rollout notes

## Please

- **Icon:** there is no `.ico` in the repo, so `gloria.toml` uses `assets/nyx-icon.png` (512x512). Nothing was created or edited.
- **Version:** `gloria.toml` says `0.1.0` (from `Cargo.toml`); the launcher catalog says `0.1.4`. A tag build overrides it, so
  tag `v0.1.4` or later, and bump `Cargo.toml` to match if you want the exe's own version to agree.
- **Runtime files:** only `nyx.exe`. Nyx looks for `sounds\` and `icons\` beside the exe, but the repo ships none, so they are not packed.
- **Data folders:** `%APPDATA%\Nyx` (config) and `%LOCALAPPDATA%\Nyx` (logs, generated shell scripts), from `src/config/paths.rs`.
- **legacy_names:** left out; Nyx has only shipped through `install.ps1`, not NSIS/Inno/MSI. `install.ps1` installs into
  `%LOCALAPPDATA%\Programs\Nyx`, the same folder GloriaSetup uses, so anyone who used it should uninstall first.
- **Secrets:** `WINDOWS_PFX_BASE64` and `WINDOWS_PFX_PASSWORD` must exist in the repo; without them the setup is unsigned.
- **Not built locally**, as instructed. The release build takes about 12 minutes cold and this workflow has not been run.
