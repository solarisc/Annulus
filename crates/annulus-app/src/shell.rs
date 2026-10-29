//! The window's layout (UI-SPEC §1–2): a top tab strip with the Vault tab first, the vault's
//! left navigation and the screen for the chosen section, and terminal tabs.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Appearance, Palette, ThemeMode, space, text};
use annulus_session::Program;
use annulus_ui::{icons, theme};

use crate::hosts_screen::{HostsScreen, Request};
use crate::terminal_view::TerminalTab;

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
    /// A terminal: a local shell or an SSH session.
    Terminal(TerminalTab),
}

impl Tab {
    /// The title shown in the strip.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Tab::Vault => "Vault",
            Tab::Terminal(terminal) => &terminal.title,
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
    /// The Hosts screen.
    pub hosts: HostsScreen,
    applied: Option<Appearance>,
}

impl Shell {
    /// A window whose saved data lives in `data_dir`.
    #[must_use]
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            tabs: vec![Tab::Vault],
            active_tab: 0,
            section: Section::Hosts,
            theme_mode: ThemeMode::System,
            hosts: HostsScreen::open(data_dir.join("hosts.json")),
            applied: None,
        }
    }
}

impl Shell {
    /// Opens a terminal tab titled `title` running `program`, in front.
    pub fn open_terminal(&mut self, title: impl Into<String>, program: Program) {
        self.tabs
            .push(Tab::Terminal(TerminalTab::new(title, program)));
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
        self.apply_theme(&ui.ctx().clone());
        self.shortcuts(ui);
        egui::Panel::top("tabs").show(ui, |ui| self.tab_strip(ui));
        if matches!(self.tabs.get(self.active_tab), Some(Tab::Vault)) {
            egui::Panel::left("vault-nav")
                .resizable(false)
                .exact_size(200.0)
                .show(ui, |ui| self.navigation(ui));
            egui::CentralPanel::default().show(ui, |ui| self.section_screen(ui));
        } else if let Some(Tab::Terminal(terminal)) = self.tabs.get_mut(self.active_tab) {
            let frame = egui::Frame::NONE.inner_margin(egui::Margin::same(4));
            egui::CentralPanel::default()
                .frame(frame)
                .show(ui, |ui| terminal.ui(ui));
        }
    }

    /// App shortcuts use Ctrl+Shift so plain Ctrl+letter stays with the shell (UI-SPEC §7).
    fn shortcuts(&mut self, ui: &Ui) {
        let command = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let (new_tab, close_tab, next_tab) = ui.input_mut(|input| {
            (
                input.consume_key(command, egui::Key::T),
                input.consume_key(command, egui::Key::W),
                input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
            )
        });
        if new_tab {
            self.open_terminal("Local", Program::DefaultShell);
        }
        if close_tab {
            self.close_tab(self.active_tab);
        }
        if next_tab {
            self.active_tab = (self.active_tab + 1) % self.tabs.len().max(1);
        }
    }

    fn tab_strip(&mut self, ui: &mut Ui) {
        let mut close = None;
        let mut select = None;
        let mut new_local = false;
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
                .clicked();
        });
        if let Some(index) = select {
            self.active_tab = index;
        }
        if let Some(index) = close {
            self.close_tab(index);
        }
        if new_local {
            self.open_terminal("Local", Program::DefaultShell);
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
                if let Some(Request::Open(title, program)) = self.hosts.ui(ui, &palette) {
                    self.open_terminal(title, program);
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
    }
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
    fn settings_is_last_so_it_sits_at_the_bottom() {
        assert_eq!(Section::ALL.last(), Some(&Section::Settings));
    }
}
