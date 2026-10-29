//! An SFTP tab in the new UI (UI-SPEC §4, F-SFTP-002…005): this computer on the left, the
//! host on the right, transfers along the bottom. The connection, listings and resumable
//! transfers are `sftp_view::SftpTab`'s; this draws them.

use std::sync::atomic::Ordering;

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::SessionEvent;
use annulus_sftp::{Entry, join, parent};
use annulus_ui::gfx::{Align, Font, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Id, Key, Modifiers, Role, Ui};
use annulus_ui::widgets::{BODY, BODY_STRONG, CONTROL, FieldStyle, Kind, SMALL, button_width};
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use crate::local_pane::read_dir;
use crate::sftp_view::{
    Command, SftpTab, TransferState, format_time, human_size, pace, valid_name,
};

const ROW: f32 = 30.0;

/// A rename or new folder being typed, on the host.
enum Edit {
    NewFolder(String),
    Rename { from: String, to: String },
}

/// The SFTP tab's view state around the engine.
pub struct SftpView {
    tab: SftpTab,
    ask: Option<Ask>,
    edit: Option<Edit>,
    remote_selected: Option<String>,
    local_selected: Option<String>,
}

impl SftpView {
    /// A view over `tab`.
    #[must_use]
    pub fn new(mut tab: SftpTab) -> Self {
        tab.native = true;
        Self {
            tab,
            ask: None,
            edit: None,
            remote_selected: None,
            local_selected: None,
        }
    }

    /// Draws the tab into `rect`.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect, waker: Option<&Waker>) {
        let waker = waker.cloned();
        self.tab.start_with(move || {
            if let Some(waker) = &waker {
                waker();
            }
        });
        self.tab.drain();
        for event in std::mem::take(&mut self.tab.questions) {
            self.ask = match event {
                SessionEvent::HostKey(question) => Some(Ask::HostKey(question)),
                SessionEvent::Password { user, host } => Some(Ask::Password {
                    user,
                    host,
                    secret: Zeroizing::new(String::new()),
                }),
                _ => self.ask.take(),
            };
        }
        let t = ui.theme;
        let mut area = rect.inset(space::L, space::M);
        if let Some(message) = &self.tab.failed {
            let line = area.take_top(24.0);
            ui.gfx.icon(
                icons::WARNING,
                line.centered(16.0, 16.0).offset(-line.w / 2.0 + 8.0, 0.0),
                t.error,
            );
            ui.gfx.text(
                Rect::new(line.x + 24.0, line.y, line.w - 24.0, line.h),
                message,
                BODY_STRONG,
                t.error,
                Align::Start,
            );
        } else if self.tab.cwd.is_none() {
            ui.gfx.text(
                area.take_top(24.0),
                &format!("Connecting to {}\u{2026}", self.tab.title),
                BODY,
                t.text2,
                Align::Start,
            );
        } else {
            let transfers_h = if self.tab.transfers.is_empty() {
                0.0
            } else {
                150.0
            };
            let transfers = area.take_bottom(transfers_h);
            let half = (area.w - space::M) / 2.0;
            let local = area.take_left(half);
            area.take_left(space::M);
            self.local_pane(ui, local);
            self.remote_pane(ui, area);
            if transfers_h > 0.0 {
                self.transfers(ui, transfers.inset(0.0, 0.0).offset(0.0, 0.0));
            }
        }
        if let (Some(ask), Some(answers)) = (self.ask.as_mut(), self.tab.answers())
            && ask::card(ui, rect, ask, answers)
        {
            self.ask = None;
        }
    }

    fn pane_frame(ui: &mut Ui<'_>, rect: Rect, title: &str, path: &str, icon: IconId) -> Rect {
        let t = ui.theme;
        ui.gfx.fill_rect(rect, 12.0, t.surface);
        ui.gfx.stroke_rect(rect, 12.0, 1.0, t.line);
        let mut inner = rect.inset(space::M, space::S);
        let mut head = inner.take_top(28.0);
        let mark = head.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(icon, mark, t.accent);
        head.take_left(space::S);
        let width = ui.gfx.measure(title, BODY_STRONG).width;
        ui.gfx.text(
            head.take_left(width + 4.0),
            title,
            BODY_STRONG,
            t.text,
            Align::Start,
        );
        head.take_left(space::S);
        ui.gfx
            .text(head, path, Font::mono(11.5), t.text2, Align::Start);
        inner.take_top(4.0);
        inner
    }

    fn header_row(ui: &mut Ui<'_>, rect: Rect) -> Rect {
        let t = ui.theme;
        let mut row = rect;
        let line = row.take_top(22.0);
        let mut cells = line.inset(space::S, 0.0);
        let modified = cells.take_right(130.0);
        let size = cells.take_right(80.0);
        for (cell, title, align) in [
            (cells, "Name", Align::Start),
            (size, "Size", Align::End),
            (modified, "Modified", Align::End),
        ] {
            ui.gfx
                .text(cell, title, SMALL, t.text3.mix(t.text2, 0.5), align);
        }
        ui.gfx
            .fill_rect(Rect::new(line.x, line.b() - 1.0, line.w, 1.0), 0.0, t.line);
        row
    }

    /// One file row; returns (clicked, double-clicked, the action button clicked).
    #[allow(clippy::too_many_arguments)] // One row's every part.
    fn file_row(
        ui: &mut Ui<'_>,
        id: Id,
        row: Rect,
        (name, is_dir): (&str, bool),
        size: &str,
        modified: &str,
        selected: bool,
        action: (IconId, &str),
    ) -> (bool, bool, bool) {
        let t = ui.theme;
        let response = ui.zone(id, row, Role::ListItem, name);
        let hover = response.hovered;
        if selected {
            ui.gfx.fill_rect(row, 6.0, t.selected);
        } else if hover {
            ui.gfx.fill_rect(row, 6.0, t.hover);
        }
        if response.focus_ring {
            ui.focus_ring(row, 6.0);
        }
        let mut cells = row.inset(space::S, 0.0);
        let button = cells.take_right(26.0).centered(26.0, 26.0);
        cells.take_right(space::S);
        let modified_cell = cells.take_right(130.0);
        let size_cell = cells.take_right(80.0);
        let icon = cells.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(
            if is_dir { icons::FOLDER } else { icons::FILE },
            icon,
            if is_dir { t.accent } else { t.text2 },
        );
        cells.take_left(space::S);
        ui.gfx.text(cells, name, BODY, t.text, Align::Start);
        ui.gfx.text(size_cell, size, SMALL, t.text2, Align::End);
        ui.gfx
            .text(modified_cell, modified, SMALL, t.text2, Align::End);
        let mut acted = false;
        if (hover || selected) && !is_dir {
            acted = ui
                .icon_button(id.with("act"), button, action.0, action.1, false)
                .clicked;
        }
        (response.clicked, response.double_clicked, acted)
    }

    fn local_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let dir = self.tab.local.dir.clone();
        let mut inner = Self::pane_frame(
            ui,
            rect,
            "This computer",
            &dir.display().to_string(),
            icons::DEVICE,
        );
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        if ui
            .icon_button(
                Id::new("sftp.local.up"),
                up,
                icons::CHEVRON_UP,
                "Up one folder",
                false,
            )
            .clicked
            && let Some(parent) = dir.parent()
        {
            self.tab.local.dir = parent.to_path_buf();
            self.local_selected = None;
        }
        inner.take_top(space::S);
        let list = Self::header_row(ui, inner);
        let entries = read_dir(&self.tab.local.dir, self.tab.show_hidden).unwrap_or_default();
        #[allow(clippy::cast_precision_loss)] // Folders hold thousands at most.
        let offset = ui.scroll_view(
            Id::new("sftp.local.scroll"),
            list,
            entries.len() as f32 * ROW,
        );
        ui.push_clip(list);
        let mut open = None;
        let mut upload = None;
        for (index, entry) in entries.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * ROW;
            if y + ROW < list.y || y > list.b() {
                continue;
            }
            let row = Rect::new(list.x, y, list.w - 10.0, ROW);
            let size = if entry.is_dir {
                String::new()
            } else {
                human_size(entry.size)
            };
            let (clicked, double, acted) = Self::file_row(
                ui,
                Id::new(("local", entry.name.as_str())),
                row,
                (&entry.name, entry.is_dir),
                &size,
                "",
                self.local_selected.as_deref() == Some(entry.name.as_str()),
                (icons::UPLOAD, "Upload to the host"),
            );
            if clicked {
                self.local_selected = Some(entry.name.clone());
            }
            if double && entry.is_dir {
                open = Some(entry.name.clone());
            } else if (double && !entry.is_dir) || acted {
                upload = Some(self.tab.local.dir.join(&entry.name));
            }
        }
        ui.pop_clip();
        if let Some(name) = open {
            self.tab.local.dir = self.tab.local.dir.join(name);
            self.local_selected = None;
        }
        if let Some(path) = upload {
            self.tab.upload(path);
        }
    }

    #[allow(clippy::too_many_lines)] // Toolbar, an inline edit and the list.
    fn remote_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let cwd = self.tab.cwd.clone().unwrap_or_default();
        let title = self.tab.title.clone();
        let mut inner = Self::pane_frame(ui, rect, &title, &cwd, icons::SERVER);
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let refresh = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let folder_w = button_width(ui.gfx, "New folder", Some(icons::PLUS));
        let new_folder = bar.take_left(folder_w);
        let hidden_w = button_width(ui.gfx, "Hidden files", None);
        let hidden = bar.take_right(hidden_w);
        if ui
            .icon_button(
                Id::new("sftp.up"),
                up,
                icons::CHEVRON_UP,
                "Up one folder",
                false,
            )
            .clicked
        {
            self.tab.navigate(parent(&cwd));
        }
        if ui
            .icon_button(
                Id::new("sftp.refresh"),
                refresh,
                icons::SYNC,
                "Refresh",
                false,
            )
            .clicked
        {
            self.tab.reload();
        }
        if ui
            .button(
                Id::new("sftp.mkdir"),
                new_folder,
                "New folder",
                Some(icons::PLUS),
                Kind::Ghost,
            )
            .clicked
        {
            self.edit = Some(Edit::NewFolder(String::new()));
        }
        let kind = if self.tab.show_hidden {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
        if ui
            .button(Id::new("sftp.hidden"), hidden, "Hidden files", None, kind)
            .clicked
        {
            self.tab.show_hidden = !self.tab.show_hidden;
            self.tab.reload();
            self.tab.local.refresh(self.tab.show_hidden);
        }
        inner.take_top(space::S);
        if let Some(edit) = &mut self.edit {
            let mut line = inner.take_top(CONTROL);
            let save = line.take_right(80.0);
            line.take_right(space::S);
            let (buffer, placeholder) = match edit {
                Edit::NewFolder(name) => (name, "Folder name"),
                Edit::Rename { to, .. } => (to, "New name"),
            };
            let field = ui.text_field(
                Id::new("sftp.edit"),
                line,
                buffer,
                FieldStyle {
                    placeholder,
                    ..FieldStyle::default()
                },
            );
            let done = ui
                .button(Id::new("sftp.edit.save"), save, "Save", None, Kind::Primary)
                .clicked
                || field.submitted;
            if done && valid_name(buffer.trim()) {
                let command = match edit {
                    Edit::NewFolder(name) => Command::CreateDir(join(&cwd, name.trim())),
                    Edit::Rename { from, to } => {
                        Command::Rename(join(&cwd, from), join(&cwd, to.trim()))
                    }
                };
                self.tab.send(command);
                self.edit = None;
            } else if ui.key(Key::Escape, Modifiers::NONE) {
                self.edit = None;
            }
            inner.take_top(space::S);
        }
        if let Some(message) = self
            .tab
            .list_error
            .clone()
            .or_else(|| self.tab.notice.clone())
        {
            ui.gfx
                .text(inner.take_top(20.0), &message, SMALL, t.error, Align::Start);
        }
        let list = Self::header_row(ui, inner);
        let mut entries: Vec<Entry> = self
            .tab
            .entries
            .iter()
            .filter(|entry| self.tab.show_hidden || !entry.name.starts_with('.'))
            .cloned()
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir()
                .cmp(&a.is_dir())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        #[allow(clippy::cast_precision_loss)] // Folders hold thousands at most.
        let offset = ui.scroll_view(
            Id::new("sftp.remote.scroll"),
            list,
            entries.len() as f32 * ROW,
        );
        ui.push_clip(list);
        let mut open = None;
        let mut download = None;
        for (index, entry) in entries.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * ROW;
            if y + ROW < list.y || y > list.b() {
                continue;
            }
            let row = Rect::new(list.x, y, list.w - 10.0, ROW);
            let size = if entry.is_dir() {
                String::new()
            } else {
                human_size(entry.size)
            };
            let modified = entry.modified.map(format_time).unwrap_or_default();
            let (clicked, double, acted) = Self::file_row(
                ui,
                Id::new(("remote", entry.name.as_str())),
                row,
                (&entry.name, entry.is_dir()),
                &size,
                &modified,
                self.remote_selected.as_deref() == Some(entry.name.as_str()),
                (icons::DOWNLOAD, "Download to this computer"),
            );
            if clicked {
                self.remote_selected = Some(entry.name.clone());
            }
            if double && entry.is_dir() {
                open = Some(join(&cwd, &entry.name));
            } else if (double && !entry.is_dir()) || acted {
                download = Some(entry.clone());
            }
        }
        ui.pop_clip();
        if let Some(dir) = open {
            self.remote_selected = None;
            self.tab.navigate(dir);
        }
        if let Some(entry) = download {
            self.tab.download(&entry);
        }
        if let Some(name) = self.remote_selected.clone() {
            if ui.key(Key::F(2), Modifiers::NONE) {
                self.edit = Some(Edit::Rename {
                    from: name.clone(),
                    to: name.clone(),
                });
            }
            if ui.key(Key::Delete, Modifiers::NONE) {
                self.tab.send(Command::Remove(join(&cwd, &name)));
                self.remote_selected = None;
            }
        }
    }

    fn transfers(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let mut area = rect;
        area.take_top(space::M);
        ui.section_label(
            area.take_top(18.0),
            &format!("Transfers \u{b7} {}", self.tab.transfers.len()),
        );
        area.take_top(space::XS);
        #[allow(clippy::cast_precision_loss)] // A few dozen transfers.
        let offset = ui.scroll_view(
            Id::new("sftp.transfers"),
            area,
            self.tab.transfers.len() as f32 * 36.0,
        );
        ui.push_clip(area);
        let mut retry = None;
        for (index, transfer) in self.tab.transfers.iter().enumerate().rev() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = area.y - offset + (self.tab.transfers.len() - 1 - index) as f32 * 36.0;
            let row = Rect::new(area.x, y, area.w - 10.0, 32.0);
            ui.gfx.fill_rect(row, 8.0, t.surface);
            let mut cells = row.inset(space::M, 0.0);
            let button = cells.take_right(26.0).centered(26.0, 26.0);
            cells.take_right(space::S);
            let state_cell = cells.take_right(200.0);
            let label_w = (cells.w * 0.4).min(320.0);
            let label = cells.take_left(label_w);
            ui.gfx
                .text(label, &transfer.label, BODY, t.text, Align::Start);
            cells.take_left(space::M);
            #[allow(clippy::cast_precision_loss)] // A share of the file.
            let share = if transfer.total == 0 {
                0.0
            } else {
                transfer.done as f32 / transfer.total as f32
            };
            let track = cells.centered(cells.w, 6.0);
            ui.gfx.fill_rect(track, 3.0, t.hover);
            let (words, color) = match &transfer.state {
                TransferState::Running => {
                    let seconds = transfer
                        .since
                        .map_or(0.0, |(at, _)| at.elapsed().as_secs_f64());
                    let moved = transfer
                        .since
                        .map_or(0, |(_, from)| transfer.done.saturating_sub(from));
                    (
                        pace(moved, transfer.total.saturating_sub(transfer.done), seconds),
                        t.accent,
                    )
                }
                TransferState::Done(_) => ("Done".to_owned(), t.ok),
                TransferState::Skipped => ("Skipped: already there".to_owned(), t.text3),
                TransferState::Failed(message) => (message.clone(), t.error),
            };
            let fill = if matches!(transfer.state, TransferState::Done(_)) {
                1.0
            } else {
                share
            };
            ui.gfx.fill_rect(
                Rect::new(track.x, track.y, track.w * fill, track.h),
                3.0,
                color,
            );
            ui.gfx.text(state_cell, &words, SMALL, t.text2, Align::End);
            let id = Id::new(("transfer", transfer.id));
            let running = matches!(transfer.state, TransferState::Running);
            let failed = matches!(transfer.state, TransferState::Failed(_));
            if running
                && ui
                    .icon_button(id, button, icons::CLOSE, "Cancel", false)
                    .clicked
            {
                transfer.cancel.store(true, Ordering::Relaxed);
            }
            if failed
                && ui
                    .icon_button(id, button, icons::SYNC, "Retry", false)
                    .clicked
            {
                retry = Some(transfer.id);
            }
        }
        ui.pop_clip();
        if let Some(id) = retry {
            self.tab.retry(id);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_session::{Hop, SshFiles, Target};
    use annulus_ui::kit::{Theme, UiState};

    use super::*;

    /// End to end on test VM 1: the new UI's SFTP tab connects, trusts the new host key
    /// through the same answer the card sends, lists the home folder, and is photographed.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_the_new_sftp_tab_lists_a_real_server() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let dir = std::env::temp_dir().join(format!("annulus-native-sftp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("local-file.txt"), "hello").unwrap();
        let hop = Hop {
            target: Target {
                host: var("ANNULUS_E2E_VM1"),
                port: 22,
                user: var("ANNULUS_E2E_USER"),
            },
            password: None,
            identity: Some(annulus_session::Password::new(
                &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
            )),
            proxy: None,
        };
        let files = SshFiles {
            known_hosts: dir.join("known_hosts"),
            key_dir: None,
        };
        let mut tab = SftpTab::new("test-1 files", hop, files, Vec::new());
        tab.local.dir.clone_from(&dir);
        let mut view = SftpView::new(tab);
        let mut state = UiState::default();
        let mut text = Some(TextEngine::new());
        let started = std::time::Instant::now();
        let viewport = Rect::new(0.0, 0.0, 1100.0, 680.0);
        loop {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "no listing; failed: {:?}",
                view.tab.failed
            );
            let mut canvas = CpuCanvas::new(1100, 680, 1.0, text.take().unwrap()).unwrap();
            canvas.clear(Theme::new(Appearance::Dark).bg);
            state.begin_frame(&[], started.elapsed().as_secs_f64());
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                &mut state,
                viewport,
            );
            view.draw(&mut ui, viewport, None);
            state.end_frame();
            if matches!(view.ask, Some(Ask::HostKey(_))) {
                view.tab.answers().unwrap().host_key(true);
                view.ask = None;
            }
            assert_eq!(view.tab.failed, None);
            if view.tab.cwd.is_some() && !view.tab.entries.is_empty() {
                let shots = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/tmp/scenes");
                std::fs::create_dir_all(&shots).unwrap();
                std::fs::write(
                    shots.join("sftp-live-vm1.png"),
                    canvas.encode_png().unwrap(),
                )
                .unwrap();
                return;
            }
            text = Some(canvas.into_text_engine());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
