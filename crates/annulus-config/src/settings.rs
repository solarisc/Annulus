//! Device settings (F-SET-001): what this computer remembers between launches, kept in a
//! small JSON file next to the vault. Nothing secret lives here, and none of it syncs.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tokens::ThemeMode;

/// The default terminal font size, in points.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// This device's settings. Unknown or missing fields take their defaults, so older and
/// newer files both load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Light, dark or follow the system.
    pub theme: ThemeMode,
    /// Terminal font size in points.
    pub font_size: f32,
    /// Lock the vault after this many idle minutes; 0 never locks.
    pub auto_lock_minutes: u32,
    /// The shell profile new local tabs open, by name; `None` uses the first detected.
    pub default_profile: Option<String>,
    /// The terminal color theme, by name.
    pub terminal_theme: String,
    /// Save each finished session's text as a log (F-LOG-001). Off by default, since output
    /// can contain secrets.
    pub record_logs: bool,
    /// Reopen the terminal tabs that were open when the app closed (F-APP-008). Off by
    /// default.
    pub restore_tabs: bool,
    /// The terminal tabs open at the last save, kept while `restore_tabs` is on.
    pub open_tabs: Vec<OpenTab>,
    /// No animations: everything changes at once (UI-SPEC §8).
    pub reduce_motion: bool,
    /// Saved hosts connected to from this device, the latest first, for the New Tab page
    /// (F-APP-004). By vault id and time only: this file is not encrypted, so no name or
    /// address is ever written here.
    pub recent: Vec<Recent>,
    /// Shortcuts given other keys in Settings (F-SET-004); the others keep their own.
    pub keys: Vec<KeyBinding>,
}

/// A shortcut with keys of the user's choosing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// Which shortcut, by its name in the app's table (`NEW_TAB`).
    pub action: String,
    /// The keys, as Settings shows them ("Ctrl+Shift+T").
    pub keys: String,
}

/// One connection to a saved host, for the New Tab page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    /// The host's id in the vault.
    pub host: u64,
    /// When, in seconds since the Unix epoch.
    pub at: u64,
}

/// How many recent connections are kept.
pub const RECENT_KEPT: usize = 20;

/// A terminal tab to reopen: what it ran, never what it showed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenTab {
    /// A local shell, by profile name (empty: the default shell).
    Local(String),
    /// A saved host, by its id in the vault (reopened once the vault is unlocked).
    Host(u64),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            font_size: DEFAULT_FONT_SIZE,
            auto_lock_minutes: 15,
            default_profile: None,
            terminal_theme: "Annulus Dark".into(),
            record_logs: false,
            restore_tabs: false,
            open_tabs: Vec::new(),
            reduce_motion: false,
            recent: Vec::new(),
            keys: Vec::new(),
        }
    }
}

impl Settings {
    /// Notes a connection to host `host` at `at`: it goes to the front of the recent ones, once,
    /// and the oldest beyond [`RECENT_KEPT`] are forgotten.
    pub fn connected(&mut self, host: u64, at: u64) {
        self.recent.retain(|recent| recent.host != host);
        self.recent.insert(0, Recent { host, at });
        self.recent.truncate(RECENT_KEPT);
    }

    /// The settings stored at `path`, or the defaults when there is no readable file.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the settings to `path` atomically.
    ///
    /// # Errors
    /// The file system refused.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write as _;
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        // On disk before it takes the name, so a power cut leaves the old file, not an empty one.
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_bad_files_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("annulus-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let settings = Settings {
            theme: ThemeMode::Dark,
            font_size: 16.0,
            auto_lock_minutes: 5,
            default_profile: Some("PowerShell 7".into()),
            terminal_theme: "Solarized Dark".into(),
            record_logs: true,
            restore_tabs: true,
            open_tabs: vec![OpenTab::Local("bash".into()), OpenTab::Host(7)],
            reduce_motion: true,
            recent: vec![Recent {
                host: 7,
                at: 1_800_000_000,
            }],
            keys: vec![KeyBinding {
                action: "NEW_TAB".into(),
                keys: "Ctrl+Alt+N".into(),
            }],
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);

        // Fields from a newer version are ignored; missing ones take defaults.
        std::fs::write(&path, br#"{"font_size": 12.0, "future": true}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!((loaded.font_size, loaded.auto_lock_minutes), (12.0, 15));

        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn a_connection_goes_to_the_front_of_the_recent_ones_once_and_old_ones_drop_off() {
        let mut settings = Settings::default();
        for host in 1..=25 {
            settings.connected(host, 1000 + host);
        }
        assert_eq!(settings.recent.len(), RECENT_KEPT);
        assert_eq!(settings.recent[0], Recent { host: 25, at: 1025 });
        // Again to one already there: it moves to the front with the new time, not twice.
        settings.connected(10, 2000);
        assert_eq!(settings.recent[0], Recent { host: 10, at: 2000 });
        assert_eq!(
            settings
                .recent
                .iter()
                .filter(|recent| recent.host == 10)
                .count(),
            1
        );
        assert_eq!(settings.recent.len(), RECENT_KEPT);
    }
}
