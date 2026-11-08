//! Bundled fonts, installed-family discovery and private font imports. No OS installation.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use parley::fontique::{Blob, Collection, CollectionOptions};

/// The bundled terminal default, with Nerd symbols supplied by a fallback font.
pub const DEFAULT_MONO: &str = "JetBrains Mono";
/// The private-use symbol fallback; a curated OFL build (see `fonts/README.md`).
pub const SYMBOL_FAMILY: &str = "Symbols Nerd Font Mono";
/// `JetBrains Mono` Regular, OFL 1.1.
pub const JETBRAINS: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
/// `JetBrains Mono` Bold, OFL 1.1.
pub const JETBRAINS_BOLD: &[u8] = include_bytes!("../fonts/JetBrainsMono-Bold.ttf");
/// Nerd symbols from the OFL-only build distributed by `WezTerm`.
pub const SYMBOLS: &[u8] = include_bytes!("../fonts/SymbolsNerdFontMono-Regular.ttf");

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_CUSTOM_BYTES: usize = 64 * 1024 * 1024;

struct Registry {
    revision: u64,
    data: Vec<Blob<u8>>,
    names: Vec<String>,
    custom_bytes: usize,
    system: Vec<Blob<u8>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            revision: 1,
            data: [
                super::cpu::text::UI_REGULAR,
                super::cpu::text::UI_BOLD,
                super::cpu::text::MONO,
                JETBRAINS,
                JETBRAINS_BOLD,
                SYMBOLS,
            ]
            .into_iter()
            .map(|bytes| Blob::new(Arc::new(bytes)))
            .collect(),
            names: ["Fira Sans", "Fira Mono", DEFAULT_MONO]
                .map(str::to_owned)
                .to_vec(),
            custom_bytes: 0,
            system: Vec::new(),
        })
    })
}

/// A revision and shared font bytes for a renderer. No disk access.
pub fn snapshot() -> (u64, Vec<Blob<u8>>) {
    let state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (
        state.revision,
        state.data.iter().chain(&state.system).cloned().collect(),
    )
}

/// Changes only when font data or the available family list changes.
pub fn revision() -> u64 {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .revision
}

/// Known bundled, imported and discovered installed font families.
pub fn families() -> Vec<String> {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .names
        .clone()
}

/// Discovers installed families. Call from a worker, never the drawing thread.
pub fn discover() {
    static DISCOVERED: OnceLock<()> = OnceLock::new();
    DISCOVERED.get_or_init(|| {
        let mut collection = Collection::new(CollectionOptions {
            shared: true,
            system_fonts: true,
        });
        let names: Vec<String> = collection.family_names().map(str::to_owned).collect();
        let mut state = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .names
            .extend(names.into_iter().filter(|name| valid_name(name)));
        state.names.sort_by_cached_key(|name| name.to_lowercase());
        state.names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        state.names.retain(|name| name != SYMBOL_FAMILY);
        state.revision += 1;
    });
}

/// Loads only the selected installed families into memory, on a worker. Replaces the
/// previous installed-font cache, so browsing families cannot grow it without bound.
///
/// # Errors
/// A selected family is unavailable or its faces exceed the 32 MB selection budget.
pub fn load_selected(ui: Option<&str>, mono: Option<&str>) -> Result<(), String> {
    let base = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .data
        .clone();
    let mut ours = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    for data in base {
        ours.register_fonts(data, None);
    }
    let mut installed = Collection::new(CollectionOptions {
        shared: true,
        system_fonts: true,
    });
    let mut cache = parley::fontique::SourceCache::default();
    let mut data = Vec::new();
    let mut size = 0;
    for name in [ui, mono].into_iter().flatten() {
        if ours.family_id(name).is_some() {
            continue;
        }
        let family = installed.family_by_name(name).ok_or_else(|| {
            format!("Font family {name} is unavailable; using the bundled fallback.")
        })?;
        for face in family.fonts().iter().take(4) {
            let bytes = face
                .load(Some(&mut cache))
                .ok_or("The selected font could not be loaded.")?;
            if data.iter().any(|known: &Blob<u8>| known.id() == bytes.id()) {
                continue;
            }
            size += bytes.len();
            if size > 32 * 1024 * 1024 {
                return Err("The selected font families exceed 32 MB.".into());
            }
            data.push(bytes);
        }
    }
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.system = data;
    state.revision += 1;
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control)
}

fn read_font(path: &Path) -> Result<Vec<u8>, String> {
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
    {
        return Err("Choose a .ttf, .otf or .ttc font file.".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "The font file could not be opened.")?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The font file could not be read.")?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("Font files must be no larger than 16 MB.".into());
    }
    Ok(bytes)
}

fn names(bytes: &[u8]) -> Vec<String> {
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    let registered = collection.register_fonts(Blob::new(Arc::new(bytes.to_vec())), None);
    registered
        .iter()
        .filter_map(|(id, _)| {
            collection
                .family_name(*id)
                .filter(|name| valid_name(name))
                .map(str::to_owned)
        })
        .collect()
}

fn register(bytes: Vec<u8>, names: &[String]) -> Result<(), String> {
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.data.iter().any(|known| known.as_ref() == bytes) {
        return Ok(());
    }
    if state.data.len() >= 38 || state.custom_bytes + bytes.len() > MAX_CUSTOM_BYTES {
        return Err(
            "The custom font limit is 32 files or 64 MB. Restart after removing unused fonts."
                .into(),
        );
    }
    state.custom_bytes += bytes.len();
    state.data.push(Blob::new(Arc::new(bytes)));
    state.names.extend_from_slice(names);
    state.names.sort_by_cached_key(|name| name.to_lowercase());
    state.names.dedup();
    state.revision += 1;
    Ok(())
}

/// Loads a previously imported font. Invalid or missing files leave bundled fonts usable.
///
/// # Errors
/// A readable explanation when the file is missing, invalid or exceeds the limits.
pub fn load(path: &Path) -> Result<Vec<String>, String> {
    let bytes = read_font(path)?;
    let names = names(&bytes);
    if names.is_empty() {
        return Err("This file contains no supported font families.".into());
    }
    register(bytes, &names)?;
    Ok(names)
}

/// Validates and copies a font into the app's private directory, then makes it available.
/// Run on a worker. Existing files are never overwritten.
///
/// # Errors
/// When the font is invalid, too large, or cannot be copied.
pub fn import(source: &Path, directory: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let bytes = read_font(source)?;
    let names = names(&bytes);
    if names.is_empty() {
        return Err("This file contains no supported font families.".into());
    }
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("ttf");
    let path = directory.join(format!("font-{:016x}.{extension}", hash.finish()));
    std::fs::create_dir_all(directory)
        .map_err(|_| "The private font folder could not be created.")?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(&bytes)
            .map_err(|_| "The font could not be copied.")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_font(&path)? != bytes {
                return Err("A different font already occupies that private filename.".into());
            }
        }
        Err(_) => return Err("The font could not be copied into the private font folder.".into()),
    }
    register(bytes, &names)?;
    Ok((path, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_terminal_fonts_and_symbols_have_the_expected_families() {
        assert!(names(JETBRAINS).iter().any(|name| name == DEFAULT_MONO));
        assert!(names(SYMBOLS).iter().any(|name| name == SYMBOL_FAMILY));
    }

    #[test]
    fn a_valid_custom_font_is_copied_privately_and_can_be_loaded_again() {
        let dir = std::env::temp_dir().join(format!("annulus-font-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("original.ttf");
        std::fs::write(&source, JETBRAINS).unwrap();
        let (path, names) = import(&source, &dir.join("private")).unwrap();
        assert_ne!(path, source);
        assert!(names.iter().any(|name| name == DEFAULT_MONO));
        assert_eq!(std::fs::read(&path).unwrap(), JETBRAINS);
        assert_eq!(load(&path).unwrap(), names);
        assert_eq!(import(&source, &dir.join("private")).unwrap().0, path);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir.join("private"));
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn invalid_fonts_are_refused_before_any_private_file_is_written() {
        let dir = std::env::temp_dir().join(format!("annulus-font-reject-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("invalid.ttf");
        std::fs::write(&source, b"not a font").unwrap();
        let target = dir.join("private");
        assert!(import(&source, &target).is_err());
        assert!(!target.exists());
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_dir(dir);
    }
}
