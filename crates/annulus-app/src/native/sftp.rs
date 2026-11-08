//! An SFTP tab in the new UI (UI-SPEC §4, F-SFTP-001…005): this computer on the left, the
//! host on the right, transfers along the bottom. Both listings filter by name, sort by their
//! column headers and show the columns the user picks (a right click on the headers): size,
//! type, modified, permissions, owner, group, accessed and created. A right click on an entry
//! (or its More button) offers what a file manager does; on empty space, what the folder
//! does. A file dragged from one pane onto the other uploads or downloads it, and files
//! dragged in from the OS (Explorer, Finder, a file manager) upload to the host or, let go
//! over this computer's pane, are copied into its folder; while anything is dragged, the pane
//! it would land in lights up and says what will happen there. A name already taken where a
//! file goes asks first: skip, replace or rename it. The connection, listings and resumable
//! transfers are `sftp_view::SftpTab`'s; this draws them.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};

use annulus_config::tokens::space;
use annulus_platform::window::{self as platform, Waker};
use annulus_sftp::{Conflict, Entry, Owners, join, keep_both_name, parent};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Font, Point, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{FOCUS_REACH, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::{Curve, Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, Tone, button_width,
};

use super::ask::{self, Ask};
use super::pages;
use crate::local_pane::{read_dir, tilde};
use crate::sftp_view::{
    ASK_NAME, Column, Command, Dialog, Row, SftpTab, TransferState, conflict_name, format_time,
    human_size, pace, parse_mode, properties, sort_rows, valid_name,
};

const ROW: f32 = 30.0;

/// Which pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    /// This computer, on the left.
    Local,
    /// The host, on the right.
    Remote,
}

/// What is being dragged from one pane to the other.
#[derive(Clone)]
enum Dragged {
    /// A file on this computer, to upload.
    Local(std::path::PathBuf),
    /// An entry on the host, to download.
    Remote(Entry),
}

impl Dragged {
    fn name(&self) -> String {
        match self {
            Dragged::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Dragged::Remote(entry) => entry.name.clone(),
        }
    }
}

/// A rename or new folder being typed, in one pane.
enum Edit {
    NewFolder(String),
    Rename { from: String, to: String },
}

/// What comes into a folder where its name is already taken, waiting on the user.
#[derive(Clone, Debug)]
enum Incoming {
    /// A file or folder on this computer, to upload to the host's folder shown.
    Upload(PathBuf),
    /// An entry on the host, to download into this computer's folder shown.
    Download(Entry),
    /// A file or folder dragged in from the OS, to copy into this computer's folder shown.
    Copy(PathBuf),
}

/// A name taken where something goes (F-SFTP-005): skip it, replace what is there, or give
/// it another name.
#[derive(Debug)]
struct Clash {
    incoming: Incoming,
    name: String,
    /// A folder: replacing merges it, what is inside replacing files of the same name.
    folder: bool,
    /// Where it goes, as the pane shows it.
    place: String,
    /// What is there now and what comes, as "12.3 KiB · 2026-11-01 10:22".
    there: String,
    coming: String,
    /// The names taken where it goes, so a new name can be checked.
    taken: HashSet<String>,
    /// The name typed for Rename; a free one to start with.
    rename: String,
}

/// The user's answer to a clash.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Answer {
    Skip,
    Replace,
    Rename(String),
}

/// What a card's button asks for.
enum Act {
    /// A command for the host.
    Send(Command),
    /// Delete this file or folder on this computer.
    DeleteLocal(PathBuf),
}

/// A copy into this computer's folder finished on its thread: the name, or why it failed.
type CopyNews = Result<String, String>;

/// The SFTP tab's view state around the engine.
#[allow(clippy::struct_excessive_bools)] // Separate switches of one view.
pub struct SftpView {
    tab: SftpTab,
    ask: Option<Ask>,
    /// A rename or new folder being typed, and in which pane.
    edit: Option<(Side, Edit)>,
    /// The folder the edit was started in: it ends when another folder is shown.
    edit_dir: String,
    remote_selected: Option<String>,
    local_selected: Option<String>,
    /// The pane last clicked in: F2 and Delete act on its selection.
    active: Side,
    /// What this computer's listing is narrowed to.
    local_filter: String,
    /// The column this computer's listing is sorted by, and whether ascending.
    local_sort: (Column, bool),
    /// The columns each pane shows beside Name.
    local_columns: Vec<Column>,
    remote_columns: Vec<Column>,
    /// The user changed the columns since [`Self::take_columns`] last looked.
    columns_changed: bool,
    /// A row being dragged this frame, and where the pointer is.
    drag: Option<(Dragged, Point)>,
    /// A row let go this frame, and where.
    dropped: Option<(Dragged, Point)>,
    /// Names taken where things go, asked one after another.
    clashes: VecDeque<Clash>,
    /// The answer given to one clash goes for the others waiting.
    clash_all: bool,
    /// Copies into this computer's folder report here when done.
    copies: (Sender<CopyNews>, Receiver<CopyNews>),
    /// Wakes the window when a copy is done.
    waker: Option<Waker>,
    /// A card just opened: its first control takes the keyboard.
    focus_card: bool,
    /// The transfers panel shows its rows, not only its header.
    transfers_open: bool,
    /// The card that just closed, on its way out, and since when.
    card_left: Option<(Dialog, f64)>,
}

impl SftpView {
    /// A view over `tab`.
    #[must_use]
    pub fn new(tab: SftpTab) -> Self {
        // In the order they are kept when room runs short.
        let defaults = || {
            [
                Column::Modified,
                Column::Size,
                Column::Permissions,
                Column::Type,
                Column::Owner,
            ]
            .to_vec()
        };
        Self {
            tab,
            ask: None,
            edit: None,
            edit_dir: String::new(),
            remote_selected: None,
            local_selected: None,
            active: Side::Remote,
            local_filter: String::new(),
            local_sort: (Column::Name, true),
            local_columns: defaults(),
            remote_columns: defaults(),
            columns_changed: false,
            drag: None,
            dropped: None,
            clashes: VecDeque::new(),
            clash_all: false,
            copies: channel(),
            waker: None,
            focus_card: false,
            transfers_open: true,
            card_left: None,
        }
    }

    /// The same view showing the columns named in the settings (`local`, `remote`).
    #[must_use]
    pub fn with_columns(mut self, local: &[String], remote: &[String]) -> Self {
        let read = |keys: &[String]| -> Vec<Column> {
            keys.iter()
                .filter_map(|key| Column::from_key(key))
                .filter(|column| *column != Column::Name)
                .collect()
        };
        self.local_columns = read(local);
        self.remote_columns = read(remote);
        self
    }

    /// The columns of each pane by name, once the user changed them, for the settings.
    pub fn take_columns(&mut self) -> Option<(Vec<String>, Vec<String>)> {
        let names = |columns: &[Column]| columns.iter().map(|c| c.key().to_owned()).collect();
        std::mem::take(&mut self.columns_changed)
            .then(|| (names(&self.local_columns), names(&self.remote_columns)))
    }

    /// Opens `dialog` over the tab.
    fn open_card(&mut self, dialog: Dialog) {
        self.tab.dialog = Some(dialog);
        self.focus_card = true;
    }

    /// Drops the connection's secrets, as when the vault locks.
    pub fn forget_secrets(&mut self) {
        self.tab.forget_secrets();
    }

    /// Draws the tab into `rect`.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect, waker: Option<&Waker>) {
        self.waker = waker.cloned();
        let waker = waker.cloned();
        self.tab.start_with(move || {
            if let Some(waker) = &waker {
                waker();
            }
        });
        self.tab.drain();
        for label in std::mem::take(&mut self.tab.finished) {
            let name = label.trim_start_matches(['\u{2191}', '\u{2193}']).trim();
            let what = if label.starts_with('\u{2191}') {
                "Uploaded"
            } else {
                "Downloaded"
            };
            ui.toast(format!("{what} {name}"), Tone::Done);
        }
        for news in self.copies.1.try_iter().collect::<Vec<_>>() {
            match news {
                Ok(name) => ui.toast(format!("Copied {name}"), Tone::Done),
                Err(why) => ui.toast(why, Tone::Problem),
            }
        }
        for event in std::mem::take(&mut self.tab.questions) {
            if let Some(question) = Ask::from_event(event) {
                self.ask = Some(question);
            }
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
            // The panel rises when the first transfer starts and folds to its header.
            let wanted = if self.tab.transfers.is_empty() {
                0.0
            } else if self.transfers_open {
                TRANSFERS_H
            } else {
                TRANSFERS_HEAD + space::M + space::S
            };
            let transfers_h = ui
                .animate(Id::new("sftp.transfers.height"), wanted, Motion::BASE)
                .max(0.0);
            let transfers = area.take_bottom(transfers_h);
            let half = (area.w - space::M) / 2.0;
            let local = area.take_left(half);
            area.take_left(space::M);
            // Each pane keeps to its place: in a small window its list does not run under
            // the transfers.
            ui.push_clip(local);
            self.local_pane(ui, local);
            ui.pop_clip();
            ui.push_clip(area);
            self.remote_pane(ui, area);
            ui.pop_clip();
            self.drag_and_drop(ui, local, area);
            self.keys(ui);
            if transfers_h > 1.0 && !self.tab.transfers.is_empty() {
                ui.push_clip(transfers);
                self.transfers(ui, transfers);
                ui.pop_clip();
            }
        }
        self.dialog_card(ui, rect);
        if self.tab.dialog.is_none() && self.card_left.is_none() {
            self.clash_card(ui, rect);
        }
        if let (Some(ask), Some(answers)) = (self.ask.as_mut(), self.tab.answers())
            && ask::card(ui, Id::new("sftp.ask"), rect, ask, answers)
        {
            self.ask = None;
        }
    }

    /// F2 renames and Delete deletes (after a yes) the selection of the pane last clicked in.
    fn keys(&mut self, ui: &mut Ui<'_>) {
        if self.tab.dialog.is_some() || !self.clashes.is_empty() || self.edit.is_some() {
            return;
        }
        let rename = ui.key(Key::F(2), Modifiers::NONE);
        let delete = ui.key(Key::Delete, Modifiers::NONE);
        if !rename && !delete {
            return;
        }
        match self.active {
            Side::Remote => {
                let Some(entry) = self
                    .remote_selected
                    .as_ref()
                    .and_then(|name| self.tab.entries.iter().find(|entry| &entry.name == name))
                else {
                    return;
                };
                if rename {
                    self.start_edit(Side::Remote, Some(entry.name.clone()));
                } else {
                    // It cannot be undone, so it asks first.
                    let card = Dialog::Delete(entry.clone());
                    self.open_card(card);
                }
            }
            Side::Local => {
                let Some(name) = self.local_selected.clone() else {
                    return;
                };
                if rename {
                    self.start_edit(Side::Local, Some(name));
                } else {
                    self.ask_delete_local(&name);
                }
            }
        }
    }

    /// Starts typing a new folder's name (`None`) or `rename`'s new one in `side`'s folder.
    fn start_edit(&mut self, side: Side, rename: Option<String>) {
        let edit = match rename {
            Some(name) => Edit::Rename {
                from: name.clone(),
                to: name,
            },
            None => Edit::NewFolder(String::new()),
        };
        self.edit = Some((side, edit));
        self.edit_dir = self.folder_of(side);
    }

    /// The folder `side` shows, as text.
    fn folder_of(&self, side: Side) -> String {
        match side {
            Side::Local => self.tab.local.dir.display().to_string(),
            Side::Remote => self.tab.cwd.clone().unwrap_or_default(),
        }
    }

    /// Opens the card that asks before `name` in this computer's folder is deleted.
    fn ask_delete_local(&mut self, name: &str) {
        let path = self.tab.local.dir.join(name);
        let folder = path.is_dir();
        self.open_card(Dialog::DeleteLocal { path, folder });
    }

    /// Rows dragged between the panes, and files dragged in from the OS: what is let go over a
    /// pane that takes it is uploaded, downloaded or copied there. While anything moves, the
    /// pane it may land in is outlined, and the one under the pointer lights up and says what
    /// will happen and where.
    fn drag_and_drop(&mut self, ui: &mut Ui<'_>, local: Rect, remote: Rect) {
        let t = ui.theme;
        let dropped = ui.take_dropped_files();
        if !dropped.is_empty() {
            // Over this computer's pane: copied into its folder. Anywhere else, and where the
            // OS does not say: up to the host.
            let here = ui.dropped_at().is_some_and(|at| local.contains(at));
            for path in dropped {
                if here {
                    self.copy_in(path);
                } else {
                    self.send_up(path);
                }
            }
        }
        if let Some((dragged, at)) = self.dropped.take() {
            match dragged {
                Dragged::Local(path) if remote.contains(at) => self.send_up(path),
                Dragged::Remote(entry) if local.contains(at) => self.send_down(entry),
                _ => {}
            }
        }
        // Where a drag on the way may land: the other pane for a row; for files from the OS
        // either pane, or the host's only when the OS does not say where they are.
        let os = ui.files_hovering().map(|(paths, at)| (paths.to_vec(), at));
        let row = self.drag.take();
        let from_os = match &os {
            Some((_, Some(at))) => FromOs::At(*at),
            Some((_, None)) => FromOs::Somewhere,
            None => FromOs::No,
        };
        let targets = drop_targets(row.as_ref(), from_os);
        let (at, what) = match (&row, &os) {
            (Some((dragged, at)), _) => (Some(*at), dragged.name()),
            (None, Some((paths, at))) => (*at, items_named(paths)),
            (None, None) => (None, String::new()),
        };
        for side in [Side::Local, Side::Remote] {
            let pane = if side == Side::Local { local } else { remote };
            let target = targets.contains(&side);
            let over = target && at.is_none_or(|at| pane.contains(at));
            let lit = ui.ease(
                Id::new(("sftp.drop.lit", side == Side::Local)),
                if over { 1.0 } else { 0.0 },
            );
            let shown = ui.ease(
                Id::new(("sftp.drop.shown", side == Side::Local)),
                if target { 1.0 } else { 0.0 },
            );
            if shown <= 0.01 && lit <= 0.01 {
                continue;
            }
            // Every pane that would take it is outlined; the one under the pointer is filled.
            let outline = pane.inset(1.0, 1.0);
            ui.gfx.fill_rect(outline, 12.0, t.accent.alpha(0.10 * lit));
            ui.gfx.stroke_rect(
                outline,
                12.0,
                1.0 + lit,
                t.accent.alpha(shown * (0.35 + 0.65 * lit)),
            );
            if lit <= 0.01 {
                continue;
            }
            let (icon, title) = match (side, &row) {
                (Side::Remote, _) => (icons::UPLOAD, format!("Upload to {}", self.host_name())),
                (Side::Local, Some(_)) => (icons::DOWNLOAD, "Download here".to_owned()),
                (Side::Local, None) => (icons::COPY, "Copy here".to_owned()),
            };
            let folder = match side {
                Side::Local => {
                    let home = home_dir();
                    tilde(&self.tab.local.dir.display().to_string(), &home)
                }
                Side::Remote => tilde(&self.tab.cwd.clone().unwrap_or_default(), &self.tab.home),
            };
            Self::drop_badge(ui, pane, lit, icon, &title, &what, &folder);
        }
        let Some((dragged, at)) = row else {
            return;
        };
        // A label follows the pointer, saying what letting go there does.
        let over = match dragged {
            Dragged::Local(_) => remote.contains(at),
            Dragged::Remote(_) => local.contains(at),
        };
        let (words, icon) = match (&dragged, over) {
            (Dragged::Local(_), true) => ("Upload to the host", icons::UPLOAD),
            (Dragged::Remote(_), true) => ("Download here", icons::DOWNLOAD),
            (Dragged::Local(_), false) => ("Drop on the host's side", icons::UPLOAD),
            (Dragged::Remote(_), false) => ("Drop on this computer's side", icons::DOWNLOAD),
        };
        let label = format!("{}  \u{b7}  {words}", dragged.name());
        let width = ui.gfx.measure(&label, SMALL).width + 16.0 + 3.0 * space::S;
        let chip = Rect::new(at.x + 14.0, at.y + 10.0, width, 28.0);
        ui.gfx.shadow(chip, 8.0, 12.0, 4.0, t.shadow);
        ui.gfx.fill_rect(chip, 8.0, t.raised);
        ui.gfx
            .stroke_rect(chip, 8.0, 1.0, if over { t.accent } else { t.line_strong });
        let mut inner = chip.inset(space::S, 0.0);
        ui.gfx.icon(
            icon,
            inner.take_left(16.0).centered(14.0, 14.0),
            if over { t.accent } else { t.text2 },
        );
        inner.take_left(space::S);
        ui.gfx.text(inner, &label, SMALL, t.text, Align::Start);
    }

    /// The host's name, as its pane's title shows it.
    fn host_name(&self) -> String {
        self.tab
            .title
            .strip_suffix(" files")
            .unwrap_or(&self.tab.title)
            .to_owned()
    }

    /// The badge in the middle of a pane something would land in: what happens (`title`), to
    /// what, and into which folder; `lit` of the way in.
    fn drop_badge(
        ui: &mut Ui<'_>,
        pane: Rect,
        lit: f32,
        icon: IconId,
        title: &str,
        what: &str,
        folder: &str,
    ) {
        let t = ui.theme;
        let width = (pane.w - 2.0 * space::L).clamp(0.0, 320.0);
        // Low in the pane, clear of the rows near the top and of the pointer's label.
        let height = 128.0;
        let top = (pane.b() - height - 3.0 * space::XL).max(pane.y + space::XL);
        let badge = Rect::new(pane.center().x - width / 2.0, top, width, height);
        ui.push_layer(Layer::appear(lit, badge.center(), 0.94, 0.0));
        ui.gfx.shadow(badge, 14.0, 24.0, 8.0, t.shadow);
        ui.gfx.fill_rect(badge, 14.0, t.raised);
        ui.gfx.stroke_rect(badge, 14.0, 1.0, t.accent.alpha(0.6));
        let mut column = badge.inset(space::L, space::M);
        let disc = column.take_top(40.0).centered(40.0, 40.0);
        ui.gfx.fill_rect(disc, 20.0, t.accent.alpha(0.16));
        ui.gfx.icon(icon, disc.centered(20.0, 20.0), t.accent);
        column.take_top(space::S);
        let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, title, BODY_STRONG, column.w);
        ui.gfx.text(
            column.take_top(20.0),
            &fitted,
            BODY_STRONG,
            t.text,
            Align::Center,
        );
        let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, what, SMALL, column.w);
        ui.gfx.text(
            column.take_top(18.0),
            &fitted,
            SMALL,
            t.text2,
            Align::Center,
        );
        let into = format!("into {folder}");
        let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &into, Font::mono(11.5), column.w);
        ui.gfx.text(
            column.take_top(18.0),
            &fitted,
            Font::mono(11.5),
            t.text3,
            Align::Center,
        );
        ui.pop_layer();
    }

    /// Uploads `path` to the host's folder shown, asking first when its name is taken there.
    fn send_up(&mut self, path: PathBuf) {
        let Some(cwd) = self.tab.cwd.clone() else {
            return;
        };
        let name = file_name(&path);
        let there = self.tab.entries.iter().find(|entry| entry.name == name);
        match there {
            Some(entry) if self.tab.ask_on_clash => {
                let taken = self.tab.entries.iter().map(|e| e.name.clone()).collect();
                let folder = path.is_dir();
                let clash = Clash {
                    name: name.clone(),
                    folder,
                    place: tilde(&cwd, &self.tab.home),
                    there: facts(entry.is_dir(), entry.size, entry.modified),
                    coming: local_facts(&path),
                    rename: free_name(&name, &taken),
                    taken,
                    incoming: Incoming::Upload(path),
                };
                self.clashes.push_back(clash);
            }
            Some(_) => self.tab.upload(path),
            None => self.tab.upload_named(path, None, Conflict::KeepBoth),
        }
    }

    /// Downloads `entry` into this computer's folder shown, asking first when its name is
    /// taken there.
    fn send_down(&mut self, entry: Entry) {
        let target = self.tab.local.dir.join(&entry.name);
        let taken_here = target.symlink_metadata().is_ok();
        if taken_here && self.tab.ask_on_clash {
            let taken = local_names(&self.tab.local.dir);
            let clash = Clash {
                name: entry.name.clone(),
                folder: entry.is_dir(),
                place: tilde(&self.tab.local.dir.display().to_string(), &home_dir()),
                there: local_facts(&target),
                coming: facts(entry.is_dir(), entry.size, entry.modified),
                rename: free_name(&entry.name, &taken),
                taken,
                incoming: Incoming::Download(entry),
            };
            self.clashes.push_back(clash);
        } else if taken_here {
            self.tab.download(&entry);
        } else {
            self.tab.download_named(&entry, None, Conflict::KeepBoth);
        }
    }

    /// Copies `path`, dragged in from the OS, into this computer's folder shown, asking first
    /// when its name is taken there. A file already in that folder stays as it is.
    fn copy_in(&mut self, path: PathBuf) {
        let dir = self.tab.local.dir.clone();
        let name = file_name(&path);
        if path.parent() == Some(dir.as_path()) {
            return;
        }
        if dir.starts_with(&path) {
            self.copy_failed(format!("{name} cannot be copied into itself"));
            return;
        }
        let target = dir.join(&name);
        if target.symlink_metadata().is_ok() {
            let taken = local_names(&dir);
            let clash = Clash {
                name: name.clone(),
                folder: path.is_dir(),
                place: tilde(&dir.display().to_string(), &home_dir()),
                there: local_facts(&target),
                coming: local_facts(&path),
                rename: free_name(&name, &taken),
                taken,
                incoming: Incoming::Copy(path),
            };
            self.clashes.push_back(clash);
        } else {
            self.copy_now(path, target, false);
        }
    }

    /// Copies `from` to `to` on a thread of its own, replacing what is there when `replace`;
    /// the toast says how it went.
    fn copy_now(&mut self, from: PathBuf, to: PathBuf, replace: bool) {
        let news = self.copies.0.clone();
        let waker = self.waker.clone();
        let name = file_name(&to);
        let spawned = std::thread::Builder::new()
            .name("local copy".into())
            .spawn(move || {
                let result = copy_into(&from, &to, replace)
                    .map(|()| name.clone())
                    .map_err(|error| format!("Could not copy {name}: {error}"));
                let _ = news.send(result);
                if let Some(waker) = waker {
                    waker();
                }
            });
        if let Err(error) = spawned {
            self.copy_failed(format!("Could not copy: {error}"));
        }
    }

    fn copy_failed(&self, why: String) {
        let _ = self.copies.0.send(Err(why));
    }

    /// Does what `answer` says with the first clash waiting; with "the same for the others",
    /// with those too (a rename then takes free names of their own).
    fn answer_clash(&mut self, answer: &Answer) {
        let Some(first) = self.clashes.pop_front() else {
            return;
        };
        let mut settled = vec![(first, answer.clone())];
        if self.clash_all {
            while let Some(next) = self.clashes.pop_front() {
                let answer = match answer {
                    Answer::Rename(_) => Answer::Rename(next.rename.clone()),
                    other => other.clone(),
                };
                settled.push((next, answer));
            }
        }
        for (clash, answer) in settled {
            let (name, conflict) = match &answer {
                Answer::Skip => continue,
                Answer::Replace => (None, Conflict::Overwrite),
                Answer::Rename(name) => (Some(name.trim().to_owned()), Conflict::KeepBoth),
            };
            match clash.incoming {
                Incoming::Upload(path) => self.tab.upload_named(path, name, conflict),
                Incoming::Download(entry) => self.tab.download_named(&entry, name, conflict),
                Incoming::Copy(path) => {
                    let to = self
                        .tab
                        .local
                        .dir
                        .join(name.unwrap_or_else(|| clash.name.clone()));
                    self.copy_now(path, to, conflict == Conflict::Overwrite);
                }
            }
        }
        if self.clashes.is_empty() {
            self.clash_all = false;
        }
    }

    /// The card asking what to do with the first name taken (F-SFTP-005): Skip, Replace (for a
    /// folder, Merge) or Rename under the name in its field, which starts free.
    #[allow(clippy::too_many_lines)] // One card, top to bottom.
    fn clash_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let waiting = self.clashes.len().saturating_sub(1);
        let Some(clash) = self.clashes.front_mut() else {
            return;
        };
        let t = ui.theme;
        let height = if waiting > 0 { 330.0 } else { 296.0 };
        ui.zone(Id::new("sftp.clash.backdrop"), rect, Role::Backdrop, "");
        let card = rect.centered(rect.w.min(500.0), height);
        let card_id = Id::new(("sftp.clash", clash.name.as_str()));
        let first = !ui.state.focus().is_some_and(|focus| {
            [
                Id::new("sftp.clash.name"),
                Id::new("sftp.clash.skip"),
                Id::new("sftp.clash.replace"),
                Id::new("sftp.clash.rename"),
                Id::new("sftp.clash.all"),
            ]
            .contains(&focus)
        });
        pages::card_in(ui, card_id, rect, card);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        let mut buttons = Rect::new(column.x, column.b() - CONTROL, column.w, CONTROL);
        let what = if clash.folder { "A folder" } else { "A file" };
        ui.strict_text(
            column.take_top(24.0),
            &format!("{what} by this name is already there"),
            HEADING,
            t.text,
            Align::Start,
        );
        column.take_top(space::S);
        let fitted =
            annulus_ui::gfx::fit_middle(&mut ui.gfx, &clash.name, Font::mono(13.0), column.w);
        ui.gfx.text(
            column.take_top(20.0),
            &fitted,
            Font::mono(13.0),
            t.text,
            Align::Start,
        );
        let into = format!("in {}", clash.place);
        let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &into, SMALL, column.w);
        ui.gfx
            .text(column.take_top(18.0), &fitted, SMALL, t.text2, Align::Start);
        column.take_top(space::S);
        for (label, value) in [("There now", &clash.there), ("Coming", &clash.coming)] {
            let mut line = column.take_top(20.0);
            ui.gfx
                .text(line.take_left(90.0), label, SMALL, t.text3, Align::Start);
            ui.gfx.text(line, value, SMALL, t.text2, Align::Start);
        }
        column.take_top(space::M);
        let field_id = Id::new("sftp.clash.name");
        if first {
            ui.state.set_focus(Some(field_id), false);
        }
        let typed = clash.rename.trim().to_owned();
        let taken = clash.taken.contains(&typed);
        let bad = !valid_name(&typed) || taken;
        let field = ui.text_field(
            field_id,
            column.take_top(CONTROL),
            &mut clash.rename,
            FieldStyle {
                placeholder: "New name",
                mono: true,
                error: bad,
                ..FieldStyle::default()
            },
        );
        ui.set_name(field_id, "Rename to");
        column.take_top(space::XS);
        let hint = if taken {
            "That name is taken too."
        } else if bad {
            "A name, without / or a folder of its own."
        } else {
            "Rename keeps both, under this name."
        };
        ui.gfx.text(
            column.take_top(18.0),
            hint,
            SMALL,
            if bad { t.error } else { t.text3 },
            Align::Start,
        );
        if waiting > 0 {
            column.take_top(space::S);
            let label = format!(
                "Do the same for the {waiting} other{} waiting",
                if waiting == 1 { "" } else { "s" }
            );
            ui.checkbox(
                Id::new("sftp.clash.all"),
                column.take_top(24.0),
                &mut self.clash_all,
                &label,
            );
        }
        let replace_label = if clash.folder { "Merge" } else { "Replace" };
        let rename = buttons.take_right(button_width(&mut ui.gfx, "Rename", None).max(88.0));
        buttons.take_right(space::S);
        let replace = buttons.take_right(button_width(&mut ui.gfx, replace_label, None));
        buttons.take_right(space::S);
        let skip = buttons.take_right(button_width(&mut ui.gfx, "Skip", None));
        let mut answer = None;
        if ui
            .button(Id::new("sftp.clash.skip"), skip, "Skip", None, Kind::Ghost)
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE)
        {
            answer = Some(Answer::Skip);
        }
        if ui
            .button(
                Id::new("sftp.clash.replace"),
                replace,
                replace_label,
                None,
                Kind::Secondary,
            )
            .clicked
        {
            answer = Some(Answer::Replace);
        }
        if clash.folder {
            ui.tooltip(
                Id::new("sftp.clash.replace"),
                replace,
                "Copies into the folder there; files of the same name are replaced",
            );
        }
        let renamed = ui
            .button(
                Id::new("sftp.clash.rename"),
                rename,
                "Rename",
                None,
                Kind::Primary,
            )
            .clicked
            || field.submitted;
        if renamed && !bad {
            answer = Some(Answer::Rename(typed));
        }
        pages::card_done(ui);
        if let Some(answer) = answer {
            self.answer_clash(&answer);
        }
    }

    /// The Delete, Permissions and Properties cards over the tab.
    fn dialog_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        /// How long a card takes to leave, in seconds.
        const GOES: f64 = 0.16;
        let cwd = self.tab.cwd.clone().unwrap_or_default();
        if let Some((dialog, since)) = &mut self.card_left {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = ((ui.time() - *since) / GOES) as f32;
            if share < 1.0 && !ui.state.reduce_motion {
                ui.push_inert();
                Self::dialog_shown(
                    ui,
                    rect,
                    dialog,
                    (&cwd, &self.tab.owners),
                    false,
                    Some(1.0 - Curve::Exit.at(share)),
                );
                ui.pop_inert();
                ui.request_repaint();
            } else {
                self.card_left = None;
            }
        }
        let Some(dialog) = &mut self.tab.dialog else {
            return;
        };
        let focus = std::mem::take(&mut self.focus_card);
        let (act, mut close) =
            Self::dialog_shown(ui, rect, dialog, (&cwd, &self.tab.owners), focus, None);
        match act {
            Some(Act::Send(command)) => {
                self.tab.send(command);
                self.remote_selected = None;
                close = true;
            }
            Some(Act::DeleteLocal(path)) => {
                let result = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(error) = result {
                    ui.toast(
                        format!("Could not delete {}: {error}", file_name(&path)),
                        Tone::Problem,
                    );
                }
                self.local_selected = None;
                close = true;
            }
            None => {}
        }
        if close {
            self.card_left = self.tab.dialog.take().map(|dialog| (dialog, ui.time()));
        }
    }

    /// `dialog`'s card over `rect`, coming in, or at `leaving` of itself on its way out;
    /// `(cwd, owners)` are the host's folder shown and its names. Returns what its button
    /// asks for and whether it closes.
    #[allow(clippy::too_many_lines)] // Five cards, each top to bottom.
    fn dialog_shown(
        ui: &mut Ui<'_>,
        rect: Rect,
        dialog: &mut Dialog,
        (cwd, owners): (&str, &Owners),
        focus: bool,
        leaving: Option<f32>,
    ) -> (Option<Act>, bool) {
        let t = ui.theme;
        let facts = match dialog {
            Dialog::Properties(entry) => properties(cwd, entry, owners),
            Dialog::LocalProperties(lines) => lines.clone(),
            _ => Vec::new(),
        };
        #[allow(clippy::cast_precision_loss)] // A handful of lines.
        let height = match dialog {
            Dialog::Delete(_) | Dialog::DeleteLocal { .. } => 200.0,
            Dialog::Permissions { .. } => 214.0,
            Dialog::Properties(_) | Dialog::LocalProperties(_) => 136.0 + facts.len() as f32 * 24.0,
        };
        ui.zone(Id::new("sftp.card.backdrop"), rect, Role::Backdrop, "");
        let card = rect.centered(rect.w.min(480.0), height);
        if let Some(left) = leaving {
            ui.gfx.fill_rect(rect, 0.0, t.scrim.alpha(left));
            ui.push_layer(
                Layer::fade(left)
                    .scaled(card.center(), 0.98 + 0.02 * left)
                    .grouped(card.outset(48.0)),
            );
        } else {
            pages::card_in(ui, Id::new("sftp.card"), rect, card);
        }
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        let mut buttons = Rect::new(column.x, column.b() - CONTROL, column.w, CONTROL);
        let mut act = None;
        let mut close = ui.key(Key::Escape, Modifiers::NONE);
        match dialog {
            Dialog::Delete(_) | Dialog::DeleteLocal { .. } => {
                let (title, name, folder) = match dialog {
                    Dialog::Delete(entry) => {
                        ("Delete from the host?", entry.name.clone(), entry.is_dir())
                    }
                    Dialog::DeleteLocal { path, folder } => {
                        ("Delete from this computer?", file_name(path), *folder)
                    }
                    _ => unreachable!("matched above"),
                };
                ui.strict_text(column.take_top(24.0), title, HEADING, t.text, Align::Start);
                column.take_top(space::S);
                ui.gfx.text(
                    column.take_top(20.0),
                    &name,
                    Font::mono(13.0),
                    t.text,
                    Align::Start,
                );
                column.take_top(space::XS);
                let warning = if folder {
                    "The folder goes with everything in it. This cannot be undone."
                } else {
                    "This cannot be undone."
                };
                ui.gfx
                    .text(column.take_top(20.0), warning, SMALL, t.text2, Align::Start);
                let delete =
                    buttons.take_right(button_width(&mut ui.gfx, "Delete", None).max(88.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                if focus {
                    // The safe choice has the keyboard.
                    ui.state.set_focus(Some(Id::new("sftp.card.cancel")), true);
                }
                close |= ui
                    .button(
                        Id::new("sftp.card.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked;
                if ui
                    .button(
                        Id::new("sftp.card.delete"),
                        delete,
                        "Delete",
                        None,
                        Kind::Danger,
                    )
                    .clicked
                {
                    act = Some(match dialog {
                        Dialog::Delete(entry) => Act::Send(Command::Remove(join(cwd, &entry.name))),
                        Dialog::DeleteLocal { path, .. } => Act::DeleteLocal(path.clone()),
                        _ => unreachable!("matched above"),
                    });
                }
            }
            Dialog::Permissions { name, mode } => {
                ui.strict_text(
                    column.take_top(24.0),
                    "Permissions",
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                ui.gfx.text(
                    column.take_top(20.0),
                    name,
                    Font::mono(13.0),
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                let field_id = Id::new("sftp.card.mode");
                if focus {
                    ui.state.set_focus(Some(field_id), false);
                }
                let parsed = parse_mode(mode);
                let invalid = parsed.is_none() && !mode.trim().is_empty();
                let field = ui.text_field(
                    field_id,
                    column.take_top(CONTROL),
                    mode,
                    FieldStyle {
                        placeholder: "Octal, for example 644",
                        mono: true,
                        error: invalid,
                        ..FieldStyle::default()
                    },
                );
                // As typed this frame.
                let parsed = parse_mode(mode);
                if invalid {
                    column.take_top(space::XS);
                    ui.gfx.text(
                        column.take_top(18.0),
                        "Three or four octal digits, 0 to 7.",
                        SMALL,
                        t.error,
                        Align::Start,
                    );
                }
                let set = buttons.take_right(button_width(&mut ui.gfx, "Set", None).max(88.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                close |= ui
                    .button(
                        Id::new("sftp.card.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked;
                let chosen = ui
                    .button(Id::new("sftp.card.set"), set, "Set", None, Kind::Primary)
                    .clicked
                    || field.submitted;
                if chosen && let Some(bits) = parsed {
                    act = Some(Act::Send(Command::Chmod(join(cwd, name), bits)));
                }
            }
            Dialog::Properties(_) | Dialog::LocalProperties(_) => {
                ui.strict_text(
                    column.take_top(24.0),
                    "Properties",
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                for (fact, value) in &facts {
                    let mut line = column.take_top(24.0);
                    let label = line.take_left(110.0);
                    ui.gfx.text(label, fact, SMALL, t.text2, Align::Start);
                    let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, value, BODY, line.w);
                    ui.gfx.text(line, &fitted, BODY, t.text, Align::Start);
                }
                let done = buttons.take_right(button_width(&mut ui.gfx, "Close", None).max(88.0));
                if focus {
                    ui.state.set_focus(Some(Id::new("sftp.card.close")), true);
                }
                close |= ui
                    .button(
                        Id::new("sftp.card.close"),
                        done,
                        "Close",
                        None,
                        Kind::Secondary,
                    )
                    .clicked;
            }
        }
        pages::card_done(ui);
        (act, close)
    }

    fn pane_frame(ui: &mut Ui<'_>, rect: Rect, title: &str, path: &str, icon: IconId) -> Rect {
        let t = ui.theme;
        ui.card_surface(rect, 12.0);
        let mut inner = rect.inset(space::M, space::S);
        let mut head = inner.take_top(28.0);
        let mark = head.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(icon, mark, t.text2);
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
        // A deep folder keeps where it starts and where it ends.
        let shown = annulus_ui::gfx::fit_path(&mut ui.gfx, path, Font::mono(11.5), head.w);
        ui.gfx
            .text(head, &shown, Font::mono(11.5), t.text2, Align::Start);
        inner.take_top(4.0);
        inner
    }

    /// The listing's header, its rows' empty space and its rows for `side`: `rows` sorted and
    /// filtered already. Returns, for the row clicked, double-clicked, acted on or whose menu
    /// item was chosen, what happened, and the empty space's menu item chosen.
    #[allow(clippy::too_many_lines)] // The header, the empty space, then the rows.
    fn listing(
        &mut self,
        ui: &mut Ui<'_>,
        side: Side,
        area: Rect,
        rows: &[Row],
        empty: Option<&str>,
    ) -> Listed {
        let t = ui.theme;
        let available = available_columns(side);
        let (shown, sort) = match side {
            Side::Local => (&mut self.local_columns, &mut self.local_sort),
            Side::Remote => (&mut self.remote_columns, &mut self.tab.sort),
        };
        let header_line = Rect::new(area.x, area.y, area.w, 22.0);
        // As wide as the rows, which leave room for the scroll bar, so the columns line up.
        let cells_w = header_line.w - 10.0 - 2.0 * space::S;
        let laid = lay_out(shown, &available, cells_w);
        let changed = Self::header_row(ui, side, header_line, &laid, sort, shown, &available);
        if changed {
            self.columns_changed = true;
        }
        let mut list = area;
        list.take_top(22.0);
        let selected = match side {
            Side::Local => self.local_selected.clone(),
            Side::Remote => self.remote_selected.clone(),
        };
        let mut listed = Listed::default();
        // The empty space under and between the rows: a right click offers what the folder
        // does. Drawn first, so the rows over it take their own clicks.
        let blank_id = Id::new(("sftp.blank", side == Side::Local));
        let blank = ui.zone(blank_id, list, Role::ScrollView, "");
        if blank.clicked {
            match side {
                Side::Local => self.local_selected = None,
                Side::Remote => self.remote_selected = None,
            }
        }
        let blank_menu = folder_menu(side, self.tab.show_hidden);
        if let Some(choice) = ui.context_menu(blank_id, &blank, &blank_menu) {
            listed.folder_choice = blank_menu.get(choice).cloned();
        }
        if let Some(why) = empty {
            let line = Rect::new(list.x, list.y + space::XL, list.w, 20.0);
            ui.gfx.text(line, why, BODY, t.text2, Align::Center);
        }
        #[allow(clippy::cast_precision_loss)] // Folders hold thousands at most.
        let content = rows.len() as f32 * ROW;
        let scroll_id = match side {
            Side::Local => Id::new("sftp.local.scroll"),
            Side::Remote => Id::new("sftp.remote.scroll"),
        };
        let offset = ui.scroll_view(scroll_id, list, content);
        ui.push_list_clip(list, offset, content, FOCUS_REACH);
        let prefix = match side {
            Side::Local => "local",
            Side::Remote => "remote",
        };
        let action = match side {
            Side::Local => (icons::UPLOAD, "Upload to the host"),
            Side::Remote => (icons::DOWNLOAD, "Download to this computer"),
        };
        for (index, row) in rows.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * ROW;
            if y + ROW < list.y || y > list.b() {
                continue;
            }
            let rect = Rect::new(list.x, y, list.w - 10.0, ROW);
            let id = Id::new((prefix, row.name.as_str()));
            let menu = row_menu(side, row.is_dir());
            let (clicked, double, acted, chosen) = Self::file_row(
                ui,
                id,
                rect,
                row,
                &laid,
                selected.as_deref() == Some(row.name.as_str()),
                action,
                &menu,
            );
            if clicked || chosen.is_some() {
                listed.clicked = Some(row.name.clone());
            }
            if double {
                listed.double = Some(row.name.clone());
            }
            if acted {
                listed.acted = Some(row.name.clone());
            }
            if let Some(item) = chosen.and_then(|at| menu.get(at)) {
                listed.choice = Some((row.name.clone(), item.clone()));
            }
            if let Some(at) = ui.drag_moved(id) {
                listed.drag = Some((row.name.clone(), at));
            }
            if let Some(at) = ui.drag_released(id) {
                listed.released = Some((row.name.clone(), at));
            }
        }
        ui.pop_clip();
        if let Some(name) = &listed.clicked {
            self.active = side;
            match side {
                Side::Local => self.local_selected = Some(name.clone()),
                Side::Remote => self.remote_selected = Some(name.clone()),
            }
        }
        if blank.clicked || blank.secondary_clicked {
            self.active = side;
        }
        listed
    }

    /// The column titles over a listing: a click sorts by one (again: the other way round),
    /// and a right click on any offers to show or hide the columns `available` to the pane.
    /// Returns whether the shown columns changed.
    fn header_row(
        ui: &mut Ui<'_>,
        side: Side,
        line: Rect,
        laid: &[(Column, Rect)],
        sort: &mut (Column, bool),
        shown: &mut Vec<Column>,
        available: &[Column],
    ) -> bool {
        let t = ui.theme;
        let mut changed = false;
        // A column shown but cut for room says so: the window is too narrow for it now.
        let menu: Vec<String> = available
            .iter()
            .map(|column| {
                let title = column.title();
                match (
                    shown.contains(column),
                    laid.iter().any(|(c, _)| c == column),
                ) {
                    (true, true) => format!("Hide {title}"),
                    (true, false) => format!("Hide {title} (no room for it now)"),
                    (false, _) => format!("Show {title}"),
                }
            })
            .collect();
        let origin = Rect::new(line.x + space::S, line.y, 0.0, line.h);
        for &(column, cell) in laid {
            let cell = Rect::new(origin.x + cell.x, line.y, cell.w, line.h);
            if cell.w <= 0.0 {
                continue;
            }
            let title = column.title();
            let align = column_align(column);
            let width = ui.gfx.measure(title, SMALL).width;
            let mut color = t.text3.mix(t.text2, 0.5);
            let id = Id::new(("sftp.sort", side == Side::Local, title));
            let response = ui.zone(
                id,
                cell,
                Role::Button,
                &format!("Sort by {}", title.to_lowercase()),
            );
            if response.clicked {
                *sort = (column, sort.0 != column || !sort.1);
            }
            if let Some(choice) = ui.context_menu(id, &response, &menu)
                && let Some(column) = available.get(choice)
            {
                if let Some(at) = shown.iter().position(|c| c == column) {
                    shown.remove(at);
                } else {
                    // What was asked for last is kept first when room runs short.
                    shown.insert(0, *column);
                }
                changed = true;
            }
            if response.focus_ring {
                ui.focus_ring(cell, 4.0);
            }
            if response.hovered {
                color = t.text;
                ui.set_cursor(annulus_ui::kit::Cursor::Pointer);
            }
            if sort.0 == column {
                color = t.text;
                let x = match align {
                    Align::End => cell.r() - width - 14.0,
                    _ => cell.x + width + 4.0,
                };
                let arrow = Rect::new(x, cell.center().y - 5.0, 10.0, 10.0);
                let icon = if sort.1 {
                    icons::CHEVRON_UP
                } else {
                    icons::CHEVRON_DOWN
                };
                ui.gfx.icon(icon, arrow, color);
            }
            let text_cell = match align {
                Align::End if sort.0 == column => Rect::new(cell.x, cell.y, cell.w - 14.0, cell.h),
                _ => cell,
            };
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, title, SMALL, text_cell.w);
            ui.gfx.text(text_cell, &fitted, SMALL, color, align);
        }
        ui.gfx
            .fill_rect(Rect::new(line.x, line.b() - 1.0, line.w, 1.0), 0.0, t.line);
        changed
    }

    /// One file row with its columns as `laid` out; returns whether it was clicked,
    /// double-clicked, its action button clicked, and the item of its menu (More, or a right
    /// click) chosen. Its buttons come up over its last columns while it is hovered.
    #[allow(clippy::too_many_arguments)] // One row's every part.
    fn file_row(
        ui: &mut Ui<'_>,
        id: Id,
        rect: Rect,
        row: &Row,
        laid: &[(Column, Rect)],
        selected: bool,
        action: (IconId, &str),
        menu: &[String],
    ) -> (bool, bool, bool, Option<usize>) {
        let t = ui.theme;
        let response = ui.zone(id, rect, Role::ListItem, &row.name);
        // Still hovered on its own buttons, and while one of its menus is open.
        let hover = ui.hover_within(rect) || ui.menu_open(id.with("more")) || ui.menu_open(id);
        let lit = ui.ease(id.with("lit"), if hover { 1.0 } else { 0.0 });
        let picked = ui.ease(id.with("picked"), if selected { 1.0 } else { 0.0 });
        let veils = |ui: &mut Ui<'_>, area: Rect| {
            ui.gfx
                .fill_rect(area, 6.0, t.hover.alpha(lit * (1.0 - picked)));
            ui.gfx.fill_rect(area, 6.0, t.selected.alpha(picked));
        };
        veils(ui, rect);
        if response.focus_ring {
            ui.focus_ring(rect, 6.0);
        }
        let origin_x = rect.x + space::S;
        for &(column, cell) in laid {
            let cell = Rect::new(origin_x + cell.x, rect.y, cell.w, rect.h);
            if column == Column::Name {
                let mut cell = cell;
                let icon = cell.take_left(16.0).centered(16.0, 16.0);
                let glyph = match row.kind {
                    annulus_sftp::Kind::Dir | annulus_sftp::Kind::Link { to_dir: true } => {
                        icons::FOLDER
                    }
                    _ => icons::FILE,
                };
                ui.gfx.icon(glyph, icon, t.text2);
                cell.take_left(space::S);
                // A long name keeps its end: "backup-2026…tar.gz".
                let shown = annulus_ui::gfx::fit_middle(&mut ui.gfx, &row.name, BODY, cell.w);
                ui.gfx.text(cell, &shown, BODY, t.text, Align::Start);
                continue;
            }
            let text = row.cell(column);
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, &text, SMALL, cell.w);
            ui.gfx
                .text(cell, &fitted, SMALL, t.text2, column_align(column));
        }
        let mut acted = false;
        let mut chosen = None;
        // They come up with the row, and stay for the moment it takes them to go.
        let near = lit.max(picked);
        if hover || selected || near > 0.0 {
            let mut end = rect.inset(space::S, 0.0);
            let more = end.take_right(26.0).centered(26.0, 26.0);
            end.take_right(space::XS);
            let button = end.take_right(26.0).centered(26.0, 26.0);
            ui.push_layer(Layer::fade(near));
            // Over the last columns, on the row's own color, so their text does not show
            // through.
            let backing = Rect::new(
                button.x - space::S,
                rect.y,
                rect.r() - button.x + space::S,
                rect.h,
            );
            ui.gfx.fill_rect(backing, 6.0, t.surface);
            veils(ui, backing);
            if !row.is_dir() {
                acted = ui
                    .icon_button(id.with("act"), button, action.0, action.1, false)
                    .clicked;
            }
            if !menu.is_empty() {
                chosen = ui.menu_button(
                    id.with("more"),
                    more,
                    "",
                    Some(icons::MORE),
                    Kind::Ghost,
                    menu,
                );
                ui.set_name(id.with("more"), "More actions");
                ui.tooltip(id.with("more"), more, "More actions");
            }
            ui.pop_layer();
        }
        // The same menu on a right click, or Shift+F10 on the row.
        if !menu.is_empty()
            && let Some(choice) = ui.context_menu(id, &response, menu)
        {
            chosen = Some(choice);
        }
        (response.clicked, response.double_clicked, acted, chosen)
    }

    /// The inline field for a new folder's name or a new name, over `side`'s listing; Enter or
    /// Save makes it so.
    fn edit_line(&mut self, ui: &mut Ui<'_>, side: Side, inner: &mut Rect) {
        // A rename typed for one folder never applies to the next one (UI review #9).
        if self.edit_dir != self.folder_of(side) && self.edit.as_ref().is_some_and(|e| e.0 == side)
        {
            self.edit = None;
        }
        let Some((_, edit)) = self.edit.as_mut().filter(|(at, _)| *at == side) else {
            return;
        };
        let mut line = inner.take_top(CONTROL);
        let save = line.take_right(80.0);
        line.take_right(space::S);
        let (buffer, placeholder) = match edit {
            Edit::NewFolder(name) => (name, "Folder name"),
            Edit::Rename { to, .. } => (to, "New name"),
        };
        let field_id = Id::new(("sftp.edit", side == Side::Local));
        let field = ui.text_field(
            field_id,
            line,
            buffer,
            FieldStyle {
                placeholder,
                ..FieldStyle::default()
            },
        );
        let done = ui
            .button(
                Id::new(("sftp.edit.save", side == Side::Local)),
                save,
                "Save",
                None,
                Kind::Primary,
            )
            .clicked
            || field.submitted;
        inner.take_top(space::S);
        if done && valid_name(buffer.trim()) {
            let Some((side, edit)) = self.edit.take() else {
                return;
            };
            match side {
                Side::Remote => {
                    let cwd = self.tab.cwd.clone().unwrap_or_default();
                    let command = match edit {
                        Edit::NewFolder(name) => Command::CreateDir(join(&cwd, name.trim())),
                        Edit::Rename { from, to } => {
                            Command::Rename(join(&cwd, &from), join(&cwd, to.trim()))
                        }
                    };
                    self.tab.send(command);
                }
                Side::Local => {
                    let dir = self.tab.local.dir.clone();
                    let result = match &edit {
                        Edit::NewFolder(name) => std::fs::create_dir(dir.join(name.trim())),
                        Edit::Rename { from, to } => {
                            let target = dir.join(to.trim());
                            if target.symlink_metadata().is_ok() && from != to.trim() {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::AlreadyExists,
                                    "that name is taken",
                                ))
                            } else {
                                std::fs::rename(dir.join(from), target)
                            }
                        }
                    };
                    if let Err(error) = result {
                        ui.toast(format!("Could not do that: {error}"), Tone::Problem);
                    }
                    if let Edit::Rename { to, .. } = edit {
                        self.local_selected = Some(to.trim().to_owned());
                    }
                }
            }
        } else if ui.key(Key::Escape, Modifiers::NONE) {
            self.edit = None;
        }
    }

    #[allow(clippy::too_many_lines)] // One pane, top to bottom.
    fn local_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let dir = self.tab.local.dir.clone();
        let home = home_dir();
        let mut inner = Self::pane_frame(
            ui,
            rect,
            "This computer",
            &tilde(&dir.display().to_string(), &home),
            icons::DEVICE,
        );
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let make = bar.take_left(CONTROL);
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
        if ui
            .icon_button(
                Id::new("sftp.local.mkdir"),
                make,
                icons::PLUS,
                "New folder on this computer",
                false,
            )
            .clicked
        {
            self.start_edit(Side::Local, None);
        }
        ui.tooltip(Id::new("sftp.local.mkdir"), make, "New folder");
        // A filter like the host's pane has, so the two lists line up.
        inner.take_top(space::S);
        ui.text_field(
            Id::new("sftp.local.filter"),
            inner.take_top(CONTROL),
            &mut self.local_filter,
            FieldStyle {
                placeholder: "Filter this folder",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        inner.take_top(space::S);
        self.edit_line(ui, Side::Local, &mut inner);
        let filter = self.local_filter.trim().to_lowercase();
        let entries = read_dir(&self.tab.local.dir, self.tab.show_hidden);
        let read_error = entries.as_ref().err().map(ToString::to_string);
        let mut rows: Vec<Row> = entries
            .unwrap_or_default()
            .iter()
            .map(crate::local_pane::LocalEntry::row)
            .filter(|row| filter.is_empty() || row.name.to_lowercase().contains(&filter))
            .collect();
        sort_rows(&mut rows, self.local_sort.0, self.local_sort.1);
        let empty = if let Some(error) = &read_error {
            Some(format!("Cannot read this folder: {error}"))
        } else if rows.is_empty() && !filter.is_empty() {
            Some(format!(
                "Nothing here matches \u{201c}{}\u{201d}.",
                self.local_filter.trim()
            ))
        } else if rows.is_empty() {
            Some("This folder is empty.".to_owned())
        } else {
            None
        };
        let listed = self.listing(ui, Side::Local, inner, &rows, empty.as_deref());
        let shown_dir = self.tab.local.dir.clone();
        let path_of = |name: &str| shown_dir.join(name);
        let is_dir = |name: &str| rows.iter().any(|row| row.name == name && row.is_dir());
        if let Some((name, at)) = &listed.drag {
            self.drag = Some((Dragged::Local(path_of(name)), *at));
        }
        if let Some((name, at)) = &listed.released {
            self.dropped = Some((Dragged::Local(path_of(name)), *at));
        }
        // A folder opens; a file goes up.
        if let Some(name) = &listed.double {
            if is_dir(name) {
                self.tab.local.dir = path_of(name);
                self.local_selected = None;
            } else {
                self.send_up(path_of(name));
            }
        }
        if let Some(name) = &listed.acted {
            self.send_up(path_of(name));
        }
        if let Some((name, item)) = listed.choice {
            let path = path_of(&name);
            match item.as_str() {
                OPEN if is_dir(&name) => {
                    self.tab.local.dir = path;
                    self.local_selected = None;
                }
                OPEN => {
                    if let Err(error) = platform::open_path(&path) {
                        ui.toast(format!("Could not open {name}: {error}"), Tone::Problem);
                    }
                }
                UPLOAD => self.send_up(path),
                RENAME => self.start_edit(Side::Local, Some(name)),
                COPY_PATH => ui.copy(path.display().to_string()),
                PROPERTIES => {
                    if let Some(row) = rows.iter().find(|row| row.name == name) {
                        let lines = local_properties(&path, row);
                        self.open_card(Dialog::LocalProperties(lines));
                    }
                }
                NEW_FOLDER => self.start_edit(Side::Local, None),
                REFRESH => self.tab.local.refresh(self.tab.show_hidden),
                DELETE => self.ask_delete_local(&name),
                _ if item.starts_with(SHOW_IN) => {
                    if let Err(error) = platform::reveal_path(&path) {
                        ui.toast(format!("Could not show {name}: {error}"), Tone::Problem);
                    }
                }
                _ => {}
            }
        }
        if let Some(item) = listed.folder_choice {
            self.folder_action(ui, Side::Local, &item);
        }
    }

    /// What the empty space's menu item `item` does in `side`'s folder.
    fn folder_action(&mut self, ui: &mut Ui<'_>, side: Side, item: &str) {
        match item {
            NEW_FOLDER => self.start_edit(side, None),
            REFRESH => match side {
                Side::Local => self.tab.local.refresh(self.tab.show_hidden),
                Side::Remote => self.tab.reload(),
            },
            COPY_PATH => {
                let path = self.folder_of(side);
                ui.copy(path);
            }
            SHOW_HIDDEN | HIDE_HIDDEN => {
                self.tab.show_hidden = !self.tab.show_hidden;
                self.tab.reload();
                self.tab.local.refresh(self.tab.show_hidden);
            }
            _ if item.starts_with(SHOW_IN) => {
                if let Err(error) = platform::open_path(&self.tab.local.dir) {
                    ui.toast(format!("Could not open the folder: {error}"), Tone::Problem);
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)] // Toolbar, an inline edit and the list.
    fn remote_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let cwd = self.tab.cwd.clone().unwrap_or_default();
        let title = self.tab.title.clone();
        let shown = tilde(&cwd, &self.tab.home);
        let mut inner = Self::pane_frame(ui, rect, &title, &shown, icons::SERVER);
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let refresh = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let folder_w = button_width(&mut ui.gfx, "New folder", Some(icons::PLUS));
        let hidden_w = button_width(&mut ui.gfx, "Hidden files", None);
        // A narrow pane keeps New folder as an icon, so neither button is cut off.
        let roomy = bar.w >= folder_w + hidden_w + space::S;
        let new_folder = bar.take_left(if roomy { folder_w } else { CONTROL });
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
        let make = if roomy {
            ui.button(
                Id::new("sftp.mkdir"),
                new_folder,
                "New folder",
                Some(icons::PLUS),
                Kind::Ghost,
            )
            .clicked
        } else {
            let clicked = ui
                .icon_button(
                    Id::new("sftp.mkdir"),
                    new_folder,
                    icons::PLUS,
                    "New folder",
                    false,
                )
                .clicked;
            ui.tooltip(Id::new("sftp.mkdir"), new_folder, "New folder");
            clicked
        };
        if make {
            self.start_edit(Side::Remote, None);
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
        let mut options = inner.take_top(CONTROL);
        // Asking first, then the fixed answers.
        let policies = [
            None,
            Some(Conflict::KeepBoth),
            Some(Conflict::Overwrite),
            Some(Conflict::Skip),
        ];
        let names: Vec<String> = policies
            .iter()
            .map(|policy| policy.map_or(ASK_NAME, conflict_name).to_owned())
            .collect();
        let widest = names
            .iter()
            .map(|name| ui.gfx.measure(name, BODY).width)
            .fold(0.0, f32::max);
        let conflict = options.take_right(widest + 44.0);
        options.take_right(space::S);
        ui.text_field(
            Id::new("sftp.filter"),
            options,
            &mut self.tab.filter,
            FieldStyle {
                placeholder: "Filter this folder",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        let current = (!self.tab.ask_on_clash).then_some(self.tab.conflict);
        let at = policies.iter().position(|policy| *policy == current);
        if let Some(chosen) = ui.select(
            Id::new("sftp.conflict"),
            conflict,
            &names,
            at,
            "Existing files",
        ) {
            match policies[chosen] {
                None => self.tab.ask_on_clash = true,
                Some(policy) => {
                    self.tab.ask_on_clash = false;
                    self.tab.conflict = policy;
                }
            }
        }
        inner.take_top(space::S);
        self.edit_line(ui, Side::Remote, &mut inner);
        if let Some(message) = self
            .tab
            .list_error
            .clone()
            .or_else(|| self.tab.notice.clone())
        {
            ui.gfx
                .text(inner.take_top(20.0), &message, SMALL, t.error, Align::Start);
        }
        let filter = self.tab.filter.trim().to_lowercase();
        let mut rows: Vec<Row> = self
            .tab
            .entries
            .iter()
            .filter(|entry| self.tab.show_hidden || !entry.name.starts_with('.'))
            .filter(|entry| filter.is_empty() || entry.name.to_lowercase().contains(&filter))
            .map(|entry| Row::remote(entry, &self.tab.owners))
            .collect();
        sort_rows(&mut rows, self.tab.sort.0, self.tab.sort.1);
        // An empty listing says why: nothing there, only hidden files, or nothing matches.
        let empty = (rows.is_empty() && self.tab.cwd.is_some()).then(|| {
            if !filter.is_empty() {
                format!(
                    "Nothing here matches \u{201c}{}\u{201d}.",
                    self.tab.filter.trim()
                )
            } else if self.tab.entries.is_empty() {
                "This folder is empty.".to_owned()
            } else {
                "Only hidden files here; Hidden files shows them.".to_owned()
            }
        });
        let listed = self.listing(ui, Side::Remote, inner, &rows, empty.as_deref());
        let entries = self.tab.entries.clone();
        let entry_of = |name: &str| entries.iter().find(|entry| entry.name == name).cloned();
        if let Some((name, at)) = &listed.drag
            && let Some(entry) = entry_of(name)
        {
            self.drag = Some((Dragged::Remote(entry), *at));
        }
        if let Some((name, at)) = &listed.released
            && let Some(entry) = entry_of(name)
        {
            self.dropped = Some((Dragged::Remote(entry), *at));
        }
        if let Some(entry) = listed.double.as_deref().and_then(entry_of) {
            if entry.is_dir() {
                self.remote_selected = None;
                self.tab.navigate(join(&cwd, &entry.name));
            } else {
                self.send_down(entry);
            }
        }
        if let Some(entry) = listed.acted.as_deref().and_then(entry_of) {
            self.send_down(entry);
        }
        if let Some((name, item)) = listed.choice
            && let Some(entry) = entry_of(&name)
        {
            match item.as_str() {
                OPEN => {
                    self.remote_selected = None;
                    self.tab.navigate(join(&cwd, &entry.name));
                }
                DOWNLOAD => self.send_down(entry),
                RENAME => self.start_edit(Side::Remote, Some(entry.name)),
                PERMISSIONS => self.open_card(Dialog::permissions(&entry)),
                PROPERTIES => self.open_card(Dialog::Properties(entry)),
                COPY_PATH => ui.copy(join(&cwd, &entry.name)),
                NEW_FOLDER => self.start_edit(Side::Remote, None),
                REFRESH => self.tab.reload(),
                DELETE => self.open_card(Dialog::Delete(entry)),
                _ => {}
            }
        }
        if let Some(item) = listed.folder_choice {
            self.folder_action(ui, Side::Remote, &item);
        }
    }

    /// The transfers along the bottom (F-SFTP-004), newest first. Each row has the file's
    /// name and how it is doing, where it goes with the folders between shortened, and a bar
    /// that glides with the progress; Cancel or Retry at its end. The panel folds to its
    /// header, and Clear finished takes the done ones away.
    #[allow(clippy::too_many_lines)] // The header, then one row after another.
    fn transfers(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let mut area = rect;
        area.take_top(space::M);
        let mut head = area.take_top(TRANSFERS_HEAD);
        let running = self
            .tab
            .transfers
            .iter()
            .filter(|transfer| matches!(transfer.state, TransferState::Running))
            .count();
        let finished = self.tab.transfers.iter().any(|transfer| {
            matches!(
                transfer.state,
                TransferState::Done(_) | TransferState::Skipped
            )
        });
        // Laid out from the right, drawn after the toggle: Tab follows the drawing order.
        let clear_label = "Clear finished";
        let clear = finished.then(|| head.take_right(button_width(&mut ui.gfx, clear_label, None)));
        let summary = match running {
            0 => format!("Transfers \u{b7} {}", self.tab.transfers.len()),
            n => format!(
                "Transfers \u{b7} {n} running of {}",
                self.tab.transfers.len()
            ),
        };
        let toggle_w = ui.gfx.measure(&summary, BODY_STRONG).width + 16.0 + 3.0 * space::S;
        let toggle = head.take_left(toggle_w.min(head.w));
        let toggle_id = Id::new("sftp.transfers.toggle");
        let name = if self.transfers_open {
            "Hide transfers"
        } else {
            "Show transfers"
        };
        let response = ui.zone(toggle_id, toggle, Role::Button, name);
        if response.clicked {
            self.transfers_open = !self.transfers_open;
        }
        let hover = ui.ease(
            toggle_id.with("hover"),
            if response.hovered { 1.0 } else { 0.0 },
        );
        ui.gfx.fill_rect(toggle, 7.0, t.hover.alpha(hover));
        if response.focus_ring {
            ui.focus_ring(toggle, 7.0);
        }
        let mut inner = toggle.inset(space::S, 0.0);
        let chevron = if self.transfers_open {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        };
        ui.gfx
            .icon(chevron, inner.take_left(16.0).centered(14.0, 14.0), t.text2);
        inner.take_left(space::S);
        ui.gfx
            .text(inner, &summary, BODY_STRONG, t.text, Align::Start);
        if let Some(clear) = clear
            && ui
                .button(
                    Id::new("sftp.transfers.clear"),
                    clear,
                    clear_label,
                    None,
                    Kind::Ghost,
                )
                .clicked
        {
            self.tab.transfers.retain(|transfer| {
                !matches!(
                    transfer.state,
                    TransferState::Done(_) | TransferState::Skipped
                )
            });
        }
        area.take_top(space::S);
        if !self.transfers_open {
            return;
        }
        let rows = self.tab.transfers.len();
        #[allow(clippy::cast_precision_loss)] // A few dozen transfers.
        let content = rows as f32 * (TRANSFER_ROW + 6.0);
        let offset = ui.scroll_view(Id::new("sftp.transfers"), area, content);
        ui.push_clip(area);
        let home = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().display().to_string())
            .unwrap_or_default();
        let mut retry = None;
        let mut moving = false;
        for (place, transfer) in self.tab.transfers.iter().rev().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = area.y - offset + place as f32 * (TRANSFER_ROW + 6.0);
            let row = Rect::new(area.x, y, area.w - 10.0, TRANSFER_ROW);
            let id = Id::new(("transfer", transfer.id));
            // A new transfer grows into its place in the list.
            let enter = ui.entrance(id.with("enter"), Motion::POP);
            ui.push_layer(Layer::appear(enter, row.center(), 0.92, 0.0));
            ui.card_surface(row, 10.0);
            let mut cells = row.inset(space::M, 0.0);
            let (state_color, status) = transfer_status(transfer, &t);
            // Its direction, in a tile tinted by how it is doing.
            let tile = cells.take_left(32.0).centered(32.0, 32.0);
            let tint = ui.animate(id.with("tint"), state_tone(transfer), Motion::SLOW);
            let tone = t.accent.mix(t.ok, tint.clamp(0.0, 1.0));
            let tone = match transfer.state {
                TransferState::Failed(_) => t.error,
                TransferState::Skipped => t.text3,
                _ => tone,
            };
            ui.gfx.fill_rect(tile, 9.0, tone.alpha(0.16));
            let glyph = match transfer.state {
                TransferState::Done(_) => icons::CHECK,
                TransferState::Failed(_) => icons::WARNING,
                _ if transfer.upload => icons::UPLOAD,
                _ => icons::DOWNLOAD,
            };
            ui.gfx.icon(glyph, tile.centered(16.0, 16.0), tone);
            cells.take_left(space::M);
            let button = cells.take_right(28.0).centered(28.0, 28.0);
            cells.take_right(space::S);
            let name = transfer
                .label
                .trim_start_matches(['\u{2191}', '\u{2193}'])
                .trim();
            let status_w = ui.gfx.measure(&status, SMALL).width.min(cells.w * 0.45);
            let top = Rect::new(cells.x, row.y + 9.0, cells.w, 20.0);
            let mut name_cell = top;
            let status_cell = name_cell.take_right(status_w);
            name_cell.take_right(space::M);
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, name, BODY_STRONG, name_cell.w);
            ui.gfx
                .text(name_cell, &fitted, BODY_STRONG, t.text, Align::Start);
            ui.gfx
                .text(status_cell, &status, SMALL, state_color, Align::End);
            let second = Rect::new(cells.x, row.y + 27.0, cells.w, 16.0);
            if let TransferState::Failed(message) = &transfer.state {
                ui.gfx.text(second, message, SMALL, t.error, Align::Start);
            } else {
                let route = format!("To {}", destination(transfer, &home));
                let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &route, SMALL, second.w);
                ui.gfx.text(second, &fitted, SMALL, t.text3, Align::Start);
            }
            // The bar along the row's foot: it glides to each new share.
            let share = match transfer.state {
                TransferState::Done(_) | TransferState::Skipped => 1.0,
                _ => share_of(transfer),
            };
            let shown = ui.animate(
                id.with("bar"),
                share,
                Motion::new(450, annulus_ui::motion::Curve::Out),
            );
            let track = Rect::new(cells.x, row.b() - 10.0, cells.w, 3.0);
            let fill = Rect::new(track.x, track.y, track.w * shown.clamp(0.0, 1.0), track.h);
            // Only while it says something: a transfer that is over needs no bar.
            if !matches!(
                transfer.state,
                TransferState::Done(_) | TransferState::Skipped
            ) {
                ui.gfx.fill_rect(track, 1.5, t.hover);
                ui.gfx.fill_rect(fill, 1.5, tone);
            }
            if matches!(transfer.state, TransferState::Running) && fill.w > 4.0 {
                // A soft light running along what is done, so a slow transfer still looks alive.
                moving = true;
                let band = 48.0;
                #[allow(clippy::cast_possible_truncation)] // A phase for drawing.
                let phase = (ui.time() * 0.7).fract() as f32;
                let x = fill.x - band + phase * (fill.w + band);
                ui.push_clip(fill);
                ui.gfx.fill_rect(
                    Rect::new(x, fill.y, band, fill.h),
                    1.5,
                    Color::WHITE.alpha(0.35),
                );
                ui.pop_clip();
            }
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
            ui.pop_layer();
        }
        ui.pop_clip();
        if moving {
            ui.request_repaint_after(std::time::Duration::from_millis(33));
        }
        if let Some(id) = retry {
            self.tab.retry(id);
        }
    }

    /// README.md dragged up where the host has one, and app.log waiting behind it, for the
    /// scenes.
    #[cfg(test)]
    pub fn demo_clash(&mut self) {
        for name in ["README.md", "app.log"] {
            let path = self.tab.local.dir.join(name);
            std::fs::write(&path, "mine").expect("a temporary file");
            self.send_up(path);
        }
    }

    /// Transfers in each state, for the scenes.
    #[cfg(test)]
    pub fn demo_transfers(&mut self) {
        self.tab.demo_transfers();
    }

    /// The same, without the one that failed.
    #[cfg(test)]
    pub fn demo_transfers_going_well(&mut self) {
        self.demo_transfers();
        self.tab
            .transfers
            .retain(|transfer| !matches!(transfer.state, TransferState::Failed(_)));
    }
}

/// Height of the transfers panel's header.
const TRANSFERS_HEAD: f32 = 30.0;
/// Height of one transfer.
const TRANSFER_ROW: f32 = 60.0;
/// Height of the transfers panel while open: three rows.
const TRANSFERS_H: f32 = 244.0;

/// How far `transfer` is, 0.0 to 1.0.
fn share_of(transfer: &crate::sftp_view::Transfer) -> f32 {
    if transfer.total == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)] // A share of the file.
    let share = transfer.done as f32 / transfer.total as f32;
    share.clamp(0.0, 1.0)
}

/// 1.0 once a transfer is done (its tile and bar turn green), else 0.0.
fn state_tone(transfer: &crate::sftp_view::Transfer) -> f32 {
    if matches!(transfer.state, TransferState::Done(_)) {
        1.0
    } else {
        0.0
    }
}

/// What a transfer's row says about it, and in which color.
fn transfer_status(
    transfer: &crate::sftp_view::Transfer,
    t: &annulus_ui::kit::Theme,
) -> (annulus_ui::gfx::Color, String) {
    match &transfer.state {
        TransferState::Running if transfer.total == 0 => (t.text2, "Starting\u{2026}".into()),
        TransferState::Running => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 to 100.
            let percent = (share_of(transfer) * 100.0).floor() as u32;
            let seconds = transfer
                .since
                .map_or(0.0, |(at, _)| at.elapsed().as_secs_f64());
            let moved = transfer
                .since
                .map_or(0, |(_, from)| transfer.done.saturating_sub(from));
            let speed = pace(moved, transfer.total.saturating_sub(transfer.done), seconds);
            let status = if speed.is_empty() {
                format!("{percent}%")
            } else {
                format!("{percent}% \u{b7} {speed}")
            };
            (t.text2, status)
        }
        TransferState::Done(_) => (t.ok, format!("Done \u{b7} {}", human_size(transfer.total))),
        TransferState::Skipped => (t.text3, "Skipped: already there".into()),
        TransferState::Failed(_) => (t.error, "Failed".into()),
    }
}

/// Where `transfer` puts its file: the path it landed at once done, else the folder it goes to,
/// with this computer's home folder as `~`.
fn destination(transfer: &crate::sftp_view::Transfer, home: &str) -> String {
    use crate::sftp_view::Recipe;
    let local = |path: &std::path::Path| tilde(&path.to_string_lossy(), home);
    match (&transfer.state, &transfer.recipe) {
        (TransferState::Done(to), Recipe::Download { .. } | Recipe::DownloadTree { .. }) => {
            tilde(to, home)
        }
        (TransferState::Done(to), _) => to.clone(),
        (_, Recipe::Download { to, .. } | Recipe::DownloadTree { to, .. }) => local(to),
        (_, Recipe::Upload { dir, .. } | Recipe::UploadTree { dir, .. }) => dir.clone(),
    }
}

/// The panes a drag may land in: the other one for a row (`row`); for files from the OS
/// (`os`: where they are, when the OS says) either pane, or only the host's when the OS does not
/// say, since a drop there uploads.
fn drop_targets(row: Option<&(Dragged, Point)>, os: FromOs) -> Vec<Side> {
    match (row, os) {
        (Some((Dragged::Local(_), _)), _) | (None, FromOs::Somewhere) => vec![Side::Remote],
        (Some((Dragged::Remote(_), _)), _) => vec![Side::Local],
        (None, FromOs::At(_)) => vec![Side::Local, Side::Remote],
        (None, FromOs::No) => Vec::new(),
    }
}

/// Files dragged in from the OS: none, over a point, or somewhere the OS does not say.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FromOs {
    No,
    At(Point),
    Somewhere,
}

/// What one listing reported this frame, by entry name.
#[derive(Default)]
struct Listed {
    clicked: Option<String>,
    double: Option<String>,
    /// Its action button (upload or download).
    acted: Option<String>,
    /// An item of its menu: the entry's name and the item.
    choice: Option<(String, String)>,
    drag: Option<(String, Point)>,
    released: Option<(String, Point)>,
    /// An item of the empty space's menu.
    folder_choice: Option<String>,
}

const OPEN: &str = "Open";
const DOWNLOAD: &str = "Download";
const UPLOAD: &str = "Upload to the host";
const RENAME: &str = "Rename";
const PERMISSIONS: &str = "Permissions\u{2026}";
const PROPERTIES: &str = "Properties";
const COPY_PATH: &str = "Copy path";
const NEW_FOLDER: &str = "New folder";
const REFRESH: &str = "Refresh";
const DELETE: &str = "Delete";
const SHOW_IN: &str = "Show in ";
const SHOW_HIDDEN: &str = "Show hidden files";
const HIDE_HIDDEN: &str = "Hide hidden files";

/// "Show in Explorer", "Show in Finder" or "Show in the file manager".
fn show_in() -> String {
    format!("{SHOW_IN}{}", platform::file_manager_name())
}

/// The menu of an entry in `side`'s listing: what a file manager offers, then the folder's
/// own items, Delete last.
fn row_menu(side: Side, folder: bool) -> Vec<String> {
    let items: Vec<String> = match side {
        Side::Remote => {
            let first = if folder {
                vec![OPEN, DOWNLOAD]
            } else {
                vec![DOWNLOAD]
            };
            first
                .into_iter()
                .chain([
                    RENAME,
                    PERMISSIONS,
                    PROPERTIES,
                    COPY_PATH,
                    NEW_FOLDER,
                    REFRESH,
                    DELETE,
                ])
                .map(str::to_owned)
                .collect()
        }
        Side::Local => [OPEN.to_owned(), UPLOAD.to_owned(), show_in()]
            .into_iter()
            .chain([RENAME, COPY_PATH, PROPERTIES, NEW_FOLDER, REFRESH, DELETE].map(str::to_owned))
            .collect(),
    };
    items
}

/// The menu of the empty space in `side`'s listing.
fn folder_menu(side: Side, hidden_shown: bool) -> Vec<String> {
    let hidden = if hidden_shown {
        HIDE_HIDDEN
    } else {
        SHOW_HIDDEN
    };
    let mut items: Vec<String> = [NEW_FOLDER, REFRESH, COPY_PATH, hidden]
        .map(str::to_owned)
        .to_vec();
    if side == Side::Local {
        items.insert(2, show_in());
    }
    items
}

/// The columns `side` can fill: the host says no creation time, and Windows keeps no Unix
/// owner or group.
fn available_columns(side: Side) -> Vec<Column> {
    Column::OPTIONAL
        .into_iter()
        .filter(|column| match side {
            Side::Remote => *column != Column::Created,
            Side::Local => cfg!(unix) || !matches!(column, Column::Owner | Column::Group),
        })
        .collect()
}

/// How wide `column` is, its gap after it included.
fn column_width(column: Column) -> f32 {
    match column {
        Column::Name => 0.0,
        Column::Size => 76.0,
        Column::Type => 100.0,
        Column::Owner | Column::Group => 84.0,
        Column::Permissions => 104.0,
        Column::Modified | Column::Accessed | Column::Created => 124.0,
    }
}

fn column_align(column: Column) -> Align {
    if column == Column::Size {
        Align::End
    } else {
        Align::Start
    }
}

/// The cells of a listing whose rows are `width` wide: Name first, then those of `shown` the
/// pane fills (`available`), left to right, each as `(column, x and width from the row's
/// start)`. Name keeps at least a third, so when room runs short the columns last in `shown`
/// are cut first (the user's newest choice is first in it).
fn lay_out(shown: &[Column], available: &[Column], width: f32) -> Vec<(Column, Rect)> {
    let mut room = width - (width * 0.34).max(140.0);
    let mut kept = Vec::new();
    for &column in shown {
        let wide = column_width(column);
        if column != Column::Name && available.contains(&column) && wide <= room {
            room -= wide;
            kept.push(column);
        }
    }
    let used: f32 = kept.iter().map(|column| column_width(*column)).sum();
    let name = (width - used).max(0.0);
    let mut laid = vec![(Column::Name, Rect::new(0.0, 0.0, name - space::S, 0.0))];
    let mut x = name;
    for column in Column::OPTIONAL {
        if kept.contains(&column) {
            let wide = column_width(column);
            laid.push((column, Rect::new(x, 0.0, wide - space::S, 0.0)));
            x += wide;
        }
    }
    laid
}

/// The last part of `path`.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// This computer's home folder, for `~`.
fn home_dir() -> String {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().display().to_string())
        .unwrap_or_default()
}

/// What files dragged in are called on a badge: the one's name, or how many.
fn items_named(paths: &[PathBuf]) -> String {
    match paths {
        [one] => file_name(one),
        many => format!("{} items", many.len()),
    }
}

/// "12.3 KiB · 2026-11-01 10:22" (a folder: "Folder · …").
fn facts(folder: bool, size: u64, modified: Option<u32>) -> String {
    let what = if folder {
        "Folder".to_owned()
    } else {
        human_size(size)
    };
    match modified {
        Some(at) => format!("{what} \u{b7} modified {}", format_time(at)),
        None => what,
    }
}

/// [`facts`] of a file or folder on this computer.
fn local_facts(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| u32::try_from(since.as_secs()).ok());
            facts(meta.is_dir(), meta.len(), modified)
        }
        Err(_) => String::new(),
    }
}

/// The names in `dir` on this computer, hidden ones too.
fn local_names(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `name` with a number, as Keep both names it, that none of `taken` has.
fn free_name(name: &str, taken: &HashSet<String>) -> String {
    (1..1000)
        .map(|n| keep_both_name(name, n))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// Copies `from` (a file, or a folder with everything in it) to `to` on this computer. Links
/// are passed over, never followed. A file already at a target stays unless `replace`; a
/// replaced one goes only once the whole new copy is there, so a failed copy loses nothing.
fn copy_into(from: &Path, to: &Path, replace: bool) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_into(&entry.path(), &to.join(entry.file_name()), replace)?;
        }
        return Ok(());
    }
    if meta.file_type().is_symlink() || (to.symlink_metadata().is_ok() && !replace) {
        return Ok(());
    }
    let part = to.with_file_name(format!(".{}.annulus-part", file_name(to)));
    std::fs::copy(from, &part)?;
    std::fs::rename(&part, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// The Properties card's lines for `row`, at `path` on this computer.
fn local_properties(path: &Path, row: &Row) -> Vec<(&'static str, String)> {
    let mut lines = vec![
        ("Name", row.name.clone()),
        ("Path", path.display().to_string()),
        ("Kind", row.cell(Column::Type)),
    ];
    if !row.is_dir() {
        lines.push((
            "Size",
            format!("{} ({} bytes)", human_size(row.size), row.size),
        ));
    }
    for (label, column) in [
        ("Modified", Column::Modified),
        ("Created", Column::Created),
        ("Accessed", Column::Accessed),
        ("Permissions", Column::Permissions),
    ] {
        let value = row.cell(column);
        if !value.is_empty() {
            lines.push((label, value));
        }
    }
    if let Some(owner) = &row.owner {
        let group = row
            .group
            .as_ref()
            .map(|group| format!(", group {group}"))
            .unwrap_or_default();
        lines.push(("Owner", format!("{owner}{group}")));
    }
    lines
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_session::{Hop, SshFiles, Target};
    use annulus_ui::kit::{Theme, UiState};

    use super::*;

    /// Draws `view` once with `events` at `time`; returns the frame's zones.
    fn frame(
        view: &mut SftpView,
        state: &mut UiState,
        events: &[annulus_ui::kit::Event],
        time: f64,
    ) -> Vec<annulus_ui::kit::Zone> {
        let mut canvas = CpuCanvas::new(1100, 680, 1.0, TextEngine::new()).unwrap();
        state.begin_frame(events, time);
        let viewport = Rect::new(0.0, 0.0, 1100.0, 680.0);
        let mut ui = Ui::new(&mut canvas, Theme::new(Appearance::Dark), state, viewport);
        view.draw(&mut ui, viewport, None);
        // Menus are drawn over everything, as the window does.
        ui.overlays();
        state.end_frame().zones
    }

    fn click(zones: &[annulus_ui::kit::Zone], name: &str) -> Vec<annulus_ui::kit::Event> {
        use annulus_ui::kit::{Button, Event};
        let zone = zones
            .iter()
            .rev()
            .find(|zone| zone.label == name)
            .unwrap_or_else(|| panic!("no zone {name:?}"));
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    }

    /// Drags the row named `from` and lets it go over the row named `onto`.
    fn drag(view: &mut SftpView, state: &mut UiState, from: &str, onto: &str, time: f64) {
        use annulus_ui::kit::{Button, Event};
        let zones = frame(view, state, &[], time);
        let at = |name: &str| {
            zones
                .iter()
                .find(|zone| zone.label == name)
                .unwrap_or_else(|| panic!("no row {name:?}"))
                .rect
                .center()
        };
        let (start, end) = (at(from), at(onto));
        let moving = [
            Event::PointerMoved(start),
            Event::PointerDown(Button::Primary),
            Event::PointerMoved(end),
        ];
        frame(view, state, &moving, time + 0.1);
        frame(
            view,
            state,
            &[Event::PointerUp(Button::Primary)],
            time + 0.2,
        );
    }

    #[test]
    fn a_file_dragged_onto_the_other_pane_is_uploaded_or_downloaded() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("drag"));
        let mut state = UiState::default();
        drag(&mut view, &mut state, "todo.txt", "README.md", 0.0);
        let labels = |view: &SftpView| -> Vec<String> {
            view.tab
                .transfers
                .iter()
                .map(|transfer| transfer.label.clone())
                .collect()
        };
        assert_eq!(labels(&view), ["\u{2191} todo.txt"], "local onto the host");
        drag(&mut view, &mut state, "deploy.sh", "backup.tar.gz", 1.0);
        assert_eq!(
            labels(&view),
            ["\u{2191} todo.txt", "\u{2193} deploy.sh"],
            "the host's onto this computer"
        );
        // Let go over its own pane, nothing moves.
        drag(&mut view, &mut state, "app.log", "README.md", 2.0);
        assert_eq!(view.tab.transfers.len(), 2);
        // A folder from this computer goes with everything in it.
        drag(&mut view, &mut state, "notes", "README.md", 3.0);
        assert_eq!(labels(&view)[2], "\u{2191} notes/");
        // And one from the host comes down whole.
        drag(&mut view, &mut state, "config", "todo.txt", 4.0);
        assert_eq!(labels(&view)[3], "\u{2193} config/");
    }

    #[test]
    fn a_right_click_on_a_file_opens_its_menu_in_either_pane() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("right-click"));
        let mut state = UiState::default();
        let zones = frame(&mut view, &mut state, &[], 0.0);
        let right_click = |zones: &[annulus_ui::kit::Zone], name: &str| {
            let at = zones
                .iter()
                .find(|zone| zone.label == name)
                .unwrap()
                .rect
                .center();
            vec![
                annulus_ui::kit::Event::PointerMoved(at),
                annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
                annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
            ]
        };
        let items = |zones: &[annulus_ui::kit::Zone]| -> Vec<String> {
            zones
                .iter()
                .filter(|zone| zone.role == Role::MenuItem)
                .map(|zone| zone.label.clone())
                .collect()
        };
        // On the host: its whole menu.
        let zones = frame(&mut view, &mut state, &right_click(&zones, "app.log"), 1.0);
        let menu = items(&zones);
        assert!(menu.contains(&"Download".to_owned()) && menu.contains(&"Rename".to_owned()));
        // Choosing from it acts like the More menu: Download starts the download.
        let zones = frame(&mut view, &mut state, &click(&zones, "Download"), 2.0);
        assert_eq!(view.tab.transfers.len(), 1);
        // On this computer: what a file manager offers, and up to the host.
        let zones = frame(&mut view, &mut state, &right_click(&zones, "todo.txt"), 3.0);
        let menu = items(&zones);
        for item in [
            "Open",
            "Upload to the host",
            "Rename",
            "Copy path",
            "Properties",
            "Delete",
        ] {
            assert!(menu.contains(&item.to_owned()), "{item} in {menu:?}");
        }
        assert!(
            menu.iter().any(|item| item.starts_with("Show in ")),
            "{menu:?}"
        );
        frame(
            &mut view,
            &mut state,
            &click(&zones, "Upload to the host"),
            4.0,
        );
        assert_eq!(view.tab.transfers.len(), 2);
    }

    /// The labels of `view`'s transfers.
    fn labels(view: &SftpView) -> Vec<String> {
        view.tab
            .transfers
            .iter()
            .map(|transfer| transfer.label.clone())
            .collect()
    }

    /// The name and policy a transfer's recipe lands with.
    fn landing(view: &SftpView, at: usize) -> (Option<String>, Conflict) {
        use crate::sftp_view::Recipe;
        match &view.tab.transfers[at].recipe {
            Recipe::Upload { name, conflict, .. }
            | Recipe::UploadTree { name, conflict, .. }
            | Recipe::Download { name, conflict, .. }
            | Recipe::DownloadTree { name, conflict, .. } => (name.clone(), *conflict),
        }
    }

    #[test]
    fn a_name_taken_where_a_file_goes_asks_to_skip_replace_or_rename_it() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("clash"));
        // The host has README.md already; so does this computer have deploy.sh.
        std::fs::write(view.tab.local.dir.join("README.md"), "mine").unwrap();
        std::fs::write(view.tab.local.dir.join("deploy.sh"), "mine").unwrap();
        let mut state = UiState::default();
        drag(&mut view, &mut state, "README.md", "app.log", 0.0);
        assert!(
            view.tab.transfers.is_empty(),
            "nothing goes before the answer"
        );
        let zones = frame(&mut view, &mut state, &[], 1.0);
        for button in ["Skip", "Replace", "Rename"] {
            assert!(zones.iter().any(|zone| zone.label == button), "{button}");
        }
        // Rename keeps both, under a free name to start with.
        assert_eq!(view.clashes[0].rename, "README (1).md");
        frame(&mut view, &mut state, &click(&zones, "Rename"), 2.0);
        assert_eq!(labels(&view), ["\u{2191} README (1).md"]);
        assert_eq!(
            landing(&view, 0),
            (Some("README (1).md".into()), Conflict::KeepBoth)
        );
        // Replace sends it under its own name, over what is there.
        drag(&mut view, &mut state, "README.md", "app.log", 3.0);
        let zones = frame(&mut view, &mut state, &[], 4.0);
        frame(&mut view, &mut state, &click(&zones, "Replace"), 5.0);
        assert_eq!(landing(&view, 1), (None, Conflict::Overwrite));
        // Skip sends nothing.
        drag(&mut view, &mut state, "README.md", "app.log", 6.0);
        let zones = frame(&mut view, &mut state, &[], 7.0);
        frame(&mut view, &mut state, &click(&zones, "Skip"), 8.0);
        assert_eq!(view.tab.transfers.len(), 2);
        assert!(view.clashes.is_empty());
        // A download onto a name taken on this computer asks the same (deploy.sh is in both
        // panes now, so it is sent as its Download item would).
        let host_copy = view
            .tab
            .entries
            .iter()
            .find(|e| e.name == "deploy.sh")
            .cloned();
        view.send_down(host_copy.unwrap());
        assert!(view.clashes.len() == 1 && view.tab.transfers.len() == 2);
        // A name typed for Rename that is taken too is refused.
        view.clashes[0].rename = "todo.txt".into();
        let zones = frame(&mut view, &mut state, &[], 10.0);
        frame(&mut view, &mut state, &click(&zones, "Rename"), 11.0);
        assert_eq!(view.clashes.len(), 1, "still asking");
        // Without asking, the tab's policy decides as before.
        view.clashes.clear();
        view.tab.ask_on_clash = false;
        drag(&mut view, &mut state, "README.md", "app.log", 12.0);
        assert_eq!(view.tab.transfers.len(), 3);
        assert_eq!(landing(&view, 2), (None, Conflict::KeepBoth));
    }

    #[test]
    fn one_answer_can_go_for_every_clash_waiting() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("clash-all"));
        let dir = view.tab.local.dir.clone();
        for name in ["README.md", "app.log"] {
            std::fs::write(dir.join(name), "mine").unwrap();
        }
        let mut state = UiState::default();
        frame(&mut view, &mut state, &[], 0.0);
        let dropped = annulus_ui::kit::Event::FilesDropped(
            vec![
                dir.join("README.md"),
                dir.join("app.log"),
                dir.join("todo.txt"),
            ],
            None,
        );
        frame(&mut view, &mut state, &[dropped], 1.0);
        assert_eq!(labels(&view), ["\u{2191} todo.txt"], "no clash: it goes");
        let zones = frame(&mut view, &mut state, &[], 2.0);
        let all = "Do the same for the 1 other waiting";
        let zones = frame(&mut view, &mut state, &click(&zones, all), 3.0);
        frame(&mut view, &mut state, &click(&zones, "Rename"), 4.0);
        assert_eq!(
            labels(&view),
            [
                "\u{2191} todo.txt",
                "\u{2191} README (1).md",
                "\u{2191} app (1).log"
            ],
            "each takes a free name of its own"
        );
        assert!(view.clashes.is_empty());
    }

    #[test]
    fn files_from_the_os_let_go_over_this_computer_are_copied_into_its_folder() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("copy-in"));
        let source = crate::native::demo::scratch("sftp", "copy-in-source");
        let _ = std::fs::remove_dir_all(&source);
        std::fs::create_dir_all(source.join("photos")).unwrap();
        std::fs::write(source.join("photos/cat.jpg"), "meow").unwrap();
        std::fs::write(source.join("todo.txt"), "theirs").unwrap();
        let mut state = UiState::default();
        let zones = frame(&mut view, &mut state, &[], 0.0);
        let here = zones
            .iter()
            .find(|zone| zone.label == "backup.tar.gz")
            .unwrap()
            .rect
            .center();
        let dropped = annulus_ui::kit::Event::FilesDropped(
            vec![source.join("photos"), source.join("todo.txt")],
            Some(here),
        );
        frame(&mut view, &mut state, &[dropped], 1.0);
        assert!(view.tab.transfers.is_empty(), "nothing goes to the host");
        // todo.txt is here already: it asks; the folder is copied meanwhile.
        assert_eq!(view.clashes.len(), 1);
        let zones = frame(&mut view, &mut state, &[], 2.0);
        frame(&mut view, &mut state, &click(&zones, "Replace"), 3.0);
        let dir = view.tab.local.dir.clone();
        let started = std::time::Instant::now();
        while std::fs::read_to_string(dir.join("todo.txt")).unwrap() != "theirs"
            || !dir.join("photos/cat.jpg").exists()
        {
            assert!(started.elapsed().as_secs() < 10, "the copies never landed");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("photos/cat.jpg")).unwrap(),
            "meow"
        );
        assert!(
            !local_names(&dir)
                .iter()
                .any(|name| name.ends_with(".annulus-part")),
            "no part file is left"
        );
    }

    #[test]
    fn a_drag_offers_the_panes_that_take_it() {
        let row = |dragged| Some((dragged, Point::new(1.0, 1.0)));
        let local = Dragged::Local(PathBuf::from("a.txt"));
        let remote = Dragged::Remote(Entry {
            name: "b.txt".into(),
            kind: annulus_sftp::Kind::File,
            size: 1,
            modified: None,
            permissions: None,
            uid: None,
            gid: None,
            accessed: None,
        });
        assert_eq!(
            drop_targets(row(local).as_ref(), FromOs::No),
            [Side::Remote]
        );
        assert_eq!(
            drop_targets(row(remote).as_ref(), FromOs::No),
            [Side::Local]
        );
        // From the OS: either pane where the OS says where it is; else the host's.
        let at = FromOs::At(Point::new(5.0, 5.0));
        assert_eq!(drop_targets(None, at), [Side::Local, Side::Remote]);
        assert_eq!(drop_targets(None, FromOs::Somewhere), [Side::Remote]);
        assert_eq!(drop_targets(None, FromOs::No), Vec::<Side>::new());
    }

    #[test]
    fn the_listings_show_permissions_and_a_right_click_on_the_headers_picks_columns() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("columns"));
        let mut state = UiState::default();
        let zones = frame(&mut view, &mut state, &[], 0.0);
        let has = |zones: &[annulus_ui::kit::Zone], name: &str| {
            zones.iter().filter(|zone| zone.label == name).count()
        };
        assert_eq!(has(&zones, "Sort by permissions"), 2, "in both panes");
        let header = zones
            .iter()
            .rev()
            .find(|zone| zone.label == "Sort by modified")
            .unwrap()
            .rect
            .center();
        let right = [
            annulus_ui::kit::Event::PointerMoved(header),
            annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
            annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
        ];
        let zones = frame(&mut view, &mut state, &right, 1.0);
        let menu: Vec<&str> = zones
            .iter()
            .filter(|zone| zone.role == Role::MenuItem)
            .map(|zone| zone.label.as_str())
            .collect();
        assert!(
            menu.contains(&"Hide Permissions") && menu.contains(&"Show Group"),
            "{menu:?}"
        );
        assert!(
            menu.contains(&"Hide Owner (no room for it now)"),
            "shown, but this window is too narrow: {menu:?}"
        );
        assert!(
            !menu.contains(&"Show Created"),
            "the host keeps no creation time"
        );
        frame(
            &mut view,
            &mut state,
            &click(&zones, "Hide Permissions"),
            2.0,
        );
        let zones = frame(&mut view, &mut state, &[], 3.0);
        assert_eq!(
            has(&zones, "Sort by permissions"),
            1,
            "gone from the host's pane only"
        );
        let (local, remote) = view.take_columns().unwrap();
        assert!(local.contains(&"permissions".to_owned()));
        assert!(!remote.contains(&"permissions".to_owned()));
        assert_eq!(view.take_columns(), None, "told once");
        // A column asked for comes first when room runs short, so it shows.
        let header = zones
            .iter()
            .rev()
            .find(|zone| zone.label == "Sort by modified")
            .unwrap()
            .rect
            .center();
        let right = [
            annulus_ui::kit::Event::PointerMoved(header),
            annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
            annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
        ];
        let zones = frame(&mut view, &mut state, &right, 4.0);
        frame(&mut view, &mut state, &click(&zones, "Show Group"), 5.0);
        let zones = frame(&mut view, &mut state, &[], 6.0);
        assert_eq!(has(&zones, "Sort by group"), 1);
        // The host's permissions read as a file manager shows them.
        let row = Row::remote(&view.tab.entries[4], &Owners::default());
        assert_eq!(row.cell(Column::Permissions), "rw-r-----");
    }

    #[test]
    fn this_computers_files_are_made_renamed_and_deleted_after_a_yes() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("local-ops"));
        let dir = view.tab.local.dir.clone();
        let mut state = UiState::default();
        let zones = frame(&mut view, &mut state, &[], 0.0);
        // New folder from the empty space's menu.
        let blank = zones
            .iter()
            .find(|zone| zone.id == Id::new(("sftp.blank", true)))
            .unwrap()
            .rect;
        let below = Point::new(blank.center().x, blank.b() - 10.0);
        let right = [
            annulus_ui::kit::Event::PointerMoved(below),
            annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
            annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
        ];
        let zones = frame(&mut view, &mut state, &right, 1.0);
        frame(&mut view, &mut state, &click(&zones, "New folder"), 2.0);
        let zones = frame(&mut view, &mut state, &[], 3.0);
        let mut typing = click(&zones, "Folder name");
        typing.push(annulus_ui::kit::Event::Text("drafts".into()));
        frame(&mut view, &mut state, &typing, 4.0);
        let enter = [annulus_ui::kit::Event::Key(Key::Enter, Modifiers::NONE)];
        frame(&mut view, &mut state, &enter, 5.0);
        assert!(dir.join("drafts").is_dir());
        // F2 on a selected file renames it.
        let zones = frame(&mut view, &mut state, &[], 6.0);
        frame(&mut view, &mut state, &click(&zones, "todo.txt"), 7.0);
        let f2 = [annulus_ui::kit::Event::Key(Key::F(2), Modifiers::NONE)];
        frame(&mut view, &mut state, &f2, 8.0);
        if let Some((Side::Local, Edit::Rename { to, .. })) = &mut view.edit {
            *to = "done.txt".into();
        } else {
            panic!("F2 did not start a rename here");
        }
        let zones = frame(&mut view, &mut state, &[], 9.0);
        frame(&mut view, &mut state, &click(&zones, "Save"), 10.0);
        assert!(dir.join("done.txt").exists() && !dir.join("todo.txt").exists());
        // Delete asks first; only the card's Delete removes it.
        let zones = frame(&mut view, &mut state, &[], 11.0);
        frame(&mut view, &mut state, &click(&zones, "done.txt"), 12.0);
        let delete = [annulus_ui::kit::Event::Key(Key::Delete, Modifiers::NONE)];
        let zones = frame(&mut view, &mut state, &delete, 13.0);
        let zones = if zones.iter().any(|zone| zone.label == "Delete") {
            zones
        } else {
            frame(&mut view, &mut state, &[], 14.0)
        };
        assert!(
            dir.join("done.txt").exists(),
            "nothing is deleted before the yes"
        );
        frame(&mut view, &mut state, &click(&zones, "Delete"), 15.0);
        assert!(!dir.join("done.txt").exists());
    }

    #[test]
    fn files_dropped_from_the_os_upload_to_the_folder_shown() {
        let mut view = SftpView::new(crate::native::demo::sftp_tab("os-drop"));
        let file = view.tab.local.dir.join("todo.txt");
        let mut state = UiState::default();
        frame(&mut view, &mut state, &[], 0.0);
        frame(
            &mut view,
            &mut state,
            &[annulus_ui::kit::Event::FilesDropped(vec![file], None)],
            1.0,
        );
        assert_eq!(view.tab.transfers.len(), 1);
        assert!(view.tab.transfers[0].upload);
    }

    #[test]
    fn a_rename_started_in_one_folder_ends_when_another_is_shown() {
        let mut tab = SftpTab::unavailable("files", "offline");
        tab.cwd = Some("/home/a".into());
        tab.failed = None;
        let mut view = SftpView::new(tab);
        view.edit = Some((
            Side::Remote,
            Edit::Rename {
                from: "notes.txt".into(),
                to: "notes.txt".into(),
            },
        ));
        view.edit_dir = "/home/a".into();
        let mut state = UiState::default();
        frame(&mut view, &mut state, &[], 0.0);
        assert!(view.edit.is_some(), "same folder: still renaming");
        view.tab.cwd = Some("/home/a/sub".into());
        frame(&mut view, &mut state, &[], 1.0);
        assert!(view.edit.is_none());
    }

    /// End to end on test VM 1: a folder made with New folder appears, and Delete removes
    /// it only after the card's yes.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_the_new_sftp_tab_makes_a_folder_and_deletes_it_after_a_yes() {
        use annulus_ui::kit::Event;
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let dir =
            std::env::temp_dir().join(format!("annulus-native-sftp-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
        let started = std::time::Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        let name = format!("annulus-e2e-rm-{}", std::process::id());
        let has = |view: &SftpView| view.tab.entries.iter().any(|entry| entry.name == name);
        // Connected, the home folder listed.
        let mut zones = frame(&mut view, &mut state, &[], clock());
        while view.tab.cwd.is_none() || view.tab.entries.is_empty() {
            assert!(clock() < 60.0, "no listing; failed: {:?}", view.tab.failed);
            if matches!(view.ask, Some(Ask::HostKey(_))) {
                view.tab.answers().unwrap().host_key(true);
                view.ask = None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            zones = frame(&mut view, &mut state, &[], clock());
        }
        // New folder, typed and saved with Enter.
        frame(&mut view, &mut state, &click(&zones, "New folder"), clock());
        zones = frame(&mut view, &mut state, &[], clock());
        let mut typing = click(&zones, "Folder name");
        typing.push(Event::Text(name.clone()));
        frame(&mut view, &mut state, &typing, clock());
        let enter = [Event::Key(annulus_ui::kit::Key::Enter, Modifiers::NONE)];
        zones = frame(&mut view, &mut state, &enter, clock());
        while !has(&view) {
            assert!(
                clock() < 90.0,
                "the folder never appeared: {:?}",
                view.tab.list_error
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
            zones = frame(&mut view, &mut state, &[], clock());
        }
        // Delete asks; only the card's Delete removes it.
        frame(&mut view, &mut state, &click(&zones, &name), clock());
        let delete = [Event::Key(annulus_ui::kit::Key::Delete, Modifiers::NONE)];
        frame(&mut view, &mut state, &delete, clock());
        zones = frame(&mut view, &mut state, &[], clock());
        assert!(matches!(view.tab.dialog, Some(Dialog::Delete(_))));
        assert!(has(&view), "nothing is deleted before the yes");
        frame(&mut view, &mut state, &click(&zones, "Delete"), clock());
        while has(&view) {
            assert!(
                clock() < 120.0,
                "the folder was not deleted: {:?}",
                view.tab.list_error
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
            frame(&mut view, &mut state, &[], clock());
        }
    }

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
