//! The vault's other screens in the new UI: Keychain, Port Forwarding, Snippets,
//! Known Hosts, Logs and Settings, built on [`super::pages`].

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::{Program, SshFiles};
use annulus_ssh::keys::{self, KeyKind, KeyPair};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{FOCUS_REACH, Id, Key, Modifiers, Ui};
use annulus_ui::motion::{Curve, Feel, Layer, Motion};
use annulus_ui::widgets::{
    CONTROL, FieldStyle, Kind, Revealed, SCROLL_GUTTER, SMALL, button_width,
};
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use super::pages::{self, Fold, ListMotion, ROW_H, RowLook};
use crate::job::{Job, Poll};
use crate::ssh_task::SshTask;
use crate::{connect, keychain_screen, snippets_screen};

/// Whether the rows of any of the vault's lists are gliding to new places at `now`.
#[cfg(test)]
pub fn lists_gliding(
    (keychain, snippets, forwards): (&KeychainView, &SnippetsView, &ForwardsView),
    (known, logs): (&KnownView, &LogsView),
    now: f64,
) -> bool {
    [
        &keychain.rows,
        &snippets.rows,
        &forwards.rows,
        &known.rows,
        &logs.rows,
    ]
    .iter()
    .any(|rows| rows.gliding(now))
}

/// What a screen asks the window to do.
pub enum ScreenAction {
    /// Open a terminal tab running a program.
    Terminal(String, Box<Program>),
    /// Open one tab of split panes, one per program (a snippet run on several hosts).
    Terminals(String, snippets_screen::Panes),
    /// Open a log, read-only, in a tab.
    Log(String, String),
}

const GAP: f32 = 8.0;
/// How long the accent stays on a new row, in milliseconds.
const ROW_GLOW: u32 = 1400;
/// Height of the script editor: about five lines.
const SCRIPT_H: f32 = 104.0;

/// Draws a row of `ROW_H` for each of `looks` in a scroll view filling `rect`. When the list
/// changes, what stays glides to its new place, a new row rises in with a moment of the
/// accent on it, and a row that went fades out where it was.
fn rows(
    ui: &mut Ui<'_>,
    id: &str,
    rect: Rect,
    looks: &[RowLook],
    motion: &mut ListMotion,
    mut each: impl FnMut(&mut Ui<'_>, usize, Rect),
) {
    #[allow(clippy::cast_precision_loss)] // Lists are short.
    let height = looks.len() as f32 * (ROW_H + GAP);
    let offset = ui.scroll_view(Id::new(id), rect.wider(SCROLL_GUTTER), height);
    let play = motion.begin(ui, looks);
    let list = Id::new(id);
    let width = rect.w;
    ui.push_list_clip(rect, offset, height, FOCUS_REACH);
    motion.draw_leaving(ui, rect, offset, ROW_H + GAP);
    for (index, look) in looks.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // Lists are short.
        let target = index as f32 * (ROW_H + GAP);
        let at = ui.spring_with(list.with(("at", look.key)), target, Feel::Soft);
        let arrived = ui.entrance_if(list.with(("in", look.key)), Motion::SLOW, play);
        let glow = 1.0
            - ui.entrance_if(
                list.with(("glow", look.key)),
                Motion::new(ROW_GLOW, Curve::Out),
                play,
            );
        let y = rect.y - offset + at;
        if y + ROW_H < rect.y || y > rect.b() {
            continue;
        }
        let row = Rect::new(rect.x, y, width, ROW_H);
        let arriving = arrived < 1.0;
        if arriving {
            ui.push_layer(
                Layer::fade(arrived)
                    .shifted(0.0, 14.0 * (1.0 - arrived))
                    .grouped(row.outset(2.0)),
            );
        }
        each(ui, index, row);
        if glow > 0.0 {
            let t = ui.theme;
            ui.gfx.fill_rect(row, 10.0, t.accent.alpha(0.14 * glow));
        }
        if arriving {
            ui.pop_layer();
        }
    }
    ui.pop_clip();
    ui.scroll_edges(rect, offset, height, ui.theme.bg);
}

/// Small icon buttons at the right of a row, right to left; returns which was clicked and
/// the remainder of the row.
fn row_buttons(
    ui: &mut Ui<'_>,
    row: Rect,
    id: Id,
    buttons: &[(IconId, &str)],
) -> (Option<usize>, Rect) {
    let mut remainder = row.inset(space::M, 0.0);
    let mut slots: Vec<Rect> = Vec::new();
    for _ in buttons {
        slots.push(remainder.take_right(30.0).centered(30.0, 30.0));
        remainder.take_right(4.0);
    }
    let mut clicked = None;
    // They come up with the pointer or the keyboard, so a list reads as a list; they stay
    // reachable either way, and their room is kept so nothing moves.
    let focused = ui.state.focus();
    let near = ui.hover_within(row)
        || (0..buttons.len()).any(|index| focused == Some(id.with(("button", index))));
    let shown = ui.ease(id.with("near"), if near { 1.0 } else { 0.0 })
        * ui.entrance(id.with("buttons"), Motion::BASE);
    ui.push_layer(Layer::fade(shown));
    // Drawn left to right, so Tab follows the eye.
    for index in (0..buttons.len()).rev() {
        let (icon, name) = buttons[index];
        if ui
            .icon_button(id.with(("button", index)), slots[index], icon, name, false)
            .clicked
        {
            clicked = Some(index);
        }
    }
    ui.pop_layer();
    (clicked, remainder)
}

/// Cancel and a red `delete` button at the right of a row, asking once more before a
/// delete; returns `Some(true)` to delete, `Some(false)` to keep, and the rest of the row.
fn confirm_delete(ui: &mut Ui<'_>, row: Rect, id: Id, delete: &str) -> (Option<bool>, Rect) {
    let mut remainder = row.inset(space::M, 0.0);
    let delete_w = button_width(&mut ui.gfx, delete, None);
    let delete_rect = remainder.take_right(delete_w).centered(delete_w, 30.0);
    remainder.take_right(space::S);
    let cancel_w = button_width(&mut ui.gfx, "Cancel", None);
    let cancel = remainder.take_right(cancel_w).centered(cancel_w, 30.0);
    let mut answer = None;
    let shown = ui.entrance(id.with("asking"), Motion::BASE);
    // They slide in from behind the row's right edge, which the list's clip no longer is.
    ui.push_clip(row);
    ui.push_layer(Layer::fade(shown).shifted(16.0 * (1.0 - shown), 0.0));
    // Drawn left to right, so Tab follows the eye.
    if ui
        .button(id.with("cancel"), cancel, "Cancel", None, Kind::Ghost)
        .clicked
    {
        answer = Some(false);
    }
    if ui
        .button(id.with("delete"), delete_rect, delete, None, Kind::Danger)
        .clicked
    {
        answer = Some(true);
    }
    ui.pop_layer();
    ui.pop_clip();
    if ui.key(Key::Escape, Modifiers::NONE) {
        answer = Some(false);
    }
    (answer, remainder)
}

/// Segmented choice of `items`; returns the one clicked.
fn segmented(
    ui: &mut Ui<'_>,
    id: &str,
    rect: Rect,
    items: &[&str],
    selected: usize,
) -> Option<usize> {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 8.0, t.field);
    ui.gfx.stroke_rect(rect, 8.0, 1.0, t.line);
    #[allow(clippy::cast_precision_loss)] // Two to four segments.
    let width = (rect.w - 4.0) / items.len() as f32;
    let mut chosen = None;
    for (index, item) in items.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // Two to four segments.
        let cell = Rect::new(
            rect.x + 2.0 + index as f32 * width,
            rect.y + 2.0,
            width,
            rect.h - 4.0,
        );
        let kind = if index == selected {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
        if ui
            .button(Id::new((id, index)), cell, item, None, kind)
            .clicked
        {
            chosen = Some(index);
        }
    }
    chosen
}

// ---------------------------------------------------------------------------------------
// Keychain

/// Height of a host line in the snippet's Run on hosts card.
const LINE: f32 = 30.0;

/// Height of the pasted private key: about six lines.
const KEY_H: f32 = 124.0;

/// What the card at the top of the Keychain is doing.
enum KeyForm {
    /// Making a new key; RSA takes seconds, so it runs on another thread.
    Generate {
        label: String,
        kind: KeyKind,
        working: Option<Job<Result<KeyPair, keys::KeyError>>>,
    },
    /// Pasting a key in; both secrets are wiped when the form closes.
    Import {
        label: String,
        private: Zeroizing<String>,
        passphrase: Zeroizing<String>,
    },
    /// Editing a saved key: its name, and its private key, hidden until
    /// asked for and then shown, copied or replaced by another pasted in. Both secrets are
    /// wiped when the form closes.
    Edit {
        id: u64,
        label: String,
        private: Zeroizing<String>,
        passphrase: Zeroizing<String>,
        /// The private key is shown as it is, not as dots.
        shown: bool,
    },
}

impl KeyForm {
    /// Wipes what was pasted and stops what is under way, as the card folds away.
    fn forget(&mut self) {
        match self {
            Self::Generate { working, .. } => *working = None,
            Self::Import {
                private,
                passphrase,
                ..
            } => {
                *private = Zeroizing::new(String::new());
                *passphrase = Zeroizing::new(String::new());
            }
            Self::Edit {
                private,
                passphrase,
                shown,
                ..
            } => {
                *private = Zeroizing::new(String::new());
                *passphrase = Zeroizing::new(String::new());
                *shown = false;
            }
        }
    }

    /// The form editing saved key `key`, its private key hidden.
    fn edit(key: &SshKey) -> Self {
        Self::Edit {
            id: key.id,
            label: key.label.clone(),
            private: Zeroizing::new(key.private.expose().to_owned()),
            passphrase: Zeroizing::new(String::new()),
            shown: false,
        }
    }
}

/// Installing a key's public half on a saved host (F-KEY-004).
struct Install {
    key_id: u64,
    host_id: Option<u64>,
    task: Option<SshTask>,
    ask: Option<Ask>,
}

impl Install {
    /// Stops the install as its card folds away.
    fn stop(&mut self) {
        (self.task, self.ask) = (None, None);
    }
}

/// What happens to the line under the Keychain's cards.
#[derive(Default)]
enum Say {
    /// It stays as it is.
    #[default]
    Same,
    /// It goes.
    Nothing,
    /// It says this; `true` for a problem.
    Line(bool, String),
}

/// What a card on the Keychain came to.
#[derive(Default)]
struct CardEnd {
    /// It folds away.
    close: bool,
    say: Say,
}

impl CardEnd {
    fn closes() -> Self {
        Self {
            close: true,
            say: Say::Same,
        }
    }

    fn says(problem: bool, line: impl Into<String>) -> Self {
        Self {
            close: false,
            say: Say::Line(problem, line.into()),
        }
    }

    fn closes_saying((problem, line): (bool, String)) -> Self {
        Self {
            close: true,
            say: Say::Line(problem, line),
        }
    }

    fn quiet() -> Self {
        Self {
            close: false,
            say: Say::Nothing,
        }
    }
}

/// Deleting a key that hosts log in with (F-KEY-004): it comes off those servers'
/// `authorized_keys` first, while it can still log in, then out of the vault.
struct Removal {
    key_id: u64,
    /// Take it off the servers too; on unless unticked.
    from_servers: bool,
    /// Each server's run, once Delete was clicked.
    runs: Vec<ServerRun>,
    /// The key's name, which outlives it while the card folds away.
    label: String,
    /// What the card last said, likewise.
    about: String,
}

impl Removal {
    /// Stops what is running as the card folds away; a server may have taken the key off.
    fn stop(&mut self) {
        for run in &mut self.runs {
            (run.task, run.ask) = (None, None);
        }
    }
}

/// Taking a key off one server.
struct ServerRun {
    label: String,
    task: Option<SshTask>,
    ask: Option<Ask>,
    ended: Option<Result<usize, String>>,
}

/// One key as the list shows it.
struct KeyRow {
    id: u64,
    label: String,
    detail: String,
    public: String,
    /// How many hosts log in with it.
    users: usize,
}

/// The vault's keys as the list shows them, each with how many hosts log in with it.
fn key_rows(vault: &Vault) -> Vec<KeyRow> {
    vault
        .ssh_keys()
        .iter()
        .map(|key| {
            let users = users_of(vault, key.id).len();
            let algorithm = key.public.split_whitespace().next().unwrap_or_default();
            KeyRow {
                id: key.id,
                label: key.label.clone(),
                // Who uses it first: a narrow row (one asking before a delete) cuts the
                // fingerprint's end, never that.
                detail: format!(
                    "Used by {}  \u{b7}  {algorithm}  {}",
                    pages::count(users, "host", "hosts"),
                    key.fingerprint
                ),
                public: key.public.clone(),
                users,
            }
        })
        .collect()
}

/// The hosts that log in with key `key_id`, by id and name: those that name it, and those
/// that take it from their group, as a connection resolves them.
fn users_of(vault: &Vault, key_id: u64) -> Vec<(u64, String)> {
    vault
        .hosts()
        .iter()
        .filter(|host| vault.resolve(host).0.key_id == Some(key_id))
        .map(|host| (host.id, host.label.clone()))
        .collect()
}

/// The Keychain screen.
#[derive(Default)]
pub struct KeychainView {
    form: Fold<KeyForm>,
    /// The last result, and whether it is a problem.
    message: Fold<(bool, String)>,
    confirm_delete: Option<u64>,
    install: Fold<Install>,
    removal: Fold<Removal>,
    rows: ListMotion,
}

impl KeychainView {
    /// Opens saved key `id` in the edit card.
    pub fn edit_key(&mut self, vault: &Vault, id: u64) {
        if let Some(key) = vault.ssh_key(id) {
            self.form.open(KeyForm::edit(key));
            self.message.close();
        }
    }

    /// Whether saved key `id` is open in the edit card.
    #[cfg(test)]
    pub fn editing(&self, id: u64) -> bool {
        matches!(self.form.get(), Some(KeyForm::Edit { id: open, .. }) if *open == id)
    }

    /// Draws the screen.
    #[allow(clippy::too_many_lines)] // Header, cards and list, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        let mut area = pages::page(rect);
        let count = vault.ssh_keys().len();
        let mut actions = pages::header(
            ui,
            &mut area,
            "Keychain",
            Some(count),
            "Keys sync with your vault, encrypted. Hosts without one use ~/.ssh.",
        );
        let generate =
            actions.take_right(button_width(&mut ui.gfx, "Generate key", Some(icons::PLUS)));
        actions.take_right(space::S);
        let import = actions.take_right(button_width(
            &mut ui.gfx,
            "Import key",
            Some(icons::DOWNLOAD),
        ));
        // Drawn left to right, so Tab follows the eye.
        if ui
            .button(
                Id::new("keys.import"),
                import,
                "Import key",
                Some(icons::DOWNLOAD),
                Kind::Secondary,
            )
            .clicked
        {
            self.form.open(KeyForm::Import {
                label: String::new(),
                private: Zeroizing::new(String::new()),
                passphrase: Zeroizing::new(String::new()),
            });
            self.message.close();
        }
        if ui
            .button(
                Id::new("keys.generate"),
                generate,
                "Generate key",
                Some(icons::PLUS),
                Kind::Primary,
            )
            .clicked
        {
            self.form.open(KeyForm::Generate {
                label: String::new(),
                kind: KeyKind::Ed25519,
                working: None,
            });
            self.message.close();
        }
        let mut end = CardEnd::default();
        self.form
            .draw(ui, Id::new("keys.form"), &mut area, |ui, area, form| {
                end = form_card(form, ui, area, vault, waker);
            });
        self.finish_card(end, |view| view.form.close_with(KeyForm::forget));
        let mut end = CardEnd::default();
        self.install.draw(
            ui,
            Id::new("keys.install"),
            &mut area,
            |ui, area, install| {
                end = install_card(install, ui, area, vault, files, waker);
            },
        );
        self.finish_card(end, |view| view.install.close_with(Install::stop));
        let mut end = CardEnd::default();
        self.removal.draw(
            ui,
            Id::new("keys.removal"),
            &mut area,
            |ui, area, removal| {
                end = removal_card(removal, ui, area, vault, files, waker);
            },
        );
        self.finish_card(end, |view| view.removal.close_with(Removal::stop));
        self.message.draw(
            ui,
            Id::new("keys.message"),
            &mut area,
            |ui, area, (problem, message)| {
                let t = ui.theme;
                let color = if *problem { t.error } else { t.text2 };
                ui.gfx
                    .text(area.take_top(20.0), message, SMALL, color, Align::Start);
                area.take_top(space::S);
            },
        );
        if count == 0 {
            self.rows.empty(ui, area, ROW_H + GAP);
            pages::empty(
                ui,
                area,
                icons::KEY,
                "No keys yet",
                "Generate one here, or import a key you already use.",
            );
        } else {
            self.list(ui, area, vault);
        }
        // A question the install's connection asks, over the screen.
        if let Some(install) = self.install.get_mut()
            && let (Some(task), Some(ask_now)) = (&install.task, &mut install.ask)
            && ask::card(ui, Id::new("install.ask"), rect, ask_now, task.answers())
        {
            install.ask = None;
        }
        // One at a time, a question a server the key comes off asks.
        if let Some(removal) = self.removal.get_mut()
            && let Some((index, run)) = removal
                .runs
                .iter_mut()
                .enumerate()
                .find(|(_, run)| run.ask.is_some())
            && let (Some(task), Some(ask_now)) = (&run.task, &mut run.ask)
            && ask::card(
                ui,
                Id::new(("removal.ask", index)),
                rect,
                ask_now,
                task.answers(),
            )
        {
            run.ask = None;
        }
    }

    /// What the line under the cards says, and whether it is a problem.
    #[cfg(test)]
    pub fn said(&self) -> Option<&(bool, String)> {
        self.message.get()
    }

    /// Applies what a card came to; `close` folds it away.
    fn finish_card(&mut self, end: CardEnd, close: impl FnOnce(&mut Self)) {
        if end.close {
            close(self);
        }
        match end.say {
            Say::Line(problem, line) => self.message.open((problem, line)),
            Say::Nothing => self.message.close(),
            Say::Same => {}
        }
    }

    /// The keys, each with Copy, Install and Delete; Delete asks once more.
    #[allow(clippy::too_many_lines)] // The rows, then what they asked for.
    fn list(&mut self, ui: &mut Ui<'_>, area: Rect, vault: &mut Vault) {
        let list = key_rows(vault);
        let looks: Vec<RowLook> = list
            .iter()
            .map(|key| RowLook {
                key: key.id,
                icon: icons::KEY,
                title: key.label.clone(),
                detail: key.detail.clone(),
                mono: true,
            })
            .collect();
        let confirm = self.confirm_delete;
        let mut confirm_next = confirm;
        let (mut remove, mut install, mut copied, mut unkey) = (None, None, None, None);
        let mut edit = None;
        rows(
            ui,
            "keys.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let key = &list[index];
                let zone = Id::new(("key", key.id));
                // A click on the key opens it, as Edit does.
                if pages::row(ui, zone, row, &key.label, false).clicked && confirm != Some(key.id) {
                    edit = Some(key.id);
                }
                let remainder = if confirm == Some(key.id) {
                    let (answer, remainder) = confirm_delete(ui, row, zone, "Delete key");
                    match answer {
                        Some(true) => remove = Some(key.id),
                        Some(false) => confirm_next = None,
                        None => {}
                    }
                    remainder
                } else {
                    let (clicked, remainder) = row_buttons(
                        ui,
                        row,
                        zone,
                        &[
                            (icons::TRASH, "Delete"),
                            (icons::UPLOAD, "Install on a host"),
                            (icons::COPY, "Copy public key"),
                            (icons::EDIT, "Edit"),
                        ],
                    );
                    match clicked {
                        Some(3) => edit = Some(key.id),
                        Some(2) => {
                            ui.copy(key.public.clone());
                            copied = Some(key.label.clone());
                        }
                        Some(1) => install = Some(key.id),
                        // A key hosts log in with is asked about in a card: their servers too?
                        Some(0) if key.users > 0 => unkey = Some(key.id),
                        Some(0) => confirm_next = Some(key.id),
                        _ => {}
                    }
                    remainder
                };
                let inner = pages::row_icon(ui, remainder, icons::KEY);
                pages::two_lines(ui, inner, &key.label, &key.detail, true);
            },
        );
        self.confirm_delete = confirm_next;
        if let Some(key) = edit.and_then(|id| vault.ssh_key(id)) {
            self.form.open(KeyForm::edit(key));
            self.message.close();
        }
        if let Some(key_id) = unkey
            && self
                .removal
                .get()
                .is_none_or(|removal| removal.runs.is_empty())
        {
            self.removal.open(Removal {
                key_id,
                from_servers: true,
                runs: Vec::new(),
                label: list
                    .iter()
                    .find(|key| key.id == key_id)
                    .map_or_else(String::new, |key| key.label.clone()),
                about: String::new(),
            });
            self.install.close_with(Install::stop);
            self.message.close();
        }
        if let Some(label) = copied {
            self.message
                .open((false, format!("Copied the public key of {label}.")));
        }
        if let Some(key_id) = install {
            if vault.hosts().is_empty() {
                self.message.open((
                    true,
                    "Save a host first, then install the key on it.".into(),
                ));
            } else {
                self.install.open(Install {
                    key_id,
                    host_id: None,
                    task: None,
                    ask: None,
                });
                self.message.close();
            }
        }
        if let Some(id) = remove {
            self.confirm_delete = None;
            self.message.open(match vault.remove_key(id) {
                Ok(()) => (false, "Key deleted.".into()),
                Err(error) => (true, error.to_string()),
            });
        }
    }
}

/// Deleting a key that hosts log in with: the hosts it comes off, ticked, then each server's
/// progress. Once every server has answered, the key leaves the vault, unless a server still
/// has it: then it stays, so no way in is left behind unseen.
#[allow(clippy::too_many_lines)] // The card, then what its runs came to.
fn removal_card(
    removal: &mut Removal,
    ui: &mut Ui<'_>,
    area: &mut Rect,
    vault: &mut Vault,
    files: &SshFiles,
    waker: Option<&Waker>,
) -> CardEnd {
    let found = vault
        .ssh_keys()
        .iter()
        .find(|key| key.id == removal.key_id)
        .cloned();
    // On its way out it shows what it last showed; the key itself may be gone by then.
    if found.is_none() && !ui.inert() {
        return CardEnd::closes();
    }
    for run in &mut removal.runs {
        if let Some(task) = &mut run.task {
            while let Some(event) = task.question() {
                if let Some(question) = Ask::from_event(event) {
                    run.ask = Some(question);
                }
            }
            if let Some(outcome) = task.outcome() {
                run.ended = Some(keychain_screen::removal_result(outcome));
                (run.task, run.ask) = (None, None);
            }
        }
    }
    let busy = !removal.runs.is_empty();
    if let Some(key) = &found
        && busy
        && !ui.inert()
        && removal.runs.iter().all(|run| run.ended.is_some())
    {
        let servers: Vec<(String, Result<usize, String>)> = removal
            .runs
            .iter()
            .filter_map(|run| run.ended.clone().map(|ended| (run.label.clone(), ended)))
            .collect();
        let (mut problem, mut message) =
            keychain_screen::removal_summary(&key.label, &servers, &[]);
        if !problem && let Err(error) = vault.remove_key(key.id) {
            problem = true;
            message = format!(
                "Took {} off its servers, but could not delete it here: {error}",
                key.label
            );
        }
        return CardEnd::closes_saying((problem, message));
    }
    let users = found
        .as_ref()
        .map_or_else(Vec::new, |key| users_of(vault, key.id));
    let names: Vec<&str> = users.iter().map(|(_, label)| label.as_str()).collect();
    let t = ui.theme;
    let mut card = pages::card(
        ui,
        area,
        &format!("Delete {}", removal.label),
        2.0 * CONTROL + 20.0 + 5.0 * space::S,
    );
    card.take_top(space::S);
    if !ui.inert() {
        removal.about = if busy {
            removal
                .runs
                .iter()
                .map(|run| {
                    let state = match &run.ended {
                        None if run.ask.is_some() => "waiting for you",
                        None => "taking it off\u{2026}",
                        Some(Ok(0)) => "not found",
                        Some(Ok(_)) => "taken off",
                        Some(Err(_)) => "could not",
                    };
                    format!("{}: {state}", run.label)
                })
                .collect::<Vec<_>>()
                .join("  \u{b7}  ")
        } else if let [one] = names.as_slice() {
            format!("It logs in to {one}. Taken off that server, it no longer opens it.")
        } else {
            format!(
                "It logs in to {}. Taken off those servers, it no longer opens them.",
                names.join(", ")
            )
        };
    }
    ui.gfx.text(
        card.take_top(20.0),
        &removal.about,
        SMALL,
        t.text2,
        Align::Start,
    );
    card.take_top(space::S);
    let tick = card.take_top(CONTROL);
    if !busy && !names.is_empty() {
        let servers = if names.len() == 1 {
            names[0].to_owned()
        } else {
            format!("these {} servers", names.len())
        };
        ui.checkbox(
            Id::new("removal.servers"),
            tick,
            &mut removal.from_servers,
            &format!("Also take it off {servers} (their ~/.ssh/authorized_keys)"),
        );
    }
    card.take_top(space::S);
    let mut line = card.take_top(CONTROL);
    let go_label = if busy {
        "Removing\u{2026}"
    } else {
        "Delete key"
    };
    let go = line.take_right(button_width(&mut ui.gfx, "Removing\u{2026}", None));
    line.take_right(space::S);
    let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
    let cancelled = ui
        .button(
            Id::new("removal.cancel"),
            cancel,
            "Cancel",
            None,
            Kind::Ghost,
        )
        .clicked
        || ui.key(Key::Escape, Modifiers::NONE);
    let start = ui
        .button(Id::new("removal.go"), go, go_label, None, Kind::Danger)
        .clicked
        && !busy;
    if busy && !ui.inert() {
        ui.sweep(go);
    }
    if cancelled {
        // Stopping the runs; a server may have taken the key off already.
        if busy {
            return CardEnd::closes_saying((
                true,
                format!(
                    "Stopped. {} is kept; some servers may no longer have it.",
                    removal.label
                ),
            ));
        }
        return CardEnd::closes();
    }
    let (true, Some(key)) = (start, found) else {
        return CardEnd::default();
    };
    if !removal.from_servers || users.is_empty() {
        let kept: Vec<String> = if users.is_empty() {
            Vec::new()
        } else {
            users.iter().map(|(_, label)| label.clone()).collect()
        };
        let (_, message) = keychain_screen::removal_summary(&key.label, &[], &kept);
        return CardEnd::closes_saying(match vault.remove_key(key.id) {
            Ok(()) => (false, message),
            Err(error) => (true, error.to_string()),
        });
    }
    let Some(commands) = keys::remove_commands(&key.public) else {
        return CardEnd::says(
            true,
            "This key's public half cannot be read, so it cannot be taken off servers. \
             Untick them to delete it here only.",
        );
    };
    for (host_id, label) in &users {
        let Some(host) = vault.hosts().iter().find(|host| host.id == *host_id) else {
            continue;
        };
        let waker = waker.cloned();
        removal.runs.push(ServerRun {
            label: label.clone(),
            task: Some(SshTask::exec(
                connect::host(vault, files, host),
                commands.clone(),
                move || {
                    if let Some(waker) = &waker {
                        waker();
                    }
                },
            )),
            ask: None,
            ended: None,
        });
    }
    CardEnd::quiet()
}

/// The Generate or Import card, while one is open.
#[allow(clippy::too_many_lines)] // Both forms, each top to bottom.
fn form_card(
    form: &mut KeyForm,
    ui: &mut Ui<'_>,
    area: &mut Rect,
    vault: &mut Vault,
    waker: Option<&Waker>,
) -> CardEnd {
    if let KeyForm::Edit {
        id,
        label,
        private,
        passphrase,
        shown,
    } = form
    {
        return edit_card(ui, area, vault, (*id, label, private, passphrase, shown));
    }
    let mut done: Option<Result<SshKey, keys::KeyError>> = None;
    let mut close = false;
    match form {
        KeyForm::Generate {
            label,
            kind,
            working,
        } => {
            if let Some(job) = working {
                match job.poll() {
                    Poll::Running => {}
                    Poll::Done(result) => {
                        done =
                            Some(result.map(|pair| keychain_screen::stored(vault, label, &pair)));
                    }
                    Poll::Lost => done = Some(Err(keys::KeyError::Generate)),
                }
            }
            if done.is_some() {
                *working = None;
            }
            let busy = working.is_some();
            let mut card = pages::card(ui, area, "New key", 2.0 * CONTROL + 5.0 * space::S);
            card.take_top(space::S);
            let mut line = card.take_top(CONTROL);
            let kind_rect = line.take_right(170.0);
            line.take_right(space::S);
            let name_field = ui.text_field(
                Id::new("key.label"),
                line,
                label,
                FieldStyle {
                    placeholder: "Name, e.g. work laptop",
                    ..FieldStyle::default()
                },
            );
            let names: Vec<String> = KeyKind::ALL
                .iter()
                .map(|choice| choice.name().to_owned())
                .collect();
            let at = KeyKind::ALL.iter().position(|choice| choice == kind);
            if let Some(chosen) = ui.select(Id::new("key.kind"), kind_rect, &names, at, "Type")
                && !busy
            {
                *kind = KeyKind::ALL[chosen];
            }
            card.take_top(space::S);
            let mut buttons = card.take_top(CONTROL);
            let go_label = if busy {
                format!("Making a {} key\u{2026}", kind.name())
            } else {
                "Generate".to_owned()
            };
            // The button grows, and its longer words show once they fit.
            let wanted = button_width(&mut ui.gfx, &go_label, None).max(96.0);
            let wide = ui.spring_with(Id::new("key.go.wide"), wanted, Feel::Soft);
            let go_label = if wide + 0.5 < wanted {
                "Generate"
            } else {
                go_label.as_str()
            };
            let go = buttons.take_right(wide);
            buttons.take_right(space::S);
            let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
            close |= ui
                .button(Id::new("key.cancel"), cancel, "Cancel", None, Kind::Ghost)
                .clicked
                || ui.key(Key::Escape, Modifiers::NONE);
            let clicked = ui
                .button(Id::new("key.go"), go, go_label, None, Kind::Primary)
                .clicked
                || name_field.submitted;
            if busy {
                ui.sweep(go);
            }
            if clicked && !busy {
                if label.trim().is_empty() {
                    *label = format!("{} key {}", kind.name(), vault.ssh_keys().len() + 1);
                }
                let (kind, comment) = (*kind, keychain_screen::comment());
                let pace = super::FramePace::new(ui, waker.cloned());
                *working = Some(Job::start(&pace, move || keys::generate(kind, &comment)));
            }
        }
        // Drawn by `edit_card`, above.
        KeyForm::Edit { .. } => {}
        KeyForm::Import {
            label,
            private,
            passphrase,
        } => {
            let mut card = pages::card(
                ui,
                area,
                "Import a key",
                2.0 * CONTROL + KEY_H + 6.0 * space::S,
            );
            card.take_top(space::S);
            ui.text_field(
                Id::new("import.label"),
                card.take_top(CONTROL),
                label,
                FieldStyle {
                    placeholder: "Name, e.g. id_ed25519",
                    ..FieldStyle::default()
                },
            );
            card.take_top(space::S);
            ui.text_area(
                Id::new("import.private"),
                card.take_top(KEY_H),
                private,
                FieldStyle {
                    placeholder: "Paste the private key (-----BEGIN \u{2026} PRIVATE KEY-----)",
                    mono: true,
                    ..FieldStyle::default()
                },
            );
            card.take_top(space::S);
            let mut line = card.take_top(CONTROL);
            let go = line.take_right(button_width(&mut ui.gfx, "Import", None).max(96.0));
            line.take_right(space::S);
            let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
            line.take_right(space::M);
            let passphrase_field = ui.text_field(
                Id::new("import.passphrase"),
                line,
                passphrase,
                FieldStyle {
                    placeholder: "Passphrase, only if the key has one",
                    ..FieldStyle::default()
                },
            );
            close |= ui
                .button(
                    Id::new("import.cancel"),
                    cancel,
                    "Cancel",
                    None,
                    Kind::Ghost,
                )
                .clicked
                || ui.key(Key::Escape, Modifiers::NONE);
            let clicked = ui
                .button(Id::new("import.go"), go, "Import", None, Kind::Primary)
                .clicked
                || passphrase_field.submitted;
            if clicked && !private.trim().is_empty() {
                done = Some(keys::import(private, Some(passphrase)).map(|pair| {
                    let name = if label.trim().is_empty() {
                        format!("Imported key {}", vault.ssh_keys().len() + 1)
                    } else {
                        label.clone()
                    };
                    keychain_screen::stored(vault, &name, &pair)
                }));
            }
        }
    }
    match done {
        Some(Ok(key)) => {
            let label = key.label.clone();
            CardEnd::closes_saying(match vault.upsert_key(key) {
                Ok(()) => (
                    false,
                    format!(
                        "Saved {label}. Copy its public key to a server, or install it on one."
                    ),
                ),
                Err(error) => (true, format!("Could not save the key: {error}")),
            })
        }
        Some(Err(error)) => CardEnd::says(true, error.to_string()),
        None if close => CardEnd {
            close: true,
            say: Say::Nothing,
        },
        None => CardEnd::default(),
    }
}

/// A saved key's card: its name; its private key as dots until the eye
/// shows it, with Copy (kept from clipboard history and cleared after a while, as every
/// secret); and the public half servers know it by. Another key pasted over it replaces it on
/// Save: the hosts that log in with it go on doing so, and its new public half needs
/// installing where the old one was.
#[allow(clippy::too_many_lines)] // The card top to bottom, then what Save came to.
fn edit_card(
    ui: &mut Ui<'_>,
    area: &mut Rect,
    vault: &mut Vault,
    (id, label, private, passphrase, shown): (
        u64,
        &mut String,
        &mut Zeroizing<String>,
        &mut Zeroizing<String>,
        &mut bool,
    ),
) -> CardEnd {
    let Some(saved) = vault.ssh_key(id) else {
        return CardEnd::closes_saying((true, "That key was deleted meanwhile.".into()));
    };
    let (saved_label, saved_public) = (saved.label.clone(), saved.public.clone());
    let replaced = saved.private.expose() != private.as_str();
    let t = ui.theme;
    let mut card = pages::card(
        ui,
        area,
        "Edit key",
        2.0 * CONTROL + KEY_H + 20.0 + 7.0 * space::S,
    );
    card.take_top(space::S);
    ui.text_field(
        Id::new("edit.label"),
        card.take_top(CONTROL),
        label,
        FieldStyle {
            placeholder: "Name, e.g. work laptop",
            ..FieldStyle::default()
        },
    );
    card.take_top(space::S);
    let key_rect = card.take_top(KEY_H);
    let style = FieldStyle {
        placeholder: "Paste a private key (-----BEGIN \u{2026} PRIVATE KEY-----)",
        mono: true,
        ..FieldStyle::default()
    };
    if *shown {
        ui.text_area(
            Id::new("edit.private"),
            key_rect,
            &mut Revealed(private),
            style,
        );
    } else {
        ui.text_area(Id::new("edit.private"), key_rect, private, style);
    }
    card.take_top(space::S);
    let public = if replaced {
        "Its public key is worked out again when you save.".to_owned()
    } else {
        format!("Public key: {saved_public}")
    };
    let public_line = card.take_top(20.0);
    let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, &public, SMALL, public_line.w);
    ui.gfx
        .text(public_line, &fitted, SMALL, t.text2, Align::Start);
    card.take_top(space::S);
    let mut line = card.take_top(CONTROL);
    let save = line.take_right(button_width(&mut ui.gfx, "Save", None).max(96.0));
    line.take_right(space::S);
    let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
    line.take_right(space::M);
    let eye = line.take_left(CONTROL).centered(30.0, 30.0);
    line.take_left(space::XS);
    let copy_label = "Copy private key";
    let copy = line.take_left(button_width(&mut ui.gfx, copy_label, Some(icons::COPY)));
    line.take_left(space::M);
    // Drawn left to right, so Tab follows the eye.
    let (icon, name) = if *shown {
        (icons::EYE_OFF, "Hide private key")
    } else {
        (icons::EYE, "Show private key")
    };
    if ui
        .icon_button(Id::new("edit.show"), eye, icon, name, *shown)
        .clicked
    {
        *shown = !*shown;
    }
    let mut end = CardEnd::default();
    if ui
        .button(
            Id::new("edit.copy"),
            copy,
            copy_label,
            Some(icons::COPY),
            Kind::Secondary,
        )
        .clicked
        && !private.trim().is_empty()
    {
        ui.copy_secret(private.as_str().to_owned());
        end = CardEnd::says(
            false,
            format!(
                "Copied the private key of {saved_label}. The clipboard forgets it in {} seconds.",
                annulus_ui::kit::SECRET_ON_CLIPBOARD.as_secs()
            ),
        );
    }
    let passphrase_field = ui.text_field(
        Id::new("edit.passphrase"),
        line,
        passphrase,
        FieldStyle {
            placeholder: "Passphrase, only if a key pasted in has one",
            ..FieldStyle::default()
        },
    );
    if ui
        .button(Id::new("edit.cancel"), cancel, "Cancel", None, Kind::Ghost)
        .clicked
        || ui.key(Key::Escape, Modifiers::NONE)
    {
        return CardEnd {
            close: true,
            say: Say::Nothing,
        };
    }
    let clicked = ui
        .button(Id::new("edit.save"), save, "Save", None, Kind::Primary)
        .clicked
        || passphrase_field.submitted;
    if !clicked {
        return end;
    }
    let name = if label.trim().is_empty() {
        saved_label.clone()
    } else {
        label.trim().to_owned()
    };
    let edited = if replaced {
        match keys::import(private, Some(passphrase)) {
            Ok(pair) => SshKey {
                id,
                label: name.clone(),
                private: Secret::new(pair.private.as_str()),
                public: pair.public.clone(),
                fingerprint: pair.fingerprint.clone(),
            },
            Err(error) => return CardEnd::says(true, error.to_string()),
        }
    } else {
        let Some(saved) = vault.ssh_key(id) else {
            return CardEnd::closes_saying((true, "That key was deleted meanwhile.".into()));
        };
        SshKey {
            label: name.clone(),
            ..saved.clone()
        }
    };
    let new_public = edited.public != saved_public;
    CardEnd::closes_saying(match vault.upsert_key(edited) {
        Ok(()) if new_public => (
            false,
            format!(
                "Saved {name}. Its public key changed: install it where the old one was trusted."
            ),
        ),
        Ok(()) => (false, format!("Saved {name}.")),
        Err(error) => (true, format!("Could not save the key: {error}")),
    })
}

/// Installing a key on a host: pick the host, then connect and add the key to its
/// `authorized_keys`. Once it is there, the host logs in with the key.
fn install_card(
    install: &mut Install,
    ui: &mut Ui<'_>,
    area: &mut Rect,
    vault: &mut Vault,
    files: &SshFiles,
    waker: Option<&Waker>,
) -> CardEnd {
    let Some(key) = vault
        .ssh_keys()
        .iter()
        .find(|key| key.id == install.key_id)
        .cloned()
    else {
        return CardEnd::closes();
    };
    if let Some(task) = &mut install.task {
        while let Some(event) = task.question() {
            if let Some(question) = Ask::from_event(event) {
                install.ask = Some(question);
            }
        }
        if let Some(outcome) = task.outcome() {
            return CardEnd::closes_saying(match keychain_screen::install_result(outcome) {
                Ok(()) => (
                    false,
                    keychain_screen::link_key(vault, install.host_id, &key),
                ),
                Err(reason) => (true, reason),
            });
        }
    }
    let busy = install.task.is_some();
    let hosts: Vec<(u64, String)> = vault
        .hosts()
        .iter()
        .map(|host| (host.id, host.label.clone()))
        .collect();
    let mut card = pages::card(
        ui,
        area,
        &format!("Install {} on a host", key.label),
        CONTROL + 4.0 * space::S,
    );
    card.take_top(space::S);
    let mut line = card.take_top(CONTROL);
    let go_label = if busy {
        "Installing\u{2026}"
    } else {
        "Install"
    };
    let go = line.take_right(button_width(&mut ui.gfx, "Installing\u{2026}", None));
    line.take_right(space::S);
    let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
    line.take_right(space::M);
    let names: Vec<String> = hosts.iter().map(|(_, label)| label.clone()).collect();
    let at = install
        .host_id
        .and_then(|id| hosts.iter().position(|(host, _)| *host == id));
    if let Some(chosen) = ui.select(Id::new("install.host"), line, &names, at, "Choose a host")
        && !busy
    {
        install.host_id = Some(hosts[chosen].0);
    }
    let cancelled = ui
        .button(
            Id::new("install.cancel"),
            cancel,
            "Cancel",
            None,
            Kind::Ghost,
        )
        .clicked
        || ui.key(Key::Escape, Modifiers::NONE);
    let start = ui
        .button(Id::new("install.go"), go, go_label, None, Kind::Primary)
        .clicked
        && !busy;
    if busy {
        ui.sweep(go);
    }
    if cancelled {
        return CardEnd::closes();
    }
    if !start {
        return CardEnd::default();
    }
    let host = install
        .host_id
        .and_then(|id| vault.hosts().iter().find(|host| host.id == id));
    let Some(host) = host else {
        return CardEnd::says(true, "Choose the host to install the key on.");
    };
    let waker = waker.cloned();
    install.task = Some(SshTask::exec(
        connect::host(vault, files, host),
        keys::install_commands(&key.public),
        move || {
            if let Some(waker) = &waker {
                waker();
            }
        },
    ));
    CardEnd::quiet()
}

// ---------------------------------------------------------------------------------------
// Snippets
mod snippets;
pub use snippets::{SnippetsView, snippet_panel};

// ---------------------------------------------------------------------------------------
// Logs
mod logs;
pub use logs::LogsView;

// ---------------------------------------------------------------------------------------
// Known hosts
mod known_hosts;
pub use known_hosts::KnownView;

// ---------------------------------------------------------------------------------------
// Port forwarding
mod forwards;
pub use forwards::ForwardsView;
#[cfg(test)]
use forwards::duplicate;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use annulus_vault::forwards::PortForward;

    use super::*;

    #[test]
    fn a_pasted_key_is_wiped_as_its_card_starts_to_fold_away() {
        let mut fold = Fold::default();
        fold.open(KeyForm::Import {
            label: "work".into(),
            private: Zeroizing::new("-----BEGIN".into()),
            passphrase: Zeroizing::new("hunter2".into()),
        });
        let mut wiped = false;
        fold.close_with(|form| {
            form.forget();
            let KeyForm::Import {
                private,
                passphrase,
                ..
            } = form
            else {
                return;
            };
            wiped = private.is_empty() && passphrase.is_empty();
        });
        assert!(wiped, "before it is drawn once more on its way out");
        assert!(fold.get().is_none());
    }

    #[test]
    fn a_key_being_made_never_cuts_off_the_words_on_its_button() {
        use annulus_config::tokens::Appearance;
        use annulus_ui::kit::{Theme, UiState};
        let mut vault = crate::native::demo::fixture_vault("key-button", false);
        let mut form = KeyForm::Generate {
            label: "Build server".into(),
            kind: KeyKind::Ed25519,
            working: None,
        };
        let mut state = UiState::default();
        // The job waits for this, so the key is being made for as long as the test looks.
        let (finish, wait) = std::sync::mpsc::channel::<()>();
        let mut wait = Some(wait);
        for frame in 0..90u32 {
            let mut canvas = canvas();
            state.begin_frame(&[], f64::from(frame) / 60.0);
            let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
            let theme = Theme::new(Appearance::Dark);
            let mut ui = Ui::new(&mut canvas, theme, &mut state, viewport);
            if frame == 10
                && let KeyForm::Generate { working, .. } = &mut form
                && let Some(wait) = wait.take()
            {
                let pace = crate::native::FramePace::new(&ui, None);
                *working = Some(Job::start(&pace, move || {
                    let _ = wait.recv();
                    keys::generate(KeyKind::Ed25519, "me@test")
                }));
            }
            let mut area = viewport;
            form_card(&mut form, &mut ui, &mut area, &mut vault, None);
            drop(ui);
            let faults = state.end_frame().faults;
            assert!(faults.is_empty(), "frame {frame}: {faults:?}");
        }
        drop(finish);
    }

    fn canvas() -> annulus_platform::cpu::CpuCanvas {
        annulus_platform::cpu::CpuCanvas::new(
            800,
            600,
            1.0,
            annulus_platform::cpu::TextEngine::new(),
        )
        .unwrap()
    }

    #[test]
    fn a_duplicate_is_a_new_rule_named_as_a_copy_that_listens_nowhere_yet() {
        let mut rule = PortForward::new(3, 7);
        (rule.label, rule.bind_port) = ("db".into(), 15432);
        (rule.destination_host, rule.destination_port) = ("db.internal".into(), 5432);
        rule.auto_start = true;
        let copy = duplicate(&rule, 9);
        assert_eq!(
            (copy.id, copy.label.as_str(), copy.bind_port),
            (9, "db copy", 0)
        );
        assert_eq!(
            (
                copy.host_id,
                copy.destination_host.as_str(),
                copy.destination_port,
                copy.auto_start
            ),
            (7, "db.internal", 5432, true)
        );
    }

    #[test]
    fn a_key_a_host_takes_from_its_group_counts_as_one_it_logs_in_with() {
        let mut vault = crate::native::demo::fixture_vault("key-from-group", false);
        let pair = keys::generate(KeyKind::Ed25519, "me@test").unwrap();
        let key = keychain_screen::stored(&vault, "deploy", &pair);
        let key_id = key.id;
        vault.upsert_key(key).unwrap();
        let mut group = vault.draft_group(None).unwrap();
        group.name = "Production".into();
        group.defaults.key_id = Some(key_id);
        let uid = group.uid.clone();
        vault.upsert_group(group).unwrap();
        let mut inherits = vault.draft_host();
        (inherits.label, inherits.address) = ("web".into(), "10.0.0.5".into());
        inherits.group = Some(uid.clone());
        vault.upsert_host(inherits.clone()).unwrap();
        // In the same group, a host with a key of its own does not use the group's.
        let other = keychain_screen::stored(&vault, "other", &pair);
        let other_id = other.id;
        vault.upsert_key(other).unwrap();
        let mut own = vault.draft_host();
        (own.label, own.address) = ("db".into(), "10.0.0.6".into());
        (own.group, own.key_id) = (Some(uid), Some(other_id));
        vault.upsert_host(own).unwrap();
        assert_eq!(
            users_of(&vault, key_id),
            vec![(inherits.id, "web".to_owned())]
        );
        let rows = key_rows(&vault);
        let row = rows.iter().find(|row| row.id == key_id).unwrap();
        assert_eq!(row.users, 1);
        // First, so a row narrowed by Cancel and Delete key still shows it.
        assert!(row.detail.starts_with("Used by 1 host  "), "{}", row.detail);
        assert_eq!(pages::count(0, "host", "hosts"), "no hosts");
        assert_eq!(pages::count(3, "host", "hosts"), "3 hosts");
    }

    #[test]
    fn known_hosts_shows_a_key_trusted_since_it_was_opened() {
        let dir = std::env::temp_dir().join(format!("annulus-known-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        let mut view = KnownView::default();
        let mut canvas = canvas();
        let mut state = annulus_ui::kit::UiState::default();
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut draw = |view: &mut KnownView| {
            state.begin_frame(&[], 0.0);
            let mut ui = Ui::new(
                &mut canvas,
                annulus_ui::kit::Theme::new(annulus_config::tokens::Appearance::Dark),
                &mut state,
                rect,
            );
            view.draw(&mut ui, rect, &path, None);
            state.end_frame();
        };
        draw(&mut view);
        assert_eq!(view.entries.as_ref().map(Vec::len), Some(0));
        // A connection trusts a new key while the screen is open.
        std::fs::write(
            &path,
            "web ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHhb4m8U3xS0ycLC2nBqGDbB0kPPw8IVPBCWd2HjWcZ1\n",
        )
        .unwrap();
        draw(&mut view);
        assert_eq!(view.entries.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn deleting_a_host_stops_its_running_rules() {
        let mut vault = crate::native::demo::fixture_vault("rule-host-gone", false);
        let mut host = vault.draft_host();
        (host.label, host.address) = ("db".into(), "127.0.0.1".into());
        let host_id = host.id;
        vault.upsert_host(host).unwrap();
        let mut rule = vault.draft_forward(host_id);
        rule.bind_port = 1;
        rule.destination_host = "127.0.0.1".into();
        rule.destination_port = 1;
        let rule_id = rule.id;
        vault.upsert_forward(rule).unwrap();
        let files = SshFiles {
            known_hosts: std::env::temp_dir().join("annulus-rule-host-gone-known_hosts"),
            key_dir: None,
        };
        let mut view = ForwardsView::default();
        view.start_rule(rule_id, &vault, &files, None);
        assert!(view.running.contains_key(&rule_id));
        vault.remove_host(host_id).unwrap();
        let mut canvas = annulus_platform::cpu::CpuCanvas::new(
            800,
            600,
            1.0,
            annulus_platform::cpu::TextEngine::new(),
        )
        .unwrap();
        let mut state = annulus_ui::kit::UiState::default();
        state.begin_frame(&[], 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            annulus_ui::kit::Theme::new(annulus_config::tokens::Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        view.draw(
            &mut ui,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            &mut vault,
            &files,
            None,
        );
        assert!(view.running.is_empty(), "the rule stopped with its host");
    }
}
