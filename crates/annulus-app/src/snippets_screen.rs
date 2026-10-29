//! Snippets (F-SNIP-001…003): the Snippets screen to write and organize saved commands,
//! and the panel beside a terminal (Ctrl+Shift+.) to run or paste one.

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::theme;
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;

/// The Snippets screen's state.
#[derive(Default)]
pub struct SnippetsScreen {
    search: String,
    editor: Option<Snippet>,
    confirm_delete: Option<u64>,
    error: Option<String>,
}

impl SnippetsScreen {
    /// Draws the screen.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault) {
        if self.editor.is_some() {
            egui::Panel::right("snippet-editor")
                .resizable(true)
                .default_size(380.0)
                .show(ui, |ui| self.editor_ui(ui, vault));
        }
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
