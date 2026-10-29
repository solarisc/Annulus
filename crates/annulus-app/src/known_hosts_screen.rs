//! The Known Hosts screen (F-KNOWN-004, F-KNOWN-005): the server keys Annulus trusts, with
//! search, "Forget" (needed after a server is reinstalled, since a changed key is a hard
//! stop) and an import of OpenSSH's own known hosts.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ssh::known::{self, KnownHost};
use annulus_ui::theme;

/// The screen's state; the list is read when the screen opens and after each change.
#[derive(Default)]
pub struct KnownHostsScreen {
    path: Option<PathBuf>,
    entries: Vec<KnownHost>,
    search: String,
    confirm: Option<usize>,
    error: Option<String>,
    /// The result of the last import.
    notice: Option<String>,
}

impl KnownHostsScreen {
    fn reload(&mut self, path: &Path) {
        self.entries = known::read(path);
        self.path = Some(path.to_path_buf());
    }

    /// "Import from OpenSSH", when OpenSSH's file exists.
    fn import_button(&mut self, ui: &mut Ui, path: &Path, openssh: Option<&Path>) {
        if let Some(openssh) = openssh.filter(|file| file.is_file())
            && ui
                .button("Import from OpenSSH")
                .on_hover_text(format!(
                    "Add the keys in {} for hosts Annulus has no key for yet",
                    openssh.display()
                ))
                .clicked()
        {
            self.error = None;
            self.notice = Some(match known::import(path, openssh) {
                Ok(done) => format!(
                    "Imported {} key(s); left out {} already known.",
                    done.added, done.skipped
                ),
                Err(error) => format!("Could not import: {error}"),
            });
            self.reload(path);
        }
    }

    /// Draws the screen for the known-hosts file at `path`; `openssh` is OpenSSH's own
    /// file, offered for import when it exists.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, path: &Path, openssh: Option<&Path>) {
        if self.path.as_deref() != Some(path) {
            self.reload(path);
        }
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search hosts or fingerprints")
                    .desired_width(320.0),
            );
            if ui.button("Refresh").clicked() {
                self.reload(path);
            }
            self.import_button(ui, path, openssh);
        });
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(theme::color(palette.text_secondary)));
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::color(palette.status_error), error);
        }
        ui.add_space(space::S);
        if self.entries.is_empty() {
            ui.label(
                RichText::new("No trusted server keys yet. They are added when you trust a server on first connect.")
                    .color(theme::color(palette.text_secondary)),
            );
            return;
        }
        let query = self.search.to_lowercase();
        let shown: Vec<KnownHost> = self
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry
                        .hosts
                        .iter()
                        .any(|host| host.to_lowercase().contains(&query))
                    || entry.fingerprint.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let mut forget = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &shown {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(entry.hosts.join(", "))
                                .size(text::BODY + 1.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!("{}  {}", entry.algorithm, entry.fingerprint))
                                .monospace()
                                .size(text::SECONDARY)
                                .color(theme::color(palette.text_secondary)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.confirm == Some(entry.line) {
                            if ui.button("Cancel").clicked() {
                                self.confirm = None;
                            }
                            if ui.button("Forget key").clicked() {
                                forget = Some(entry.line);
                            }
                        } else if ui
                            .button("Forget")
                            .on_hover_text("The next connection asks to trust the server again")
                            .clicked()
                        {
                            self.confirm = Some(entry.line);
                        }
                    });
                });
                ui.separator();
            }
        });
        if let Some(line) = forget {
            self.confirm = None;
            self.error = known::forget(path, line)
                .err()
                .map(|error| format!("Could not change {}: {error}", path.display()));
            self.reload(path);
        }
    }
}
