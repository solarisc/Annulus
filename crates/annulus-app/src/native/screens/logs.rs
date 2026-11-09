//! The Logs screen: saved session rows, export, open, and confirmed deletion.

use annulus_ui::gfx::Rect;
use annulus_ui::icons;
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::Tone;
use annulus_vault::vault::Vault;

use super::super::pages::{self, ListMotion, ROW_H, RowLook};
use super::{GAP, ScreenAction, confirm_delete, row_buttons, rows};
use crate::logs_screen;

/// What a log's context menu offers.
const LOG_MENU: [&str; 3] = ["Open", "Export to Downloads", "Delete"];

/// The Logs screen.
#[derive(Default)]
pub struct LogsView {
    /// The log a delete waits for a yes on.
    confirm: Option<u64>,
    pub(super) rows: ListMotion,
}

impl LogsView {
    /// Draws the screen; opening a log shows it in a tab, and a log is deleted only after a
    /// yes (it cannot be brought back).
    #[allow(clippy::too_many_lines)] // The header, then each row with its buttons and menu.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        recording: bool,
    ) -> Option<ScreenAction> {
        let mut area = pages::page(rect);
        let count = vault.logs().len();
        let subtitle = if recording {
            "Finished sessions are saved here, encrypted (the last 5 per host)."
        } else {
            "Saving session logs is off; turn it on in Settings."
        };
        pages::header(ui, &mut area, "Logs", Some(count), subtitle);
        if count == 0 {
            self.rows.empty(ui, area, ROW_H + GAP);
            pages::empty(
                ui,
                area,
                icons::LOGS,
                "No logs yet",
                "Sessions you close will appear here.",
            );
            return None;
        }
        let list: Vec<(u64, String, u64, String)> = vault
            .logs()
            .iter()
            .rev()
            .map(|log| (log.id, log.title.clone(), log.saved_at, log.text.clone()))
            .collect();
        let mut action = None;
        let mut export = None;
        let mut remove = None;
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        let menu: Vec<String> = LOG_MENU.iter().map(|item| (*item).to_owned()).collect();
        let about = |saved_at: u64, text: &str| {
            #[allow(clippy::cast_precision_loss)] // Logs are a few MiB at most.
            let size = format!("{:.1} KB", text.len() as f64 / 1024.0);
            format!("{} \u{b7} {size}", logs_screen::date(saved_at))
        };
        let looks: Vec<RowLook> = list
            .iter()
            .map(|(id, title, saved_at, text)| RowLook {
                key: *id,
                icon: icons::LOGS,
                title: title.clone(),
                detail: about(*saved_at, text),
                mono: false,
            })
            .collect();
        rows(
            ui,
            "logs.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let (id, title, saved_at, text) = &list[index];
                let zone = Id::new(("log", *id));
                let response = pages::row(ui, zone, row, title, false);
                let (clicked, remainder) = if confirm == Some(*id) {
                    let (answer, remainder) = confirm_delete(ui, row, zone, "Delete log");
                    match answer {
                        Some(true) => remove = Some(*id),
                        Some(false) => confirm_next = None,
                        None => {}
                    }
                    (None, remainder)
                } else {
                    row_buttons(
                        ui,
                        row,
                        zone,
                        &[
                            (icons::TRASH, "Delete"),
                            (icons::DOWNLOAD, "Export to Downloads"),
                        ],
                    )
                };
                let inner = pages::row_icon(ui, remainder, icons::LOGS);
                pages::two_lines(ui, inner, title, &about(*saved_at, text), false);
                // The same on a right click, or Shift+F10.
                let picked = ui.context_menu(zone, &response, &menu);
                match (clicked, picked) {
                    (Some(0), _) | (_, Some(2)) => confirm_next = Some(*id),
                    (Some(_), _) | (_, Some(1)) => export = Some(*id),
                    (_, Some(_)) => action = Some(ScreenAction::Log(title.clone(), text.clone())),
                    (None, None) if response.clicked && confirm != Some(*id) => {
                        action = Some(ScreenAction::Log(title.clone(), text.clone()));
                    }
                    (None, None) => {}
                }
            },
        );
        self.confirm = confirm_next;
        if let Some(id) = export
            && let Some(log) = vault.logs().iter().find(|log| log.id == id)
        {
            match logs_screen::export(log, &logs_screen::downloads()) {
                Ok(path) => ui.toast(format!("Saved to {}", path.display()), Tone::Done),
                Err(error) => ui.toast(format!("Could not export: {error}"), Tone::Problem),
            }
        }
        if let Some(id) = remove {
            self.confirm = None;
            let title = vault
                .logs()
                .iter()
                .find(|log| log.id == id)
                .map(|log| log.title.clone())
                .unwrap_or_default();
            match vault.remove_log(id) {
                Ok(()) => ui.toast(format!("Deleted the log of {title}"), Tone::Done),
                Err(error) => ui.toast(format!("Could not delete: {error}"), Tone::Problem),
            }
        }
        action
    }
}
