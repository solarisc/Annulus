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
use std::sync::atomic::{AtomicU64, Ordering};
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
use crate::local_pane::{LocalChange, tilde};
use crate::sftp_view::{
    ASK_NAME, Column, Command, Dialog, Row, SftpTab, TransferState, conflict_name, format_time,
    human_size, pace, parse_mode, properties, sort_rows, valid_name,
};

const ROW: f32 = 30.0;

/// Makes local-copy part files unique across tabs and copy threads in this process.
static NEXT_LOCAL_COPY: AtomicU64 = AtomicU64::new(1);

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
    /// A copy in this tab has reserved the target; ask only after that copy has finished.
    waiting_on: Option<PathBuf>,
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

/// A copy into this computer's folder finished on its thread.
struct CopyNews {
    target: PathBuf,
    result: Result<String, String>,
    /// This message owns `target`'s reservation and must release it.
    release: bool,
}

/// The SFTP tab's view state around the engine.
#[allow(clippy::struct_excessive_bools)] // Separate switches of one view.
pub struct SftpView {
    pub(super) tab: SftpTab,
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
    /// Targets held by copies still running in this tab.
    copying: HashSet<PathBuf>,
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
            copying: HashSet::new(),
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
        let start_waker = waker.cloned();
        self.tab.start_with(move || {
            if let Some(waker) = &start_waker {
                waker();
            }
        });
        self.tab.drain();
        self.drain_local(ui, waker);
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
            if news.release {
                self.copying.remove(&news.target);
            }
            match news.result {
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

    fn drain_local(&mut self, ui: &mut Ui<'_>, waker: Option<&Waker>) {
        for change in self.tab.local.drain() {
            match change {
                Ok(LocalChange::Renamed(name)) => self.local_selected = Some(name),
                Ok(LocalChange::Created(name) | LocalChange::Deleted(name)) => {
                    let _ = name;
                }
                Err(error) => ui.toast(error, Tone::Problem),
            }
        }
        self.tab.local.refresh_if_dirty(self.tab.show_hidden, waker);
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
        let folder = self
            .tab
            .local
            .entries
            .iter()
            .any(|entry| entry.name == name && entry.is_dir);
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
                    waiting_on: None,
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
                waiting_on: None,
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
        let target = dir.join(&name);
        if path.parent() == Some(dir.as_path()) {
            return;
        }
        if dir.starts_with(&path) {
            self.copy_failed(target, format!("{name} cannot be copied into itself"));
            return;
        }
        let reserved = self.copying.contains(&target);
        if target.symlink_metadata().is_ok() || reserved {
            let mut taken = local_names(&dir);
            taken.extend(
                self.copying
                    .iter()
                    .filter(|path| path.parent() == Some(dir.as_path()))
                    .map(|path| file_name(path)),
            );
            let clash = Clash {
                name: name.clone(),
                folder: path.is_dir(),
                place: tilde(&dir.display().to_string(), &home_dir()),
                there: if reserved {
                    "Another copy is writing this name".to_owned()
                } else {
                    local_facts(&target)
                },
                coming: local_facts(&path),
                rename: free_name(&name, &taken),
                taken,
                incoming: Incoming::Copy(path),
                waiting_on: reserved.then_some(target),
            };
            self.clashes.push_back(clash);
        } else {
            self.copy_now(path, target, false);
        }
    }

    /// Copies `from` to `to` on a thread of its own, replacing what is there when `replace`;
    /// the toast says how it went.
    fn copy_now(&mut self, from: PathBuf, to: PathBuf, replace: bool) {
        if !self.copying.insert(to.clone()) {
            self.copy_failed(to, "A copy to that name is already running".to_owned());
            return;
        }
        let news = self.copies.0.clone();
        let waker = self.waker.clone();
        let name = file_name(&to);
        let target = to.clone();
        let destination = to.clone();
        let spawned = std::thread::Builder::new()
            .name("local copy".into())
            .spawn(move || {
                let result = copy_into(&from, &destination, replace)
                    .map(|()| name.clone())
                    .map_err(|error| format!("Could not copy {name}: {error}"));
                let _ = news.send(CopyNews {
                    target,
                    result,
                    release: true,
                });
                if let Some(waker) = waker {
                    waker();
                }
            });
        if let Err(error) = spawned {
            self.copying.remove(&to);
            self.copy_failed(to, format!("Could not copy: {error}"));
        }
    }

    fn copy_failed(&self, target: PathBuf, why: String) {
        let _ = self.copies.0.send(CopyNews {
            target,
            result: Err(why),
            release: false,
        });
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
}

mod view;

impl SftpView {
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
    let copy = NEXT_LOCAL_COPY.fetch_add(1, Ordering::Relaxed);
    let part = to.with_file_name(format!(
        ".{}.annulus-part-{}-{copy}",
        file_name(to),
        std::process::id()
    ));
    let result = std::fs::copy(from, &part).and_then(|_| install_copy(&part, to, replace, copy));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Puts a complete part file in place. Unix rename replaces a file atomically. Windows rename
/// does not, so keep the old file beside it until the new rename has worked.
fn install_copy(part: &Path, to: &Path, replace: bool, copy: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    if replace && to.symlink_metadata().is_ok() {
        if to.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "a folder already has that name",
            ));
        }
        let old = to.with_file_name(format!(
            ".{}.annulus-old-{}-{copy}",
            file_name(to),
            std::process::id()
        ));
        std::fs::rename(to, &old)?;
        return match std::fs::rename(part, to) {
            Ok(()) => {
                let _ = std::fs::remove_file(old);
                Ok(())
            }
            Err(error) => match std::fs::rename(&old, to) {
                Ok(()) => Err(error),
                Err(restore) => Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "{error}; the old file is at {} because it could not be restored: {restore}",
                        old.display()
                    ),
                )),
            },
        };
    }
    let _ = (replace, copy);
    std::fs::rename(part, to)
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
mod tests;
