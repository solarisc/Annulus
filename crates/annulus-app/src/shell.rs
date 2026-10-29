//! The window's layout (UI-SPEC §1–2): a top tab strip with the Vault tab first, the vault's
//! left navigation and the screen for the chosen section, and terminal tabs.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Appearance, Palette, ThemeMode, space, text};
use annulus_session::{Program, SshFiles};
use annulus_ui::{icons, theme};
use annulus_vault::vault::Vault;

use crate::forwards_screen::ForwardsScreen;
use crate::hosts_screen::{HostsScreen, Request};
use crate::keychain_screen::KeychainScreen;
use crate::lock_screen::LockScreen;
use crate::panes::{Layout as PaneLayout, Panes};
use crate::password_screen::PasswordScreen;
use crate::sftp_view::SftpTab;
use crate::snippets_screen::{self, SnippetsScreen};
use crate::sync_screen::SyncScreen;
use crate::terminal_view::FONT_SIZES;
use annulus_config::settings::{DEFAULT_FONT_SIZE, Settings};
use annulus_session::profiles::{self, Profile};
use annulus_term::themes;

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
    /// The sections in the order the navigation lists them (Settings last, at the bottom).
    pub const ALL: [Section; 7] = [
        Section::Hosts,
        Section::Keychain,
        Section::PortForwarding,
        Section::Snippets,
        Section::KnownHosts,
        Section::Logs,
        Section::Settings,
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

    fn icon(self) -> icons::IconId {
        match self {
            Section::Hosts => icons::SERVER,
            Section::Keychain => icons::KEY,
            Section::PortForwarding => icons::CHEVRON_RIGHT,
            Section::Snippets => icons::TERMINAL,
            Section::KnownHosts => icons::LOCK,
            Section::Logs => icons::FOLDER,
            Section::Settings => icons::SETTINGS,
        }
    }
}

/// One tab of the top strip.
pub enum Tab {
    /// The vault: hosts, keychain and the rest. Always first, never closed.
    Vault,
    /// Terminals: one, or several split panes.
    Terminal(Box<Panes>),
    /// A file browser on a host.
    Sftp(Box<SftpTab>),
}

impl Tab {
    /// The title shown in the strip.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Tab::Vault => "Vault",
            Tab::Terminal(panes) => panes.title(),
            Tab::Sftp(sftp) => &sftp.title,
        }
    }
}

/// The whole window's state.
pub struct Shell {
    /// Open tabs; the first is always the vault.
    pub tabs: Vec<Tab>,
    /// The tab in front.
    pub active_tab: usize,
    /// The vault section shown.
    pub section: Section,
    /// The light/dark choice from Settings.
    pub theme_mode: ThemeMode,
    /// The Hosts screen, while the vault is unlocked.
    pub hosts: Option<HostsScreen>,
    /// Port-forwarding rules and the running ones, while the vault is unlocked.
    forwards: ForwardsScreen,
    /// Account sync, while the vault is unlocked.
    sync: SyncScreen,
    /// The Keychain screen.
    keychain: KeychainScreen,
    /// The Snippets screen.
    snippets: SnippetsScreen,
    /// Changing the master password, in Settings.
    password: PasswordScreen,
    /// The snippet panel beside terminals is open (Ctrl+Shift+.), and its search text.
    snippet_panel: Option<String>,
    /// Setup or unlock, while the vault is locked.
    lock: LockScreen,
    data_dir: std::path::PathBuf,
    /// Terminal font size in points, shared by all terminal tabs.
    pub font_size: f32,
    /// Lock the vault after this many idle minutes; 0 never locks.
    pub auto_lock_minutes: u32,
    /// When input last arrived, in egui seconds.
    last_input: f64,
    applied: Option<Appearance>,
    /// The settings as last saved.
    saved: Settings,
    /// The local shells found at start (F-LOCAL-002).
    profiles: Vec<Profile>,
    /// The profile chosen for new local tabs, by name.
    default_profile: Option<String>,
    /// The terminal color theme, by name (F-TERM-013).
    terminal_theme: String,
}

/// The device settings' file name in the data folder.
const SETTINGS_FILE: &str = "settings.json";

/// The vault's file name in the data folder.
const VAULT_FILE: &str = "vault.anv";

impl Shell {
    /// A window whose saved data lives in `data_dir`.
    #[must_use]
    pub fn new(data_dir: &std::path::Path) -> Self {
        let settings = Settings::load(&data_dir.join(SETTINGS_FILE));
        Self {
            tabs: vec![Tab::Vault],
            active_tab: 0,
            section: Section::Hosts,
            theme_mode: settings.theme,
            hosts: None,
            forwards: ForwardsScreen::default(),
            sync: SyncScreen::default(),
            keychain: KeychainScreen::default(),
            snippets: SnippetsScreen::default(),
            password: PasswordScreen::default(),
            snippet_panel: None,
            lock: LockScreen::new(&data_dir.join(VAULT_FILE)),
            data_dir: data_dir.to_path_buf(),
            font_size: settings
                .font_size
                .clamp(*FONT_SIZES.start(), *FONT_SIZES.end()),
            auto_lock_minutes: settings.auto_lock_minutes,
            last_input: 0.0,
            applied: None,
            default_profile: settings.default_profile.clone(),
            terminal_theme: settings.terminal_theme.clone(),
            saved: settings,
            profiles: profiles::detect(),
        }
    }

    /// The profile new local tabs use: the one chosen in Settings, else the first found.
    fn default_profile(&self) -> Option<&Profile> {
        self.default_profile
            .as_ref()
            .and_then(|name| self.profiles.iter().find(|profile| &profile.name == name))
            .or_else(|| self.profiles.first())
    }

    /// Split right (Ctrl+Shift+D) and down (Ctrl+Shift+E) with the focused pane's program,
    /// and Alt+Arrows to move between panes (UI-SPEC §7).
    fn pane_shortcuts(&mut self, ui: &Ui) {
        let Some(Tab::Terminal(panes)) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let command = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let several = panes.len() > 1;
        let (right, down, back, forward) = ui.input_mut(|input| {
            let mut arrow = |key| several && input.consume_key(egui::Modifiers::ALT, key);
            let back = arrow(egui::Key::ArrowLeft) | arrow(egui::Key::ArrowUp);
            let forward = arrow(egui::Key::ArrowRight) | arrow(egui::Key::ArrowDown);
            (
                input.consume_key(command, egui::Key::D),
                input.consume_key(command, egui::Key::E),
                back,
                forward,
            )
        });
        if let Some((title, program)) = panes.focused_program() {
            if right {
                panes.split(PaneLayout::Row, title.clone(), program.clone());
            }
            if down {
                panes.split(PaneLayout::Column, title, program);
            }
        }
        if back {
            panes.move_focus(-1);
        }
        if forward {
            panes.move_focus(1);
        }
    }

    /// Opens a local terminal with `profile`, or the default one.
    pub fn open_local(&mut self, profile: Option<usize>) {
        let chosen = profile
            .and_then(|index| self.profiles.get(index))
            .or_else(|| self.default_profile())
            .cloned();
        match chosen {
            Some(profile) => self.open_terminal(profile.name.clone(), profile.program()),
            None => self.open_terminal("Local", Program::DefaultShell),
        }
    }

    /// The settings as they are now.
    fn current_settings(&self) -> Settings {
        Settings {
            theme: self.theme_mode,
            font_size: self.font_size,
            auto_lock_minutes: self.auto_lock_minutes,
            default_profile: self.default_profile.clone(),
            terminal_theme: self.terminal_theme.clone(),
        }
    }

    /// Saves the settings when they changed (a small file; checked once per frame).
    fn persist(&mut self) {
        let now = self.current_settings();
        if now != self.saved && now.save(&self.data_dir.join(SETTINGS_FILE)).is_ok() {
            self.saved = now;
        }
    }
}

impl Shell {
    /// Opens a terminal tab titled `title` running `program`, in front.
    pub fn open_terminal(&mut self, title: impl Into<String>, program: Program) {
        self.tabs
            .push(Tab::Terminal(Box::new(Panes::single(title, program))));
        self.active_tab = self.tabs.len() - 1;
    }

    /// Opens an SFTP tab titled `title` for the SSH connection `program`, in front.
    pub fn open_sftp(&mut self, title: impl Into<String>, program: Program) {
        let title = title.into();
        let tab = match program {
            Program::Ssh {
                target,
                files,
                password,
                identity,
                via,
            } => SftpTab::new(
                title,
                annulus_session::Hop {
                    target,
                    password,
                    identity,
                },
                files,
                via,
            ),
            _ => SftpTab::unavailable(title, "SFTP needs a host Annulus can connect to itself."),
        };
        self.tabs.push(Tab::Sftp(Box::new(tab)));
        self.active_tab = self.tabs.len() - 1;
    }

    /// Closes tab `index`; the vault tab cannot be closed. The tab to its left comes forward
    /// if the closed tab was in front or before it.
    pub fn close_tab(&mut self, index: usize) {
        if index == 0 || index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active_tab >= index {
            self.active_tab = self.active_tab.saturating_sub(1).min(self.tabs.len() - 1);
        }
    }

    /// Applies the theme for this frame: the Settings choice, or the OS theme for System.
    fn apply_theme(&mut self, ctx: &egui::Context) {
        let system = match ctx.system_theme() {
            Some(egui::Theme::Light) => Appearance::Light,
            _ => Appearance::Dark,
        };
        let appearance = self.theme_mode.resolve(system);
        if self.applied != Some(appearance) {
            ctx.set_global_style(theme::style(appearance));
            self.applied = Some(appearance);
        }
    }

    fn palette(&self) -> Palette {
        Palette::for_appearance(self.applied.unwrap_or(Appearance::Dark))
    }

    /// Draws the window.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.persist();
        self.apply_theme(&ui.ctx().clone());
        // Ctrl+Shift+/-/0 zoom the terminal, not the whole UI.
        ui.ctx()
            .options_mut(|options| options.zoom_with_keyboard = false);
        self.shortcuts(ui);
        self.auto_lock(ui.ctx());
        egui::Panel::top("tabs").show(ui, |ui| self.tab_strip(ui));
        let palette = self.palette();
        let vault_path = self.data_dir.join(VAULT_FILE);
        if let Some(hosts) = &mut self.hosts {
            if self.sync.tick(ui.ctx(), hosts, &vault_path) {
                // Signing in renumbered hosts and rules; running rules would no longer match.
                self.forwards = ForwardsScreen::default();
            }
            self.forwards.auto_start(ui.ctx(), hosts);
        }
        self.forwards.prompts(ui.ctx());
        if matches!(self.tabs.get(self.active_tab), Some(Tab::Vault)) {
            egui::Panel::left("vault-nav")
                .resizable(false)
                .exact_size(200.0)
                .show(ui, |ui| self.navigation(ui));
            egui::CentralPanel::default().show(ui, |ui| self.section_screen(ui));
        } else if let Some(Tab::Terminal(panes)) = self.tabs.get_mut(self.active_tab) {
            let font_size = self.font_size;
            let theme = themes::theme(&self.terminal_theme)
                .unwrap_or_else(annulus_term::Palette::annulus_dark);
            for pane in panes.all_mut() {
                pane.set_palette(&theme);
            }
            if let Some(search) = &mut self.snippet_panel {
                let snippets = self
                    .hosts
                    .as_ref()
                    .map(|hosts| hosts.vault().snippets().to_vec())
                    .unwrap_or_default();
                let chosen = egui::Panel::right("terminal-snippets")
                    .resizable(true)
                    .default_size(260.0)
                    .show(ui, |ui| snippets_screen::side_panel(ui, search, &snippets))
                    .inner;
                if let Some((snippet, run)) = chosen
                    && let Some(pane) = panes.focused_mut()
                {
                    pane.send_snippet(&snippet, run);
                }
            }
            let frame = egui::Frame::NONE.inner_margin(egui::Margin::same(4));
            egui::CentralPanel::default()
                .frame(frame)
                .show(ui, |ui| panes.ui(ui, font_size));
        } else if let Some(Tab::Sftp(sftp)) = self.tabs.get_mut(self.active_tab) {
            egui::CentralPanel::default().show(ui, |ui| sftp.ui(ui, &palette));
        }
    }

    /// Shows the hosts of a just-unlocked vault, first moving in any plain host file from
    /// earlier builds.
    fn unlocked(&mut self, mut vault: Vault) {
        // A damaged or unreadable plain file is kept as it is; the vault opens regardless.
        let _ = vault.import_plain_hosts(&self.data_dir.join("hosts.json"));
        let files = SshFiles {
            known_hosts: self.data_dir.join("known_hosts"),
            key_dir: directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".ssh")),
        };
        self.hosts = Some(HostsScreen::new(vault, files));
        self.forwards = ForwardsScreen::default();
        self.sync = SyncScreen::default();
    }

    /// Locks the vault: its keys and decrypted hosts are dropped (F-SEC-004).
    pub fn lock_vault(&mut self) {
        self.hosts = None;
        // Running rules stop with the lock: their connections used the vault's secrets.
        self.forwards = ForwardsScreen::default();
        self.sync = SyncScreen::default();
        // Typed passwords, pasted keys and drafts go with it.
        self.password = PasswordScreen::default();
        self.keychain = KeychainScreen::default();
        self.snippets = SnippetsScreen::default();
        self.lock = LockScreen::new(&self.data_dir.join(VAULT_FILE));
    }

    /// App shortcuts use Ctrl+Shift so plain Ctrl+letter stays with the shell (UI-SPEC §7).
    fn shortcuts(&mut self, ui: &Ui) {
        let command = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let (new_tab, close_tab, next_tab, lock, snippets) = ui.input_mut(|input| {
            (
                input.consume_key(command, egui::Key::T),
                input.consume_key(command, egui::Key::W),
                input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
                input.consume_key(command, egui::Key::L),
                input.consume_key(command, egui::Key::Period),
            )
        });
        self.pane_shortcuts(ui);
        if snippets {
            self.snippet_panel = match self.snippet_panel.take() {
                Some(_) => None,
                None => Some(String::new()),
            };
        }
        if lock {
            self.lock_vault();
        }
        if new_tab {
            self.open_local(None);
        }
        if close_tab {
            // A split tab closes its focused pane first.
            let closed_pane = match self.tabs.get_mut(self.active_tab) {
                Some(Tab::Terminal(panes)) => panes.close_focused(),
                _ => false,
            };
            if !closed_pane {
                self.close_tab(self.active_tab);
            }
        }
        if next_tab {
            self.active_tab = (self.active_tab + 1) % self.tabs.len().max(1);
        }
        // Terminal zoom: Ctrl+Shift with + / - / 0, or Ctrl+wheel (egui reports it as zoom).
        let (bigger, smaller, reset, wheel) = ui.input_mut(|input| {
            (
                input.consume_key(command, egui::Key::Plus)
                    || input.consume_key(command, egui::Key::Equals),
                input.consume_key(command, egui::Key::Minus),
                input.consume_key(command, egui::Key::Num0),
                input.zoom_delta(),
            )
        });
        let mut size = self.font_size;
        if bigger {
            size += 1.0;
        }
        if smaller {
            size -= 1.0;
        }
        if reset {
            size = DEFAULT_FONT_SIZE;
        }
        if (wheel - 1.0).abs() > f32::EPSILON {
            size *= wheel;
        }
        self.font_size = size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
    }

    fn tab_strip(&mut self, ui: &mut Ui) {
        let mut close = None;
        let mut select = None;
        let mut new_local: Option<Option<usize>> = None;
        ui.horizontal(|ui| {
            for (index, tab) in self.tabs.iter().enumerate() {
                let response = ui.selectable_label(index == self.active_tab, tab.title());
                if response.clicked() {
                    select = Some(index);
                }
                let close_clicked = index > 0
                    && ui
                        .small_button("\u{d7}")
                        .on_hover_text("Close tab (Ctrl+Shift+W)")
                        .clicked();
                if index > 0 && (response.middle_clicked() || close_clicked) {
                    close = Some(index);
                }
            }
            new_local = ui
                .button("+")
                .on_hover_text("New local terminal (Ctrl+Shift+T)")
                .clicked()
                .then_some(None);
            ui.menu_button("\u{25be}", |ui| {
                for (index, profile) in self.profiles.iter().enumerate() {
                    if ui.button(&profile.name).clicked() {
                        new_local = Some(Some(index));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Open a terminal with another shell");
        });
        if let Some(index) = select {
            self.active_tab = index;
        }
        if let Some(index) = close {
            self.close_tab(index);
        }
        if let Some(profile) = new_local {
            self.open_local(profile);
        }
    }

    fn navigation(&mut self, ui: &mut Ui) {
        ui.add_space(space::M);
        let (main, bottom) = Section::ALL.split_at(Section::ALL.len() - 1);
        for section in main {
            self.nav_item(ui, *section);
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(space::M);
            for section in bottom {
                self.nav_item(ui, *section);
            }
        });
    }

    fn nav_item(&mut self, ui: &mut Ui, section: Section) {
        let selected = self.section == section;
        let palette = self.palette();
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 32.0), Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Tab, true, selected, section.title())
        });
        if selected || response.hovered() {
            let fill = if selected {
                palette.bg_selected
            } else {
                palette.bg_hover
            };
            ui.painter().rect_filled(
                rect.shrink2(egui::vec2(space::S, 2.0)),
                6.0,
                theme::color(fill),
            );
        }
        let ink = if selected {
            theme::color(palette.accent)
        } else {
            theme::color(palette.text_secondary)
        };
        let icon =
            egui::Rect::from_min_size(rect.min + egui::vec2(space::L, 8.0), egui::vec2(16.0, 16.0));
        icons::paint(ui.painter(), section.icon(), icon, ink);
        ui.painter().text(
            rect.min + egui::vec2(space::L + 26.0, 16.0),
            egui::Align2::LEFT_CENTER,
            section.title(),
            egui::FontId::proportional(text::BODY),
            theme::color(palette.text_primary),
        );
        if response.clicked() {
            self.section = section;
        }
    }

    fn section_screen(&mut self, ui: &mut Ui) {
        ui.add_space(space::S);
        match self.section {
            Section::Hosts => {
                let palette = self.palette();
                if let Some(hosts) = &mut self.hosts {
                    match hosts.ui(ui, &palette) {
                        Some(Request::Open(title, Program::DefaultShell)) if title == "Local" => {
                            self.open_local(None);
                        }
                        Some(Request::Open(title, program)) => self.open_terminal(title, program),
                        Some(Request::Sftp(title, program)) => self.open_sftp(title, program),
                        None => {}
                    }
                } else if let Some(vault) =
                    self.lock.ui(ui, &self.data_dir.join(VAULT_FILE), &palette)
                {
                    self.unlocked(vault);
                }
            }
            Section::Snippets => {
                let palette = self.palette();
                if let Some(hosts) = &mut self.hosts {
                    self.snippets.ui(ui, &palette, hosts.vault_mut());
                } else {
                    self.section = Section::Hosts;
                }
            }
            Section::Keychain => {
                let palette = self.palette();
                if let Some(hosts) = &mut self.hosts {
                    self.keychain.ui(ui, &palette, hosts.vault_mut());
                } else {
                    self.section = Section::Hosts;
                }
            }
            Section::PortForwarding => {
                let palette = self.palette();
                if let Some(hosts) = &mut self.hosts {
                    self.forwards.ui(ui, &palette, hosts);
                } else {
                    self.section = Section::Hosts;
                }
            }
            Section::Settings => self.settings(ui),
            other => empty_state(
                ui,
                other.title(),
                "Arrives with the vault.",
                &self.palette(),
            ),
        }
    }

    /// Locks an unlocked vault after the idle timeout (F-SEC-004). Instead of polling, one
    /// repaint is requested for the moment the timeout would pass.
    fn auto_lock(&mut self, ctx: &egui::Context) {
        let (now, active) = ctx.input(|input| {
            (
                input.time,
                !input.events.is_empty() || input.pointer.is_moving(),
            )
        });
        if active || self.last_input == 0.0 {
            self.last_input = now;
        }
        if self.hosts.is_none() || self.auto_lock_minutes == 0 {
            return;
        }
        let timeout = f64::from(self.auto_lock_minutes) * 60.0;
        let idle = now - self.last_input;
        if idle_expired(idle, timeout) {
            self.lock_vault();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                (timeout - idle).max(1.0),
            ));
        }
    }

    fn settings(&mut self, ui: &mut Ui) {
        ui.heading("Settings");
        ui.add_space(space::M);
        ui.label("Theme");
        ui.horizontal(|ui| {
            for (mode, label) in [
                (ThemeMode::System, "System"),
                (ThemeMode::Light, "Light"),
                (ThemeMode::Dark, "Dark"),
            ] {
                ui.selectable_value(&mut self.theme_mode, mode, label);
            }
        });
        ui.add_space(space::M);
        ui.label("Lock the vault after idle minutes (0 = never)");
        ui.add(egui::DragValue::new(&mut self.auto_lock_minutes).range(0..=240));
        ui.add_space(space::M);
        ui.label("Terminal colors");
        egui::ComboBox::from_id_salt("terminal-theme")
            .selected_text(&self.terminal_theme)
            .show_ui(ui, |ui| {
                for name in themes::THEMES {
                    ui.selectable_value(&mut self.terminal_theme, name.to_owned(), name);
                }
            });
        ui.add_space(space::M);
        ui.label("New local terminals open");
        let current = self
            .default_profile()
            .map(|profile| profile.name.clone())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("default-profile")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for profile in &self.profiles {
                    if ui
                        .selectable_label(profile.name == current, &profile.name)
                        .clicked()
                    {
                        self.default_profile = Some(profile.name.clone());
                    }
                }
            });
        ui.add_space(space::L);
        let palette = self.palette();
        let vault_path = self.data_dir.join(VAULT_FILE);
        if let Some(hosts) = &mut self.hosts {
            self.sync.ui(ui, &palette, hosts, &vault_path);
            ui.add_space(space::L);
            self.password.ui(ui, &palette, hosts);
        }
    }
}

/// Whether `idle` seconds have reached the `timeout`.
fn idle_expired(idle: f64, timeout: f64) -> bool {
    idle >= timeout
}

fn empty_state(ui: &mut Ui, title: &str, detail: &str, palette: &Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 4.0);
        ui.label(
            RichText::new(title)
                .size(text::TITLE)
                .color(theme::color(palette.text_primary)),
        );
        ui.add_space(space::S);
        ui.label(RichText::new(detail).color(theme::color(palette.text_secondary)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_shell() -> Shell {
        Shell::new(&std::env::temp_dir().join(format!("annulus-shell-test-{}", std::process::id())))
    }

    fn titles(shell: &Shell) -> Vec<&str> {
        shell.tabs.iter().map(Tab::title).collect()
    }

    #[test]
    fn the_vault_tab_cannot_be_closed_and_closing_moves_focus_left() {
        let mut shell = test_shell();
        shell.open_terminal("a", Program::DefaultShell);
        shell.open_terminal("b", Program::DefaultShell);
        shell.close_tab(0);
        assert_eq!(shell.tabs.len(), 3);
        shell.close_tab(2);
        assert_eq!((shell.tabs.len(), shell.active_tab), (2, 1));
        shell.close_tab(1);
        assert_eq!((titles(&shell), shell.active_tab), (vec!["Vault"], 0));
    }

    #[test]
    fn closing_a_background_tab_keeps_the_front_tab() {
        let mut shell = test_shell();
        shell.open_terminal("a", Program::DefaultShell);
        shell.open_terminal("b", Program::DefaultShell);
        shell.active_tab = 2;
        shell.close_tab(1);
        assert_eq!(shell.tabs[shell.active_tab].title(), "b");
    }

    #[test]
    fn idle_time_at_or_past_the_timeout_locks() {
        assert!(!idle_expired(899.0, 900.0));
        assert!(idle_expired(900.0, 900.0));
    }

    #[test]
    fn locking_drops_the_unlocked_hosts() {
        let mut shell = test_shell();
        shell.lock_vault();
        assert!(shell.hosts.is_none());
    }

    #[test]
    fn settings_is_last_so_it_sits_at_the_bottom() {
        assert_eq!(Section::ALL.last(), Some(&Section::Settings));
    }
}
