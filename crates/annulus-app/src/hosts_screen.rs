//! The Hosts screen (F-HOST-001, F-HOST-005, F-APP-006): saved hosts from the unlocked
//! vault, filtered by the search field, quick connect from the same field, and the host
//! editor in a right side panel.

use eframe::egui::{self, Align, Button, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::{Password, Program, SshFiles, Target, local_user};
use annulus_ui::secret::{SecretText, secret_field};
use annulus_ui::{icons, theme};
use annulus_vault::hosts::{DEFAULT_PORT, Host, Secret};
use annulus_vault::vault::Vault;

/// Something the screen asks the shell to do.
pub enum Request {
    /// Open a terminal tab titled `.0` running `.1`.
    Open(String, Program),
    /// Open an SFTP tab titled `.0` for the SSH connection in `.1`.
    Sftp(String, Program),
}

/// The editor's working copy; fields are text until saved.
struct Draft {
    host: Host,
    port: String,
    tags: String,
    problem: Option<String>,
    is_new: bool,
    /// A new password to save; empty keeps the saved one.
    password: SecretText,
    /// Remove the saved password on save.
    forget_password: bool,
}

/// The Hosts screen's state, over an unlocked vault.
pub struct HostsScreen {
    vault: Vault,
    search: String,
    editor: Option<Draft>,
    ssh_files: SshFiles,
}

impl HostsScreen {
    /// The screen for `vault`; connections use `ssh_files`.
    #[must_use]
    pub fn new(vault: Vault, ssh_files: SshFiles) -> Self {
        Self {
            vault,
            search: String::new(),
            editor: None,
            ssh_files,
        }
    }

    /// Annulus's SSH client for `target` text (`user@host:port`), or the system `ssh` if the
    /// text is not a target Annulus understands.
    fn ssh(&self, target: &str) -> Program {
        self.ssh_with(target, None)
    }

    /// As [`Self::ssh`], trying `password` before asking.
    fn ssh_with(&self, target: &str, password: Option<&Secret>) -> Program {
        match Target::parse(target, &local_user()) {
            Some(target) => Program::Ssh {
                target,
                files: self.ssh_files.clone(),
                password: password.map(|secret| Password::new(secret.expose())),
            },
            None => Program::system_ssh(target),
        }
    }

    /// The program that connects to a saved host, with its saved password.
    pub fn connect_host(&self, host: &Host) -> Program {
        self.ssh_with(&host.target(), host.password.as_ref())
    }

    /// The unlocked vault.
    #[must_use]
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// The unlocked vault, for screens that save other records in it.
    pub fn vault_mut(&mut self) -> &mut Vault {
        &mut self.vault
    }

    /// Draws the screen; returns what the shell should do.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Request> {
        let mut request = None;
        if self.editor.is_some() {
            egui::Panel::right("host-editor")
                .resizable(false)
                .exact_size(340.0)
                .show(ui, |ui| self.editor_ui(ui, palette));
        }
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search hosts, or type user@host:port to connect")
                .desired_width(420.0);
            let response = ui.add(field);
            let submitted =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let query = self.search.trim().to_owned();
            let connect = ui
                .add_enabled(!query.is_empty(), Button::new("Connect"))
                .clicked();
            if (submitted || connect) && !query.is_empty() {
                request = Some(self.quick_connect(&query));
                self.search.clear();
            }
            if ui.button("Local terminal").clicked() {
                request = Some(Request::Open("Local".into(), Program::DefaultShell));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("New host").clicked() {
                    let host = self.vault.draft_host();
                    self.editor = Some(Draft {
                        port: host.port.to_string(),
                        tags: String::new(),
                        problem: None,
                        is_new: true,
                        password: SecretText::new(),
                        forget_password: false,
                        host,
                    });
                }
            });
        });
        ui.add_space(space::M);

        let matches: Vec<Host> = self.vault.search(&self.search).cloned().collect();
        if self.vault.hosts().is_empty() {
            empty(
                ui,
                palette,
                "No hosts yet",
                "Add one with New host, or type user@host:port above to connect now.",
            );
            return request;
        }
        if matches.is_empty() {
            empty(
                ui,
                palette,
                "No matching hosts",
                "Press Enter to connect to what you typed.",
            );
            return request;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for host in &matches {
                match host_row(ui, host, palette) {
                    RowAction::Connect => {
                        request = Some(Request::Open(host.label.clone(), self.connect_host(host)));
                    }
                    RowAction::Sftp => {
                        request = Some(Request::Sftp(host.label.clone(), self.connect_host(host)));
                    }
                    RowAction::Edit => {
                        self.editor = Some(Draft {
                            port: host.port.to_string(),
                            tags: host.tags.join(", "),
                            problem: None,
                            is_new: false,
                            password: SecretText::new(),
                            forget_password: false,
                            host: host.clone(),
                        });
                    }
                    RowAction::None => {}
                }
            }
        });
        request
    }

    /// A query that names a saved host (by label, case-insensitively) connects to it;
    /// anything else is an SSH target.
    fn quick_connect(&self, query: &str) -> Request {
        if let Some(host) = self
            .vault
            .hosts()
            .iter()
            .find(|host| host.label.eq_ignore_ascii_case(query))
        {
            return Request::Open(host.label.clone(), self.connect_host(host));
        }
        Request::Open(query.to_owned(), self.ssh(query))
    }

    fn editor_ui(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(draft) = &mut self.editor else {
            return;
        };
        ui.add_space(space::M);
        ui.heading(if draft.is_new {
            "New host"
        } else {
            "Edit host"
        });
        ui.add_space(space::M);
        egui::Grid::new("host-fields")
            .num_columns(2)
            .spacing([space::M, space::S])
            .show(ui, |ui| {
                ui.label("Label");
                ui.add(egui::TextEdit::singleline(&mut draft.host.label).hint_text("prod-db"));
                ui.end_row();
                ui.label("Address");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.host.address)
                        .hint_text("10.0.4.12 or db.example.com"),
                );
                ui.end_row();
                ui.label("Port");
                ui.add(egui::TextEdit::singleline(&mut draft.port).desired_width(80.0));
                ui.end_row();
                ui.label("Username");
                ui.add(egui::TextEdit::singleline(&mut draft.host.username).hint_text("admin"));
                ui.end_row();
                ui.label("Tags");
                ui.add(egui::TextEdit::singleline(&mut draft.tags).hint_text("postgres, prod"));
                ui.end_row();
                ui.label("Password");
                let saved = draft.host.password.is_some() && !draft.forget_password;
                secret_field(
                    ui,
                    &mut draft.password,
                    if saved {
                        "Saved (type to replace)"
                    } else {
                        "Optional"
                    },
                );
                ui.end_row();
                if saved {
                    ui.label("");
                    if ui.small_button("Forget saved password").clicked() {
                        draft.forget_password = true;
                    }
                    ui.end_row();
                }
            });
        if let Some(problem) = &draft.problem {
            ui.add_space(space::S);
            ui.colored_label(theme::color(palette.status_error), problem);
        }
        ui.add_space(space::M);
        let (mut save, mut delete, mut cancel) = (false, false, false);
        ui.horizontal(|ui| {
            save = ui.button("Save").clicked();
            cancel = ui.button("Cancel").clicked();
            if !draft.is_new {
                delete = ui
                    .button(RichText::new("Delete").color(theme::color(palette.status_error)))
                    .clicked();
            }
        });
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if cancel || escape {
            self.editor = None;
        } else if delete {
            let id = draft.host.id;
            match self.vault.remove_host(id) {
                Ok(()) => self.editor = None,
                Err(error) => {
                    if let Some(draft) = &mut self.editor {
                        draft.problem = Some(format!("Could not delete: {error}"));
                    }
                }
            }
        } else if save {
            self.save();
        }
    }

    fn save(&mut self) {
        let Some(draft) = &mut self.editor else {
            return;
        };
        let port = draft.port.trim();
        draft.host.port = if port.is_empty() {
            DEFAULT_PORT
        } else {
            port.parse().unwrap_or(0)
        };
        if draft.forget_password {
            draft.host.password = None;
        }
        if !draft.password.is_empty() {
            draft.host.password = Some(Secret::new(draft.password.expose()));
        }
        draft.host.tags = draft
            .tags
            .split(',')
            .map(|tag| tag.trim().to_owned())
            .filter(|tag| !tag.is_empty())
            .collect();
        if draft.host.label.trim().is_empty() {
            draft.host.label = draft.host.address.trim().to_owned();
        }
        if let Some(problem) = draft.host.problem() {
            draft.problem = Some(problem.to_owned());
            return;
        }
        let host = draft.host.clone();
        match self.vault.upsert_host(host) {
            Ok(()) => self.editor = None,
            Err(error) => {
                if let Some(draft) = &mut self.editor {
                    draft.problem = Some(format!("Could not save: {error}"));
                }
            }
        }
    }
}

enum RowAction {
    None,
    Connect,
    Sftp,
    Edit,
}

fn host_row(ui: &mut Ui, host: &Host, palette: &Palette) -> RowAction {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("Connect to {}", host.label),
        )
    });
    let painter = ui.painter();
    if response.hovered() || response.has_focus() {
        painter.rect_filled(rect, 6.0, theme::color(palette.bg_hover));
    }
    let icon = egui::Rect::from_min_size(
        rect.min + egui::vec2(space::M, 14.0),
        egui::vec2(16.0, 16.0),
    );
    icons::paint(painter, icons::SERVER, icon, theme::color(palette.accent));
    let left = rect.min.x + space::M + 28.0;
    painter.text(
        egui::pos2(left, rect.min.y + 13.0),
        egui::Align2::LEFT_CENTER,
        &host.label,
        egui::FontId::proportional(text::BODY + 1.0),
        theme::color(palette.text_primary),
    );
    let detail = if host.tags.is_empty() {
        host.target()
    } else {
        format!("{}  \u{b7}  {}", host.target(), host.tags.join(", "))
    };
    painter.text(
        egui::pos2(left, rect.min.y + 31.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(text::SECONDARY),
        theme::color(palette.text_secondary),
    );
    let edit_rect = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - 70.0, rect.min.y + 8.0),
        egui::vec2(60.0, 28.0),
    );
    if ui.put(edit_rect, Button::new("Edit")).clicked() {
        return RowAction::Edit;
    }
    let sftp_rect = edit_rect.translate(egui::vec2(-68.0, 0.0));
    if ui
        .put(sftp_rect, Button::new("SFTP"))
        .on_hover_text("Browse this host's files")
        .clicked()
    {
        return RowAction::Sftp;
    }
    if response.clicked()
        || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
    {
        return RowAction::Connect;
    }
    RowAction::None
}

fn empty(ui: &mut Ui, palette: &Palette, title: &str, detail: &str) {
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
    use annulus_vault::crypto::KdfParams;

    use super::*;

    fn screen_with(label: &str) -> HostsScreen {
        let dir =
            std::env::temp_dir().join(format!("annulus-app-test-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cheap = KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        let (mut vault, _) = Vault::create(dir.join("vault.anv"), "pw", cheap).unwrap();
        let mut host = vault.draft_host();
        host.label = label.into();
        host.address = "10.0.4.12".into();
        host.username = "admin".into();
        host.port = 2222;
        vault.upsert_host(host).unwrap();
        HostsScreen::new(
            vault,
            SshFiles {
                known_hosts: dir.join("known_hosts"),
                key_dir: None,
            },
        )
    }

    fn opened(request: &Request) -> (&str, String) {
        match request {
            Request::Open(title, Program::Ssh { target, .. }) => (
                title.as_str(),
                format!("{}@{}:{}", target.user, target.host, target.port),
            ),
            Request::Open(title, _) | Request::Sftp(title, _) => (title.as_str(), String::new()),
        }
    }

    #[test]
    fn quick_connect_to_a_saved_label_uses_its_details() {
        let screen = screen_with("prod-db");
        assert_eq!(
            opened(&screen.quick_connect("PROD-DB")),
            ("prod-db", "admin@10.0.4.12:2222".to_owned())
        );
    }

    #[test]
    fn a_saved_password_goes_with_the_saved_host_only() {
        let mut screen = screen_with("db-pw");
        let mut host = screen.vault.hosts()[0].clone();
        host.password = Some(Secret::new("hunter2"));
        screen.vault.upsert_host(host).unwrap();
        let password_of = |request: Request| match request {
            Request::Open(_, Program::Ssh { password, .. }) => password,
            Request::Open(..) | Request::Sftp(..) => None,
        };
        assert_eq!(
            password_of(screen.quick_connect("db-pw")),
            Some(Password::new("hunter2"))
        );
        assert_eq!(
            password_of(screen.quick_connect("admin@10.0.4.12:2222")),
            None
        );
    }

    #[test]
    fn quick_connect_to_anything_else_is_an_ssh_target() {
        let screen = screen_with("prod-db2");
        assert_eq!(
            opened(&screen.quick_connect("root@other:22")),
            ("root@other:22", "root@other:22".to_owned())
        );
    }
}
