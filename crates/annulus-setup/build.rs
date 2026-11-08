//! Packs the programs the installer puts in place, and gives the Windows exe our icon.
//!
//! `cargo xtask dist` builds the app and `annulus-agent` first, then this crate with
//! `ANNULUS_SETUP_PAYLOAD` naming the folder they are in: each is deflated into `OUT_DIR` and
//! `payload.rs` includes them. Without it (a debug build, clippy, the tests) the installer
//! carries nothing and says so when asked to install.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The programs, by their names in the payload folder, then the licenses from the repository.
const PROGRAMS: [&str; 2] = ["annulus-app.exe", "annulus-agent.exe"];
const LICENSES: [&str; 4] = [
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "LICENSE-THEMES",
    "LICENSE-FONTS",
];

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed=ANNULUS_SETUP_PAYLOAD");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|error| error.to_string())?);
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if windows && msvc {
        // The app's icon (`cargo xtask icon`), and a manifest that asks for no administrator:
        // without one, Windows takes an exe named "setup" for an old installer and asks for
        // elevation the per-user install never needs.
        let res = manifest.join("../annulus-app/assets/annulus.res");
        println!("cargo:rerun-if-changed={}", res.display());
        println!("cargo:rustc-link-arg-bins={}", res.display());
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
    }
    let mut code = String::from(
        "/// The files the installer puts in place: name, size, and the bytes deflated.\n\
         pub const FILES: &[(&str, u64, &[u8])] = &[\n",
    );
    if let Ok(folder) = std::env::var("ANNULUS_SETUP_PAYLOAD") {
        let folder = PathBuf::from(folder);
        let sources = PROGRAMS
            .iter()
            .map(|name| (*name, folder.join(name)))
            .chain(
                LICENSES
                    .iter()
                    .map(|name| (*name, manifest.join("../..").join(name))),
            );
        for (name, source) in sources {
            println!("cargo:rerun-if-changed={}", source.display());
            let bytes =
                std::fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;
            let packed = out.join(format!("{name}.z"));
            pack(&bytes, &packed)?;
            let _ = writeln!(
                code,
                "    ({name:?}, {}, include_bytes!({:?})),",
                bytes.len(),
                packed.display().to_string()
            );
        }
    }
    code.push_str("];\n");
    std::fs::write(out.join("payload.rs"), code).map_err(|error| error.to_string())
}

/// `bytes` deflated into `to`.
fn pack(bytes: &[u8], to: &Path) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", to.display());
    let file = std::fs::File::create(to).map_err(failed)?;
    let mut encoder = flate2::write::DeflateEncoder::new(file, flate2::Compression::best());
    encoder.write_all(bytes).map_err(failed)?;
    encoder.finish().map(drop).map_err(failed)
}
