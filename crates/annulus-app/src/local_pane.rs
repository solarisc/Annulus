//! The local side of the SFTP tab (F-SFTP-001): a folder on this computer, browsed like the
//! remote one. Downloads land in the folder shown; the selected file uploads to the remote
//! folder shown.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::{icons, theme};

/// One entry of a local folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEntry {
    /// The file name.
    pub name: String,
    /// A folder.
    pub is_dir: bool,
    /// Size in bytes (0 for folders).
    pub size: u64,
}

/// The entries of `dir`: folders first, then files, by name; hidden ones (dot files) only
/// when `hidden`.
pub fn read_dir(dir: &Path, hidden: bool) -> std::io::Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        entries.push(LocalEntry {
            name,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// What the local pane asks the tab to do.
pub enum LocalAction {
    /// Upload this file to the remote folder shown.
    Upload(PathBuf),
}

/// The local pane's state.
pub struct LocalPane {
    /// The folder shown.
    pub dir: PathBuf,
    pub(crate) entries: Vec<LocalEntry>,
    pub(crate) selected: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) path_field: String,
}

impl LocalPane {
    /// A pane showing `dir`.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        let mut pane = Self {
            path_field: dir.display().to_string(),
            dir,
            entries: Vec::new(),
            selected: None,
            error: None,
        };
        pane.refresh(false);
        pane
    }

    /// Reads the folder again (after a download, or on Refresh).
    pub fn refresh(&mut self, hidden: bool) {
        match read_dir(&self.dir, hidden) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Cannot read {}: {error}", self.dir.display())),
        }
        self.path_field = self.dir.display().to_string();
    }

    fn go(&mut self, dir: PathBuf, hidden: bool) {
        if dir.is_dir() {
            self.dir = dir;
            self.selected = None;
        }
        self.refresh(hidden);
    }

    /// Draws the pane; returns an action for the tab.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, hidden: bool) -> Option<LocalAction> {
        let mut action = None;
        ui.add_space(space::XS);
        ui.label(RichText::new("This computer").strong());
        ui.horizontal(|ui| {
            if ui.button("Up").clicked()
                && let Some(parent) = self.dir.parent().map(Path::to_path_buf)
            {
                self.go(parent, hidden);
            }
            if ui.button("Refresh").clicked() {
                self.refresh(hidden);
            }
            let field = ui
                .add(egui::TextEdit::singleline(&mut self.path_field).desired_width(f32::INFINITY));
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                let target = PathBuf::from(self.path_field.trim());
                self.go(target, hidden);
            }
        });
        let selected_file = self
            .selected
            .as_ref()
            .filter(|name| {
                self.entries
                    .iter()
                    .any(|entry| &entry.name == *name && !entry.is_dir)
            })
            .map(|name| self.dir.join(name));
        if ui
            .add_enabled(
                selected_file.is_some(),
                egui::Button::new("Upload \u{2192}"),
            )
            .on_hover_text("Upload the selected file to the remote folder")
            .clicked()
            && let Some(path) = selected_file
        {
            action = Some(LocalAction::Upload(path));
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::color(palette.status_error), error);
        }
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("local-entries")
            .show(ui, |ui| {
                for entry in &self.entries {
                    let selected = self.selected.as_deref() == Some(entry.name.as_str());
                    let row = ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        let icon = if entry.is_dir {
                            icons::FOLDER
                        } else {
                            icons::FILE
                        };
                        icons::paint(
                            ui.painter(),
                            icon,
                            rect,
                            theme::color(palette.text_secondary),
                        );
                        ui.selectable_label(selected, &entry.name)
                    });
                    let response = row.inner;
                    if response.clicked() {
                        self.selected = Some(entry.name.clone());
                    }
                    if response.double_clicked()
                        || (response.has_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                    {
                        open = Some(entry.clone());
                    }
                    if !entry.is_dir {
                        ui.label(
                            RichText::new(crate::sftp_view::human_size(entry.size))
                                .size(text::SECONDARY),
                        );
                    }
                }
            });
        if let Some(entry) = open {
            if entry.is_dir {
                let dir = self.dir.join(&entry.name);
                self.go(dir, hidden);
            } else {
                action = Some(LocalAction::Upload(self.dir.join(&entry.name)));
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_first_and_dot_files_hide_unless_asked() {
        let dir = std::env::temp_dir().join(format!("annulus-local-pane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Zeta")).unwrap();
        std::fs::write(dir.join("alpha.txt"), b"12345").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let names: Vec<String> = read_dir(&dir, false)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["Zeta", "alpha.txt"]);
        let all = read_dir(&dir, true).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().find(|e| e.name == "alpha.txt").unwrap().size, 5);
    }
}
