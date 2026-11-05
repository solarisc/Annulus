//! The window drawn by our own toolkit ([ADR-0017]): a title bar that holds the tabs and the
//! window buttons, the vault's navigation, and the screen of the chosen section.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

mod account;
mod ask;
mod colors;
pub mod control;
pub mod demo;
mod forward_wizard;
pub mod hosts;
mod live;
pub mod lock;
mod os;
mod pages;
mod palette;
mod panes;
#[cfg(test)]
mod scenes;
mod screens;
mod settings;
mod sftp;
mod shortcuts;
mod start;
mod terminal;
mod titlebar;

use annulus_config::settings::{OpenTab, Settings};
use annulus_config::tokens::{Appearance, motion, radius, space};
use annulus_platform::window::{MenuItem, Waker, WindowApp, WindowError, WindowRequest, run};
use annulus_session::profiles::{self, Profile};
use annulus_session::{Program, SshFiles};
use annulus_term::Emulator;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Font, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Theme, Ui};
use annulus_ui::motion::{Curve, Feel, Layer, Motion};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, Kind, Tone};
use annulus_vault::vault::Vault;

use crate::connect;
use crate::terminal_view::TabStatus;

/// Height of the title bar, in DIPs.
pub const TITLE_H: f32 = 44.0;
/// Width of the vault navigation.
pub const NAV_W: f32 = 216.0;
/// Width of the navigation collapsed to icons, below [`COLLAPSE_BELOW`] window width.
pub const NAV_COMPACT_W: f32 = 64.0;
/// The window width under which the navigation shows icons only (UI-SPEC §2).
pub const COLLAPSE_BELOW: f32 = 900.0;

/// Where a host just connected to is on screen, its label and its OS.
type Launch = (Rect, String, Option<&'static os::Os>);

/// A host just connected to, flying from its card to its tab.
struct Flyer {
    /// The tab it goes to.
    uid: u64,
    /// The card it left, on screen.
    from: Rect,
    title: String,
    os: Option<&'static os::Os>,
    /// When it left.
    since: f64,
}

/// A section of the vault's left navigation (F-APP-003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Hosts and groups.
    Hosts,
    /// Keys, identities and certificates.
    Keychain,
    /// Port forwarding rules.
    PortForwarding,
    /// Saved commands.
    Snippets,
    /// Trusted host keys.
    KnownHosts,
    /// Session logs.
    Logs,
    /// App settings, pinned to the bottom.
    Settings,
}

impl Section {
    /// The sections listed at the top of the navigation, in order.
    pub const MAIN: [Section; 6] = [
        Section::Hosts,
        Section::Keychain,
        Section::PortForwarding,
        Section::Snippets,
        Section::KnownHosts,
        Section::Logs,
    ];

    /// The navigation label.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Section::Hosts => "Hosts",
            Section::Keychain => "Keychain",
            Section::PortForwarding => "Port Forwarding",
            Section::Snippets => "Snippets",
            Section::KnownHosts => "Known Hosts",
            Section::Logs => "Logs",
            Section::Settings => "Settings",
        }
    }

    fn icon(self) -> IconId {
        match self {
            Section::Hosts => icons::SERVER,
            Section::Keychain => icons::KEY,
            Section::PortForwarding => icons::FORWARD,
            Section::Snippets => icons::CODE,
            Section::KnownHosts => icons::SHIELD,
            Section::Logs => icons::LOGS,
            Section::Settings => icons::SETTINGS,
        }
    }
}

/// What a tab holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// The vault; always first, never closed.
    Vault,
    /// The SFTP tab beside the vault (F-APP-001): always there, never closed; it picks a host,
    /// then shows that host's files.
    Files,
    /// A terminal, local or SSH.
    Terminal,
    /// A host's files.
    Sftp,
    /// A new tab, picking what to open (F-APP-004).
    NewTab,
}

/// How many tabs are fixed at the start of the strip: the vault and SFTP.
pub const FIXED_TABS: usize = 2;

/// One tab of the title bar.
pub struct Tab {
    /// Stable for the tab's life: its zones keep their ids when other tabs close.
    pub uid: u64,
    /// What it holds.
    pub kind: TabKind,
    /// Its label.
    pub title: String,
    /// Its status dot.
    pub status: Option<TabStatus>,
    /// The terminal of a terminal tab.
    pub terminal: Option<TermView>,
    /// The files of an SFTP tab, or of the host the SFTP tab picked.
    pub sftp: Option<Box<sftp::SftpView>>,
    /// The picker of a new tab or of the SFTP tab.
    pub start: Option<Box<start::StartView>>,
    /// What to reopen at the next start (F-APP-008).
    pub origin: Option<OpenTab>,
    /// What an agent typed here, when an agent opened it (ADR-0020); in memory only.
    pub agent: Option<Box<control::AgentLog>>,
}

impl Tab {
    /// A tab holding nothing yet.
    fn new(uid: u64, kind: TabKind, title: impl Into<String>) -> Self {
        Self {
            uid,
            kind,
            title: title.into(),
            status: None,
            terminal: None,
            sftp: None,
            start: None,
            origin: None,
            agent: None,
        }
    }

    /// The vault and SFTP tabs: always there, first, and never closed or moved.
    #[must_use]
    pub fn fixed(&self) -> bool {
        matches!(self.kind, TabKind::Vault | TabKind::Files)
    }
}

/// What a terminal tab shows.
pub enum TermView {
    /// Recorded output (the scenes and the demo).
    Replay(Box<Emulator>),
    /// A running local shell or SSH session.
    Live(Box<panes::Panes>),
}

/// Something only the window can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// Minimize the window.
    Minimize,
    /// Maximize it, or restore it when maximized.
    ToggleMaximize,
    /// Close it.
    Close,
}

/// The sync state shown in the title bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncBadge {
    /// No account: the vault lives on this device only.
    Local,
    /// Signed in and up to date.
    Synced,
    /// Changes are waiting to be sent.
    Pending(usize),
    /// The last round failed; it retries.
    Failed,
}

/// Who draws the window buttons (minimize, maximize, close).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButtons {
    /// The title bar, at its right end (Windows, Linux).
    Ours,
    /// The OS, at the title bar's left end, which the bar keeps free (macOS).
    Os,
}

/// The whole window's state.
pub struct NativeApp {
    /// The tab that was in front last frame and its place, to tell when another comes in front.
    front: (Option<u64>, usize),
    /// A tab just came in front: when, and from which side.
    switched: Option<(f64, f32)>,
    /// When the vault opened, while the lock screen is still leaving.
    lock_left: Option<f64>,
    /// The section drawn last frame.
    section_drawn: Section,
    /// The section just left and when, while it fades out.
    section_left: Option<(Section, f64)>,
    /// Where each tab on the strip is, by uid.
    tab_places: Vec<(u64, Rect)>,
    /// A card just connected to: where it is on screen, its host's label and OS, until its
    /// tab opens.
    launched: Option<Launch>,
    /// A host on its way from its card to its tab.
    flyer: Option<Flyer>,
    /// Open tabs; the first is always the vault.
    pub tabs: Vec<Tab>,
    /// The tab in front.
    pub active: usize,
    /// The vault section shown.
    pub section: Section,
    /// The unlocked vault.
    pub vault: Option<Vault>,
    /// The Hosts screen.
    pub hosts: hosts::HostsView,
    /// The window is maximized (the maximize button shows "restore").
    pub maximized: bool,
    /// Who draws the window buttons.
    pub window_buttons: WindowButtons,
    /// The lock screen, shown while there is no unlocked vault.
    pub lock: lock::LockView,
    /// Where the vault file lives; `None` for the preview's throwaway vault.
    pub vault_path: Option<std::path::PathBuf>,
    /// Where SSH keeps its known hosts and default keys.
    pub ssh_files: SshFiles,
    /// Repaints the window from a session's thread.
    pub waker: Option<Waker>,
    /// Terminal tabs to open after this frame: title, program and where it came from.
    opens: Vec<(String, Program, Option<OpenTab>)>,
    /// One tab of panes to open after this frame (a snippet run on several hosts).
    open_split: Option<(String, Vec<(String, Program)>)>,
    /// Another Annulus window has this data folder open: the vault stays closed here, since
    /// two windows saving one vault would lose each other's changes.
    pub blocked: Option<String>,
    /// The lock on the data folder, held while the window is open.
    instance: Option<std::fs::File>,
    /// Host tabs to reopen once the vault is unlocked.
    pending_hosts: Vec<u64>,
    /// Logs of sessions that ended while the vault was locked, saved once it opens.
    waiting_logs: Vec<(String, String)>,
    /// The snippet panel beside terminals (Ctrl+Shift+.), and its search.
    snippet_panel: Option<String>,
    /// The device settings, and where they are saved (`None` in the preview).
    pub settings: Settings,
    settings_path: Option<std::path::PathBuf>,
    /// The local shells found at start.
    profiles: Vec<Profile>,
    keychain: screens::KeychainView,
    snippets: screens::SnippetsView,
    forwards: screens::ForwardsView,
    known: screens::KnownView,
    logs: screens::LogsView,
    /// The command palette, while open (Ctrl+Shift+K).
    palette: Option<palette::PaletteView>,
    /// Account sync, while the vault is unlocked.
    sync: crate::sync_screen::SyncScreen,
    account: account::AccountView,
    account_h: f32,
    /// When input last arrived, on the window's clock (auto-lock).
    last_input: Option<f64>,
    next_uid: u64,
    /// The new tab (by uid) that the tab opened next takes the place of: what a New Tab page
    /// chose replaces it.
    replacing: Option<u64>,
    /// The tab being dragged along the strip (by uid), and where the pointer holds it from
    /// its left edge.
    tab_grab: Option<(u64, f32)>,
    /// Settings' own navigation and search.
    pub settings_view: settings::SettingsView,
    /// The OS was asked for fewer animations; read when the window opens.
    os_reduces_motion: bool,
    /// The window's colors on their way to new ones.
    fade: colors::ThemeFade,
    /// The control door, while automation is allowed and the vault unlocked (ADR-0020).
    door: Option<annulus_control::door::Door>,
    /// Opening the door failed; it is not tried again until the switch or the vault changes.
    door_failed: bool,
    /// Agents' commands waiting to be typed or for their output to settle.
    runs: Vec<control::Running>,
    /// What agents changed, to show as toasts this frame (a host's notes).
    agent_news: Vec<String>,
}

impl NativeApp {
    /// A window showing the Hosts screen of `vault`.
    #[must_use]
    pub fn new(vault: Option<Vault>) -> Self {
        Self {
            front: (None, 0),
            switched: None,
            lock_left: None,
            section_drawn: Section::Hosts,
            section_left: None,
            tab_places: Vec::new(),
            launched: None,
            flyer: None,
            tabs: vec![
                Tab::new(0, TabKind::Vault, "Vault"),
                Tab {
                    start: Some(Box::default()),
                    ..Tab::new(1, TabKind::Files, "SFTP")
                },
            ],
            active: 0,
            section: Section::Hosts,
            vault,
            hosts: hosts::HostsView::default(),
            maximized: false,
            window_buttons: if annulus_platform::window::OS_WINDOW_BUTTONS {
                WindowButtons::Os
            } else {
                WindowButtons::Ours
            },
            lock: lock::LockView::new(lock::Mode::Unlock),
            vault_path: None,
            ssh_files: SshFiles {
                known_hosts: std::env::temp_dir().join("annulus-preview-known_hosts"),
                key_dir: directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".ssh")),
            },
            waker: None,
            opens: Vec::new(),
            open_split: None,
            blocked: None,
            instance: None,
            pending_hosts: Vec::new(),
            waiting_logs: Vec::new(),
            snippet_panel: None,
            settings: Settings::default(),
            settings_path: None,
            profiles: profiles::detect(),
            keychain: screens::KeychainView::default(),
            snippets: screens::SnippetsView::default(),
            forwards: screens::ForwardsView::default(),
            known: screens::KnownView::default(),
            logs: screens::LogsView::default(),
            palette: None,
            sync: crate::sync_screen::SyncScreen::default(),
            account: account::AccountView::default(),
            account_h: 0.0,
            last_input: None,
            next_uid: FIXED_TABS as u64,
            replacing: None,
            tab_grab: None,
            settings_view: settings::SettingsView::default(),
            os_reduces_motion: false,
            fade: colors::ThemeFade::default(),
            door: None,
            door_failed: false,
            runs: Vec::new(),
            agent_news: Vec::new(),
        }
    }

    /// Closes every session tab but tab `uid`, or (`right_only`) only those to its right; `uid`
    /// is then in front.
    pub fn close_others(&mut self, uid: u64, right_only: bool) {
        let Some(at) = self.tabs.iter().position(|tab| tab.uid == uid) else {
            return;
        };
        let from = if right_only { at + 1 } else { FIXED_TABS };
        for index in (from..self.tabs.len()).rev() {
            if self.tabs[index].uid != uid {
                self.close_tab(index);
            }
        }
        if let Some(at) = self.tabs.iter().position(|tab| tab.uid == uid) {
            self.active = at;
        }
    }

    /// Moves tab `from` to place `to`, the tab in front staying in front; fixed tabs never move
    /// and nothing moves among them.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let count = self.tabs.len();
        if from == to || from < FIXED_TABS || to < FIXED_TABS || from >= count || to >= count {
            return;
        }
        let front = self.tabs.get(self.active).map(|tab| tab.uid);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(at) = front.and_then(|uid| self.tabs.iter().position(|tab| tab.uid == uid)) {
            self.active = at;
        }
    }

    /// Puts `tab` in front: in the place of the new tab it was chosen from, or last.
    fn place(&mut self, tab: Tab) {
        let at = self
            .replacing
            .take()
            .and_then(|uid| self.tabs.iter().position(|tab| tab.uid == uid));
        if let Some(at) = at {
            self.tabs[at] = tab;
            self.active = at;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
    }

    /// A uid for a new tab.
    fn take_uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid - 1
    }

    /// Adds a terminal tab showing `view`, in front.
    pub fn push_terminal(&mut self, title: impl Into<String>, view: TermView) {
        let uid = self.take_uid();
        self.place(Tab {
            terminal: Some(view),
            ..Tab::new(uid, TabKind::Terminal, title)
        });
    }

    /// Adds an SFTP tab in front.
    pub fn push_sftp(&mut self, tab: crate::sftp_view::SftpTab) {
        let uid = self.take_uid();
        let title = tab.title.clone();
        self.place(Tab {
            sftp: Some(Box::new(sftp::SftpView::new(tab))),
            ..Tab::new(uid, TabKind::Sftp, title)
        });
    }

    /// Opens a New Tab page in front, its search taking the keyboard (+, Ctrl+Shift+T).
    pub fn open_new_tab(&mut self) {
        let uid = self.take_uid();
        self.tabs.push(Tab {
            start: Some(Box::default()),
            ..Tab::new(uid, TabKind::NewTab, "New Tab")
        });
        self.active = self.tabs.len() - 1;
    }

    /// Opens a live terminal tab running `program` now, in front (last, or where the new tab
    /// it was chosen from was); returns its uid.
    fn open_terminal(&mut self, title: String, program: Program, origin: Option<OpenTab>) -> u64 {
        let mut terminal = live::LiveTerminal::new(program);
        // A saved host's tab connects under its name and its server's mark.
        if let (Some(OpenTab::Host(id)), Some(vault)) = (&origin, &self.vault)
            && let Some(host) = vault.hosts().iter().find(|host| host.id == *id)
        {
            terminal.identity = Some((host.label.clone(), os::of(host)));
        }
        self.push_terminal(title, TermView::Live(Box::new(panes::Panes::new(terminal))));
        let tab = &mut self.tabs[self.active];
        tab.origin = origin;
        tab.uid
    }

    /// Opens a live terminal tab running `program` after this frame.
    pub fn open(&mut self, title: impl Into<String>, program: Program) {
        self.opens.push((title.into(), program, None));
    }

    /// Closes tab `index` (never a fixed one), saving its sessions' logs when logging is on.
    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.get(index).is_none_or(Tab::fixed) {
            return;
        }
        let tab = self.tabs.remove(index);
        if let Some(TermView::Live(panes)) = &tab.terminal {
            self.save_logs(panes.transcripts());
        }
        let was_front = self.active == index;
        if self.active >= index {
            self.active = self.active.saturating_sub(1);
        }
        // The last session closed: back to the vault rather than onto the SFTP picker.
        if was_front && self.active < FIXED_TABS {
            self.active = 0;
        }
    }

    /// Keeps finished sessions' text in the vault, encrypted (F-LOG-001).
    fn save_logs(&mut self, logs: Vec<(String, String)>) {
        if !self.settings.record_logs {
            return;
        }
        let Some(vault) = self.vault.as_mut() else {
            // Locked: kept until the vault opens (UI review #5).
            self.waiting_logs.extend(logs);
            return;
        };
        let now = crate::sync_screen::now_secs();
        for (title, text) in logs {
            if !text.trim().is_empty() {
                let _ = vault.add_log(&title, &text, now);
            }
        }
    }

    /// Reopens the tabs of the last run (Settings, Reopen my tabs): local shells now, saved
    /// hosts once the vault is unlocked.
    pub fn reopen(&mut self, tabs: &[OpenTab]) {
        for tab in tabs {
            match tab {
                OpenTab::Local(name) => self.open_profile(Some(name.as_str())),
                OpenTab::Host(id) => self.pending_hosts.push(*id),
            }
        }
    }

    /// Reopens the last run's tabs when Settings ask for it (F-APP-008).
    pub fn restore(&mut self) {
        if self.settings.restore_tabs {
            let tabs = self.settings.open_tabs.clone();
            self.reopen(&tabs);
        }
    }

    /// The tabs to reopen next time, saved when they change.
    fn remember_tabs(&mut self) {
        if !self.settings.restore_tabs {
            return;
        }
        // Host tabs still waiting for the vault count too, or quitting at the lock screen
        // would forget them (UI review #8).
        let open: Vec<OpenTab> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.origin.clone())
            .chain(self.pending_hosts.iter().map(|id| OpenTab::Host(*id)))
            .collect();
        if open != self.settings.open_tabs {
            self.settings.open_tabs = open;
            if let Some(path) = &self.settings_path {
                let _ = self.settings.save(path);
            }
        }
    }

    /// Does what the picker of tab `uid` chose: a New Tab page turns into the session it chose;
    /// the SFTP tab shows the files of the host it chose.
    fn start_choice(&mut self, uid: u64, choice: start::Choice) {
        let Some(index) = self.tabs.iter().position(|tab| tab.uid == uid) else {
            return;
        };
        if self.tabs[index].kind == TabKind::Files {
            if let start::Choice::Host(id) = choice
                && let Some(files) = self.sftp_for(id)
            {
                self.tabs[index].sftp = Some(Box::new(sftp::SftpView::new(files)));
            }
            return;
        }
        match choice {
            start::Choice::Host(id) => {
                self.replacing = Some(uid);
                self.act(hosts::HostsAction::Connect(id));
            }
            start::Choice::Quick(target) => {
                self.replacing = Some(uid);
                self.act(hosts::HostsAction::QuickConnect(target));
            }
            start::Choice::Local => {
                self.replacing = Some(uid);
                self.open_local();
            }
            start::Choice::Sftp => {
                self.tabs.remove(index);
                self.active = 1;
            }
            start::Choice::NewHost => {
                self.tabs.remove(index);
                self.active = 0;
                self.section = Section::Hosts;
                if let Some(vault) = &self.vault {
                    self.hosts.new_host(vault);
                }
            }
            start::Choice::Import => {
                self.tabs.remove(index);
                self.active = 0;
                self.section = Section::Hosts;
                if let Some(vault) = &self.vault {
                    self.hosts.start_import(vault);
                }
            }
            start::Choice::TogglePin(id) => {
                self.settings.toggle_pin(id);
                if let Some(path) = &self.settings_path {
                    let _ = self.settings.save(path);
                }
            }
        }
    }

    /// The SFTP session for saved host `id`, or why there can be none.
    fn sftp_for(&mut self, id: u64) -> Option<crate::sftp_view::SftpTab> {
        let vault = self.vault.as_ref()?;
        let host = vault.hosts().iter().find(|host| host.id == id)?;
        let title = format!("{} files", host.label);
        self.sync.sync_soon();
        Some(match connect::login(vault, host) {
            Some((last, via)) => {
                crate::sftp_view::SftpTab::new(title, last, self.ssh_files.clone(), via)
            }
            None => crate::sftp_view::SftpTab::unavailable(
                title,
                "Annulus cannot reach this host itself: check its address and port.",
            ),
        })
    }

    /// Opens the default local shell (Settings, Local shells).
    pub fn open_local(&mut self) {
        let name = self.settings.default_profile.clone();
        self.open_profile(name.as_deref());
    }

    /// Opens local shell profile `name`, or the first one found.
    fn open_profile(&mut self, name: Option<&str>) {
        let profile = name
            .and_then(|name| self.profiles.iter().find(|profile| profile.name == name))
            .or_else(|| self.profiles.first());
        let (title, program) =
            profile.map_or(("Local".to_owned(), Program::DefaultShell), |profile| {
                (
                    profile.name.clone(),
                    Program::Command {
                        program: profile.program.clone(),
                        args: profile.args.clone(),
                    },
                )
            });
        self.opens
            .push((title.clone(), program, Some(OpenTab::Local(title))));
    }

    /// The tab, zoom and app shortcuts (UI-SPEC §7, [`shortcuts`]), taken before the screen in
    /// front sees the keys.
    fn app_shortcuts(&mut self, ui: &mut Ui<'_>) {
        let count = self.tabs.len();
        if shortcuts::NEXT_TAB.pressed(ui) && count > 0 {
            self.active = (self.active + 1) % count;
        }
        if shortcuts::PREVIOUS_TAB.pressed(ui) && count > 0 {
            self.active = (self.active + count - 1) % count;
        }
        // Alt+1 to Alt+9 (Cmd on a Mac): the tab at that place.
        for (place, digit) in ('1'..='9').enumerate() {
            if shortcuts::GO_TO_TAB.pressed_with(ui, Key::Char(digit)) && place < count {
                self.active = place;
            }
        }
        if shortcuts::NEW_TAB.pressed(ui) {
            self.open_new_tab();
        }
        if shortcuts::LOCAL.pressed(ui) {
            self.open_local();
        }
        // The tab in front, one place left or right (as dragging it would).
        if shortcuts::MOVE_LEFT.pressed(ui)
            && let Some(to) = self.active.checked_sub(1)
        {
            self.move_tab(self.active, to);
        }
        if shortcuts::MOVE_RIGHT.pressed(ui) {
            self.move_tab(self.active, self.active + 1);
        }
        if shortcuts::SETTINGS.pressed(ui) {
            self.show_settings();
        }
        if shortcuts::LOCK.pressed(ui) {
            self.lock_vault();
            return;
        }
        let size = self.settings.font_size;
        // Within the sizes Settings lists, so it always shows the size in use.
        let zoomed = if shortcuts::ZOOM_IN.pressed(ui) {
            (size + 1.0).min(20.0)
        } else if shortcuts::ZOOM_OUT.pressed(ui) {
            (size - 1.0).max(11.0)
        } else if shortcuts::ZOOM_RESET.pressed(ui) {
            annulus_config::settings::DEFAULT_FONT_SIZE
        } else {
            size
        };
        if (zoomed - size).abs() > f32::EPSILON {
            self.settings.font_size = zoomed;
            if let Some(path) = &self.settings_path {
                let _ = self.settings.save(path);
            }
        }
    }

    /// Split, close, broadcast, focus mode and moving between panes of the terminal tab in
    /// front (UI-SPEC §7). Taken before the terminal sees the keys.
    fn pane_shortcuts(&mut self, ui: &mut Ui<'_>) {
        let close = shortcuts::CLOSE.pressed(ui);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let uid = tab.uid;
        let Some(TermView::Live(panes)) = tab.terminal.as_mut() else {
            if close {
                let index = self.active;
                self.close_tab(index);
            }
            return;
        };
        let mut closed = Vec::new();
        if close && !panes.close_focused(&mut closed) {
            let index = self.active;
            self.close_tab(index);
            return;
        }
        if !closed.is_empty() {
            self.save_logs(closed);
            return;
        }
        if shortcuts::SPLIT_RIGHT.pressed(ui) {
            panes.split(true);
        }
        if shortcuts::SPLIT_DOWN.pressed(ui) {
            panes.split(false);
        }
        if shortcuts::BROADCAST.pressed(ui) {
            panes.toggle_broadcast();
        }
        if shortcuts::LEAVE_OUT.pressed(ui) {
            panes.toggle_left_out();
        }
        if shortcuts::FOCUS_MODE.pressed(ui) {
            panes.toggle_focus_mode();
        }
        if panes.len() > 1 {
            for (key, dx, dy) in [
                (Key::Left, -1.0, 0.0),
                (Key::Right, 1.0, 0.0),
                (Key::Up, 0.0, -1.0),
                (Key::Down, 0.0, 1.0),
            ] {
                if shortcuts::PANE_FOCUS.pressed_with(ui, key) {
                    panes.move_focus(dx, dy);
                    ui.state
                        .set_focus(Some(Id::new(("terminal", uid, panes.focused))), false);
                }
            }
        }
    }

    /// Everything the palette can find.
    fn palette_items(&self) -> Vec<crate::command_palette::Item> {
        use crate::command_palette::{Action, Item};
        let mut items: Vec<Item> = Vec::new();
        // The sessions; the SFTP tab comes as a command below.
        for (index, tab) in self.tabs.iter().enumerate().skip(FIXED_TABS) {
            items.push(Item {
                label: tab.title.clone(),
                kind: "Tab",
                action: Action::Tab(index),
            });
        }
        if let Some(vault) = &self.vault {
            for host in vault.hosts() {
                items.push(Item {
                    label: host.label.clone(),
                    kind: "Host",
                    action: Action::Connect(host.id),
                });
            }
            for snippet in vault.snippets() {
                items.push(Item {
                    label: snippet.label.clone(),
                    kind: "Snippet",
                    action: Action::Snippet(snippet.id),
                });
            }
        }
        for (label, action) in [
            ("New local terminal", Action::NewLocal),
            ("SFTP", Action::Tab(1)),
            ("Settings", Action::Settings),
            ("Lock the vault", Action::Lock),
        ] {
            items.push(Item {
                label: label.into(),
                kind: "Command",
                action,
            });
        }
        items
    }

    fn palette_frame(&mut self, ui: &mut Ui<'_>) {
        if self.palette.is_none() {
            return;
        }
        let items = self.palette_items();
        let Some(outcome) = self.palette.as_mut().and_then(|view| view.draw(ui, &items)) else {
            return;
        };
        self.palette = None;
        let palette::Outcome::Run(action) = outcome else {
            return;
        };
        self.run_action(action);
    }

    /// Does what the palette's chosen line says.
    fn run_action(&mut self, action: crate::command_palette::Action) {
        use crate::command_palette::Action;
        match action {
            Action::QuickConnect(target) => self.act(hosts::HostsAction::QuickConnect(target)),
            Action::Connect(id) => self.act(hosts::HostsAction::Connect(id)),
            Action::Tab(index) => self.active = index.min(self.tabs.len().saturating_sub(1)),
            Action::Snippet(id) => {
                if let Some(snippet) = self
                    .vault
                    .as_ref()
                    .and_then(|vault| vault.snippets().iter().find(|s| s.id == id))
                {
                    let program =
                        crate::snippets_screen::with_script(Program::DefaultShell, &snippet.script);
                    let title = snippet.label.clone();
                    self.open(title, program);
                }
            }
            Action::NewLocal => self.open_local(),
            Action::Lock => self.lock_vault(),
            Action::Settings => self.show_settings(),
        }
    }

    /// Settings, in the vault tab.
    fn show_settings(&mut self) {
        self.active = 0;
        self.section = Section::Settings;
    }

    /// An item chosen in the menu bar (macOS): it does what its shortcut does, and like the
    /// shortcuts nothing while the vault is locked.
    pub fn menu(&mut self, item: MenuItem) {
        if self.vault.is_none() {
            return;
        }
        match item {
            MenuItem::About => {
                self.show_settings();
                self.settings_view.page = settings::Page::About;
            }
            MenuItem::Settings => self.show_settings(),
            MenuItem::Lock => self.lock_vault(),
        }
    }

    /// The keys the menu bar shows beside `item`: its shortcut's, none for About, and none
    /// while Settings records keys, which must reach the recording rather than the menu.
    #[must_use]
    pub fn menu_keys(&self, item: MenuItem) -> Option<(Key, Modifiers)> {
        if self.settings_view.recording() {
            return None;
        }
        match item {
            MenuItem::About => None,
            MenuItem::Settings => shortcuts::SETTINGS.current(),
            MenuItem::Lock => shortcuts::LOCK.current(),
        }
    }

    /// What another screen asked for.
    fn screen_action(&mut self, action: screens::ScreenAction) {
        match action {
            screens::ScreenAction::Terminal(title, program) => self.open(title, *program),
            screens::ScreenAction::Terminals(title, programs) => {
                self.open_split = Some((title, programs));
            }
            screens::ScreenAction::Log(title, text) => {
                let mut emulator = Emulator::new(120, 40);
                emulator.advance(text.replace('\n', "\r\n").as_bytes());
                self.push_terminal(
                    format!("{title} (log)"),
                    TermView::Replay(Box::new(emulator)),
                );
            }
        }
    }

    /// What the Hosts screen asked for, as a tab to open.
    fn act(&mut self, action: hosts::HostsAction) {
        match action {
            hosts::HostsAction::Connect(id) => {
                let Some(vault) = &self.vault else { return };
                let Some(host) = vault.hosts().iter().find(|host| host.id == id) else {
                    return;
                };
                let program = connect::host(vault, &self.ssh_files, host);
                let title = host.label.clone();
                self.launched = self
                    .hosts
                    .launched
                    .take()
                    .map(|from| (from, title.clone(), os::of(host)));
                self.forwards
                    .host_opened(id, vault, &self.ssh_files, self.waker.as_ref());
                self.opens.push((title, program, Some(OpenTab::Host(id))));
                // Connecting: the account's latest comes in now, not at the next minute.
                self.sync.sync_soon();
                // For the New Tab page's recent connections.
                self.settings.connected(id, crate::sync_screen::now_secs());
                if let Some(path) = &self.settings_path {
                    let _ = self.settings.save(path);
                }
            }
            hosts::HostsAction::ConnectAll(uid) => {
                // The hosts directly in the group, not those of its subgroups.
                let ids: Vec<u64> = self
                    .vault
                    .iter()
                    .flat_map(Vault::hosts)
                    .filter(|host| host.group.as_deref() == Some(uid.as_str()))
                    .map(|host| host.id)
                    .collect();
                for id in ids {
                    self.act(hosts::HostsAction::Connect(id));
                }
            }
            hosts::HostsAction::QuickConnect(target) => {
                // A saved host's label connects to that host, with its details.
                let saved = self
                    .vault
                    .as_ref()
                    .and_then(|vault| connect::saved(vault, &target))
                    .map(|host| host.id);
                if let Some(id) = saved {
                    self.act(hosts::HostsAction::Connect(id));
                } else {
                    let program = connect::quick(&target, &self.ssh_files);
                    self.open(target, program);
                }
            }
            hosts::HostsAction::LocalTerminal => self.open_local(),
            hosts::HostsAction::Sftp(id) => {
                if let Some(tab) = self.sftp_for(id) {
                    self.push_sftp(tab);
                }
            }
            // The Hosts screen opens its import card itself.
            hosts::HostsAction::ImportSshConfig => {}
        }
    }

    /// Creates or unlocks the vault as the lock screen asked, on another thread (Argon2id is
    /// slow on purpose).
    fn open_vault(&mut self, ui: &Ui<'_>, action: lock::LockAction) {
        if let Some(why) = &self.blocked {
            self.lock.error = Some(why.clone());
            return;
        }
        let Some(path) = self.vault_path.clone() else {
            self.lock.error = Some("This preview has no vault file.".into());
            return;
        };
        let pace = FramePace::new(ui, self.waker.clone());
        self.lock.working = Some(crate::job::Job::start(&pace, move || match action {
            lock::LockAction::Create(password) => {
                Vault::create(path, &password, annulus_vault::crypto::KdfParams::DEFAULT)
                    .map(|(vault, key)| (vault, Some(key)))
            }
            lock::LockAction::Unlock { secret, recovery } => if recovery {
                Vault::recover(path, &secret)
            } else {
                Vault::unlock(path, &secret)
            }
            .map(|vault| (vault, None)),
            // Handled by the caller before any work starts.
            lock::LockAction::Confirmed => Err(annulus_vault::vault::VaultError::WrongPassword),
        }));
    }

    /// After an unlock, host tabs get their login back from the vault, for reconnecting.
    fn unlocked(&mut self) {
        let Some(vault) = &self.vault else { return };
        for tab in &mut self.tabs {
            if let (Some(OpenTab::Host(id)), Some(TermView::Live(panes))) =
                (&tab.origin, &mut tab.terminal)
                && let Some(host) = vault.hosts().iter().find(|host| host.id == *id)
            {
                panes.set_program(&connect::host(vault, &self.ssh_files, host));
            }
        }
        let waiting = std::mem::take(&mut self.waiting_logs);
        self.save_logs(waiting);
    }

    /// Takes the vault the lock screen's work opened, once it is done.
    fn poll_lock(&mut self, now: f64) {
        let Some(job) = &self.lock.working else {
            return;
        };
        let result = match job.poll() {
            crate::job::Poll::Running => return,
            crate::job::Poll::Done(result) => result,
            crate::job::Poll::Lost => Err(annulus_vault::vault::VaultError::WrongPassword),
        };
        self.lock.working = None;
        match result {
            // A new vault shows its recovery key first; it opens once that is confirmed.
            Ok((vault, Some(key))) => self.lock.show_recovery(vault, key),
            Ok((vault, None)) => {
                self.vault = Some(vault);
                self.lock_left = Some(now);
                self.unlocked();
            }
            Err(annulus_vault::vault::VaultError::WrongPassword) => {
                self.lock.refuse(if self.lock.recovery_mode() {
                    "That recovery key does not open this vault.".into()
                } else {
                    "Wrong password.".into()
                });
            }
            Err(error) => self
                .lock
                .refuse(format!("Could not open the vault: {error}")),
        }
    }

    /// Saves on each logged-in host what its server said it runs, for the host's icon
    /// (F-HOST-009), once it differs from what the host shows and was not picked by hand; and
    /// the user it logged in as when it names none, so other devices log in as them too.
    fn learn_server_os(&mut self) {
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        for tab in &self.tabs {
            let (Some(OpenTab::Host(id)), Some(TermView::Live(panes))) =
                (&tab.origin, &tab.terminal)
            else {
                continue;
            };
            let Some(server) = panes.server() else {
                continue;
            };
            let Some(host) = vault.hosts().iter().find(|host| host.id == *id) else {
                continue;
            };
            let mut learned = host.clone();
            if let Some(os) = os::learned(host, server) {
                os.clone_into(&mut learned.os);
            }
            crate::connect::pin_user(vault, &mut learned, &annulus_session::local_user());
            if learned != *host {
                let _ = vault.upsert_host(learned);
            }
        }
    }

    /// The sync badge for the vault's account state.
    fn sync_badge(&self) -> SyncBadge {
        let Some(state) = self.vault.as_ref().map(Vault::sync_state) else {
            return SyncBadge::Local;
        };
        match (state.account.is_some(), state.pending_count()) {
            (false, _) => SyncBadge::Local,
            (true, _) if self.sync.round_failed() => SyncBadge::Failed,
            (true, 0) => SyncBadge::Synced,
            (true, pending) => SyncBadge::Pending(pending),
        }
    }

    /// What runs every frame whatever is shown: sync rounds and auto-lock (F-SEC-004).
    fn background(&mut self, ui: &mut Ui<'_>) {
        let now = ui.time();
        if ui.had_input() || self.last_input.is_none() {
            self.last_input = Some(now);
        }
        // Tabs not on screen keep their dots current and reconnect dropped sessions; not while
        // the vault is locked, when their logins are gone.
        let active = if self.vault.is_none() {
            usize::MAX
        } else {
            self.active
        };
        let waker = self.waker.clone();
        let (scrollback, unlocked) = (self.settings.scrollback, self.vault.is_some());
        for tab in &mut self.tabs {
            if let Some(TermView::Live(panes)) = &mut tab.terminal {
                panes.keep_scrollback(scrollback);
            }
        }
        let next_try = self
            .tabs
            .iter_mut()
            .enumerate()
            .filter(|(index, _)| *index != active)
            .filter_map(|(_, tab)| match &mut tab.terminal {
                Some(TermView::Live(panes)) => {
                    // An agent's tab opened behind starts its shell unseen, so it can run.
                    if unlocked && tab.agent.is_some() {
                        panes.start_unseen(waker.as_ref(), now);
                    }
                    panes.poll(now, waker.as_ref())
                }
                _ => None,
            })
            .min_by(f64::total_cmp);
        if let Some(wait) = next_try {
            ui.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        self.forwards
            .auto_start(vault, &self.ssh_files, self.waker.as_ref());
        for id in std::mem::take(&mut self.pending_hosts) {
            self.act(hosts::HostsAction::Connect(id));
        }
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        if let Some(path) = self.vault_path.clone() {
            let pace = FramePace::new(ui, self.waker.clone());
            if self.sync.tick(&pace, vault, &path) {
                // Signing in renumbered hosts and rules: drafts and running rules go, and open
                // host tabs are no longer remembered by their old numbers.
                self.hosts.editor = None;
                self.pending_hosts.clear();
                for tab in &mut self.tabs {
                    if matches!(tab.origin, Some(OpenTab::Host(_))) {
                        tab.origin = None;
                    }
                }
                self.forwards = screens::ForwardsView::default();
            }
            pace.apply(ui);
        }
        let minutes = self.settings.auto_lock_minutes;
        if minutes > 0 {
            let timeout = f64::from(minutes) * 60.0;
            let idle = now - self.last_input.unwrap_or(now);
            if idle >= timeout {
                self.lock_vault();
            } else {
                ui.request_repaint_after(std::time::Duration::from_secs_f64(timeout - idle));
            }
        }
    }

    /// Locks the vault: its keys, decrypted entities, drafts and typed passwords go, and so do
    /// the secrets open tabs hold for reconnecting; the tabs stay out of sight until unlock.
    pub fn lock_vault(&mut self) {
        for tab in &mut self.tabs {
            if let Some(TermView::Live(panes)) = &mut tab.terminal {
                panes.forget_secrets();
            }
            if let Some(view) = &mut tab.sftp {
                view.forget_secrets();
            }
        }
        self.palette = None;
        self.vault = None;
        // Nothing drives the app while it is locked: the door closes now, not next frame, and
        // no waiting command is typed later.
        self.door = None;
        self.door_failed = false;
        self.cut_runs(std::time::Instant::now());
        self.lock = lock::LockView::new(lock::Mode::Unlock);
        self.hosts = hosts::HostsView::default();
        self.forwards = screens::ForwardsView::default();
        self.sync = crate::sync_screen::SyncScreen::default();
        self.account = account::AccountView::default();
        self.keychain = screens::KeychainView::default();
        self.snippets = screens::SnippetsView::default();
        self.active = 0;
    }

    /// Draws one frame. Returns what the window itself must do.
    #[allow(clippy::too_many_lines)] // The window top to bottom: title bar, then the tab in front.
    pub fn frame(&mut self, ui: &mut Ui<'_>) -> Option<WindowAction> {
        ui.state.reduce_motion = self.settings.reduce_motion || self.os_reduces_motion;
        // New colors (a scheme previewed or chosen, the OS turning dark) blend in, never flash.
        let (theme, fading) = self.fade.step(ui.theme, ui.time(), ui.state.reduce_motion);
        ui.theme = theme;
        if fading {
            ui.request_repaint();
        }
        // Settings → Appearance sets it again while its scheme menu is open and drawn.
        self.settings_view.previewing = None;
        let t = ui.theme;
        let mut area = ui.viewport;
        ui.gfx.fill_rect(area, 0.0, t.chrome);
        let bar = area.take_top(TITLE_H);
        // Locked: the lock screen is all there is (sessions keep running out of sight).
        let locked = self.vault.is_none();
        if locked {
            self.active = 0;
        }
        self.background(ui);
        if self.control() {
            // A command's output is settling: look again soon.
            ui.request_repaint_after(std::time::Duration::from_millis(100));
        }
        for news in std::mem::take(&mut self.agent_news) {
            ui.toast(news, Tone::Info);
        }
        shortcuts::choose(&self.settings.keys);
        // Keys being recorded in Settings reach no shortcut.
        let in_settings = !locked
            && self.section == Section::Settings
            && self.tabs.get(self.active).map(|tab| tab.kind) == Some(TabKind::Vault);
        self.settings_view.catch_keys(ui, in_settings);
        if !locked && shortcuts::PALETTE.pressed(ui) {
            self.palette = match self.palette.take() {
                Some(_) => None,
                None => Some(palette::PaletteView::default()),
            };
        }
        // The open palette has the keyboard: Escape closes it, not the editor under it.
        let for_palette = self.palette.as_ref().map(|_| ui.take_typed());
        let action = titlebar::draw(self, ui, bar);
        if !locked {
            self.app_shortcuts(ui);
            self.pane_shortcuts(ui);
        }
        let kind = self.tabs.get(self.active).map(|tab| tab.kind);
        let picking = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.start.is_some() && tab.sftp.is_none());
        let switching = self.tab_switch(ui);
        if let Some((shown, side)) = switching {
            // Nothing pokes out of the window as it slides, what it draws or what it takes.
            ui.push_clip(area);
            ui.push_layer(
                Layer::fade(shown)
                    .shifted(side * 10.0 * (1.0 - shown), 0.0)
                    .grouped(area),
            );
            ui.request_repaint();
        }
        if kind == Some(TabKind::Vault) {
            let compact = ui.viewport.w < COLLAPSE_BELOW;
            let nav = area.take_left(if compact { NAV_COMPACT_W } else { NAV_W });
            // Nothing there to go to while locked: it rests dim, and wakes as the vault opens.
            let awake = ui.animate(
                Id::new("nav.awake"),
                if locked { 0.38 } else { 1.0 },
                Motion::SLOW,
            );
            if locked {
                ui.push_inert();
            }
            ui.push_layer(Layer::fade(awake).grouped(nav));
            self.navigation(ui, nav, compact);
            ui.pop_layer();
            if locked {
                ui.pop_inert();
            }
            let content = panel(ui, area);
            if self.vault.is_none() {
                self.poll_lock(ui.time());
                match self.lock.draw(ui, content) {
                    Some(lock::LockAction::Confirmed) => {
                        self.vault = self.lock.take_vault();
                        self.lock_left = Some(ui.time());
                    }
                    Some(action) => self.open_vault(ui, action),
                    None => {}
                }
                ui.request_repaint_if(self.lock.working.is_some());
            } else {
                self.section_screen(ui, content);
                self.lock_leaving(ui, content);
            }
        } else if picking {
            area.take_left(PANEL_GAP);
            let content = panel(ui, area);
            let hosts = self.vault.as_ref().map_or(&[][..], Vault::hosts);
            let now = crate::sync_screen::now_secs();
            let tab = &mut self.tabs[self.active];
            let uid = tab.uid;
            let files = tab.kind == TabKind::Files;
            let chosen = tab.start.as_mut().and_then(|start| {
                let settings = &self.settings;
                start.draw(
                    ui,
                    content,
                    uid,
                    (hosts, &settings.recent, &settings.pinned, now),
                    files,
                )
            });
            if let Some(choice) = chosen {
                self.start_choice(uid, choice);
            }
        } else if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.sftp.is_some()
        {
            area.take_left(PANEL_GAP);
            let mut content = panel(ui, area);
            // The SFTP tab goes back to its host picker; another SFTP tab is closed instead.
            if tab.kind == TabKind::Files {
                let bar = content.take_top(44.0).inset(space::L, 6.0);
                let back = Rect::new(bar.x, bar.y, 92.0, bar.h);
                let id = Id::new("files.back");
                if ui
                    .button(id, back, "Hosts", Some(icons::CHEVRON_LEFT), Kind::Ghost)
                    .clicked
                {
                    tab.sftp = None;
                }
                ui.set_name(id, "Back to the hosts");
            }
            if let Some(view) = tab.sftp.as_mut() {
                view.draw(ui, content, self.waker.as_ref());
            }
        } else if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(view) = tab.terminal.as_mut()
        {
            let font = Font::mono(self.settings.font_size.clamp(8.0, 32.0));
            let palette = colors::terminal_palette(&self.settings.terminal_theme);
            let background = Color::rgba(
                palette.background.r,
                palette.background.g,
                palette.background.b,
                255,
            );
            let mut area = area;
            area.take_left(PANEL_GAP);
            if shortcuts::SNIPPETS.pressed(ui) {
                self.snippet_panel = match self.snippet_panel.take() {
                    Some(_) => None,
                    None => Some(String::new()),
                };
            }
            // Side panels are cards of their own beside the terminal's, a gap apart.
            let side = self
                .snippet_panel
                .is_some()
                .then(|| panel(ui, area.take_right(300.0 + PANEL_GAP)));
            // An agent's tab shows what the agent typed beside it (ADR-0020 §6, F-AUTO-004).
            let strip = tab
                .agent
                .as_deref()
                .map(|log| panel(ui, area.take_right(control::panel_width(log) + PANEL_GAP)));
            let content = panel_filled(ui, area, background);
            let allowed = self.settings.automation;
            let agent_action = match (strip, tab.agent.as_deref_mut()) {
                (Some(rect), Some(log)) => {
                    control::agent_panel(ui, rect, log, (&tab.title, tab.status, allowed))
                }
                _ => None,
            };
            if let (Some(log), TermView::Live(panes)) = (tab.agent.as_deref(), &mut *view) {
                panes.mark_focused(&log.marks());
                match agent_action {
                    Some(control::PanelAction::Show(line)) => panes.scroll_focused_to(line),
                    Some(control::PanelAction::Newest) => panes.scroll_focused_to(usize::MAX),
                    _ => {}
                }
            }
            // Stop closes the door now; the next frame marks what was running as cut off.
            let switch = match agent_action {
                Some(control::PanelAction::Stop) => Some(false),
                Some(control::PanelAction::Allow) => Some(true),
                _ => None,
            };
            if let Some(on) = switch {
                self.settings.automation = on;
                if !on {
                    self.door = None;
                }
                if let Some(path) = &self.settings_path {
                    let _ = self.settings.save(path);
                }
                let said = if on {
                    "Agents can use Annulus again"
                } else {
                    "Agents stopped: every agent is cut off"
                };
                ui.toast(said, Tone::Info);
            }
            if let (Some(side), Some(search), TermView::Live(panes)) =
                (side, self.snippet_panel.as_mut(), &*view)
            {
                let snippets = self
                    .vault
                    .as_ref()
                    .map(|vault| vault.snippets().to_vec())
                    .unwrap_or_default();
                if let Some(bytes) = screens::snippet_panel(ui, side, search, &snippets) {
                    panes.write_snippet(&bytes);
                }
            }
            match view {
                TermView::Replay(emulator) => {
                    let inner = content.inset(space::L, space::M);
                    let (columns, rows) = terminal::grid_size(ui, inner, font);
                    emulator.resize(columns, rows);
                    emulator.set_palette(&palette);
                    let screen = emulator.screen();
                    terminal::draw(ui, inner, &screen, font, true);
                }
                TermView::Live(panes) => {
                    let uid = tab.uid;
                    panes.draw(ui, content, (font, &palette), self.waker.as_ref(), |pane| {
                        Id::new(("terminal", uid, pane))
                    });
                    if let Some(title) = panes.title() {
                        title.clone_into(&mut tab.title);
                    }
                }
            }
        } else {
            // A terminal tab always holds a terminal; an empty panel is never more than a frame.
            panel(ui, area);
        }
        if switching.is_some() {
            ui.pop_layer();
            ui.pop_clip();
        }
        if let Some(typed) = for_palette {
            ui.give_back_typed(typed);
        }
        self.palette_frame(ui);
        self.flight(ui);
        ui.overlays();
        for tab in &mut self.tabs {
            if let Some(TermView::Live(panes)) = &tab.terminal {
                tab.status = panes.status();
            }
        }
        self.learn_server_os();
        for (title, program, origin) in std::mem::take(&mut self.opens) {
            self.open_terminal(title, program, origin);
            ui.request_repaint();
            // The host just connected to flies to the tab it opened.
            if let (Some((from, title, os)), Some(tab)) =
                (self.launched.take(), self.tabs.get(self.active))
            {
                self.flyer = Some(Flyer {
                    uid: tab.uid,
                    from,
                    title,
                    os,
                    since: ui.time(),
                });
            }
        }
        if let Some((title, programs)) = self.open_split.take()
            && let Some(panes) = panes::Panes::several(programs)
        {
            self.push_terminal(title, TermView::Live(Box::new(panes)));
            ui.request_repaint();
        }
        // A choice that opened nothing leaves its new tab as it was.
        self.replacing = None;
        self.remember_tabs();
        action
    }

    /// How far the tab that just came in front has arrived (0.0 to 1.0) and which side it
    /// comes from, while it is arriving.
    fn tab_switch(&mut self, ui: &Ui<'_>) -> Option<(f32, f32)> {
        let uid = self.tabs.get(self.active).map(|tab| tab.uid);
        if uid != self.front.0 {
            if self.front.0.is_some() && !ui.state.reduce_motion && self.vault.is_some() {
                let side = if self.active >= self.front.1 {
                    1.0
                } else {
                    -1.0
                };
                self.switched = Some((ui.time(), side));
            }
            self.front = (uid, self.active);
        }
        let (since, side) = self.switched?;
        #[allow(clippy::cast_possible_truncation)] // A share for drawing.
        let share = ((ui.time() - since) * 1000.0 / f64::from(motion::BASE)) as f32;
        if share >= 1.0 {
            self.switched = None;
            return None;
        }
        Some((Curve::Emphasized.at(share), side))
    }

    /// The lock screen on its way out, over the vault coming in.
    fn lock_leaving(&mut self, ui: &mut Ui<'_>, content: Rect) {
        /// How long the lock takes to open and leave, in seconds.
        const LEAVES: f64 = 0.28;
        let Some(since) = self.lock_left else { return };
        #[allow(clippy::cast_possible_truncation)] // A share for drawing.
        let share = ((ui.time() - since) / LEAVES) as f32;
        if ui.state.reduce_motion || share >= 1.0 {
            self.lock_left = None;
            return;
        }
        self.lock.draw_leaving(ui, content, share);
        ui.request_repaint();
    }

    /// The host just connected to, on its way from its card to its tab.
    fn flight(&mut self, ui: &mut Ui<'_>) {
        let Some(flyer) = &self.flyer else { return };
        let to = self
            .tab_places
            .iter()
            .find(|(uid, _)| *uid == flyer.uid)
            .map(|(_, place)| *place);
        #[allow(clippy::cast_possible_truncation)] // A share for drawing.
        let share = ((ui.time() - flyer.since) / FLIGHT) as f32;
        let (Some(to), false, true) = (to, ui.state.reduce_motion, share < 1.0) else {
            self.flyer = None;
            return;
        };
        let t = ui.theme;
        let eased = Curve::Emphasized.at(share);
        let along = |a: f32, b: f32| a + (b - a) * eased;
        // From the tile on its card to the icon on its tab, where they sit in each.
        let side = flyer.from.h.min(36.0);
        let from = Rect::new(
            flyer.from.x + space::M,
            flyer.from.center().y - side / 2.0,
            side,
            side,
        );
        let to = Rect::new(to.x + space::M, to.center().y - 8.0, 16.0, 16.0);
        let rect = Rect::new(
            along(from.x, to.x),
            along(from.y, to.y),
            along(from.w, to.w),
            along(from.h, to.h),
        );
        // It lands by melting into the tab.
        let shown = 1.0 - Curve::Exit.at(((share - LANDS_AT) / (1.0 - LANDS_AT)).clamp(0.0, 1.0));
        ui.push_layer(Layer::fade(shown).grouped(rect.outset(30.0)));
        ui.gfx
            .shadow(rect, rect.w * 0.26, 18.0, 6.0, t.shadow.alpha(0.8));
        os::tile(ui, rect, &flyer.title, flyer.os);
        ui.pop_layer();
        ui.request_repaint();
    }

    /// How much of tab `uid`'s own icon and name shows, 0.0 to 1.0: none while a host is flying
    /// to it, coming up as the host melts away.
    pub(crate) fn tab_landing(&self, uid: u64, now: f64) -> f32 {
        let Some(flyer) = self.flyer.as_ref().filter(|flyer| flyer.uid == uid) else {
            return 1.0;
        };
        #[allow(clippy::cast_possible_truncation)] // A share for drawing.
        let share = ((now - flyer.since) / FLIGHT) as f32;
        ((share - LANDS_AT) / (1.0 - LANDS_AT)).clamp(0.0, 1.0)
    }

    /// Whether tab `index` holds a session still connecting.
    pub(crate) fn tab_connecting(&self, index: usize) -> bool {
        matches!(
            self.tabs.get(index).and_then(|tab| tab.terminal.as_ref()),
            Some(TermView::Live(panes)) if panes.connecting()
        )
    }

    fn section_screen(&mut self, ui: &mut Ui<'_>, content: Rect) {
        // The section left behind fades out over the one chosen, which already works.
        if self.section != self.section_drawn {
            self.section_left = Some((self.section_drawn, ui.time()));
            self.section_drawn = self.section;
        }
        // Each section fades and rises a little into place when chosen, just after what was
        // there has mostly gone, so their words are never read over each other.
        let wait = if self.lock_left.is_some() {
            AFTER_THE_LOCK
        } else {
            motion::EXIT / 2
        };
        self.hosts.enter_after = wait;
        let shown = ui.entrance(
            Id::new(("section", self.section.title())),
            Motion::BASE.delayed(wait),
        );
        ui.push_clip(content);
        ui.push_layer(Layer::fade(shown).shifted(0.0, 10.0 * (1.0 - shown)));
        let action = self.draw_section(ui, content, self.section);
        ui.pop_layer();
        ui.pop_clip();
        if let Some(action) = action {
            self.screen_action(action);
        }
        if let Some((left, since)) = self.section_left {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let gone = ((ui.time() - since) * 1000.0 / f64::from(motion::EXIT)) as f32;
            if ui.state.reduce_motion || gone >= 1.0 {
                self.section_left = None;
            } else {
                ui.push_inert();
                ui.push_layer(Layer::fade(1.0 - Curve::Out.at(gone)));
                let _ = self.draw_section(ui, content, left);
                ui.pop_layer();
                ui.pop_inert();
                ui.request_repaint();
            }
        }
    }

    /// Draws `section` into `content`; returns what it asks the window to do.
    fn draw_section(
        &mut self,
        ui: &mut Ui<'_>,
        content: Rect,
        section: Section,
    ) -> Option<screens::ScreenAction> {
        let vault = self.vault.as_mut()?;
        match section {
            Section::Hosts => {
                if let Some(action) = self.hosts.draw(ui, content, Some(vault)) {
                    self.act(action);
                }
                None
            }
            Section::Keychain => {
                self.keychain
                    .draw(ui, content, vault, &self.ssh_files, self.waker.as_ref());
                None
            }
            Section::Snippets => self.snippets.draw(ui, content, vault, &self.ssh_files),
            Section::PortForwarding => {
                self.forwards
                    .draw(ui, content, vault, &self.ssh_files, self.waker.as_ref());
                None
            }
            Section::KnownHosts => {
                let openssh = directories::BaseDirs::new()
                    .map(|dirs| dirs.home_dir().join(".ssh").join("known_hosts"))
                    .filter(|path| path.exists());
                self.known
                    .draw(ui, content, &self.ssh_files.known_hosts, openssh.as_deref());
                None
            }
            Section::Logs => self
                .logs
                .draw(ui, content, vault, self.settings.record_logs),
            Section::Settings => {
                let pace = FramePace::new(ui, self.waker.clone());
                let (sync, view) = (&mut self.sync, &mut self.account);
                let path = self.vault_path.clone();
                let mut draw = |ui: &mut Ui<'_>, rect: Rect| {
                    path.as_ref()
                        .map_or(0.0, |path| view.draw(ui, rect, (sync, vault, &pace, path)))
                };
                let account: Option<settings::AccountCard<'_>> =
                    self.vault_path.is_some().then_some(&mut draw);
                let (changed, height) = self.settings_view.draw(
                    ui,
                    content,
                    (&mut self.settings, &self.profiles),
                    account,
                    self.account_h,
                );
                self.account_h = height;
                pace.apply(ui);
                if changed && let Some(path) = &self.settings_path {
                    let _ = self.settings.save(path);
                }
                None
            }
        }
    }

    fn navigation(&mut self, ui: &mut Ui<'_>, rect: Rect, compact: bool) {
        let mut column = rect.inset(space::S + 2.0, 0.0);
        column.take_top(space::S);
        let mut rows: Vec<(Section, Rect)> = Section::MAIN
            .iter()
            .map(|section| (*section, column.take_top(36.0).inset(0.0, 1.0)))
            .collect();
        let mut footer = column;
        let settings = footer.take_bottom(space::M + 36.0);
        rows.push((
            Section::Settings,
            Rect::new(settings.x, settings.y, settings.w, 36.0).inset(0.0, 1.0),
        ));
        // The selection: one pill that glides to the section chosen, even down to Settings.
        // Nothing is chosen while there is nothing to choose: the vault is locked.
        let chosen = rows
            .iter()
            .find(|(section, _)| *section == self.section)
            .filter(|_| self.vault.is_some());
        if let Some((_, target)) = chosen {
            let t = ui.theme;
            let y = ui.spring_with(Id::new("nav.pill"), target.y, Feel::Soft);
            let pill = Rect::new(target.x, y, target.w, target.h);
            ui.gfx.fill_rect(pill, 8.0, t.selected);
        }
        for (section, row) in rows {
            self.nav_item(ui, row, section, compact);
        }
    }

    fn nav_item(&mut self, ui: &mut Ui<'_>, rect: Rect, section: Section, compact: bool) {
        let id = Id::new(("nav", section.title()));
        let selected = self.section == section && self.vault.is_some();
        let response = ui.zone(id, rect, Role::Tab, section.title());
        if response.clicked {
            self.section = section;
        }
        let t = ui.theme;
        let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        if !selected {
            ui.gfx.fill_rect(rect, 8.0, t.hover.alpha(hover));
        }
        if response.focus_ring {
            ui.focus_ring(rect, 8.0);
        }
        let ink = if selected { t.text } else { t.text2 };
        if compact {
            ui.gfx.icon(section.icon(), rect.centered(18.0, 18.0), ink);
            if response.hovered {
                ui.set_cursor(Cursor::Pointer);
            }
            return;
        }
        let mut inner = rect.inset(space::M, 0.0);
        let icon = inner.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(section.icon(), icon, ink);
        inner.take_left(space::M - 2.0);
        let (font, color) = if selected {
            (BODY_MEDIUM, t.text)
        } else {
            (BODY, t.text.mix(t.text2, 0.35 * (1.0 - hover)))
        };
        ui.strict_text(inner, section.title(), font, color, Align::Start);
        if response.hovered {
            ui.set_cursor(Cursor::Pointer);
        }
    }
}

/// The window's clock and waker for background work, collecting the soonest frame it asks
/// for.
struct FramePace {
    now: f64,
    waker: Option<Waker>,
    later: std::cell::Cell<Option<std::time::Duration>>,
}

impl FramePace {
    fn new(ui: &Ui<'_>, waker: Option<Waker>) -> Self {
        Self {
            now: ui.time(),
            waker,
            later: std::cell::Cell::new(None),
        }
    }

    /// Passes the soonest frame asked for on to the window.
    fn apply(&self, ui: &mut Ui<'_>) {
        if let Some(after) = self.later.get() {
            ui.request_repaint_after(after);
        }
    }
}

impl crate::job::Pace for FramePace {
    fn now(&self) -> f64 {
        self.now
    }

    fn waker(&self) -> crate::job::Wake {
        self.waker
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(|| {}))
    }

    fn later(&self, after: std::time::Duration) {
        let soonest = self.later.get().map_or(after, |earlier| earlier.min(after));
        self.later.set(Some(soonest));
    }
}

/// The rounded content panel beside the navigation; returns its inside.
fn panel(ui: &mut Ui<'_>, rect: Rect) -> Rect {
    let shape = panel_shape(rect);
    ui.gfx.fill_rect(shape, PANEL_RADIUS, ui.theme.bg);
    shape
}

/// The content panel filled with `fill` (a terminal's own background), with a hairline: that
/// background can be anything, even the window's own color.
fn panel_filled(ui: &mut Ui<'_>, rect: Rect, fill: Color) -> Rect {
    let shape = panel_shape(rect);
    ui.gfx.fill_rect(shape, PANEL_RADIUS, fill);
    ui.gfx
        .stroke_rect(shape, PANEL_RADIUS, 1.0, ui.theme.line.alpha(0.7));
    shape
}

/// A rounded sheet on every side, clear of the window's right and bottom edges, so the window
/// reads as a frame holding a sheet rather than one flat box.
fn panel_shape(rect: Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        (rect.w - PANEL_GAP).max(0.0),
        (rect.h - PANEL_GAP).max(0.0),
    )
}

/// How long a host takes to fly from its card to its tab, in seconds.
const FLIGHT: f64 = 0.44;
/// How far through its flight, 0.0 to 1.0, a host starts to melt into its tab.
const LANDS_AT: f32 = 0.62;
/// How long the vault waits to come in while the lock leaves, in milliseconds.
const AFTER_THE_LOCK: u32 = 150;
/// The space between the content panel and the window's right and bottom edges.
const PANEL_GAP: f32 = 8.0;
/// The content panel's corner radius.
pub(crate) const PANEL_RADIUS: f32 = radius::SHEET;

impl Drop for NativeApp {
    /// The window closed: sessions still open keep their logs too, as when their tab closes
    /// (UI review #5).
    fn drop(&mut self) {
        let logs: Vec<(String, String)> = self
            .tabs
            .iter()
            .filter_map(|tab| match &tab.terminal {
                Some(TermView::Live(panes)) => Some(panes.transcripts()),
                _ => None,
            })
            .flatten()
            .collect();
        self.save_logs(logs);
    }
}

/// The new UI in a window over a throwaway demo vault: `annulus-app --preview` (add `--light`
/// for the light theme).
struct Preview {
    app: NativeApp,
    theme: Theme,
}

impl WindowApp for Preview {
    fn set_waker(&mut self, waker: Waker) {
        self.app.waker = Some(waker);
    }

    fn frame(&mut self, ui: &mut Ui<'_>, maximized: bool) -> Option<WindowRequest> {
        self.app.maximized = maximized;
        self.app.frame(ui).map(|action| match action {
            WindowAction::Minimize => WindowRequest::Minimize,
            WindowAction::ToggleMaximize => WindowRequest::ToggleMaximize,
            WindowAction::Close => WindowRequest::Close,
        })
    }

    fn drag_zone(&self) -> Id {
        Id::new(titlebar::DRAG)
    }

    fn maximize_zone(&self) -> Id {
        Id::new(titlebar::MAXIMIZE)
    }

    fn theme(&self) -> Theme {
        colors::previewed_theme(
            &self.app.settings,
            self.app.settings_view.previewing,
            self.theme,
        )
    }

    fn menu(&mut self, item: MenuItem) {
        self.app.menu(item);
    }

    fn menu_keys(&self, item: MenuItem) -> Option<(Key, Modifiers)> {
        self.app.menu_keys(item)
    }
}

/// Runs the new UI over the real vault in `data_dir` (`annulus-app --native`): the lock screen
/// first, then live sessions.
///
/// # Errors
/// When the OS refuses a window.
/// The data folder's lock, or `None` when another Annulus window holds it. The OS frees it
/// when the window's process ends, even after a crash.
fn instance_lock(data_dir: &std::path::Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("annulus.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

pub fn run_native(light: bool, data_dir: &std::path::Path) -> Result<(), WindowError> {
    let mut app = NativeApp::new(None);
    app.os_reduces_motion = annulus_platform::window::os_reduces_motion();
    let vault_path = data_dir.join("vault.anv");
    app.lock = lock::LockView::new(if vault_path.exists() {
        lock::Mode::Unlock
    } else {
        lock::Mode::Setup
    });
    app.vault_path = Some(vault_path);
    app.ssh_files.known_hosts = data_dir.join("known_hosts");
    let settings_path = data_dir.join("settings.json");
    app.settings = Settings::load(&settings_path);
    app.settings_path = Some(settings_path);
    let _ = std::fs::create_dir_all(data_dir);
    app.instance = instance_lock(data_dir);
    if app.instance.is_some() {
        app.restore();
    } else {
        let why = "Annulus is already open in another window. Use that window, or close it                    and open Annulus again.";
        app.blocked = Some(why.to_owned());
        app.lock.error = Some(why.to_owned());
    }

    run(
        "Annulus",
        (1100.0, 720.0),
        Preview {
            app,
            theme: Theme::new(if light {
                Appearance::Light
            } else {
                Appearance::Dark
            }),
        },
    )
}

/// Runs the preview window until it is closed.
///
/// # Errors
/// When the OS refuses a window.
pub fn run_preview(light: bool) -> Result<(), WindowError> {
    let appearance = if light {
        Appearance::Light
    } else {
        Appearance::Dark
    };
    let mut app = NativeApp::new(Some(demo::fixture_vault("preview", true)));
    app.os_reduces_motion = annulus_platform::window::os_reduces_motion();
    app.ssh_files.known_hosts = demo::known_hosts("preview");
    app.push_terminal("prod-db", TermView::Replay(Box::new(demo::terminal())));
    app.push_sftp(demo::sftp_tab("preview"));
    app.active = 0;
    run(
        "Annulus",
        (1100.0, 720.0),
        Preview {
            app,
            theme: Theme::new(appearance),
        },
    )
}
