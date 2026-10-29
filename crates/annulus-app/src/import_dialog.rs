//! Import from `~/.ssh/config` with a preview (F-IMP-001): every host found is listed, those
//! whose name is already saved start unticked, and nothing is saved until Import.

use std::fmt::Write as _;

use eframe::egui::{self, Key, RichText, Vec2};
use annulus_import::ssh_config::{self, Found};
use annulus_vault::forwards::PortForward;
use annulus_vault::hosts::Host;
use annulus_vault::vault::Vault;

/// The preview window.
pub struct ImportPreview {
    rows: Vec<(Found, bool)>,
    existing: Vec<String>,
}

/// The hosts and rules to import, with temporary ids that `Vault::adopt` renumbers.
#[must_use]
pub fn entities(found: &[&Found]) -> (Vec<Host>, Vec<PortForward>) {
    let mut hosts = Vec::new();
    let mut rules = Vec::new();
    for (host_id, entry) in (1..).zip(found) {
        hosts.push(entry.to_host(host_id));
        let first = u64::try_from(rules.len()).unwrap_or(u64::MAX) + 1;
        rules.extend(entry.to_rules(host_id, first));
    }
    (hosts, rules)
}

impl ImportPreview {
    /// Reads `~/.ssh/config` (and what it includes).
    ///
    /// # Errors
    /// No home folder, or no config file.
    pub fn load(vault: &Vault) -> Result<Self, String> {
        let home = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or("No home folder found.")?;
        let ssh_dir = home.join(".ssh");
        let text = std::fs::read_to_string(ssh_dir.join("config"))
            .map_err(|_| format!("No OpenSSH config at {}.", ssh_dir.join("config").display()))?;
        let found = ssh_config::parse(
            &text,
            &ssh_dir,
            &home,
            &|path| std::fs::read_to_string(path).ok(),
            &|dir| {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(|entry| entry.ok().map(|e| e.path()))
                            .collect()
                    })
                    .unwrap_or_default()
            },
        );
        if found.is_empty() {
            return Err("The OpenSSH config names no hosts.".into());
        }
        let existing: Vec<String> = vault
            .hosts()
            .iter()
            .map(|host| host.label.to_lowercase())
            .collect();
        let rows = found
            .into_iter()
            .map(|entry| {
                let new = !existing.contains(&entry.alias.to_lowercase());
                (entry, new)
            })
            .collect();
        Ok(Self { rows, existing })
    }

    /// Draws the window; returns a message once it closes (imported or cancelled).
    pub fn ui(&mut self, ctx: &egui::Context, vault: &mut Vault) -> Option<String> {
        let mut result = None;
        egui::Window::new("Import from OpenSSH config")
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    "Choose the hosts to add. Keys and jump hosts are listed but not imported yet.",
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (entry, selected) in &mut self.rows {
                            let mut detail = format!("{}:{}", entry.address, entry.port);
                            if let Some(user) = &entry.user {
                                detail = format!("{user}@{detail}");
                            }
                            if let Some(jump) = &entry.proxy_jump {
                                let _ = write!(detail, "  via {jump}");
                            }
                            if !entry.forwards.is_empty() {
                                let _ = write!(detail, "  {} forward(s)", entry.forwards.len());
                            }
                            ui.horizontal(|ui| {
                                ui.checkbox(selected, &entry.alias);
                                ui.label(RichText::new(detail).weak());
                                if self.existing.contains(&entry.alias.to_lowercase()) {
                                    ui.label(RichText::new("already saved").italics());
                                }
                            });
                        }
                    });
                ui.add_space(8.0);
                let chosen: Vec<&Found> = self
                    .rows
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(entry, _)| entry)
                    .collect();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !chosen.is_empty(),
                            egui::Button::new(format!("Import {}", chosen.len())),
                        )
                        .clicked()
                    {
                        let (hosts, rules) = entities(&chosen);
                        result = Some(match vault.adopt(&hosts, &rules, &[]) {
                            Ok(()) => format!(
                                "Imported {} host(s) and {} forward(s).",
                                hosts.len(),
                                rules.len()
                            ),
                            Err(error) => format!("Nothing was imported: {error}"),
                        });
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|input| input.key_pressed(Key::Escape))
                    {
                        result = Some(String::new());
                    }
                });
            });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_point_at_their_own_host_before_renumbering() {
        let text =
            "Host a\n  LocalForward 1 x:1\nHost b\n  LocalForward 2 y:2\n  DynamicForward 3\n";
        let found = ssh_config::parse(
            text,
            std::path::Path::new("/s"),
            std::path::Path::new("/h"),
            &|_| None,
            &|_| Vec::new(),
        );
        let chosen: Vec<&Found> = found.iter().collect();
        let (hosts, rules) = entities(&chosen);
        assert_eq!(hosts.len(), 2);
        let ids: Vec<(u64, u64)> = rules.iter().map(|rule| (rule.id, rule.host_id)).collect();
        assert_eq!(ids, [(1, 1), (2, 2), (3, 2)]);
    }
}
