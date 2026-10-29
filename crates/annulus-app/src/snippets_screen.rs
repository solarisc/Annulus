//! Snippets (F-SNIP-001…004): the Snippets screen to write and organize saved commands,
//! the panel beside a terminal (Ctrl+Shift+.) to run or paste one, and running one on
//! several saved hosts at once, a pane each.

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::Program;
use annulus_ui::theme;
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;

use crate::hosts_screen::HostsScreen;

/// Panes one multi-host run opens at most.
pub const MAX_TARGETS: usize = 16;

/// Tabs to open: a title and a program for each pane.
pub type Panes = Vec<(String, Program)>;

/// `program` with `script` typed once its shell opens, after the host's own startup
/// command if it has one. Programs that are not Annulus SSH sessions are left alone.
#[must_use]
pub fn with_script(mut program: Program, script: &str) -> Program {
    if let Program::Ssh { startup, .. } = &mut program {
        let lines: Vec<&str> = [startup.as_deref().unwrap_or_default(), script.trim_end()]
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect();
        *startup = Some(lines.join("\n"));
    }
    program
}

/// The Snippets screen's state.
#[derive(Default)]
pub struct SnippetsScreen {
    search: String,
    editor: Option<Snippet>,
    confirm_delete: Option<u64>,
    error: Option<String>,
    /// Running a snippet on hosts: its id and the hosts ticked so far.
    targets: Option<(u64, Vec<u64>)>,
}

impl SnippetsScreen {
    /// Draws the screen; returns the panes of a multi-host run to open in a new tab.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, hosts: &mut HostsScreen) -> Option<Panes> {
        let mut run = None;
        if self.editor.is_some() {
            egui::Panel::right("snippet-editor")
                .resizable(true)
                .default_size(380.0)
                .show(ui, |ui| self.editor_ui(ui, hosts.vault_mut()));
        } else if self.targets.is_some() {
            run = egui::Panel::right("snippet-targets")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| self.targets_ui(ui, hosts))
                .inner;
        }
        self.list_ui(ui, palette, hosts.vault_mut());
        run
    }

    /// Ticking hosts for a run; returns the panes once Run is pressed.
    fn targets_ui(&mut self, ui: &mut Ui, hosts: &HostsScreen) -> Option<Panes> {
        let (snippet_id, chosen) = self.targets.as_mut()?;
        let Some(snippet) = hosts
            .vault()
            .snippets()
            .iter()
            .find(|snippet| snippet.id == *snippet_id)
            .cloned()
        else {
            self.targets = None;
            return None;
        };
        ui.add_space(space::M);
        ui.heading(format!("Run {} on", snippet.label));
        ui.label(format!(
            "Each host opens in a pane of one tab (up to {MAX_TARGETS})."
        ));
        ui.add_space(space::S);
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for host in hosts.vault().hosts() {
                    let mut on = chosen.contains(&host.id);
                    let room = on || chosen.len() < MAX_TARGETS;
                    if ui
                        .add_enabled(room, egui::Checkbox::new(&mut on, &host.label))
                        .changed()
                    {
                        if on {
                            chosen.push(host.id);
                        } else {
                            chosen.retain(|id| *id != host.id);
                        }
                    }
                }
            });
        let (mut go, mut cancel) = (false, false);
        ui.horizontal(|ui| {
            go = ui
                .add_enabled(!chosen.is_empty(), egui::Button::new("Run"))
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            self.targets = None;
            return None;
        }
        if !go {
            return None;
        }
        let panes = chosen
            .iter()
            .filter_map(|id| hosts.vault().hosts().iter().find(|host| host.id == *id))
            .map(|host| {
                let program = with_script(hosts.connect_host(host), &snippet.script);
                (host.label.clone(), program)
            })
            .collect();
        self.targets = None;
        Some(panes)
    }

    fn list_ui(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault) {
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search snippets")
                    .desired_width(320.0),
            );
            if ui.button("New snippet").clicked() {
                self.editor = Some(vault.draft_snippet());
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(theme::color(palette.status_error), error);
        }
        ui.add_space(space::S);
        if vault.snippets().is_empty() {
            ui.label(
                RichText::new("No snippets yet. Save commands you type often; run them from the panel beside any terminal with Ctrl+Shift+.")
                    .color(theme::color(palette.text_secondary)),
            );
            return;
        }
        let snippets: Vec<Snippet> = vault
            .snippets()
            .iter()
            .filter(|snippet| snippet.matches(&self.search))
            .cloned()
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for snippet in &snippets {
                self.row(ui, palette, vault, snippet);
                ui.separator();
            }
        });
    }

    fn row(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault, snippet: &Snippet) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let title = if snippet.package.is_empty() {
                    snippet.label.clone()
                } else {
                    format!("{}  \u{b7}  {}", snippet.label, snippet.package)
                };
                ui.label(RichText::new(title).size(text::BODY + 1.0).strong());
                ui.label(
                    RichText::new(snippet.script.lines().next().unwrap_or_default())
                        .monospace()
                        .color(theme::color(palette.text_secondary)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.confirm_delete == Some(snippet.id) {
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                    if ui.button("Delete snippet").clicked() {
                        self.confirm_delete = None;
                        self.error = vault
                            .remove_snippet(snippet.id)
                            .err()
                            .map(|error| error.to_string());
                    }
                    return;
                }
                if ui.button("Delete").clicked() {
                    self.confirm_delete = Some(snippet.id);
                }
                if ui.button("Edit").clicked() {
                    self.editor = Some(snippet.clone());
                }
                if ui
                    .button("Run on hosts\u{2026}")
                    .on_hover_text("Open the chosen hosts in one tab and run it in each")
                    .clicked()
                {
                    self.targets = Some((snippet.id, Vec::new()));
                }
            });
        });
    }

    fn editor_ui(&mut self, ui: &mut Ui, vault: &mut Vault) {
        let Some(draft) = &mut self.editor else {
            return;
        };
        let mut close = false;
        ui.add_space(space::M);
        ui.heading("Snippet");
        ui.add_space(space::S);
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut draft.label).hint_text("Disk usage"));
        ui.label("Package");
        ui.add(egui::TextEdit::singleline(&mut draft.package).hint_text("Optional, e.g. ops"));
        ui.label("Commands");
        ui.add(
            egui::TextEdit::multiline(&mut draft.script)
                .font(egui::TextStyle::Monospace)
                .desired_rows(8)
                .desired_width(f32::INFINITY)
                .hint_text("df -h"),
        );
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            let ready = !draft.label.trim().is_empty() && !draft.script.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Save")).clicked() {
                let mut snippet = draft.clone();
                draft.label.trim().clone_into(&mut snippet.label);
                draft.package.trim().clone_into(&mut snippet.package);
                self.error = vault
                    .upsert_snippet(snippet)
                    .err()
                    .map(|error| error.to_string());
                close = self.error.is_none();
            }
            close |= ui.button("Cancel").clicked();
        });
        if close {
            self.editor = None;
        }
    }
}

/// The panel beside a terminal: search, then Run or Paste. Returns the bytes to send and
/// whether to send them as a paste.
pub fn side_panel(
    ui: &mut Ui,
    search: &mut String,
    snippets: &[Snippet],
) -> Option<(Snippet, bool)> {
    let mut chosen = None;
    ui.add_space(space::S);
    ui.label(RichText::new("Snippets").strong());
    ui.add(
        egui::TextEdit::singleline(search)
            .hint_text("Search")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(space::S);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut any = false;
        for snippet in snippets.iter().filter(|snippet| snippet.matches(search)) {
            any = true;
            ui.label(RichText::new(&snippet.label).strong());
            ui.label(
                RichText::new(snippet.script.lines().next().unwrap_or_default())
                    .monospace()
                    .size(text::SECONDARY),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Run").clicked() {
                    chosen = Some((snippet.clone(), true));
                }
                if ui
                    .small_button("Paste")
                    .on_hover_text("Type it at the prompt without running it")
                    .clicked()
                {
                    chosen = Some((snippet.clone(), false));
                }
            });
            ui.separator();
        }
        if !any {
            ui.label("No snippets match. Add them in the Snippets section.");
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_runs_after_the_hosts_own_startup_command() {
        let ssh = |startup: Option<&str>| Program::Ssh {
            target: annulus_session::Target {
                host: "web".into(),
                port: 22,
                user: "me".into(),
            },
            files: annulus_session::SshFiles {
                known_hosts: std::path::PathBuf::from("known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: true,
            env: Vec::new(),
            startup: startup.map(Into::into),
            term: String::new(),
        };
        let startup = |program: Program| match program {
            Program::Ssh { startup, .. } => startup,
            _ => None,
        };
        assert_eq!(
            startup(with_script(ssh(None), "df -h\n")).as_deref(),
            Some("df -h")
        );
        assert_eq!(
            startup(with_script(ssh(Some("cd /srv")), "ls")).as_deref(),
            Some("cd /srv\nls")
        );
        assert_eq!(
            with_script(Program::DefaultShell, "ls"),
            Program::DefaultShell
        );
    }
}
