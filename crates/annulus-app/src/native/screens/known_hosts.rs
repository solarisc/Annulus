//! The Known Hosts screen and its file-change refresh.

use annulus_config::tokens::space;
use annulus_ssh::known::{self, KnownHost};
use annulus_ui::gfx::{Align, Canvas as _, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::{Kind, SMALL};

use super::super::pages::{self, Fold, ListMotion, ROW_H, RowLook};
use super::{GAP, row_buttons, rows};

/// The Known Hosts screen.
#[derive(Default)]
pub struct KnownView {
    pub(super) entries: Option<Vec<KnownHost>>,
    /// The file's size and time when `entries` was read: a change (a key trusted while
    /// connecting) reads it again (UI review #10).
    read_at: Option<(u64, Option<std::time::SystemTime>)>,
    message: Fold<String>,
    pub(super) rows: ListMotion,
}

/// Which row `entry` is for as long as the file has it: its line number changes when a line
/// above it goes.
fn known_key(entry: &KnownHost) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (&entry.hosts, &entry.algorithm, &entry.fingerprint).hash(&mut hash);
    hash.finish()
}

impl KnownView {
    /// Draws the screen over the known-hosts file at `path`.
    #[allow(clippy::too_many_lines)] // Header, message and list, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        path: &std::path::Path,
        openssh: Option<&std::path::Path>,
    ) {
        let stamp = std::fs::metadata(path)
            .ok()
            .map(|meta| (meta.len(), meta.modified().ok()));
        if stamp != self.read_at {
            self.read_at = stamp;
            self.entries = None;
        }
        let entries = self
            .entries
            .get_or_insert_with(|| known::read(path))
            .clone();
        let mut area = pages::page(rect);
        let mut actions = pages::header(
            ui,
            &mut area,
            "Known Hosts",
            Some(entries.len()),
            "Server keys you trusted. A changed key stops the connection.",
        );
        if let Some(openssh) = openssh
            && pages::action(
                ui,
                &mut actions,
                "known.import",
                "Import from OpenSSH",
                Some(icons::DOWNLOAD),
                Kind::Secondary,
            )
        {
            self.message.open(match known::import(path, openssh) {
                Ok(_) => "Imported the keys OpenSSH trusts.".into(),
                Err(error) => format!("Could not import: {error}"),
            });
            self.entries = None;
        }
        self.message.draw(
            ui,
            Id::new("known.message"),
            &mut area,
            |ui, area, message| {
                let t = ui.theme;
                ui.gfx
                    .text(area.take_top(20.0), message, SMALL, t.text2, Align::Start);
                area.take_top(space::S);
            },
        );
        if entries.is_empty() {
            self.rows.empty(ui, area, ROW_H + GAP);
            pages::empty(
                ui,
                area,
                icons::SHIELD,
                "No trusted keys yet",
                "The first connection to a server asks you to trust its key.",
            );
            return;
        }
        let mut forget = None;
        let looks: Vec<RowLook> = entries
            .iter()
            .map(|entry| RowLook {
                key: known_key(entry),
                icon: icons::SHIELD,
                title: entry.hosts.join(", "),
                detail: format!("{}  {}", entry.algorithm, entry.fingerprint),
                mono: true,
            })
            .collect();
        rows(
            ui,
            "known.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let entry = &entries[index];
                let names = entry.hosts.join(", ");
                let zone = Id::new(("known", entry.line));
                pages::row(ui, zone, row, &names, false);
                let (clicked, remainder) =
                    row_buttons(ui, row, zone, &[(icons::TRASH, "Forget this key")]);
                let inner = pages::row_icon(ui, remainder, icons::SHIELD);
                pages::two_lines(
                    ui,
                    inner,
                    &names,
                    &format!("{}  {}", entry.algorithm, entry.fingerprint),
                    true,
                );
                if clicked == Some(0) {
                    forget = Some(entry.clone());
                }
            },
        );
        if let Some(entry) = forget {
            self.message.open(match known::forget(path, &entry) {
                Ok(()) => "Forgotten. The next connection asks again.".into(),
                Err(error) => format!("Could not change the file: {error}"),
            });
            self.entries = None;
        }
    }
}
