//! Embeds `nyx.manifest` into the executable.
//!
//! The manifest is done with linker arguments rather than a crate. `embed-resource` is used only
//! for the VERSIONINFO block below, which does need `rc.exe`; a manifest needs none of that —
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

    embed_version_info();
}

/// VERSIONINFO, so Windows names the app "Nyx" (Task Manager, taskbar, "Open with") and the
/// release check can read FileDescription. Only the version block goes through rc.exe here.
fn embed_version_info() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let v: Vec<u16> = env!("CARGO_PKG_VERSION")
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let (vc, vs) = (format!("{},{},{},{}", v[0], v[1], v[2], v[3]), format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3]));
    let rc = format!(
        "1 VERSIONINFO\nFILEVERSION {vc}\nPRODUCTVERSION {vc}\nFILEOS 0x40004\nFILETYPE 0x1\n\
         BEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904B0\"\n  BEGIN\n\
         \x20  VALUE \"CompanyName\", \"Project Gloria\"\n\
         \x20  VALUE \"FileDescription\", \"Nyx\"\n\
         \x20  VALUE \"ProductName\", \"Nyx\"\n\
         \x20  VALUE \"InternalName\", \"Nyx\"\n\
         \x20  VALUE \"OriginalFilename\", \"nyx.exe\"\n\
         \x20  VALUE \"FileVersion\", \"{vs}\"\n\
         \x20  VALUE \"ProductVersion\", \"{vs}\"\n\
         \x20 END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n"
    );
    let path = out.join("version.rc");
    std::fs::write(&path, rc).unwrap();
    embed_resource::compile(&path, embed_resource::NONE).manifest_optional().unwrap();
}
