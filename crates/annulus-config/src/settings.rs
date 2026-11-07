//! Device settings (F-SET-001): what this computer remembers between launches, kept in a
//! small JSON file next to the vault. Nothing secret lives here, and none of it syncs.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tokens::ThemeMode;

/// The default terminal font size, in points.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Lines of scrollback each terminal keeps unless the user picks another number (F-TERM-004).
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// This device's settings. Unknown or missing fields take their defaults, so older and
/// newer files both load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
// Each is a switch of its own in Settings; no two of them make one state.
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    /// Light, dark or follow the system.
    pub theme: ThemeMode,
    /// A color scheme for the whole window, by name ([`crate::schemes`]); `None` draws our
    /// own light or dark theme as `theme` says. A scheme is light or dark by itself.
    pub app_scheme: Option<String>,
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
    /// Saved hosts pinned to the top of the New Tab page (F-APP-004), in the order pinned.
    /// By vault id only, as `recent`.
    pub pinned: Vec<u64>,
    /// Shortcuts given other keys in Settings (F-SET-004); the others keep their own.
    pub keys: Vec<KeyBinding>,
    /// Let the `annulus` command and AI agents drive the app while the vault is unlocked
    /// (F-AUTO-001). Off by default.
    pub automation: bool,
    /// Lines of scrollback each terminal keeps (F-TERM-004).
    pub scrollback: usize,
    /// A tab an agent opens comes to the front; off, it opens behind the one in use
    /// (F-AUTO-006).
    pub agent_tabs_in_front: bool,
    /// Agents may read the user's own terminal tabs; off, only the tabs they opened.
    pub agents_read_my_tabs: bool,
    /// An agent's tab closes once its shell or connection has ended; off, it stays with
    /// Start again.
    pub close_ended_agent_tabs: bool,
    /// The panel beside a new agent tab starts folded to its rail.
    pub agent_panel_folded: bool,
    /// Agents may not close tabs, not even the ones they opened, so what they did stays on
    /// screen for the user; off, an agent may close its own tabs. On by default.
    pub stop_agents_closing_tabs: bool,
    /// A click on a saved host in the Vault connects to it, as on the New Tab page; off, a
    /// click selects it and a double-click connects. Off by default.
    pub connect_on_click: bool,
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
            app_scheme: None,
            font_size: DEFAULT_FONT_SIZE,
            auto_lock_minutes: 15,
            default_profile: None,
            terminal_theme: "Annulus Dark".into(),
            record_logs: false,
            restore_tabs: false,
            open_tabs: Vec::new(),
            reduce_motion: false,
            recent: Vec::new(),
            pinned: Vec::new(),
            keys: Vec::new(),
            automation: false,
            scrollback: DEFAULT_SCROLLBACK,
            agent_tabs_in_front: true,
            agents_read_my_tabs: true,
            close_ended_agent_tabs: false,
            agent_panel_folded: false,
            stop_agents_closing_tabs: true,
            connect_on_click: false,
        }
    }
}

impl Settings {
    /// Whether an agent's tab closes once its shell has ended. Never while agents may not
    /// close tabs: an agent could end its shell with `exit` to get round that.
    #[must_use]
    pub fn closes_ended_agent_tabs(&self) -> bool {
        self.close_ended_agent_tabs && !self.stop_agents_closing_tabs
    }

    /// Pins host `host` to the top of the New Tab page, last among the pinned, or unpins it.
    pub fn toggle_pin(&mut self, host: u64) {
        if let Some(at) = self.pinned.iter().position(|pinned| *pinned == host) {
            self.pinned.remove(at);
        } else {
            self.pinned.push(host);
        }
    }

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
    fn an_ended_agents_tab_closes_only_while_agents_may_close_tabs() {
        let mut settings = Settings {
            close_ended_agent_tabs: true,
            ..Settings::default()
        };
        assert!(
            !settings.closes_ended_agent_tabs(),
            "the guard keeps it, even after exit"
        );
        settings.stop_agents_closing_tabs = false;
        assert!(settings.closes_ended_agent_tabs());
        settings.close_ended_agent_tabs = false;
        assert!(!settings.closes_ended_agent_tabs());
    }

    #[test]
    fn settings_round_trip_and_bad_files_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("annulus-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let settings = Settings {
            theme: ThemeMode::Dark,
            app_scheme: Some("Gruvbox Dark".into()),
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
            pinned: vec![7, 3],
            keys: vec![KeyBinding {
                action: "NEW_TAB".into(),
                keys: "Ctrl+Alt+N".into(),
            }],
            automation: true,
            scrollback: 50_000,
            agent_tabs_in_front: false,
            agents_read_my_tabs: false,
            close_ended_agent_tabs: true,
            agent_panel_folded: true,
            stop_agents_closing_tabs: false,
            connect_on_click: true,
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);

        // Fields from a newer version are ignored; missing ones take defaults.
        std::fs::write(&path, br#"{"font_size": 12.0, "future": true}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!((loaded.font_size, loaded.auto_lock_minutes), (12.0, 15));
        // A file from before the scrollback and agent settings keeps today's behavior, and
        // one from before the close guard gets it switched on.
        assert_eq!(
            (
                loaded.scrollback,
                loaded.agent_tabs_in_front,
                loaded.agents_read_my_tabs,
                loaded.close_ended_agent_tabs,
                loaded.agent_panel_folded,
                loaded.stop_agents_closing_tabs,
                loaded.connect_on_click
            ),
            (DEFAULT_SCROLLBACK, true, true, false, false, true, false)
        );
        // A file from before color schemes keeps its theme and draws our own colors.
        std::fs::write(&path, br#"{"theme": "light", "terminal_theme": "Nord"}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(
            (
                loaded.theme,
                loaded.app_scheme,
                loaded.terminal_theme.as_str()
            ),
            (ThemeMode::Light, None, "Nord")
        );

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
