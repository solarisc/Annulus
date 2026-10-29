//! The window's layout (UI-SPEC §1–2): a top tab strip with the Vault tab first, the vault's
//! left navigation, and the screen for the chosen section. Session tabs show a terminal
//! once the terminal lands.

use eframe::egui::{self, Align, Button, Color32, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Appearance, Palette, ThemeMode, space, text};
use annulus_ui::{icons, theme};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    /// The vault: hosts, keychain and the rest. Always first, never closed.
    Vault,
    /// A terminal session to `target`.
    Session {
        /// What the session connects to, e.g. `admin@prod-db:22` or a local shell name.
        target: String,
    },
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
    /// Text of the search / quick-connect field on the Hosts screen.
    pub search: String,
    applied: Option<Appearance>,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            tabs: vec![Tab::Vault],
            active_tab: 0,
            section: Section::Hosts,
            theme_mode: ThemeMode::System,
            search: String::new(),
            applied: None,
        }
    }
}

impl Shell {
    /// Opens a session tab to `target` and brings it to the front.
    pub fn open_session(&mut self, target: impl Into<String>) {
        self.tabs.push(Tab::Session {
            target: target.into(),
        });
        self.active_tab = self.tabs.len() - 1;
    }

    /// Closes tab `index`; the vault tab cannot be closed. The tab to its left comes forward
    /// if the closed tab was in front.
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
        egui::Panel::top("tabs").show(ui, |ui| self.tab_strip(ui));
        if self.tabs.get(self.active_tab) == Some(&Tab::Vault) {
            egui::Panel::left("vault-nav")
                .resizable(false)
                .exact_size(200.0)
                .show(ui, |ui| self.navigation(ui));
            egui::CentralPanel::default().show(ui, |ui| self.section_screen(ui));
        } else {
            let target = match self.tabs.get(self.active_tab) {
                Some(Tab::Session { target }) => target.clone(),
                _ => String::new(),
            };
            egui::CentralPanel::default().show(ui, |ui| session_placeholder(ui, &target));
        }
    }

    fn tab_strip(&mut self, ui: &mut Ui) {
        let mut close = None;
        ui.horizontal(|ui| {
            for (index, tab) in self.tabs.iter().enumerate() {
                let title = match tab {
                    Tab::Vault => "Vault".to_owned(),
                    Tab::Session { target } => target.clone(),
                };
                let selected = index == self.active_tab;
                let response = ui.selectable_label(selected, title);
                if response.clicked() {
                    self.active_tab = index;
                }
                if index > 0
                    && (response.middle_clicked()
                        || ui
                            .small_button("\u{d7}")
                            .on_hover_text("Close tab")
                            .clicked())
                {
                    close = Some(index);
                }
            }
            if ui.button("+").on_hover_text("New local terminal").clicked() {
                self.open_session("Local shell");
            }
        });
        if let Some(index) = close {
            self.close_tab(index);
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
            Section::Hosts => self.hosts(ui),
            Section::Settings => self.settings(ui),
            other => empty_state(
                ui,
                other.title(),
                "Arrives with the vault.",
                &self.palette(),
            ),
        }
    }

    fn hosts(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search hosts or type user@host:port to connect")
                .desired_width(420.0);
            let response = ui.add(field);
            let submitted =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let connect = ui
                .add_enabled(!self.search.trim().is_empty(), Button::new("Connect"))
                .clicked();
            if (submitted || connect) && !self.search.trim().is_empty() {
                let target = self.search.trim().to_owned();
                self.search.clear();
                self.open_session(target);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let _ = ui
                    .add_enabled(false, Button::new("New host"))
                    .on_disabled_hover_text("Arrives with the vault.");
            });
        });
        ui.add_space(space::L);
        empty_state(
            ui,
            "No hosts yet",
            "Type user@host:port above to connect now. Saved hosts arrive with the vault.",
            &self.palette(),
        );
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

fn session_placeholder(ui: &mut Ui, target: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        ui.label(RichText::new(target).size(text::TITLE));
        ui.label(RichText::new("The terminal and SSH arrive later.").color(Color32::GRAY));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vault_tab_cannot_be_closed_and_closing_moves_focus_left() {
        let mut shell = Shell::default();
        shell.open_session("a");
        shell.open_session("b");
        shell.close_tab(0);
        assert_eq!(shell.tabs.len(), 3);
        shell.close_tab(2);
        assert_eq!((shell.tabs.len(), shell.active_tab), (2, 1));
        shell.close_tab(1);
        assert_eq!((shell.tabs, shell.active_tab), (vec![Tab::Vault], 0));
    }

    #[test]
    fn closing_a_background_tab_keeps_the_front_tab() {
        let mut shell = Shell::default();
        shell.open_session("a");
        shell.open_session("b");
        shell.active_tab = 2;
        shell.close_tab(1);
        assert_eq!(
            shell.tabs.get(shell.active_tab),
            Some(&Tab::Session { target: "b".into() })
        );
    }

    #[test]
    fn settings_is_last_so_it_sits_at_the_bottom() {
        assert_eq!(Section::ALL.last(), Some(&Section::Settings));
    }
}
