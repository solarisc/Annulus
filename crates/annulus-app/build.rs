//! Gives the Windows exe our icon: `assets/annulus.res` (made by `cargo xtask icon`) goes to
//! the MSVC linker, which accepts compiled resources directly.

fn main() {
    println!("cargo:rerun-if-changed=assets/annulus.res");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if windows && msvc {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let res = std::path::Path::new(&manifest).join("assets/annulus.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
