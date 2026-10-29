//! The Logs screen (F-LOG-002…004): saved session logs, newest first, filtered by title;
//! one opens read-only with search, saves as a text file, or is deleted.

use std::path::PathBuf;

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::theme;
use annulus_vault::logs::SessionLog;
use annulus_vault::vault::Vault;

/// The Logs screen's state.
#[derive(Default)]
pub struct LogsScreen {
    filter: String,
    open: Option<u64>,
    search: String,
    message: Option<(bool, String)>,
}

/// `YYYY-MM-DD HH:MM` (UTC) for seconds since the Unix epoch.
pub(crate) fn date(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let minutes = seconds % 86_400 / 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        minutes / 60,
        minutes % 60
    )
}

/// A file name for a log: its title and date, with characters files cannot have replaced.
fn file_name(log: &SessionLog) -> String {
    let title: String = log
        .title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{title}-{}.txt",
        date(log.saved_at).replace([' ', ':'], "-")
    )
}

/// The user's Downloads folder, else home, else the temp folder.
pub(crate) fn downloads() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| {
            dirs.download_dir()
                .map(std::path::Path::to_path_buf)
                .or_else(|| Some(dirs.home_dir().to_path_buf()))
        })
        .unwrap_or_else(std::env::temp_dir)
}

/// Writes `log` to `dir` under a free name (never over an existing file).
pub(crate) fn export(log: &SessionLog, dir: &std::path::Path) -> std::io::Result<PathBuf> {
    let name = file_name(log);
    let mut path = dir.join(&name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(name.replace(".txt", &format!(" ({n}).txt")));
        n += 1;
    }
    std::fs::write(&path, &log.text)?;
    Ok(path)
}

impl LogsScreen {
    /// Draws the screen.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault, recording: bool) {
        ui.add_space(space::S);
        if !recording {
            ui.label(
                RichText::new("Session logs are off. Turn on \"Save session logs\" in Settings to keep the text of finished sessions.")
                    .color(theme::color(palette.text_secondary)),
            );
        }
        if let Some(id) = self.open {
            self.viewer(ui, palette, vault, id);
            return;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter by host")
                .desired_width(280.0),
        );
        if let Some((error, message)) = &self.message {
            let color = if *error {
                palette.status_error
            } else {
                palette.text_secondary
            };
            ui.colored_label(theme::color(color), message);
        }
        ui.add_space(space::S);
        let filter = self.filter.to_lowercase();
        let logs: Vec<SessionLog> = vault
            .logs()
            .iter()
            .filter(|log| filter.is_empty() || log.title.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        if logs.is_empty() {
            ui.label("No logs.");
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for log in &logs {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&log.title).size(text::BODY + 1.0).strong());
                        ui.label(
                            RichText::new(format!(
                                "{}  \u{b7}  {} lines",
                                date(log.saved_at),
                                log.text.lines().count()
                            ))
                            .size(text::SECONDARY)
                            .color(theme::color(palette.text_secondary)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Delete").clicked() {
                            self.message = vault
                                .remove_log(log.id)
                                .err()
                                .map(|error| (true, error.to_string()));
                        }
                        if ui.button("Open").clicked() {
                            self.open = Some(log.id);
                            self.search.clear();
                        }
                    });
                });
                ui.separator();
            }
        });
    }

    fn viewer(&mut self, ui: &mut Ui, palette: &Palette, vault: &Vault, id: u64) {
        let Some(log) = vault.logs().iter().find(|log| log.id == id).cloned() else {
            self.open = None;
            return;
        };
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape))
            {
                self.open = None;
            }
            ui.label(
                RichText::new(format!("{}  \u{b7}  {}", log.title, date(log.saved_at))).strong(),
            );
            if ui.button("Save as text file").clicked() {
                self.message = Some(match export(&log, &downloads()) {
                    Ok(path) => (false, format!("Saved to {}", path.display())),
                    Err(error) => (true, format!("Could not save: {error}")),
                });
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Show lines with\u{2026}")
                    .desired_width(200.0),
            );
        });
        if let Some((error, message)) = &self.message {
            let color = if *error {
                palette.status_error
            } else {
                palette.text_secondary
            };
            ui.colored_label(theme::color(color), message);
        }
        let query = self.search.to_lowercase();
        let shown: String = if query.is_empty() {
            log.text
        } else {
            log.text
                .lines()
                .filter(|line| line.to_lowercase().contains(&query))
                .collect::<Vec<_>>()
                .join("\n")
        };
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut shown.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_get_a_safe_unique_name_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("annulus-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = SessionLog {
            id: 1,
            title: "admin@db:22 / prod".into(),
            saved_at: 0,
            text: "hello\n".into(),
        };
        let first = export(&log, &dir).unwrap();
        let second = export(&log, &dir).unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "admin_db_22___prod-1970-01-01-00-00.txt"
        );
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(second).unwrap(), "hello\n");
    }
}
