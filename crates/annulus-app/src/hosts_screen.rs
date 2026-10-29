//! The Hosts screen (F-HOST-001, F-HOST-003, F-HOST-005, F-APP-006): saved hosts from the
//! unlocked vault, browsed group by group or filtered by the search field, quick connect from
//! the same field, and the host and group editors in a right side panel. Connections use the
//! resolved host: what it sets itself, filled in from its groups.

use eframe::egui::{self, Align, Button, Layout, RichText, Sense, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::{Hop, Password, Program, SshFiles, Target, local_user};
use annulus_ui::secret::{SecretText, secret_field, secret_line};
use annulus_ui::{icons, theme};
use annulus_vault::hosts::{DEFAULT_PORT, Host, Secret};
use annulus_vault::vault::Vault;

use crate::groups_view::{self, Crumb, GroupAction, GroupDraft};
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
    /// Open a terminal tab for each `(title, program)` (a group's Connect all).
    OpenAll(Vec<(String, Program)>),
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
    /// Environment variables, one `NAME=value` per line.
    env: String,
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
    /// The group being browsed (its uid); `None` is the top level.
    group: Option<String>,
    group_editor: Option<GroupDraft>,
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
            group: None,
            group_editor: None,
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
                env: Vec::new(),
                startup: None,
            },
            None => Program::system_ssh(target),
        }
    }

    /// How to log in to a saved host: its target, saved password and keychain key, with
    /// what it inherits from its groups.
    fn hop(&self, host: &Host) -> Option<Hop> {
        let (host, _) = self.vault.resolve(host);
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

    /// The jump hosts in front of `host`, outermost first, each as its groups resolve it. A
    /// loop or a missing host ends the chain where it is.
    fn jump_chain(&self, host: &Host) -> Vec<Hop> {
        let mut chain = Vec::new();
        let mut seen = vec![host.id];
        let mut next = self.vault.resolve(host).0.jump_id;
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
            next = self.vault.resolve(jump).0.jump_id;
        }
        chain.reverse();
        chain
    }

    /// The program that connects to a saved host: through its jump hosts, with its key and
    /// saved password.
    pub fn connect_host(&self, host: &Host) -> Program {
        let (resolved, _) = self.vault.resolve(host);
        match self.hop(host) {
            Some(hop) => Program::Ssh {
                target: hop.target,
                files: self.ssh_files.clone(),
                password: hop.password,
                identity: hop.identity,
                via: self.jump_chain(host),
                proxy: hop.proxy,
                reconnect: !host.no_reconnect,
                env: resolved.env,
                startup: Some(resolved.startup).filter(|command| !command.trim().is_empty()),
            },
            None => Program::system_ssh(&self.vault.resolve(host).0.target()),
        }
    }

    /// The import and export buttons for `ssh_config` (F-IMP-001, F-IMP-003).
    fn import_export_buttons(&mut self, ui: &mut Ui) {
        if ui
            .button("Copy as ssh_config")
            .on_hover_text("Hosts as an OpenSSH config, without passwords")
            .clicked()
        {
            let resolved: Vec<Host> = self
                .vault
                .hosts()
                .iter()
                .map(|host| self.vault.resolve(host).0)
                .collect();
            ui.ctx()
                .copy_text(annulus_import::ssh_config::export(&resolved));
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

    /// Acts on a host row's button or menu choice; a connect becomes a request. `groups` is
    /// the list the row's "Move to group" menu showed.
    fn row_action(
        &mut self,
        ui: &Ui,
        host: &Host,
        action: RowAction,
        groups: &[(String, String)],
    ) -> Option<Request> {
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
                    .and_then(|id| self.vault.hosts().iter().find(|h| h.id == id))
                    .map(|jump| self.vault.resolve(jump).0);
                let resolved = self.vault.resolve(host).0;
                ui.ctx().copy_text(ssh_command(&resolved, jump.as_ref()));
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
                self.open_editor(host.clone(), false);
            }
            RowAction::MoveTo(index) => {
                let mut moved = host.clone();
                moved.group = index
                    .and_then(|index| groups.get(index))
                    .map(|(uid, _)| uid.clone());
                self.notice = self
                    .vault
                    .upsert_host(moved)
                    .err()
                    .map(|error| format!("Could not move: {error}"));
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
        self.group_editor = None;
        self.group = None;
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
        } else if let Some(draft) = &mut self.group_editor {
            let closed = egui::Panel::right("group-editor")
                .resizable(false)
                .exact_size(340.0)
                .show(ui, |ui| draft.ui(ui, palette, &mut self.vault))
                .inner;
            if closed {
                self.group_editor = None;
            }
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
            if let Some(host) = self.unsaved_target(&query)
                && ui
                    .button("Save as host")
                    .on_hover_text("Open the host editor with this address filled in")
                    .clicked()
            {
                self.open_editor(host, true);
                self.search.clear();
            }
            if ui.button("Local terminal").clicked() {
                request = Some(Request::Open("Local".into(), Program::DefaultShell));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.import_export_buttons(ui);
                self.new_buttons(ui);
            });
        });
        self.import_preview(ui, palette);
        ui.add_space(space::M);

        if self.search.trim().is_empty() {
            match groups_view::breadcrumb(ui, self.vault.groups(), self.group.as_deref()) {
                Some(Crumb::Top) => self.group = None,
                Some(Crumb::Group(uid)) => self.group = Some(uid),
                None => {}
            }
            if let Some(chosen) = self.browse(ui, palette) {
                request = Some(chosen);
            }
            return request;
        }
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
        let targets = groups_view::choices(self.vault.groups(), None);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for host in &matches {
                let shown = self.vault.resolve(host).0;
                let action = host_row(ui, &shown, palette, &targets);
                if let Some(chosen) = self.row_action(ui, host, action, &targets) {
                    request = Some(chosen);
                }
            }
        });
        request
    }

    /// "New group" and "New host": both start inside the group being browsed.
    fn new_buttons(&mut self, ui: &mut Ui) {
        if ui.button("New group").clicked() {
            match self.vault.draft_group(self.group.clone()) {
                Ok(group) => {
                    self.editor = None;
                    self.group_editor = Some(GroupDraft::new(group, true));
                }
                Err(error) => self.notice = Some(error.to_string()),
            }
        }
        if ui.button("New host").clicked() {
            let mut host = self.vault.draft_host();
            host.group.clone_from(&self.group);
            self.open_editor(host, true);
        }
    }

    /// What the search field holds as a new host, when it is an SSH target (`user@host:port`)
    /// that no saved host has the label of (F-APP-006).
    fn unsaved_target(&self, query: &str) -> Option<Host> {
        if !query.contains(['@', ':', '.']) || self.vault.hosts().iter().any(|h| h.label == query) {
            return None;
        }
        let target = Target::parse(query, "")?;
        let mut host = self.vault.draft_host();
        host.label.clone_from(&target.host);
        host.address = target.host;
        host.port = (target.port != DEFAULT_PORT).then_some(target.port);
        host.username = target.user;
        host.group.clone_from(&self.group);
        Some(host)
    }

    /// Opens the host editor on `host`.
    fn open_editor(&mut self, host: Host, is_new: bool) {
        self.group_editor = None;
        self.editor = Some(Draft {
            port: host.port.map(|port| port.to_string()).unwrap_or_default(),
            tags: host.tags.join(", "),
            problem: None,
            is_new,
            password: SecretText::new(),
            forget_password: false,
            proxy: SecretText::from(host.proxy.as_ref().map_or("", |url| url.expose())),
            env: groups_view::format_env(&host.env),
            host,
        });
    }

    /// The groups and hosts inside the group being browsed (a host whose group is gone shows
    /// at the top level). Returns a connect request.
    fn browse(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Request> {
        let groups = self.vault.groups().to_vec();
        // A group deleted (here or by sync) while open: go back to the top.
        if self
            .group
            .as_deref()
            .is_some_and(|uid| !groups.iter().any(|group| group.uid == uid))
        {
            self.group = None;
        }
        let current = self.group.clone();
        let in_here = |uid: Option<&str>| {
            let known = uid.filter(|uid| groups.iter().any(|group| group.uid == *uid));
            known == current.as_deref()
        };
        let subgroups: Vec<_> = groups
            .iter()
            .filter(|group| in_here(group.parent.as_deref()))
            .cloned()
            .collect();
        let hosts: Vec<Host> = self
            .vault
            .hosts()
            .iter()
            .filter(|host| in_here(host.group.as_deref()))
            .cloned()
            .collect();
        if subgroups.is_empty() && hosts.is_empty() {
            let (title, detail) = if current.is_none() {
                (
                    "No hosts yet",
                    "Add one with New host, or type user@host:port above to connect now.",
                )
            } else {
                (
                    "This group is empty",
                    "Add a host or a group to it with the buttons above.",
                )
            };
            empty(ui, palette, title, detail);
            return None;
        }
        let mut request = None;
        let targets = groups_view::choices(&groups, None);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for group in &subgroups {
                let count = |uid: Option<&str>| uid == Some(group.uid.as_str());
                let holds = groups_view::holds(
                    self.vault
                        .hosts()
                        .iter()
                        .filter(|h| count(h.group.as_deref()))
                        .count(),
                    groups.iter().filter(|g| count(g.parent.as_deref())).count(),
                );
                match groups_view::row(ui, group, &holds, palette) {
                    GroupAction::Open => self.group = Some(group.uid.clone()),
                    GroupAction::ConnectAll => request = Some(self.connect_all(&group.uid)),
                    GroupAction::Edit => {
                        self.editor = None;
                        self.group_editor = Some(GroupDraft::new(group.clone(), false));
                    }
                    GroupAction::None => {}
                }
            }
            for host in &hosts {
                let shown = self.vault.resolve(host).0;
                let action = host_row(ui, &shown, palette, &targets);
                if let Some(chosen) = self.row_action(ui, host, action, &targets) {
                    request = Some(chosen);
                }
            }
        });
        request
    }

    /// A tab for every host directly in group `uid` (F-HOST-003 "Connect all").
    fn connect_all(&self, uid: &str) -> Request {
        Request::OpenAll(
            self.vault
                .hosts()
                .iter()
                .filter(|host| host.group.as_deref() == Some(uid))
                .map(|host| (host.label.clone(), self.connect_host(host)))
                .collect(),
        )
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
        let group_choices = groups_view::choices(self.vault.groups(), None);
        let inherited = self
            .editor
            .as_ref()
            .map(|draft| inherited_hints(&self.vault, &draft.host, &keys, &others))
            .unwrap_or_default();
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
                ui.label("Group");
                groups_view::picker(ui, "host-group", &mut draft.host.group, &group_choices);
                ui.end_row();
                inheritable_rows(ui, draft, &inherited, &keys, &others);
                groups_view::session_rows(
                    ui,
                    &mut draft.env,
                    &mut draft.host.startup,
                    (&inherited.env, &inherited.startup),
                );
                reconnect_row(ui, &mut draft.host);
                password_rows(ui, draft);
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
        match groups_view::parse_env(&draft.env) {
            Ok(env) => draft.host.env = env,
            Err(problem) => {
                draft.problem = Some(problem);
                return;
            }
        }
        draft.host.startup = draft.host.startup.trim().to_owned();
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

/// A "↺" after a field the host sets itself while its group would set it too: a click
/// clears the host's own value, so the group's applies again (UI-SPEC §2).
fn reset_button(ui: &mut Ui, own: bool, group: Option<&String>) -> bool {
    let Some(group) = group.filter(|_| own) else {
        return false;
    };
    ui.small_button("\u{21ba}")
        .on_hover_text(format!("Use the value from {group}"))
        .clicked()
}

/// The editor rows a group can fill in: port, user, tags, key, jump host and proxy, each
/// with its inherited value as the hint and a "↺" to go back to it.
fn inheritable_rows(
    ui: &mut Ui,
    draft: &mut Draft,
    inherited: &Hints,
    keys: &[(u64, String)],
    others: &[(u64, String)],
) {
    let from = &inherited.groups;
    ui.label("Port");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.port)
                .hint_text(inherited.port.as_str())
                .desired_width(160.0),
        );
        if reset_button(ui, !draft.port.trim().is_empty(), from.port.as_ref()) {
            draft.port.clear();
        }
    });
    ui.end_row();
    ui.label("Username");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.host.username)
                .hint_text(inherited.username.as_str()),
        );
        let own = !draft.host.username.trim().is_empty();
        if reset_button(ui, own, from.username.as_ref()) {
            draft.host.username.clear();
        }
    });
    ui.end_row();
    ui.label("Tags");
    ui.add(egui::TextEdit::singleline(&mut draft.tags).hint_text("postgres, prod"));
    ui.end_row();
    ui.label("Key");
    ui.horizontal(|ui| {
        key_picker(ui, &mut draft.host.key_id, keys, &inherited.key);
        if reset_button(ui, draft.host.key_id.is_some(), from.key_id.as_ref()) {
            draft.host.key_id = None;
        }
    });
    ui.end_row();
    ui.label("Jump host");
    ui.horizontal(|ui| {
        jump_picker(ui, &mut draft.host.jump_id, others, &inherited.jump);
        if reset_button(ui, draft.host.jump_id.is_some(), from.jump_id.as_ref()) {
            draft.host.jump_id = None;
        }
    });
    ui.end_row();
    ui.label("Proxy");
    ui.horizontal(|ui| {
        secret_line(ui, &mut draft.proxy, &inherited.proxy);
        if reset_button(ui, !draft.proxy.is_empty(), from.proxy.as_ref()) {
            draft.proxy = SecretText::new();
        }
    });
    ui.end_row();
}

/// The editor rows for the saved password: a new one, or forgetting the saved one.
fn password_rows(ui: &mut Ui, draft: &mut Draft) {
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
}

/// The host editor's choice of a saved host to connect through; `unset` names what "no jump
/// host of its own" means (the group's, or direct).
fn jump_picker(ui: &mut Ui, jump_id: &mut Option<u64>, hosts: &[(u64, String)], unset: &str) {
    let chosen = jump_id
        .and_then(|id| hosts.iter().find(|(host, _)| *host == id))
        .map_or(unset, |(_, label)| label.as_str());
    egui::ComboBox::from_id_salt("host-jump")
        .selected_text(chosen)
        .show_ui(ui, |ui| {
            ui.selectable_value(jump_id, None, unset);
            for (id, label) in hosts {
                ui.selectable_value(jump_id, Some(*id), label);
            }
        });
}

/// The host editor's choice of keychain key; `unset` names what "no key of its own" means
/// (the group's key, or the default keys).
fn key_picker(ui: &mut Ui, key_id: &mut Option<u64>, keys: &[(u64, String)], unset: &str) {
    let chosen = key_id
        .and_then(|id| keys.iter().find(|(key, _)| *key == id))
        .map_or(unset, |(_, label)| label.as_str());
    egui::ComboBox::from_id_salt("host-key")
        .selected_text(chosen)
        .show_ui(ui, |ui| {
            ui.selectable_value(key_id, None, unset);
            for (id, label) in keys {
                ui.selectable_value(key_id, Some(*id), label);
            }
        });
}

/// What the host editor shows in a field the host leaves unset: the inherited value and the
/// group it comes from, or the plain default.
#[derive(Default)]
struct Hints {
    port: String,
    username: String,
    key: String,
    jump: String,
    proxy: String,
    env: String,
    startup: String,
    /// The groups each value would come from, if the host did not set it.
    groups: annulus_vault::groups::Sources,
}

fn inherited_hints(
    vault: &Vault,
    host: &Host,
    keys: &[(u64, String)],
    hosts: &[(u64, String)],
) -> Hints {
    let mut bare = host.clone();
    (bare.port, bare.key_id, bare.jump_id, bare.proxy) = (None, None, None, None);
    bare.startup.clear();
    bare.username.clear();
    let (resolved, sources) = vault.resolve(&bare);
    let groups = sources.clone();
    let from = |value: String, source: Option<String>, otherwise: &str| match source {
        Some(group) => format!("{value} (from {group})"),
        None => otherwise.to_owned(),
    };
    Hints {
        port: from(resolved.port().to_string(), sources.port, "22"),
        username: from(resolved.username, sources.username, "admin"),
        key: from(
            resolved
                .key_id
                .and_then(|id| keys.iter().find(|(key, _)| *key == id))
                .map_or_else(String::new, |(_, label)| label.clone()),
            sources.key_id,
            "Default keys",
        ),
        jump: from(
            resolved
                .jump_id
                .and_then(|id| hosts.iter().find(|(host, _)| *host == id))
                .map_or_else(String::new, |(_, label)| label.clone()),
            sources.jump_id,
            "None (direct)",
        ),
        proxy: from(
            "group proxy".to_owned(),
            sources.proxy,
            "socks5://host:1080 or http://host:8080",
        ),
        env: if resolved.env.len() > host.env.len() {
            let names: Vec<&str> = resolved
                .env
                .iter()
                .filter(|(name, _)| !host.env.iter().any(|(own, _)| own == name))
                .map(|(name, _)| name.as_str())
                .collect();
            format!("Groups add: {}", names.join(", "))
        } else {
            "LANG=en_US.UTF-8".to_owned()
        },
        startup: from(resolved.startup, sources.startup, "e.g. tmux attach"),
        groups,
    }
}

#[derive(Clone, Copy)]
enum RowAction {
    None,
    Connect,
    Sftp,
    Edit,
    CopyCommand,
    Duplicate,
    /// Move into the group at this index of the row's group list; `None` is the top level.
    MoveTo(Option<usize>),
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

/// One host in the list. `groups` are the `(uid, path)` choices of "Move to group".
fn host_row(ui: &mut Ui, host: &Host, palette: &Palette, groups: &[(String, String)]) -> RowAction {
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
            ui.menu_button("Move to group", |ui| {
                if ui.button("Top level").clicked() {
                    chosen = RowAction::MoveTo(None);
                    ui.close();
                }
                for (index, (_, path)) in groups.iter().enumerate() {
                    if ui.button(path).clicked() {
                        chosen = RowAction::MoveTo(Some(index));
                        ui.close();
                    }
                }
            });
        })
        .response
        .on_hover_text("More actions")
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!("More actions for {}", host.label),
            )
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
    fn a_host_in_a_group_connects_with_what_it_inherits() {
        let mut screen = screen_with("grp");
        let mut bastion = screen.vault.draft_host();
        (bastion.label, bastion.address) = ("bastion".into(), "jump.example.com".into());
        screen.vault.upsert_host(bastion.clone()).unwrap();
        let mut group = screen.vault.draft_group(None).unwrap();
        group.name = "prod".into();
        group.defaults.username = "deploy".into();
        group.defaults.port = Some(2200);
        group.defaults.jump_id = Some(bastion.id);
        screen.vault.upsert_group(group.clone()).unwrap();
        let mut host = screen
            .vault
            .hosts()
            .iter()
            .find(|host| host.label == "grp")
            .unwrap()
            .clone();
        (host.username, host.port, host.group) = (String::new(), None, Some(group.uid));
        screen.vault.upsert_host(host.clone()).unwrap();
        let Program::Ssh { target, via, .. } = screen.connect_host(&host) else {
            panic!("an ssh program");
        };
        assert_eq!((target.user.as_str(), target.port), ("deploy", 2200));
        let hops: Vec<&str> = via.iter().map(|hop| hop.target.host.as_str()).collect();
        assert_eq!(hops, ["jump.example.com"], "through the group's jump host");
        let hints = inherited_hints(&screen.vault, &host, &[], &[]);
        assert_eq!(hints.port, "2200 (from prod)");
        assert_eq!(hints.username, "deploy (from prod)");
        assert_eq!(hints.key, "Default keys");
        assert_eq!(
            hints.groups.port.as_deref(),
            Some("prod"),
            "the \u{21ba} names it"
        );
        assert_eq!(hints.groups.key_id, None);
    }

    #[test]
    fn move_to_group_puts_the_host_in_the_chosen_group_or_back_on_top() {
        let mut screen = screen_with("move");
        let mut group = screen.vault.draft_group(None).unwrap();
        group.name = "prod".into();
        screen.vault.upsert_group(group.clone()).unwrap();
        let targets = groups_view::choices(screen.vault.groups(), None);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let host = screen.vault.hosts()[0].clone();
            screen.row_action(ui, &host, RowAction::MoveTo(Some(0)), &targets);
            assert_eq!(
                screen.vault.hosts()[0].group.as_deref(),
                Some(group.uid.as_str())
            );
            let host = screen.vault.hosts()[0].clone();
            screen.row_action(ui, &host, RowAction::MoveTo(None), &targets);
            assert_eq!(screen.vault.hosts()[0].group, None);
        });
        output.textures_delta.clear();
    }

    /// End to end on the test VMs (docs/TESTING.md §Test VMs): a host that sets nothing but
    /// its address and group is reached through the group's jump host, as the group's user,
    /// with the group's keychain key.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_a_grouped_host_is_reached_with_everything_it_inherits() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let key_text = std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap();
        let pair = annulus_ssh::keys::import(&key_text, None).unwrap();
        let mut screen = screen_with("e2e-group");
        let key = annulus_vault::keychain::SshKey {
            id: screen.vault.next_key_id(),
            label: "test key".into(),
            private: Secret::new(pair.private.as_str()),
            public: pair.public.clone(),
            fingerprint: pair.fingerprint.clone(),
        };
        screen.vault.upsert_key(key.clone()).unwrap();
        let mut bastion = screen.vault.draft_host();
        (bastion.label, bastion.address) = ("bastion".into(), var("ANNULUS_E2E_VM1"));
        // The jump host is outside the group, so it sets its own login.
        (bastion.username, bastion.key_id) = (var("ANNULUS_E2E_USER"), Some(key.id));
        screen.vault.upsert_host(bastion.clone()).unwrap();
        let mut group = screen.vault.draft_group(None).unwrap();
        group.name = "lab".into();
        group.defaults.username = var("ANNULUS_E2E_USER");
        group.defaults.key_id = Some(key.id);
        group.defaults.jump_id = Some(bastion.id);
        screen.vault.upsert_group(group.clone()).unwrap();
        let mut inner = screen.vault.draft_host();
        (inner.label, inner.address) = ("inner".into(), var("ANNULUS_E2E_VM2"));
        inner.group = Some(group.uid);
        screen.vault.upsert_host(inner.clone()).unwrap();

        let Program::Ssh {
            target,
            files,
            password,
            identity,
            via,
            proxy,
            ..
        } = screen.connect_host(&inner)
        else {
            panic!("an ssh program");
        };
        let last = Hop {
            target,
            password,
            identity,
            proxy,
        };
        let answers = annulus_session::Answers::default();
        let trusting = answers.clone();
        let notify: std::sync::Arc<dyn Fn(annulus_session::SessionEvent) + Send + Sync> =
            std::sync::Arc::new(move |event| match event {
                annulus_session::SessionEvent::HostKey(_) => trusting.host_key(true),
                // Nothing should ask for a password: fail at once instead of waiting.
                annulus_session::SessionEvent::Password { .. } => trusting.password(None),
                _ => {}
            });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let output = runtime.block_on(async {
            let connecting = annulus_session::connect_chain(&via, &last, &files, &answers, notify);
            let connection = tokio::time::timeout(std::time::Duration::from_secs(30), connecting)
                .await
                .expect("connected within 30 s")
                .expect("reached through the group's jump host");
            connection.exec("hostname; whoami").await.unwrap().output
        });
        let output = String::from_utf8_lossy(&output);
        assert!(output.starts_with("annulus-test-2\n"), "{output}");
        assert!(
            output.ends_with(&format!("{}\n", var("ANNULUS_E2E_USER"))),
            "{output}"
        );
    }

    #[test]
    fn duplicate_saves_a_copy_with_a_new_id_and_the_same_address() {
        let mut screen = screen_with("dup");
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let host = screen.vault.hosts()[0].clone();
            screen.row_action(ui, &host, RowAction::Duplicate, &[]);
        });
        output.textures_delta.clear();
        let hosts = screen.vault.hosts();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[1].label, "dup copy");
        assert_eq!(hosts[1].address, hosts[0].address);
        assert_ne!(hosts[1].id, hosts[0].id);
    }

    #[test]
    fn connect_all_opens_every_host_directly_in_the_group() {
        let mut screen = screen_with("all");
        let mut group = screen.vault.draft_group(None).unwrap();
        group.name = "web".into();
        screen.vault.upsert_group(group.clone()).unwrap();
        for label in ["web1", "web2"] {
            let mut host = screen.vault.draft_host();
            (host.label, host.address) = (label.into(), format!("{label}.lan"));
            host.group = Some(group.uid.clone());
            screen.vault.upsert_host(host).unwrap();
        }
        let Request::OpenAll(tabs) = screen.connect_all(&group.uid) else {
            panic!("one request for all");
        };
        let titles: Vec<&str> = tabs.iter().map(|(title, _)| title.as_str()).collect();
        assert_eq!(titles, ["web1", "web2"], "not the host outside the group");
    }

    /// End to end on test VM 1: a group's environment variable (Ubuntu's sshd accepts
    /// `LC_*`) and startup command reach a real shell through a terminal session.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_the_groups_environment_and_startup_command_reach_the_shell() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let pair = annulus_ssh::keys::import(
            &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
            None,
        )
        .unwrap();
        let mut screen = screen_with("e2e-startup");
        let key = annulus_vault::keychain::SshKey {
            id: screen.vault.next_key_id(),
            label: "test key".into(),
            private: Secret::new(pair.private.as_str()),
            public: pair.public.clone(),
            fingerprint: pair.fingerprint.clone(),
        };
        screen.vault.upsert_key(key.clone()).unwrap();
        let mut group = screen.vault.draft_group(None).unwrap();
        group.name = "lab".into();
        group.defaults.username = var("ANNULUS_E2E_USER");
        group.defaults.key_id = Some(key.id);
        group.defaults.env = vec![("LC_ANNULUS".into(), "hello".into())];
        group.defaults.startup = "echo started-$((6*7)) [$LC_ANNULUS]".into();
        screen.vault.upsert_group(group.clone()).unwrap();
        let mut host = screen.vault.draft_host();
        (host.label, host.address) = ("vm1".into(), var("ANNULUS_E2E_VM1"));
        host.group = Some(group.uid);
        screen.vault.upsert_host(host.clone()).unwrap();

        let program = screen.connect_host(&host);
        let session = annulus_session::Session::spawn(
            &program,
            annulus_term::Size::new(100, 30),
            8,
            16,
            || {},
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            for event in session.take_events() {
                match event {
                    annulus_session::SessionEvent::HostKey(_) => session.answers().host_key(true),
                    annulus_session::SessionEvent::Password { .. } => {
                        session.answers().password(None);
                    }
                    annulus_session::SessionEvent::Failed(why) => panic!("{why}"),
                    _ => {}
                }
            }
            if session.transcript().contains("started-42 [hello]") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no startup output: {}",
                session.transcript()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    #[test]
    fn a_typed_target_can_be_saved_as_a_host() {
        let screen = screen_with("known");
        let host = screen.unsaved_target("deploy@10.0.9.9:2200").unwrap();
        assert_eq!(
            (
                host.label.as_str(),
                host.address.as_str(),
                host.username.as_str(),
                host.port
            ),
            ("10.0.9.9", "10.0.9.9", "deploy", Some(2200))
        );
        let plain = screen.unsaved_target("db.example.com").unwrap();
        assert_eq!((plain.port, plain.username.as_str()), (None, ""));
        assert!(
            screen.unsaved_target("known").is_none(),
            "a saved host's label"
        );
        assert!(screen.unsaved_target("just words").is_none());
        assert!(
            screen.unsaved_target("web").is_none(),
            "a bare word is a search"
        );
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
            Request::OpenAll(_) => ("", String::new()),
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
            Request::Open(..) | Request::Sftp(..) | Request::OpenAll(_) => None,
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
