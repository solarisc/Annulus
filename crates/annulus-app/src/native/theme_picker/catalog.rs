//! Theme families and the cached search index.

use super::super::colors;
use annulus_config::schemes::{BUILT_INS, SCHEMES};
use std::sync::OnceLock;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    Dark,
    Light,
}

pub(super) struct Entry {
    pub(super) index: usize,
    pub(super) family: String,
    pub(super) search: String,
    pub(super) dark: Option<bool>,
}

pub(super) fn catalog() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = vec![Entry {
            index: 0,
            family: "Annulus".into(),
            search: "annulus built in system default".into(),
            dark: None,
        }];
        for (index, scheme) in SCHEMES.iter().chain(BUILT_INS).enumerate() {
            let family = if scheme.name.starts_with("rbit Classic") {
                "rbit Classic".into()
            } else if scheme.name.starts_with("rbit ") {
                "rbit".into()
            } else {
                family(scheme.name)
            };
            entries.push(Entry {
                index: index + 1,
                search: format!("{} {} {}", scheme.name, family, scheme.author).to_lowercase(),
                family,
                dark: Some(scheme.dark()),
            });
        }
        for (name, dark) in [(colors::OWN_LIGHT, false), (colors::OWN_DARK, true)] {
            if let Some(index) = colors::app_schemes().iter().position(|known| known == name) {
                entries.push(Entry {
                    index,
                    family: "Annulus".into(),
                    search: format!("{name} built in default").to_lowercase(),
                    dark: Some(dark),
                });
            }
        }
        let mut counts = std::collections::HashMap::new();
        for entry in entries.iter().skip(1) {
            *counts.entry(entry.family.clone()).or_insert(0) += 1;
        }
        for entry in entries.iter_mut().skip(1) {
            if counts[&entry.family] == 1 {
                entry.family = "More themes".into();
            }
        }
        entries[1..].sort_by_cached_key(|entry| {
            (
                if entry.family == "Annulus" {
                    0
                } else if entry.family == "rbit" {
                    1
                } else if entry.family == "rbit Classic" {
                    2
                } else if entry.family == "More themes" {
                    4
                } else {
                    3
                },
                entry.family.to_lowercase(),
                colors::app_schemes()[entry.index].to_lowercase(),
            )
        });
        entries
    })
}

fn words(name: &str) -> Vec<&str> {
    name.split(|c: char| c.is_whitespace() || ",()-_/".contains(c))
        .filter(|word| !word.is_empty())
        .collect()
}

pub(super) fn family(name: &str) -> String {
    static PREFIXES: OnceLock<std::collections::HashMap<String, usize>> = OnceLock::new();
    let lower = name.to_lowercase();
    for (prefix, title) in [
        ("rosé pine", "Rosé Pine"),
        ("rose pine", "Rosé Pine"),
        ("tokyodark", "Tokyo Night"),
        ("tokyo night", "Tokyo Night"),
        ("tokyo city", "Tokyo City"),
        ("one dark", "One"),
        ("one light", "One"),
        ("onedark", "One"),
        ("gruvbox material", "Gruvbox Material"),
        ("gruvbox", "Gruvbox"),
        ("catppuccin", "Catppuccin"),
        ("solarized", "Solarized"),
        ("selenized", "Selenized"),
        ("atelier", "Atelier"),
        ("black metal", "Black Metal"),
        ("github", "GitHub"),
        ("material", "Material"),
        ("everforest", "Everforest"),
        ("kanagawa", "Kanagawa"),
        ("nord", "Nord"),
        ("tomorrow", "Tomorrow"),
        ("horizon", "Horizon"),
        ("windows", "Windows"),
        ("papercolor", "PaperColor"),
        ("penumbra", "Penumbra"),
        ("precious", "Precious"),
        ("sakura", "Sakura"),
        ("silk", "Silk"),
        ("standardized", "Standardized"),
        ("stella", "Stella"),
        ("summerfruit", "Summerfruit"),
        ("tango", "Tango"),
        ("vulcan", "Vulcan"),
        ("equilibrium", "Equilibrium"),
        ("harmonic16", "Harmonic16"),
        ("humanoid", "Humanoid"),
        ("mellow", "Mellow"),
        ("oxocarbon", "Oxocarbon"),
        ("primer", "Primer"),
        ("da one", "Da One"),
        ("brush trees", "Brush Trees"),
        ("arroz con dulce", "Arroz con Dulce"),
        ("boo-shnickle", "boo-shnickle"),
        ("cerulean signal", "Cerulean Signal"),
        ("builtin", "iTerm2 Builtin"),
    ] {
        if lower.starts_with(prefix) {
            return title.into();
        }
    }
    let original = words(name);
    for n in 1..=original.len() {
        let prefix = original[..n].join(" ").to_lowercase();
        if original[..n].iter().all(|word| {
            [
                "black", "blue", "dark", "dawn", "day", "deep", "green", "light", "night", "red",
                "space", "white",
            ]
            .contains(&word.to_lowercase().as_str())
        }) {
            continue;
        }
        let counts = PREFIXES.get_or_init(|| {
            let mut counts = std::collections::HashMap::new();
            for scheme in SCHEMES {
                let words = words(scheme.name);
                for count in 1..=words.len() {
                    *counts
                        .entry(words[..count].join(" ").to_lowercase())
                        .or_insert(0) += 1;
                }
            }
            counts
        });
        if counts.get(&prefix).copied().unwrap_or(0) >= 2 {
            return original[..n].join(" ");
        }
    }
    name.into()
}
