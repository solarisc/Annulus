//! The window drawn by our own toolkit ([ADR-0017]): a title bar that holds the tabs and the
//! window buttons, the vault's navigation, and the screen of the chosen section.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

pub mod demo;
pub mod hosts;
mod live;
pub mod lock;
mod pages;
mod panes;
#[cfg(test)]
mod scenes;
mod screens;
mod terminal;
mod titlebar;

use annulus_config::settings::Settings;
use annulus_config::tokens::{Appearance, ThemeMode, space};
use annulus_platform::window::{Waker, WindowApp, WindowError, WindowRequest, run};
use annulus_session::profiles::{self, Profile};
use annulus_session::{Program, SshFiles};
use annulus_term::Emulator;
use annulus_ui::gfx::{Align, Color, Font, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Theme, Ui};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, SMALL};
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
    /// A terminal, local or SSH.
    Terminal,
}

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
}

/// The whole window's state.
pub struct NativeApp {
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
    /// The lock screen, shown while there is no unlocked vault.
    pub lock: lock::LockView,
    /// Where the vault file lives; `None` for the preview's throwaway vault.
    pub vault_path: Option<std::path::PathBuf>,
    /// Where SSH keeps its known hosts and default keys.
    pub ssh_files: SshFiles,
    /// Repaints the window from a session's thread.
    pub waker: Option<Waker>,
    /// Terminal tabs to open after this frame: title and program.
    opens: Vec<(String, Program)>,
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
    next_uid: u64,
}

impl NativeApp {
    /// A window showing the Hosts screen of `vault`.
    #[must_use]
    pub fn new(vault: Option<Vault>) -> Self {
        Self {
            tabs: vec![Tab {
                uid: 0,
                kind: TabKind::Vault,
                title: "Vault".into(),
                status: None,
                terminal: None,
            }],
            active: 0,
            section: Section::Hosts,
            vault,
            hosts: hosts::HostsView::default(),
            maximized: false,
            lock: lock::LockView::new(lock::Mode::Unlock),
            vault_path: None,
            ssh_files: SshFiles {
                known_hosts: std::env::temp_dir().join("annulus-preview-known_hosts"),
                key_dir: directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".ssh")),
            },
            waker: None,
            opens: Vec::new(),
            settings: Settings::default(),
            settings_path: None,
            profiles: profiles::detect(),
            keychain: screens::KeychainView::default(),
            snippets: screens::SnippetsView::default(),
            forwards: screens::ForwardsView::default(),
            known: screens::KnownView::default(),
            logs: screens::LogsView::default(),
            next_uid: 1,
        }
    }

    /// Adds a terminal tab showing `view`, in front.
    pub fn push_terminal(&mut self, title: impl Into<String>, view: TermView) {
        self.tabs.push(Tab {
            uid: self.next_uid,
            kind: TabKind::Terminal,
            title: title.into(),
            status: None,
            terminal: Some(view),
        });
        self.next_uid += 1;
        self.active = self.tabs.len() - 1;
    }

    /// Opens a live terminal tab running `program` after this frame.
    pub fn open(&mut self, title: impl Into<String>, program: Program) {
        self.opens.push((title.into(), program));
    }

    /// Opens the default local shell (Settings, Local shells).
    fn open_local(&mut self) {
        let profile = self
            .settings
            .default_profile
            .as_ref()
            .and_then(|name| self.profiles.iter().find(|profile| &profile.name == name))
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
        self.open(title, program);
    }

    /// Split, close, broadcast and move between panes of the terminal tab in front
    /// (UI-SPEC §7). Taken before the terminal sees the keys.
    fn pane_shortcuts(&mut self, ui: &mut Ui<'_>) {
        let close = ui.key(Key::Char('w'), Modifiers::CTRL_SHIFT);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let uid = tab.uid;
        let Some(TermView::Live(panes)) = tab.terminal.as_mut() else {
            if close && self.active > 0 {
                self.tabs.remove(self.active);
                self.active -= 1;
            }
            return;
        };
        if close && !panes.close_focused() {
            self.tabs.remove(self.active);
            self.active -= 1;
            return;
        }
        if ui.key(Key::Char('d'), Modifiers::CTRL_SHIFT) {
            panes.split(true);
        }
        if ui.key(Key::Char('e'), Modifiers::CTRL_SHIFT) {
            panes.split(false);
        }
        if ui.key(Key::Char('b'), Modifiers::CTRL_SHIFT) {
            panes.broadcast = !panes.broadcast;
        }
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        if panes.len() > 1 {
            for (key, dx, dy) in [
                (Key::Left, -1.0, 0.0),
                (Key::Right, 1.0, 0.0),
                (Key::Up, 0.0, -1.0),
                (Key::Down, 0.0, 1.0),
            ] {
                if ui.key(key, alt) {
                    panes.move_focus(dx, dy);
                    ui.state
                        .set_focus(Some(Id::new(("terminal", uid, panes.focused))), false);
                }
            }
        }
    }

    /// What another screen asked for.
    fn screen_action(&mut self, action: screens::ScreenAction) {
        match action {
            screens::ScreenAction::Terminal(title, program) => self.open(title, *program),
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
                self.open(title, program);
            }
            hosts::HostsAction::QuickConnect(target) => {
                let program = connect::quick(&target, &self.ssh_files);
                self.open(target, program);
            }
            hosts::HostsAction::LocalTerminal => self.open_local(),
            // TODO: the ssh_config import preview in the new UI.
            hosts::HostsAction::ImportSshConfig => {}
        }
    }

    /// Creates or unlocks the vault as the lock screen asked.
    fn open_vault(&mut self, action: lock::LockAction) {
        let Some(path) = self.vault_path.clone() else {
            self.lock.error = Some("This preview has no vault file.".into());
            return;
        };
        let opened = match action {
            lock::LockAction::Create(password) => {
                Vault::create(path, &password, annulus_vault::crypto::KdfParams::DEFAULT)
                    .map(|(vault, _)| vault)
            }
            lock::LockAction::Unlock { secret, recovery } => {
                if recovery {
                    Vault::recover(path, &secret)
                } else {
                    Vault::unlock(path, &secret)
                }
            }
        };
        match opened {
            Ok(vault) => self.vault = Some(vault),
            Err(annulus_vault::vault::VaultError::WrongPassword) => {
                self.lock.error = Some("Wrong password.".into());
            }
            Err(error) => self.lock.error = Some(format!("Could not open the vault: {error}")),
        }
    }

    /// The sync badge for the vault's account state.
    fn sync_badge(&self) -> SyncBadge {
        let Some(state) = self.vault.as_ref().map(Vault::sync_state) else {
            return SyncBadge::Local;
        };
        match (state.account.is_some(), state.pending_count()) {
            (false, _) => SyncBadge::Local,
            (true, 0) => SyncBadge::Synced,
            (true, pending) => SyncBadge::Pending(pending),
        }
    }

    /// Draws one frame. Returns what the window itself must do.
    pub fn frame(&mut self, ui: &mut Ui<'_>) -> Option<WindowAction> {
        let t = ui.theme;
        let mut area = ui.viewport;
        ui.gfx.fill_rect(area, 0.0, t.chrome);
        let bar = area.take_top(TITLE_H);
        let action = titlebar::draw(self, ui, bar);
        if ui.key(Key::Tab, Modifiers::CTRL) && !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
        if ui.key(Key::Char('t'), Modifiers::CTRL_SHIFT) {
            self.open_local();
        }
        self.pane_shortcuts(ui);
        let kind = self.tabs.get(self.active).map(|tab| tab.kind);
        if kind == Some(TabKind::Vault) {
            let compact = ui.viewport.w < COLLAPSE_BELOW;
            let nav = area.take_left(if compact { NAV_COMPACT_W } else { NAV_W });
            self.navigation(ui, nav, compact);
            let content = panel(ui, area);
            if self.vault.is_none() {
                if let Some(action) = self.lock.draw(ui, content) {
                    self.open_vault(action);
                }
            } else {
                self.section_screen(ui, content);
            }
        } else if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(view) = tab.terminal.as_mut()
        {
            let font = Font::mono(self.settings.font_size.clamp(8.0, 32.0));
            let palette = annulus_term::themes::theme(&self.settings.terminal_theme)
                .unwrap_or_else(annulus_term::Palette::annulus_dark);
            let background = Color::rgba(
                palette.background.r,
                palette.background.g,
                palette.background.b,
                255,
            );
            let content = panel_filled(ui, area, background);
            match view {
                TermView::Replay(emulator) => {
                    let inner = content.inset(space::L, space::M);
                    let (columns, rows) = terminal::grid_size(ui, inner, font);
                    emulator.resize(columns, rows);
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
        ui.overlays();
        for tab in &mut self.tabs {
            if let Some(TermView::Live(panes)) = &tab.terminal {
                tab.status = panes.status();
            }
        }
        for (title, program) in std::mem::take(&mut self.opens) {
            self.push_terminal(
                title,
                TermView::Live(Box::new(panes::Panes::new(live::LiveTerminal::new(
                    program,
                )))),
            );
            ui.request_repaint();
        }
        action
    }

    fn section_screen(&mut self, ui: &mut Ui<'_>, content: Rect) {
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        let action = match self.section {
            Section::Hosts => {
                if let Some(action) = self.hosts.draw(ui, content, Some(vault)) {
                    self.act(action);
                }
                None
            }
            Section::Keychain => {
                self.keychain.draw(ui, content, vault);
                None
            }
            Section::Snippets => self.snippets.draw(ui, content, vault),
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
                if screens::settings(ui, content, &mut self.settings, &self.profiles)
                    && let Some(path) = &self.settings_path
                {
                    let _ = self.settings.save(path);
                }
                None
            }
        };
        if let Some(action) = action {
            self.screen_action(action);
        }
    }

    fn navigation(&mut self, ui: &mut Ui<'_>, rect: Rect, compact: bool) {
        let mut column = rect.inset(space::S + 2.0, 0.0);
        column.take_top(space::S);
        for section in Section::MAIN {
            let row = column.take_top(36.0);
            self.nav_item(ui, row.inset(0.0, 1.0), section, compact);
        }
        let mut footer = column;
        let settings = footer.take_bottom(space::M + 36.0);
        self.nav_item(
            ui,
            Rect::new(settings.x, settings.y, settings.w, 36.0).inset(0.0, 1.0),
            Section::Settings,
            compact,
        );
        if !compact {
            let status = footer.take_bottom(52.0).inset(0.0, space::XS);
            let badge = self.sync_badge();
            vault_status(ui, status, self.vault.is_some(), &badge);
        }
    }

    fn nav_item(&mut self, ui: &mut Ui<'_>, rect: Rect, section: Section, compact: bool) {
        let id = Id::new(("nav", section.title()));
        let selected = self.section == section;
        let response = ui.zone(id, rect, Role::Tab, section.title());
        if response.clicked {
            self.section = section;
        }
        let t = ui.theme;
        let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let fill = if selected {
            t.selected
        } else {
            t.hover.alpha(hover * 0.8)
        };
        ui.gfx.fill_rect(rect, 7.0, fill);
        if selected {
            let bar = Rect::new(rect.x, rect.y + 9.0, 3.0, rect.h - 18.0);
            ui.gfx.fill_rect(bar, 1.5, t.accent);
        }
        if response.focus_ring {
            ui.focus_ring(rect, 7.0);
        }
        let ink = if selected { t.accent } else { t.text2 };
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

/// The rounded content panel beside the navigation; returns its inside.
fn panel(ui: &mut Ui<'_>, rect: Rect) -> Rect {
    let fill = ui.theme.bg;
    panel_filled(ui, rect, fill)
}

/// The content panel filled with `fill` (a terminal's own background).
fn panel_filled(ui: &mut Ui<'_>, rect: Rect, fill: Color) -> Rect {
    let t = ui.theme;
    // Only the top-left corner is rounded: the others sit on the window's edges, so the
    // rectangle runs past them.
    let shape = Rect::new(rect.x, rect.y, rect.w + 24.0, rect.h + 24.0);
    ui.gfx.fill_rect(shape, 12.0, fill);
    ui.gfx.stroke_rect(shape, 12.0, 1.0, t.line);
    rect
}

/// The lock state and sync badge above Settings.
fn vault_status(ui: &mut Ui<'_>, rect: Rect, unlocked: bool, sync: &SyncBadge) {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 8.0, t.surface.alpha(0.6));
    ui.gfx.stroke_rect(rect, 8.0, 1.0, t.line);
    let mut inner = rect.inset(space::M, 0.0);
    let icon = inner.take_left(16.0).centered(16.0, 16.0);
    let (title, detail, dot) = match (unlocked, sync) {
        (false, _) => ("Vault locked", "Unlock to see your hosts", t.text3),
        (true, SyncBadge::Local) => ("Local vault", "On this device only", t.text3),
        (true, SyncBadge::Synced) => ("Synced", "Up to date", t.ok),
        (true, SyncBadge::Pending(_)) => ("Syncing", "Sending changes", t.warn),
    };
    let glyph = match (unlocked, sync) {
        (false, _) => icons::LOCK,
        (true, SyncBadge::Local) => icons::DEVICE,
        (true, _) => icons::SYNC,
    };
    ui.gfx.icon(glyph, icon, t.text2);
    inner.take_left(space::M - 2.0);
    let top = Rect::new(inner.x, inner.y + 8.0, inner.w - 10.0, 18.0);
    let bottom = Rect::new(inner.x, inner.y + 25.0, inner.w, 16.0);
    ui.gfx.text(top, title, BODY_MEDIUM, t.text, Align::Start);
    ui.gfx.text(bottom, detail, SMALL, t.text2, Align::Start);
    ui.gfx.fill_circle(
        annulus_ui::gfx::Point::new(inner.r() - 4.0, top.center().y),
        3.5,
        dot,
    );
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
        match self.app.settings.theme {
            ThemeMode::Light => Theme::new(Appearance::Light),
            ThemeMode::Dark => Theme::new(Appearance::Dark),
            ThemeMode::System => self.theme,
        }
    }
}

/// Runs the new UI over the real vault in `data_dir` (`annulus-app --native`): the lock screen
/// first, then live sessions.
///
/// # Errors
/// When the OS refuses a window.
pub fn run_native(light: bool, data_dir: &std::path::Path) -> Result<(), WindowError> {
    let mut app = NativeApp::new(None);
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
    app.ssh_files.known_hosts = demo::known_hosts("preview");
    app.push_terminal("prod-db", TermView::Replay(Box::new(demo::terminal())));
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
