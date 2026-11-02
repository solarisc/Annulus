//! `cargo xtask mac-app`: builds the release binary and wraps it in `Annulus.app` in the
//! target directory's `release` folder, so macOS shows the app's name and icon in the Dock,
//! the menu bar and Finder:
//!
//! - `Contents/Info.plist`: the name, bundle identifier, version and icon;
//! - `Contents/MacOS/Annulus`: the binary;
//! - `Contents/Resources/annulus.icns`: the icon `cargo xtask icon` drew.
//!
//! The bundle is signed ad hoc, which binds the Info.plist to it; signing with a Developer ID
//! and notarizing comes later.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::repo;

/// The name macOS shows, and the binary's name inside the bundle.
const NAME: &str = "Annulus";
/// Reverse DNS on the project's GitHub account.
const IDENTIFIER: &str = "io.github.solarisc.annulus";
/// The oldest macOS the bundle runs on: Big Sur, the first for Apple silicon.
const MINIMUM_MACOS: &str = "11.0";

pub fn run() -> Result<bool> {
    if !cfg!(target_os = "macos") {
        bail!("Annulus.app is built on a Mac: the binary in it must be a macOS one");
    }
    let root = repo::root()?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let built = Command::new(cargo)
        .args(["build", "-p", "annulus-app", "--release", "--locked"])
        .current_dir(&root)
        .status()
        .context("could not run `cargo build`")?;
    if !built.success() {
        return Ok(false);
    }
    let release = repo::target_dir(&root)?.join("release");
    let app = release.join(format!("{NAME}.app"));
    bundle(
        &app,
        &release.join("annulus-app"),
        &root.join("crates/annulus-app/assets/annulus.icns"),
        env!("CARGO_PKG_VERSION"),
    )?;
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .status()
        .context("could not run `codesign`")?;
    if !signed.success() {
        bail!("`codesign` could not sign {} ad hoc", app.display());
    }
    println!("built {}", app.display());
    Ok(true)
}

/// Writes the bundle `app` around `binary` and `icon`, replacing one built before.
fn bundle(app: &Path, binary: &Path, icon: &Path, version: &str) -> Result<()> {
    if app.exists() {
        std::fs::remove_dir_all(app).with_context(|| format!("removing {}", app.display()))?;
    }
    let contents = app.join("Contents");
    let programs = contents.join("MacOS");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&programs)?;
    std::fs::create_dir_all(&resources)?;
    // `copy` keeps the binary's permissions, so it stays executable.
    std::fs::copy(binary, programs.join(NAME))
        .with_context(|| format!("copying {}", binary.display()))?;
    std::fs::copy(icon, resources.join("annulus.icns"))
        .with_context(|| format!("copying {}", icon.display()))?;
    std::fs::write(contents.join("Info.plist"), info_plist(version))?;
    std::fs::write(contents.join("PkgInfo"), "APPL????")?;
    Ok(())
}

/// The bundle's Info.plist for `version` (Cargo's, which needs no escaping in XML).
fn info_plist(version: &str) -> String {
    let entries = [
        ("CFBundleDevelopmentRegion", Value::Text("en")),
        ("CFBundleDisplayName", Value::Text(NAME)),
        ("CFBundleExecutable", Value::Text(NAME)),
        ("CFBundleIconFile", Value::Text("annulus")),
        ("CFBundleIdentifier", Value::Text(IDENTIFIER)),
        ("CFBundleInfoDictionaryVersion", Value::Text("6.0")),
        ("CFBundleName", Value::Text(NAME)),
        ("CFBundlePackageType", Value::Text("APPL")),
        ("CFBundleShortVersionString", Value::Text(version)),
        ("CFBundleVersion", Value::Text(version)),
        (
            "LSApplicationCategoryType",
            Value::Text("public.app-category.developer-tools"),
        ),
        ("LSMinimumSystemVersion", Value::Text(MINIMUM_MACOS)),
        // Without it macOS draws a bundled app's window at half resolution, scaled up.
        ("NSHighResolutionCapable", Value::Yes),
        // Our frames are drawn on the CPU: no reason to wake the discrete GPU.
        ("NSSupportsAutomaticGraphicsSwitching", Value::Yes),
    ];
    let mut out = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
    ));
    for (key, value) in entries {
        let _ = writeln!(out, "\t<key>{key}</key>");
        let _ = match value {
            Value::Text(text) => writeln!(out, "\t<string>{text}</string>"),
            Value::Yes => writeln!(out, "\t<true/>"),
        };
    }
    out.push_str("</dict>\n</plist>\n");
    out
}

/// A value in the Info.plist.
enum Value<'a> {
    Text(&'a str),
    Yes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_holds_the_binary_its_icon_and_an_info_plist_naming_both() {
        let dir = std::env::temp_dir().join(format!("annulus-mac-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (binary, icon) = (dir.join("annulus-app"), dir.join("annulus.icns"));
        std::fs::write(&binary, b"binary").unwrap();
        std::fs::write(&icon, b"icon").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let app = dir.join("Annulus.app");
        // A bundle built before is replaced, nothing of it left.
        std::fs::create_dir_all(app.join("Contents/Stale")).unwrap();
        bundle(&app, &binary, &icon, "1.2.3").unwrap();
        let contents = app.join("Contents");
        assert!(!contents.join("Stale").exists());
        let read = |path: &str| std::fs::read(contents.join(path)).unwrap();
        assert_eq!(read("MacOS/Annulus"), b"binary");
        assert_eq!(read("Resources/annulus.icns"), b"icon");
        assert_eq!(read("PkgInfo"), b"APPL????");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(contents.join("MacOS/Annulus"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "still executable");
        }
        let plist = String::from_utf8(read("Info.plist")).unwrap();
        for (key, value) in [
            ("CFBundleExecutable", "<string>Annulus</string>"),
            ("CFBundleName", "<string>Annulus</string>"),
            ("CFBundleIconFile", "<string>annulus</string>"),
            ("CFBundleShortVersionString", "<string>1.2.3</string>"),
            (
                "CFBundleIdentifier",
                "<string>io.github.solarisc.annulus</string>",
            ),
            ("NSHighResolutionCapable", "<true/>"),
        ] {
            assert!(
                plist.contains(&format!("\t<key>{key}</key>\n\t{value}\n")),
                "{key} in\n{plist}"
            );
        }
        // macOS's own reader takes it.
        #[cfg(target_os = "macos")]
        {
            let lint = Command::new("plutil")
                .arg("-lint")
                .arg(contents.join("Info.plist"))
                .output()
                .unwrap();
            assert!(lint.status.success(), "{lint:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
