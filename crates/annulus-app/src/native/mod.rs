//! The window drawn by our own toolkit ([ADR-0017]): a title bar that holds the tabs and the
//! window buttons, the vault's navigation, and the screen of the chosen section.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

pub mod demo;
pub mod hosts;
pub mod lock;
#[cfg(test)]
mod scenes;
mod terminal;
mod titlebar;

use annulus_config::tokens::{Appearance, space};
use annulus_platform::window::{WindowApp, WindowError, WindowRequest, run};
use annulus_term::Emulator;
use annulus_ui::gfx::{Align, Color, Font, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Theme, Ui};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, SMALL};
use annulus_vault::vault::Vault;

use crate::terminal_view::TabStatus;

/// The terminal font size in DIPs (the setting joins with live sessions).
const TERMINAL_FONT: f32 = 13.5;

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
    /// What it holds.
    pub kind: TabKind,
    /// Its label.
    pub title: String,
    /// Its status dot.
    pub status: Option<TabStatus>,
    /// The terminal of a terminal tab. Live sessions join in a later step; the preview and the
    /// scenes replay recorded output.
    pub terminal: Option<Emulator>,
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
}

impl NativeApp {
    /// A window showing the Hosts screen of `vault`.
    #[must_use]
    pub fn new(vault: Option<Vault>) -> Self {
        Self {
            tabs: vec![Tab {
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
                match self.section {
                    Section::Hosts => {
                        self.hosts.draw(ui, content, self.vault.as_mut());
                    }
                    other => placeholder(ui, content, other.title()),
                }
            }
        } else if let Some(emulator) = self
            .tabs
            .get_mut(self.active)
            .and_then(|tab| tab.terminal.as_mut())
        {
            let font = Font::mono(TERMINAL_FONT);
            let screen = emulator.screen();
            let background = Color::rgba(
                screen.background.r,
                screen.background.g,
                screen.background.b,
                255,
            );
            let content = panel_filled(ui, area, background);
            let inner = content.inset(space::L, space::M);
            let (columns, rows) = terminal::grid_size(ui, inner, font);
            emulator.resize(columns, rows);
            let screen = emulator.screen();
            terminal::draw(ui, inner, &screen, font, true);
        } else {
            let content = panel(ui, area);
            placeholder(ui, content, "Terminal");
        }
        ui.overlays();
        action
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

/// A screen that is not ported yet.
fn placeholder(ui: &mut Ui<'_>, rect: Rect, title: &str) {
    let t = ui.theme;
    let mut inner = rect.inset(space::XL + 4.0, space::XL);
    let head = inner.take_top(32.0);
    ui.strict_text(
        head,
        title,
        annulus_ui::widgets::TITLE,
        t.text,
        Align::Start,
    );
    inner.take_top(space::S);
    let line = inner.take_top(20.0);
    ui.gfx.text(
        line,
        "This screen moves to the new UI next.",
        BODY,
        t.text2,
        Align::Start,
    );
}

/// The new UI in a window over a throwaway demo vault: `annulus-app --preview` (add `--light`
/// for the light theme).
struct Preview {
    app: NativeApp,
    theme: Theme,
}

impl WindowApp for Preview {
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
        self.theme
    }
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
    app.tabs.push(Tab {
        kind: TabKind::Terminal,
        title: "prod-db".into(),
        status: None,
        terminal: Some(demo::terminal()),
    });
    run(
        "Annulus",
        (1100.0, 720.0),
        Preview {
            app,
            theme: Theme::new(appearance),
        },
    )
}
