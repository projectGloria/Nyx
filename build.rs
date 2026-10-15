//! Embeds `nyx.manifest` into the executable.
//!
//! Done with linker arguments rather than a crate. `winresource` and `embed-resource` both exist
//! to shell out to `rc.exe` for icons and version resources; a manifest needs none of that —
//! `link.exe` takes one directly, and `/MANIFESTINPUT` may be given more than once, so ours is
//! merged with whatever rustc already supplies instead of colliding with it.
//!
//! Everything in the manifest has to be declared *before* the process starts. `dpiAwareness` set
//! at runtime arrives after the first window exists, which on a 150% monitor means one frame at
//! two thirds of the right cell size; `longPathAware` is read when the process is created and
//! cannot be set at all afterwards.

fn main() {
    println!("cargo:rerun-if-changed=nyx.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    // MSVC's linker only. A `gnu` host would fail on the flags, and cross-compiling to anything
    // else has no use for a Windows manifest.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("nyx.manifest");

    // `-bins`, not the bare form: applying these to test binaries as well works, but embeds a
    // manifest into every `cargo test` artifact for no reason.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
}
