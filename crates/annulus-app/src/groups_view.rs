//! Host groups on the Hosts screen (F-HOST-003): the breadcrumb, group rows, the group
//! editor and the group picker of the host editor.

use eframe::egui::{self, Button, RichText, Sense, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::secret::{SecretText, secret_line};
use annulus_ui::{icons, theme};
use annulus_vault::groups::{self, Group};
use annulus_vault::hosts::Secret;
use annulus_vault::vault::Vault;

/// Ids and labels for a picker.
type Choices = Vec<(u64, String)>;

/// `uid`'s place in the tree, top first: "prod / db". Unknown uids read as the top level.
#[must_use]
pub fn path(groups: &[Group], uid: Option<&str>) -> String {
    let mut names: Vec<&str> = groups::chain(groups, uid)
        .iter()
        .map(|group| group.name.as_str())
        .collect();
    names.reverse();
    if names.is_empty() {
        "Top level".to_owned()
    } else {
        names.join(" / ")
    }
}

/// Every group as `(uid, path)`, sorted by path, for pickers; leaves out `skip` and what is
/// inside it (a group cannot move into itself).
#[must_use]
pub fn choices(groups: &[Group], skip: Option<&str>) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = groups
        .iter()
        .filter(|group| skip.is_none_or(|skip| !groups::is_within(groups, &group.uid, skip)))
        .map(|group| (group.uid.clone(), path(groups, Some(&group.uid))))
        .collect();
    list.sort_by_key(|(_, path)| path.to_lowercase());
    list
}

/// A picker of the group to put something in (`None` is the top level).
pub fn picker(ui: &mut Ui, id: &str, chosen: &mut Option<String>, choices: &[(String, String)]) {
    let shown = chosen
        .as_deref()
        .and_then(|uid| choices.iter().find(|(choice, _)| choice == uid))
        .map_or("Top level", |(_, path)| path.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(chosen, None, "Top level");
            for (uid, path) in choices {
                ui.selectable_value(chosen, Some(uid.clone()), path);
            }
        });
}

/// A part of the breadcrumb that was clicked.
pub enum Crumb {
    /// "All hosts": the top level.
    Top,
    /// A group, by uid.
    Group(String),
}

/// The breadcrumb above the list: "All hosts › prod › db". Returns the part clicked.
pub fn breadcrumb(ui: &mut Ui, groups: &[Group], current: Option<&str>) -> Option<Crumb> {
    let mut chain = groups::chain(groups, current);
    chain.reverse();
    let mut open = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!chain.is_empty(), Button::new("All hosts").frame(false))
            .clicked()
        {
            open = Some(Crumb::Top);
        }
        for (index, group) in chain.iter().enumerate() {
            ui.label("\u{203a}");
            let last = index + 1 == chain.len();
            if ui
                .add_enabled(!last, Button::new(&group.name).frame(false))
                .clicked()
            {
                open = Some(Crumb::Group(group.uid.clone()));
            }
        }
    });
    open
}

/// What a group row asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GroupAction {
    /// Nothing this frame.
    None,
    /// Show the group's hosts.
    Open,
    /// Edit the group.
    Edit,
    /// Open a terminal for each host in it.
    ConnectAll,
}

/// One group in the list: icon, name and what it holds; click or Enter opens it.
pub fn row(ui: &mut Ui, group: &Group, holds: &str, palette: &Palette) -> GroupAction {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 40.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("Open group {}", group.name),
        )
    });
    let painter = ui.painter();
    if response.hovered() || response.has_focus() {
        painter.rect_filled(rect, 6.0, theme::color(palette.bg_hover));
    }
    let icon = egui::Rect::from_min_size(
        rect.min + egui::vec2(space::M, 12.0),
        egui::vec2(16.0, 16.0),
    );
    icons::paint(
        painter,
        icons::FOLDER,
        icon,
        theme::color(palette.text_secondary),
    );
    let left = rect.min.x + space::M + 28.0;
    painter.text(
        egui::pos2(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &group.name,
        egui::FontId::proportional(text::BODY + 1.0),
        theme::color(palette.text_primary),
    );
    painter.text(
        egui::pos2(rect.max.x - 190.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        holds,
        egui::FontId::proportional(text::SECONDARY),
        theme::color(palette.text_secondary),
    );
    let edit = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - 70.0, rect.min.y + 6.0),
        egui::vec2(60.0, 28.0),
    );
    if ui.put(edit, Button::new("Edit")).clicked() {
        return GroupAction::Edit;
    }
    let all = egui::Rect::from_min_size(edit.min - egui::vec2(104.0, 0.0), egui::vec2(96.0, 28.0));
    if ui
        .put(all, Button::new("Connect all"))
        .on_hover_text("Open a terminal for each host in this group")
        .clicked()
    {
        return GroupAction::ConnectAll;
    }
    if response.clicked()
        || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
    {
        return GroupAction::Open;
    }
    GroupAction::None
}

/// "3 hosts · 1 group": what a group holds directly.
#[must_use]
pub fn holds(hosts: usize, subgroups: usize) -> String {
    let hosts = match hosts {
        1 => "1 host".to_owned(),
        n => format!("{n} hosts"),
    };
    match subgroups {
        0 => hosts,
        1 => format!("{hosts} \u{b7} 1 group"),
        n => format!("{hosts} \u{b7} {n} groups"),
    }
}

/// Environment variables as the editors show them: one `NAME=value` per line.
#[must_use]
pub fn format_env(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads `NAME=value` lines (blank lines skipped), or says which line is wrong. Names are
/// letters, digits and `_`, not starting with a digit, as shells require.
pub fn parse_env(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut env = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let valid = |name: &str| {
            name.chars()
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        };
        match line.split_once('=') {
            Some((name, value)) if valid(name.trim()) => {
                env.push((name.trim().to_owned(), value.to_owned()));
            }
            _ => {
                return Err(format!(
                    "Environment line {}: write NAME=value (letters, digits and _).",
                    number + 1
                ));
            }
        }
    }
    Ok(env)
}

/// The editors' Environment and Startup command rows; `hints` are the placeholders.
pub fn session_rows(ui: &mut Ui, env: &mut String, startup: &mut String, hints: (&str, &str)) {
    ui.label("Environment");
    ui.add(
        egui::TextEdit::multiline(env)
            .hint_text(hints.0)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    ui.end_row();
    ui.label("Startup command");
    ui.add(egui::TextEdit::singleline(startup).hint_text(hints.1))
        .on_hover_text("Typed into the shell once it opens");
    ui.end_row();
}

/// The editors' terminal type row (F-SSH-007): empty keeps what the group (or the default)
/// gives.
pub fn term_row(ui: &mut Ui, term: &mut String, unset: &str) {
    ui.label("Terminal type");
    let shown = if term.is_empty() {
        unset
    } else {
        term.as_str()
    };
    egui::ComboBox::from_id_salt("term-type")
        .selected_text(shown.to_owned())
        .show_ui(ui, |ui| {
            ui.selectable_value(term, String::new(), unset);
            for choice in annulus_ssh::TERM_CHOICES {
                ui.selectable_value(term, choice.to_owned(), choice);
            }
        })
        .response
        .on_hover_text("What the server is told this terminal is (TERM)");
    ui.end_row();
}

/// A picker of a key or host by id, or "Not set".
fn id_picker(ui: &mut Ui, id: &str, chosen: &mut Option<u64>, choices: &[(u64, String)]) {
    let shown = chosen
        .and_then(|id| choices.iter().find(|(choice, _)| *choice == id))
        .map_or("Not set", |(_, label)| label.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(chosen, None, "Not set");
            for (value, label) in choices {
                ui.selectable_value(chosen, Some(*value), label);
            }
        });
}

/// The keychain keys and the hosts, as `(id, label)` for the editor's pickers.
fn pick_lists(vault: &Vault) -> (Choices, Choices) {
    let keys = vault
        .ssh_keys()
        .iter()
        .map(|key| (key.id, key.label.clone()))
        .collect();
    let hosts = vault
        .hosts()
        .iter()
        .map(|host| (host.id, host.label.clone()))
        .collect();
    (keys, hosts)
}

/// "1 host" or "3 hosts".
fn groups_holds(hosts: usize) -> String {
    if hosts == 1 {
        "1 host".to_owned()
    } else {
        format!("{hosts} hosts")
    }
}

/// The group editor's working copy.
pub struct GroupDraft {
    group: Group,
    port: String,
    env: String,
    proxy: SecretText,
    problem: Option<String>,
    is_new: bool,
    /// Delete was pressed: asking what happens to the hosts inside.
    confirm_delete: bool,
}

/// What deleting a group does with the hosts directly in it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deletion {
    /// They (and its subgroups) move up into its parent.
    MoveHostsUp,
    /// They are deleted with it; its subgroups still move up.
    WithHosts,
}

/// Deletes `group` from `vault` as `how` says.
fn delete_group(vault: &mut Vault, group: &Group, how: Deletion) -> Result<(), String> {
    if how == Deletion::WithHosts {
        let inside: Vec<u64> = vault
            .hosts()
            .iter()
            .filter(|host| host.group.as_deref() == Some(group.uid.as_str()))
            .map(|host| host.id)
            .collect();
        vault
            .remove_hosts(&inside)
            .map_err(|error| error.to_string())?;
    }
    vault
        .remove_group(group.id)
        .map_err(|error| error.to_string())
}

impl GroupDraft {
    /// Edits `group`; `is_new` when it is not saved yet.
    #[must_use]
    pub fn new(group: Group, is_new: bool) -> Self {
        Self {
            port: group
                .defaults
                .port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            proxy: SecretText::from(group.defaults.proxy.as_ref().map_or("", |url| url.expose())),
            env: format_env(&group.defaults.env),
            problem: None,
            is_new,
            confirm_delete: false,
            group,
        }
    }

    /// Draws the editor; true once it is closed (saved, deleted or cancelled).
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault) -> bool {
        let parents = choices(vault.groups(), Some(&self.group.uid));
        let (keys, hosts) = pick_lists(vault);
        ui.add_space(space::M);
        ui.heading(if self.is_new {
            "New group"
        } else {
            "Edit group"
        });
        ui.label(
            RichText::new("Hosts inside take these values unless they set their own.")
                .color(theme::color(palette.text_secondary)),
        );
        ui.add_space(space::M);
        egui::Grid::new("group-fields")
            .num_columns(2)
            .spacing([space::M, space::S])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.group.name).hint_text("Production"));
                ui.end_row();
                ui.label("Inside");
                picker(ui, "group-parent", &mut self.group.parent, &parents);
                ui.end_row();
                ui.label("Username");
                ui.add(
                    egui::TextEdit::singleline(&mut self.group.defaults.username)
                        .hint_text("Not set"),
                );
                ui.end_row();
                ui.label("Port");
                ui.add(
                    egui::TextEdit::singleline(&mut self.port)
                        .hint_text("Not set")
                        .desired_width(80.0),
                );
                ui.end_row();
                ui.label("Key");
                id_picker(ui, "group-key", &mut self.group.defaults.key_id, &keys);
                ui.end_row();
                ui.label("Jump host");
                id_picker(ui, "group-jump", &mut self.group.defaults.jump_id, &hosts);
                ui.end_row();
                ui.label("Proxy");
                secret_line(ui, &mut self.proxy, "Not set");
                ui.end_row();
                session_rows(
                    ui,
                    &mut self.env,
                    &mut self.group.defaults.startup,
                    ("LANG=en_US.UTF-8", "Not set"),
                );
                term_row(ui, &mut self.group.defaults.term, "Not set");
            });
        if let Some(problem) = &self.problem {
            ui.add_space(space::S);
            ui.colored_label(theme::color(palette.status_error), problem);
        }
        ui.add_space(space::M);
        let (mut save, mut delete, mut cancel) = (false, None, false);
        if self.confirm_delete {
            delete = self.delete_prompt(ui, vault);
            cancel = !self.confirm_delete;
        } else {
            ui.horizontal(|ui| {
                save = ui.button("Save").clicked();
                cancel = ui.button("Cancel").clicked();
                if !self.is_new
                    && ui
                        .button(RichText::new("Delete").color(theme::color(palette.status_error)))
                        .clicked()
                {
                    self.confirm_delete = true;
                }
            });
        }
        if cancel || ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            return true;
        }
        let result = if let Some(how) = delete {
            delete_group(vault, &self.group, how)
        } else if save {
            self.apply()
                .and_then(|group| vault.upsert_group(group).map_err(|error| error.to_string()))
        } else {
            return false;
        };
        match result {
            Ok(()) => true,
            Err(problem) => {
                self.problem = Some(problem);
                false
            }
        }
    }

    /// Asks what happens to the hosts inside; `None` until a choice is made. Cancel ends the
    /// prompt (`confirm_delete` goes false).
    fn delete_prompt(&mut self, ui: &mut Ui, vault: &Vault) -> Option<Deletion> {
        let inside = vault
            .hosts()
            .iter()
            .filter(|host| host.group.as_deref() == Some(self.group.uid.as_str()))
            .count();
        let mut chosen = None;
        ui.label(format!("Delete the group {}?", self.group.name));
        ui.horizontal(|ui| {
            let up = if inside == 0 {
                "Delete group".to_owned()
            } else {
                format!("Delete, move its {} up", groups_holds(inside))
            };
            if ui.button(up).clicked() {
                chosen = Some(Deletion::MoveHostsUp);
            }
            if inside > 0
                && ui
                    .button(format!("Delete it and its {}", groups_holds(inside)))
                    .on_hover_text("Synced hosts can be brought back from the trash")
                    .clicked()
            {
                chosen = Some(Deletion::WithHosts);
            }
            if ui.button("Keep it").clicked() {
                self.confirm_delete = false;
            }
        });
        chosen
    }

    /// The group as typed, or why it can't be saved.
    fn apply(&self) -> Result<Group, String> {
        let mut group = self.group.clone();
        group.name = group.name.trim().to_owned();
        group.defaults.username = group.defaults.username.trim().to_owned();
        let port = self.port.trim();
        group.defaults.port = if port.is_empty() {
            None
        } else {
            match port.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => return Err("The port must be between 1 and 65535.".into()),
            }
        };
        let proxy = self.proxy.expose().trim();
        if !proxy.is_empty() && annulus_ssh::proxy::Proxy::parse(proxy).is_none() {
            return Err("The proxy must look like socks5://host:port or http://host:port.".into());
        }
        group.defaults.proxy = (!proxy.is_empty()).then(|| Secret::new(proxy));
        group.defaults.env = parse_env(&self.env)?;
        group.defaults.startup = group.defaults.startup.trim().to_owned();
        Ok(group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(uid: &str, name: &str, parent: Option<&str>) -> Group {
        Group {
            id: 0,
            uid: uid.into(),
            name: name.into(),
            parent: parent.map(Into::into),
            defaults: groups::Defaults::default(),
        }
    }

    #[test]
    fn paths_read_top_down_and_a_group_cannot_pick_itself_as_parent() {
        let list = vec![
            group("a", "prod", None),
            group("b", "db", Some("a")),
            group("c", "web", None),
        ];
        assert_eq!(path(&list, Some("b")), "prod / db");
        assert_eq!(path(&list, None), "Top level");
        let names: Vec<String> = choices(&list, None).into_iter().map(|c| c.1).collect();
        assert_eq!(names, ["prod", "prod / db", "web"]);
        let for_prod: Vec<String> = choices(&list, Some("a")).into_iter().map(|c| c.0).collect();
        assert_eq!(for_prod, ["c"], "not itself, not its subgroup");
        assert_eq!(holds(1, 0), "1 host");
        assert_eq!(holds(3, 2), "3 hosts \u{b7} 2 groups");
    }

    #[test]
    fn environment_lines_round_trip_and_bad_names_are_refused() {
        let env = parse_env("LANG=C.UTF-8\n\n  TZ=Etc/UTC\nEMPTY=\nURL=a=b").unwrap();
        assert_eq!(
            format_env(&env),
            "LANG=C.UTF-8\nTZ=Etc/UTC\nEMPTY=\nURL=a=b",
            "a value may hold = and may be empty"
        );
        assert_eq!(
            parse_env("OK=1\n2BAD=x"),
            Err("Environment line 2: write NAME=value (letters, digits and _).".into())
        );
        assert!(parse_env("no equals sign").is_err());
        assert!(parse_env("A B=1").is_err());
    }

    #[test]
    fn deleting_a_group_moves_its_hosts_up_or_takes_them_along() {
        let dir = std::env::temp_dir().join(format!("annulus-delgroup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cheap = annulus_vault::crypto::KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        let (mut vault, _) = Vault::create(dir.join("vault.anv"), "pw", cheap).unwrap();
        let add = |vault: &mut Vault, name: &str| {
            let mut group = vault.draft_group(None).unwrap();
            group.name = name.into();
            vault.upsert_group(group.clone()).unwrap();
            let mut host = vault.draft_host();
            (host.label, host.address) = (format!("{name}-1"), "x".into());
            host.group = Some(group.uid.clone());
            vault.upsert_host(host).unwrap();
            group
        };
        let keep = add(&mut vault, "keep");
        let drop = add(&mut vault, "drop");
        delete_group(&mut vault, &keep, Deletion::MoveHostsUp).unwrap();
        delete_group(&mut vault, &drop, Deletion::WithHosts).unwrap();
        let labels: Vec<&str> = vault.hosts().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["keep-1"]);
        assert_eq!(vault.hosts()[0].group, None, "moved to the top level");
        assert_eq!(vault.groups().len(), 0);
    }

    #[test]
    fn a_group_port_must_be_a_real_port_and_empty_fields_stay_unset() {
        let mut draft = GroupDraft::new(group("a", " prod ", None), true);
        draft.port = "70000".into();
        assert!(draft.apply().is_err());
        draft.port = String::new();
        let saved = draft.apply().unwrap();
        assert_eq!(saved.name, "prod");
        assert_eq!(saved.defaults, groups::Defaults::default());
        draft.port = "2222".into();
        assert_eq!(draft.apply().unwrap().defaults.port, Some(2222));
    }
}
