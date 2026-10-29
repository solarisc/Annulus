//! The Hosts screen (F-HOST-001, F-HOST-005, F-APP-006): saved hosts from the unlocked
//! vault, filtered by the search field, quick connect from the same field, and the host
//! editor in a right side panel.

use eframe::egui::{self, Align, Button, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::{Hop, Password, Program, SshFiles, Target, local_user};
use annulus_ui::secret::{SecretText, secret_field, secret_line};
use annulus_ui::{icons, theme};
use annulus_vault::hosts::{DEFAULT_PORT, Host, Secret};
use annulus_vault::vault::Vault;

use crate::import_dialog::ImportPreview;

/// Longest jump chain followed; longer ones are cut, so a mistake cannot hang a connect.
const MAX_JUMPS: usize = 8;

/// Ids and labels for a picker.
type Choices = Vec<(u64, String)>;

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
    /// The proxy URL, edited as text (it may hold a login).
    proxy: SecretText,
}

/// The Hosts screen's state, over an unlocked vault.
pub struct HostsScreen {
    vault: Vault,
    search: String,
    editor: Option<Draft>,
    ssh_files: SshFiles,
    import: Option<ImportPreview>,
    /// The result of the last import or export, shown under the toolbar.
    notice: Option<String>,
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
            import: None,
            notice: None,
        }
    }

    /// Annulus's SSH client for `target` text (`user@host:port`), or the system `ssh` if the
    /// text is not a target Annulus understands.
    fn ssh(&self, target: &str) -> Program {
        match Target::parse(target, &local_user()) {
            Some(target) => Program::Ssh {
                target,
                files: self.ssh_files.clone(),
                password: None,
                identity: None,
                via: Vec::new(),
                proxy: None,
                reconnect: true,
            },
            None => Program::system_ssh(target),
        }
    }

    /// How to log in to a saved host: its target, saved password and keychain key.
    fn hop(&self, host: &Host) -> Option<Hop> {
        Some(Hop {
            target: Target::parse(&host.target(), &local_user())?,
            password: host
                .password
                .as_ref()
                .map(|secret| Password::new(secret.expose())),
            identity: host
                .key_id
                .and_then(|id| self.vault.ssh_key(id))
                .map(|key| Password::new(key.private.expose())),
            proxy: host.proxy.as_ref().map(|url| Password::new(url.expose())),
        })
    }

    /// The jump hosts in front of `host`, outermost first. A loop or a missing host ends the
    /// chain where it is.
    fn jump_chain(&self, host: &Host) -> Vec<Hop> {
        let mut chain = Vec::new();
        let mut seen = vec![host.id];
        let mut next = host.jump_id;
        while let Some(id) = next {
            if seen.contains(&id) || chain.len() >= MAX_JUMPS {
                break;
            }
            seen.push(id);
            let Some(jump) = self
                .vault
                .hosts()
                .iter()
                .find(|candidate| candidate.id == id)
            else {
                break;
            };
            let Some(hop) = self.hop(jump) else { break };
            chain.push(hop);
            next = jump.jump_id;
        }
        chain.reverse();
        chain
    }

    /// The program that connects to a saved host: through its jump hosts, with its key and
    /// saved password.
    pub fn connect_host(&self, host: &Host) -> Program {
        match self.hop(host) {
            Some(hop) => Program::Ssh {
                target: hop.target,
                files: self.ssh_files.clone(),
                password: hop.password,
                identity: hop.identity,
                via: self.jump_chain(host),
                proxy: hop.proxy,
                reconnect: !host.no_reconnect,
            },
            None => Program::system_ssh(&host.target()),
        }
    }

    /// The import and export buttons for `ssh_config` (F-IMP-001, F-IMP-003).
    fn import_export_buttons(&mut self, ui: &mut Ui) {
        if ui
            .button("Copy as ssh_config")
            .on_hover_text("Hosts as an OpenSSH config, without passwords")
            .clicked()
        {
            ui.ctx()
                .copy_text(annulus_import::ssh_config::export(self.vault.hosts()));
            self.notice = Some("Copied the hosts as an OpenSSH config (no passwords).".into());
        }
        if ui.button("Import ssh_config").clicked() {
            match ImportPreview::load(&self.vault) {
                Ok(preview) => self.import = Some(preview),
                Err(message) => self.notice = Some(message),
            }
        }
    }

    /// The import preview window, and the last import or export result.
    fn import_preview(&mut self, ui: &mut Ui, palette: &Palette) {
        if let Some(preview) = &mut self.import
            && let Some(message) = preview.ui(ui.ctx(), &mut self.vault)
        {
            self.import = None;
            self.notice = (!message.is_empty()).then_some(message);
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(theme::color(palette.text_secondary)));
        }
    }

    /// Acts on a host row's button or menu choice; a connect becomes a request.
    fn row_action(&mut self, ui: &Ui, host: &Host, action: RowAction) -> Option<Request> {
        match action {
            RowAction::Connect => {
                return Some(Request::Open(host.label.clone(), self.connect_host(host)));
            }
            RowAction::Sftp => {
                return Some(Request::Sftp(host.label.clone(), self.connect_host(host)));
            }
            RowAction::CopyCommand => {
                let jump = host
                    .jump_id
                    .and_then(|id| self.vault.hosts().iter().find(|h| h.id == id));
                ui.ctx().copy_text(ssh_command(host, jump));
                self.notice = Some(format!("Copied the ssh command for {}.", host.label));
            }
            RowAction::Duplicate => {
                let mut copy = self.vault.draft_host();
                let id = copy.id;
                copy = Host {
                    id,
                    label: format!("{} copy", host.label),
                    ..host.clone()
                };
                self.notice = self
                    .vault
                    .upsert_host(copy)
                    .err()
                    .map(|error| format!("Could not duplicate: {error}"));
            }
            RowAction::Edit => {
                self.editor = Some(Draft {
                    port: host.port.map(|port| port.to_string()).unwrap_or_default(),
                    tags: host.tags.join(", "),
                    problem: None,
                    is_new: false,
                    password: SecretText::new(),
                    forget_password: false,
                    proxy: SecretText::from(host.proxy.as_ref().map_or("", |url| url.expose())),
                    host: host.clone(),
                });
            }
            RowAction::None => {}
        }
        None
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

    /// Swaps in another vault (after signing in to a sync account) and returns the old one.
    pub fn replace_vault(&mut self, vault: Vault) -> Vault {
        self.editor = None;
        std::mem::replace(&mut self.vault, vault)
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
                self.import_export_buttons(ui);
                if ui.button("New host").clicked() {
                    let host = self.vault.draft_host();
                    self.editor = Some(Draft {
                        port: host.port.map(|port| port.to_string()).unwrap_or_default(),
                        tags: String::new(),
                        problem: None,
                        is_new: true,
                        password: SecretText::new(),
                        forget_password: false,
                        proxy: SecretText::new(),
                        host,
                    });
                }
            });
        });
        self.import_preview(ui, palette);
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
                let action = host_row(ui, host, palette);
                if let Some(chosen) = self.row_action(ui, host, action) {
                    request = Some(chosen);
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

    /// The other hosts (as jump hosts) and the keychain keys, for the editor's pickers.
    fn pick_lists(&self) -> (Choices, Choices) {
        let editing = self.editor.as_ref().map(|draft| draft.host.id);
        let others = self
            .vault
            .hosts()
            .iter()
            .filter(|host| Some(host.id) != editing)
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let keys = self
            .vault
            .ssh_keys()
            .iter()
            .map(|key| (key.id, key.label.clone()))
            .collect();
        (others, keys)
    }

    fn editor_ui(&mut self, ui: &mut Ui, palette: &Palette) {
        let (others, keys) = self.pick_lists();
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
                ui.label("Key");
                key_picker(ui, &mut draft.host.key_id, &keys);
                ui.end_row();
                ui.label("Jump host");
                jump_picker(ui, &mut draft.host.jump_id, &others);
                ui.end_row();
                ui.label("Proxy");
                secret_line(
                    ui,
                    &mut draft.proxy,
                    "socks5://host:1080 or http://host:8080",
                );
                ui.end_row();
                reconnect_row(ui, &mut draft.host);
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
        // Empty inherits the group's port (or 22).
        draft.host.port = if port.is_empty() {
            None
        } else {
            Some(port.parse().unwrap_or(0))
        };
        if draft.forget_password {
            draft.host.password = None;
        }
        if !draft.password.is_empty() {
            draft.host.password = Some(Secret::new(draft.password.expose()));
        }
        let proxy = draft.proxy.expose().trim();
        if !proxy.is_empty() && annulus_ssh::proxy::Proxy::parse(proxy).is_none() {
            draft.problem =
                Some("The proxy must look like socks5://host:port or http://host:port.".into());
            return;
        }
        draft.host.proxy = (!proxy.is_empty()).then(|| Secret::new(proxy));
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

/// The host editor's choice of a saved host to connect through.
fn jump_picker(ui: &mut Ui, jump_id: &mut Option<u64>, hosts: &[(u64, String)]) {
    let chosen = jump_id
        .and_then(|id| hosts.iter().find(|(host, _)| *host == id))
        .map_or("None (direct)", |(_, label)| label.as_str());
    egui::ComboBox::from_id_salt("host-jump")
        .selected_text(chosen)
        .show_ui(ui, |ui| {
            ui.selectable_value(jump_id, None, "None (direct)");
            for (id, label) in hosts {
                ui.selectable_value(jump_id, Some(*id), label);
            }
        });
}

/// The host editor's choice of keychain key.
fn key_picker(ui: &mut Ui, key_id: &mut Option<u64>, keys: &[(u64, String)]) {
    let chosen = key_id
        .and_then(|id| keys.iter().find(|(key, _)| *key == id))
        .map_or("Default keys", |(_, label)| label.as_str());
    egui::ComboBox::from_id_salt("host-key")
        .selected_text(chosen)
        .show_ui(ui, |ui| {
            ui.selectable_value(key_id, None, "Default keys");
            for (id, label) in keys {
                ui.selectable_value(key_id, Some(*id), label);
            }
        });
}

#[derive(Clone, Copy)]
enum RowAction {
    None,
    Connect,
    Sftp,
    Edit,
    CopyCommand,
    Duplicate,
}

/// The editor row for the host's auto-reconnect switch (F-SSH-009).
fn reconnect_row(ui: &mut Ui, host: &mut Host) {
    ui.label("Reconnect");
    let mut reconnect = !host.no_reconnect;
    ui.checkbox(&mut reconnect, "Reconnect when the connection drops");
    host.no_reconnect = !reconnect;
    ui.end_row();
}

/// An OpenSSH command line that reaches `host` (through `jump`, if it has one), for pasting
/// into another terminal (F-HOST-010). Saved passwords are never part of it.
fn ssh_command(host: &Host, jump: Option<&Host>) -> String {
    let mut parts = vec!["ssh".to_owned()];
    if host.port() != DEFAULT_PORT {
        parts.push(format!("-p {}", host.port()));
    }
    if let Some(jump) = jump {
        parts.push(format!("-J {}", jump.target()));
    }
    let (user, address) = (host.username.trim(), host.address.trim());
    parts.push(if user.is_empty() {
        address.to_owned()
    } else {
        format!("{user}@{address}")
    });
    parts.join(" ")
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
    let more_rect = egui::Rect::from_min_size(
        sftp_rect.min - egui::vec2(36.0, 0.0),
        egui::vec2(28.0, 28.0),
    );
    let mut chosen = RowAction::None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(more_rect), |ui| {
        ui.menu_button("\u{22ef}", |ui| {
            if ui.button("Copy ssh command").clicked() {
                chosen = RowAction::CopyCommand;
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                chosen = RowAction::Duplicate;
                ui.close();
            }
        });
    });
    if !matches!(chosen, RowAction::None) {
        return chosen;
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
        host.port = Some(2222);
        vault.upsert_host(host).unwrap();
        HostsScreen::new(
            vault,
            SshFiles {
                known_hosts: dir.join("known_hosts"),
                key_dir: None,
            },
        )
    }

    #[test]
    fn a_host_can_turn_off_reconnecting() {
        let mut screen = screen_with("rc");
        let mut host = screen.vault.hosts()[0].clone();
        let reconnects = |screen: &HostsScreen, host: &Host| {
            matches!(
                screen.connect_host(host),
                Program::Ssh {
                    reconnect: true,
                    ..
                }
            )
        };
        assert!(reconnects(&screen, &host), "on by default");
        host.no_reconnect = true;
        screen.vault.upsert_host(host.clone()).unwrap();
        assert!(!reconnects(&screen, &host));
    }

    #[test]
    fn the_ssh_command_has_port_jump_and_user_but_never_a_password() {
        let mut host = screen_with("cmd").vault.hosts()[0].clone();
        host.password = Some(Secret::new("hunter2"));
        let mut jump = host.clone();
        jump.address = "bastion.example.com".into();
        jump.port = Some(22);
        let command = ssh_command(&host, Some(&jump));
        assert_eq!(
            command,
            "ssh -p 2222 -J admin@bastion.example.com admin@10.0.4.12"
        );
        assert!(!command.contains("hunter2"));
        host.username.clear();
        host.port = Some(22);
        assert_eq!(ssh_command(&host, None), "ssh 10.0.4.12");
    }

    #[test]
    fn a_jump_chain_runs_outermost_first_and_stops_at_a_loop() {
        let mut screen = screen_with("inner");
        let mut add = |label: &str| {
            let mut host = screen.vault.draft_host();
            host.label = label.into();
            host.address = format!("{label}.example.com");
            screen.vault.upsert_host(host.clone()).unwrap();
            host.id
        };
        let middle = add("middle");
        let outer = add("outer");
        let set_jump = |screen: &mut HostsScreen, id: u64, jump: u64| {
            let mut host = screen
                .vault
                .hosts()
                .iter()
                .find(|h| h.id == id)
                .unwrap()
                .clone();
            host.jump_id = Some(jump);
            screen.vault.upsert_host(host).unwrap();
        };
        let inner = screen
            .vault
            .hosts()
            .iter()
            .find(|h| h.label == "inner")
            .unwrap()
            .id;
        set_jump(&mut screen, inner, middle);
        set_jump(&mut screen, middle, outer);
        let host = screen
            .vault
            .hosts()
            .iter()
            .find(|h| h.id == inner)
            .unwrap()
            .clone();
        let hops: Vec<String> = screen
            .jump_chain(&host)
            .into_iter()
            .map(|hop| hop.target.host)
            .collect();
        assert_eq!(hops, ["outer.example.com", "middle.example.com"]);

        // outer → inner closes a loop: the chain stops instead of going round.
        set_jump(&mut screen, outer, inner);
        let host = screen
            .vault
            .hosts()
            .iter()
            .find(|h| h.id == inner)
            .unwrap()
            .clone();
        assert_eq!(screen.jump_chain(&host).len(), 2);
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
