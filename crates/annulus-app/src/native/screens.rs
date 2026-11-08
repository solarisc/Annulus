//! The vault's other screens in the new UI: Keychain, Port Forwarding, Snippets,
//! Known Hosts, Logs and Settings, built on [`super::pages`].

use std::collections::HashMap;

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::{Program, SshFiles};
use annulus_ssh::keys::{self, KeyKind, KeyPair};
use annulus_ssh::known::{self, KnownHost};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{FOCUS_REACH, Id, Key, Modifiers, Ui};
use annulus_ui::motion::{Curve, Feel, Layer, Motion};
use annulus_ui::widgets::{
    CONTROL, FieldStyle, Kind, Revealed, SCROLL_GUTTER, SMALL, Tone, button_width,
};
use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use super::forward_wizard::{Finish, Wizard};
use super::pages::{self, Fold, ListMotion, ROW_H, RowLook};
use crate::forwards_screen::{Draft, Running, Status, start};
use crate::job::{Job, Poll};
use crate::ssh_task::SshTask;
use crate::{connect, keychain_screen, logs_screen, snippets_screen};

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

/// The Snippets screen.
#[derive(Default)]
pub struct SnippetsView {
    draft: Fold<(Snippet, String, String)>,
    /// The snippet whose Delete asks once more.
    confirm: Option<u64>,
    /// Running a snippet on hosts: the snippet, and the hosts ticked.
    targets: Fold<(u64, Vec<u64>)>,
    rows: ListMotion,
}

impl SnippetsView {
    /// Opens saved snippet `id` in the edit card.
    pub fn edit_snippet(&mut self, vault: &Vault, id: u64) {
        if let Some(snippet) = vault.snippets().iter().find(|snippet| snippet.id == id) {
            self.draft.open((
                snippet.clone(),
                snippet.label.clone(),
                snippet.script.clone(),
            ));
        }
    }

    /// Whether saved snippet `id` is open in the edit card.
    #[cfg(test)]
    pub fn editing(&self, id: u64) -> bool {
        self.draft
            .get()
            .is_some_and(|(snippet, _, _)| snippet.id == id)
    }

    /// The hosts to run a snippet on (F-SNIP-004): tick up to [`snippets_screen::MAX_TARGETS`],
    /// then each gets a terminal that runs it after the host's own startup command. Returns
    /// whether the card closes, and what to run.
    #[allow(clippy::too_many_lines)] // The list, then its buttons.
    fn targets_card(
        (snippet_id, ticked): &mut (u64, Vec<u64>),
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &Vault,
        files: &SshFiles,
    ) -> (bool, Option<ScreenAction>) {
        let Some(snippet) = vault
            .snippets()
            .iter()
            .find(|s| s.id == *snippet_id)
            .cloned()
        else {
            return (true, None);
        };
        let hosts: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        #[allow(clippy::cast_precision_loss)] // Lists are short.
        let list_h = (hosts.len() as f32 * LINE).min(5.0 * LINE);
        let mut card = pages::card(
            ui,
            area,
            &format!("Run {} on hosts", snippet.label),
            list_h + CONTROL + 4.0 * space::S,
        );
        card.take_top(space::S);
        let list = card.take_top(list_h);
        #[allow(clippy::cast_precision_loss)] // As above.
        let content = hosts.len() as f32 * LINE;
        let offset = ui.scroll_view(Id::new("snippet.targets"), list, content);
        ui.push_list_clip(list, offset, content, FOCUS_REACH);
        let full = ticked.len() >= snippets_screen::MAX_TARGETS;
        for (index, (id, label)) in hosts.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * LINE;
            if y + LINE < list.y || y > list.b() {
                continue;
            }
            let mut on = ticked.contains(id);
            let before = on;
            ui.checkbox(
                Id::new(("snippet.target", *id)),
                Rect::new(list.x, y, list.w - 10.0, LINE - 2.0),
                &mut on,
                label,
            );
            if on && !before && !full {
                ticked.push(*id);
            } else if !on && before {
                ticked.retain(|known| known != id);
            }
        }
        ui.pop_clip();
        card.take_top(space::S);
        let mut line = card.take_top(CONTROL);
        let go_label = match ticked.len() {
            1 => "Run on 1 host".to_owned(),
            n => format!("Run on {n} hosts"),
        };
        let go = line.take_right(button_width(&mut ui.gfx, &go_label, Some(icons::PLAY)));
        line.take_right(space::S);
        let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
        line.take_right(space::M);
        let t = ui.theme;
        if full {
            ui.gfx.text(
                line,
                &format!("At most {} at once.", snippets_screen::MAX_TARGETS),
                SMALL,
                t.text2,
                Align::Start,
            );
        }
        let cancelled = ui
            .button(
                Id::new("snippet.targets.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Ghost,
            )
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE);
        let run = ui
            .button(
                Id::new("snippet.targets.go"),
                go,
                &go_label,
                Some(icons::PLAY),
                Kind::Primary,
            )
            .clicked
            && !ticked.is_empty();
        if cancelled {
            return (true, None);
        }
        if !run {
            return (false, None);
        }
        let tabs: snippets_screen::Panes = ticked
            .iter()
            .filter_map(|id| vault.hosts().iter().find(|host| host.id == *id))
            .map(|host| {
                (
                    host.label.clone(),
                    snippets_screen::with_script(
                        connect::host(vault, files, host),
                        &snippet.script,
                    ),
                )
            })
            .collect();
        let title = match tabs.len() {
            1 => snippet.label.clone(),
            n => format!("{} on {n} hosts", snippet.label),
        };
        (true, Some(ScreenAction::Terminals(title, tabs)))
    }

    /// Draws the screen; running a snippet opens a terminal, or one per host.
    #[allow(clippy::too_many_lines)] // Header, editor and list, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        files: &SshFiles,
    ) -> Option<ScreenAction> {
        let mut area = pages::page(rect);
        let count = vault.snippets().len();
        let mut actions = pages::header(
            ui,
            &mut area,
            "Snippets",
            Some(count),
            "Commands you run often, on a local shell or your hosts.",
        );
        if pages::action(
            ui,
            &mut actions,
            "snippets.new",
            "New snippet",
            Some(icons::PLUS),
            Kind::Primary,
        ) {
            self.draft
                .open((vault.draft_snippet(), String::new(), String::new()));
        }
        let mut close_draft = false;
        self.draft.draw(
            ui,
            Id::new("snippets.draft"),
            &mut area,
            |ui, area, draft| {
                let (snippet, label, script) = draft;
                let title = if vault.snippets().iter().any(|saved| saved.id == snippet.id) {
                    "Edit snippet"
                } else {
                    "New snippet"
                };
                let mut card =
                    pages::card(ui, area, title, 2.0 * CONTROL + SCRIPT_H + 4.0 * space::M);
                card.take_top(space::S);
                ui.text_field(
                    Id::new("snippet.label"),
                    card.take_top(CONTROL),
                    label,
                    FieldStyle {
                        placeholder: "Label, e.g. Restart nginx",
                        ..FieldStyle::default()
                    },
                );
                card.take_top(space::S);
                ui.text_area(
                    Id::new("snippet.script"),
                    card.take_top(SCRIPT_H),
                    script,
                    FieldStyle {
                        placeholder: "sudo systemctl restart nginx",
                        mono: true,
                        ..FieldStyle::default()
                    },
                );
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let save = line.take_right(88.0);
                line.take_right(space::S);
                let cancel = line.take_right(80.0);
                let cancelled = ui
                    .button(
                        Id::new("snippet.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE);
                let saved = ui
                    .button(Id::new("snippet.save"), save, "Save", None, Kind::Primary)
                    .clicked;
                if saved && !script.trim().is_empty() {
                    let mut snippet = snippet.clone();
                    snippet.label = if label.trim().is_empty() {
                        script.trim().to_owned()
                    } else {
                        label.trim().to_owned()
                    };
                    script.trim().clone_into(&mut snippet.script);
                    let _ = vault.upsert_snippet(snippet);
                    close_draft = true;
                } else if cancelled {
                    close_draft = true;
                }
            },
        );
        if close_draft {
            self.draft.close();
        }
        let mut action = None;
        let mut close_targets = false;
        self.targets.draw(
            ui,
            Id::new("snippets.targets"),
            &mut area,
            |ui, area, targets| {
                (close_targets, action) = Self::targets_card(targets, ui, area, vault, files);
            },
        );
        if close_targets {
            self.targets.close();
        }
        if count == 0 {
            self.rows.empty(ui, area, ROW_H + GAP);
            pages::empty(
                ui,
                area,
                icons::CODE,
                "No snippets yet",
                "Save a command once, run it anywhere.",
            );
            return action;
        }
        let list: Vec<Snippet> = vault.snippets().to_vec();
        let mut remove = None;
        let mut pick = None;
        let looks: Vec<RowLook> = list
            .iter()
            .map(|snippet| RowLook {
                key: snippet.id,
                icon: icons::CODE,
                title: snippet.label.clone(),
                detail: snippet.script.lines().next().unwrap_or_default().to_owned(),
                mono: true,
            })
            .collect();
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        rows(
            ui,
            "snippets.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let snippet = &list[index];
                let zone = Id::new(("snippet", snippet.id));
                pages::row(ui, zone, row, &snippet.label, false);
                let (clicked, remainder) = if confirm == Some(snippet.id) {
                    let (answer, remainder) = confirm_delete(ui, row, zone, "Delete snippet");
                    match answer {
                        Some(true) => remove = Some(snippet.id),
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
                            (icons::SERVER, "Run on hosts"),
                            (icons::PLAY, "Run in a local terminal"),
                        ],
                    )
                };
                let inner = pages::row_icon(ui, remainder, icons::CODE);
                let first = snippet.script.lines().next().unwrap_or_default();
                pages::two_lines(ui, inner, &snippet.label, first, true);
                match clicked {
                    Some(1) => pick = Some(snippet.id),
                    Some(2) => {
                        action = Some(ScreenAction::Terminal(
                            snippet.label.clone(),
                            Box::new(snippets_screen::with_script(
                                Program::DefaultShell,
                                &snippet.script,
                            )),
                        ));
                    }
                    Some(0) => confirm_next = Some(snippet.id),
                    _ => {}
                }
            },
        );
        self.confirm = confirm_next;
        if let Some(id) = pick {
            self.targets.open((id, Vec::new()));
        }
        if let Some(id) = remove {
            self.confirm = None;
            let _ = vault.remove_snippet(id);
        }
        action
    }
}

// ---------------------------------------------------------------------------------------
// Logs

/// What a log's context menu offers.
const LOG_MENU: [&str; 3] = ["Open", "Export to Downloads", "Delete"];

/// The Logs screen.
#[derive(Default)]
pub struct LogsView {
    /// The log a delete waits for a yes on.
    confirm: Option<u64>,
    rows: ListMotion,
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

// ---------------------------------------------------------------------------------------
// Known hosts

/// The Known Hosts screen.
#[derive(Default)]
pub struct KnownView {
    entries: Option<Vec<KnownHost>>,
    /// The file's size and time when `entries` was read: a change (a key trusted while
    /// connecting) reads it again (UI review #10).
    read_at: Option<(u64, Option<std::time::SystemTime>)>,
    message: Fold<String>,
    rows: ListMotion,
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

// ---------------------------------------------------------------------------------------
// Port forwarding

/// The Port Forwarding screen and the rules it runs.
#[derive(Default)]
pub struct ForwardsView {
    running: HashMap<u64, Running>,
    draft: Fold<Draft>,
    rows: ListMotion,
    /// Questions rules' connections are waiting on, by rule.
    asks: HashMap<u64, Ask>,
    /// The rules marked to start with the vault were started.
    auto_started: bool,
    /// The rule whose Delete asks once more.
    confirm: Option<u64>,
    /// Making a new rule step by step (F-FWD-006); with no rules, the whole screen.
    wizard: Option<Wizard>,
}

const KINDS: [(&str, ForwardKind); 3] = [
    ("Local", ForwardKind::Local),
    ("Remote", ForwardKind::Remote),
    ("SOCKS", ForwardKind::Dynamic),
];

/// `rule` as a new rule with id `id`: named as its copy, and with no port to listen on yet,
/// so the two never fight over one.
fn duplicate(rule: &PortForward, id: u64) -> PortForward {
    PortForward {
        id,
        label: format!("{} copy", rule.label.trim()).trim().to_owned(),
        bind_port: 0,
        ..rule.clone()
    }
}

/// The second line of `rule`'s row.
fn forward_detail(rule: &PortForward, host: &str) -> String {
    let kind = KINDS
        .iter()
        .find(|(_, kind)| *kind == rule.kind)
        .map_or("", |(name, _)| *name);
    if rule.label.trim().is_empty() {
        format!("{kind} forward")
    } else {
        format!("{kind} \u{b7} {}", describe(rule, host))
    }
}

fn describe(rule: &PortForward, host: &str) -> String {
    let bind = format!("{}:{}", rule.bind_address, rule.bind_port);
    match rule.kind {
        ForwardKind::Local => format!(
            "{bind} \u{2192} {}:{} via {host}",
            rule.destination_host, rule.destination_port
        ),
        ForwardKind::Remote => format!(
            "{host}:{} \u{2192} {}:{}",
            rule.bind_port, rule.destination_host, rule.destination_port
        ),
        ForwardKind::Dynamic => format!("SOCKS on {bind} via {host}"),
    }
}

impl ForwardsView {
    /// Opens saved forwarding rule `id` in the edit card.
    pub fn edit_rule(&mut self, vault: &Vault, id: u64) {
        if let Some(rule) = vault.forwards().iter().find(|rule| rule.id == id) {
            self.draft.open(Draft::new(rule.clone(), false));
        }
    }

    /// Whether saved rule `id` is open in the edit card.
    #[cfg(test)]
    pub fn editing(&self, id: u64) -> bool {
        self.draft.get().is_some_and(|draft| draft.rule.id == id)
    }

    /// Starts rule `id` through its host.
    fn start_rule(&mut self, id: u64, vault: &Vault, files: &SshFiles, waker: Option<&Waker>) {
        let Some(rule) = vault.forwards().iter().find(|rule| rule.id == id).cloned() else {
            return;
        };
        let Some(host) = vault.hosts().iter().find(|host| host.id == rule.host_id) else {
            return;
        };
        let program = connect::host(vault, files, host);
        let waker = waker.cloned();
        let running = start(&rule, program, move || {
            if let Some(waker) = &waker {
                waker();
            }
        });
        self.running.insert(id, running);
    }

    /// Starts the rules marked to start when the vault unlocks, once (F-FWD-004).
    pub fn auto_start(&mut self, vault: &Vault, files: &SshFiles, waker: Option<&Waker>) {
        if std::mem::replace(&mut self.auto_started, true) {
            return;
        }
        let ids: Vec<u64> = vault
            .forwards()
            .iter()
            .filter(|rule| rule.auto_start)
            .map(|rule| rule.id)
            .collect();
        for id in ids {
            self.start_rule(id, vault, files, waker);
        }
    }

    /// Starts the rules marked to start with `host_id`'s terminal that are not running.
    pub fn host_opened(
        &mut self,
        host_id: u64,
        vault: &Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        let ids: Vec<u64> = vault
            .forwards()
            .iter()
            .filter(|rule| {
                crate::forwards_screen::starts_with(rule, host_id)
                    && !self.running.contains_key(&rule.id)
            })
            .map(|rule| rule.id)
            .collect();
        for id in ids {
            self.start_rule(id, vault, files, waker);
        }
    }

    /// Draws the screen; switching a rule on starts it through its host.
    #[allow(clippy::too_many_lines)] // Header, editor and list, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        // A rule's connection may ask about a new server key or a password.
        for (id, running) in &self.running {
            for event in running.take_events() {
                if let Some(question) = Ask::from_event(event) {
                    self.asks.insert(*id, question);
                }
            }
        }
        // A rule whose host was deleted stops (it would keep that host's login alive).
        self.running.retain(|id, _| {
            vault.forwards().iter().any(|rule| {
                rule.id == *id && vault.hosts().iter().any(|host| host.id == rule.host_id)
            })
        });
        self.asks.retain(|id, _| self.running.contains_key(id));
        let mut area = pages::page(rect);
        let count = vault.forwards().len();
        let mut actions = pages::header(
            ui,
            &mut area,
            "Port Forwarding",
            Some(count),
            "Reach services behind your hosts, or share local ones.",
        );
        let first_host = vault.hosts().first().map(|host| host.id);
        // With no rules the wizard is the screen, so a first rule explains how tunnels work.
        if count == 0 && self.wizard.is_none() {
            self.wizard = Some(Wizard::new(vault.draft_forward(first_host.unwrap_or(0))));
        }
        if self.wizard.is_none()
            && let Some(host) = first_host
            && pages::action(
                ui,
                &mut actions,
                "forwards.new",
                "New rule",
                Some(icons::PLUS),
                Kind::Primary,
            )
        {
            self.wizard = Some(Wizard::new(vault.draft_forward(host)));
            self.draft.close();
        }
        if self.wizard.is_some() {
            let shown = ui.entrance(Id::new("forwards.wizard"), Motion::BASE);
            ui.push_layer(
                Layer::fade(shown)
                    .shifted(0.0, 10.0 * (1.0 - shown))
                    .grouped(area),
            );
            self.wizard_page(ui, area, vault, files, waker);
            ui.pop_layer();
            self.ask_card(ui, rect);
            return;
        }
        // Coming back from the wizard, the list comes in as it went.
        let shown = ui.entrance(Id::new("forwards.list.page"), Motion::BASE);
        ui.push_layer(
            Layer::fade(shown)
                .shifted(0.0, 10.0 * (1.0 - shown))
                .grouped(area),
        );
        let (mut saved, mut close_draft) = (None, false);
        let running = &mut self.running;
        self.draft.draw(
            ui,
            Id::new("forwards.draft"),
            &mut area,
            |ui, area, draft| {
                let hosts: Vec<(u64, String)> = vault
                    .hosts()
                    .iter()
                    .map(|host| (host.id, host.label.clone()))
                    .collect();
                let mut card = pages::card(
                    ui,
                    area,
                    if draft.is_new {
                        "New rule"
                    } else {
                        "Edit rule"
                    },
                    4.0 * CONTROL + 5.0 * space::M + 4.0,
                );
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let kind_at = KINDS
                    .iter()
                    .position(|(_, kind)| *kind == draft.rule.kind)
                    .unwrap_or(0);
                let names: Vec<&str> = KINDS.iter().map(|(name, _)| *name).collect();
                let kinds = line.take_left(330.0);
                if let Some(chosen) = segmented(ui, "forward.kind", kinds, &names, kind_at) {
                    draft.rule.kind = KINDS[chosen].1;
                }
                line.take_left(space::M);
                let items: Vec<String> = hosts.iter().map(|(_, label)| label.clone()).collect();
                let host_at = hosts.iter().position(|(id, _)| *id == draft.rule.host_id);
                if let Some(chosen) =
                    ui.select(Id::new("forward.host"), line, &items, host_at, "Host")
                {
                    draft.rule.host_id = hosts[chosen].0;
                }
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let bind = line.take_left(140.0);
                ui.text_field(
                    Id::new("forward.bind"),
                    bind,
                    &mut draft.bind_port,
                    FieldStyle {
                        placeholder: "Port, e.g. 8080",
                        ..FieldStyle::default()
                    },
                );
                line.take_left(space::M);
                if draft.rule.has_destination() {
                    let port = line.take_right(140.0);
                    line.take_right(space::M);
                    ui.text_field(
                        Id::new("forward.dest"),
                        line,
                        &mut draft.rule.destination_host,
                        FieldStyle {
                            placeholder: "Destination host, e.g. localhost",
                            ..FieldStyle::default()
                        },
                    );
                    ui.text_field(
                        Id::new("forward.dport"),
                        port,
                        &mut draft.destination_port,
                        FieldStyle {
                            placeholder: "Port, e.g. 5432",
                            ..FieldStyle::default()
                        },
                    );
                }
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let first = line.take_left(line.w / 2.0);
                ui.checkbox(
                    Id::new("forward.auto"),
                    first,
                    &mut draft.rule.auto_start,
                    "Start when the vault unlocks",
                );
                ui.checkbox(
                    Id::new("forward.with"),
                    line,
                    &mut draft.rule.with_host,
                    "Start with the host's terminal",
                );
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let save = line.take_right(88.0);
                line.take_right(space::S);
                let cancel = line.take_right(80.0);
                line.take_right(space::M);
                if let Some(problem) = &draft.problem {
                    let t = ui.theme;
                    ui.gfx.text(line, problem, SMALL, t.error, Align::Start);
                } else {
                    ui.text_field(
                        Id::new("forward.label"),
                        line,
                        &mut draft.rule.label,
                        FieldStyle {
                            placeholder: "Label (optional)",
                            ..FieldStyle::default()
                        },
                    );
                }
                let cancelled = ui
                    .button(
                        Id::new("forward.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE);
                if ui
                    .button(Id::new("forward.save"), save, "Save", None, Kind::Primary)
                    .clicked
                {
                    match draft.finish() {
                        // A running rule starts again with what was changed. Not saved, the
                        // form stays with the reason, so nothing typed is lost.
                        Ok(rule) => match vault.upsert_forward(rule.clone()) {
                            Ok(id) => {
                                close_draft = true;
                                if running.remove(&rule.id).is_some() {
                                    saved = Some(id);
                                }
                            }
                            Err(error) => {
                                draft.problem = Some(format!("Could not save the rule: {error}"));
                            }
                        },
                        Err(problem) => draft.problem = Some(problem),
                    }
                } else if cancelled {
                    close_draft = true;
                }
            },
        );
        if close_draft {
            self.draft.close();
        }
        if let Some(id) = saved {
            self.start_rule(id, vault, files, waker);
        }
        let rules: Vec<PortForward> = vault.forwards().to_vec();
        let host_names: HashMap<u64, String> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let mut toggles = Vec::new();
        let mut remove = None;
        let (mut edit, mut copied) = (None, None);
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        let statuses: HashMap<u64, Status> = self
            .running
            .iter()
            .map(|(id, running)| (*id, running.status()))
            .collect();
        let looks: Vec<RowLook> = rules
            .iter()
            .map(|rule| RowLook {
                key: rule.id,
                icon: icons::FORWARD,
                title: if rule.label.trim().is_empty() {
                    describe(
                        rule,
                        host_names.get(&rule.host_id).map_or("?", String::as_str),
                    )
                } else {
                    rule.label.clone()
                },
                detail: forward_detail(
                    rule,
                    host_names.get(&rule.host_id).map_or("?", String::as_str),
                ),
                mono: false,
            })
            .collect();
        rows(
            ui,
            "forwards.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let rule = &rules[index];
                let host = host_names.get(&rule.host_id).map_or("?", String::as_str);
                let zone = Id::new(("forward", rule.id));
                let title = if rule.label.trim().is_empty() {
                    describe(rule, host)
                } else {
                    rule.label.clone()
                };
                // Listening beyond this computer (an imported `*:1080`) is said, seen and heard.
                let exposed = rule.is_exposed();
                let name = if exposed {
                    format!("{title}, open to your network")
                } else {
                    title.clone()
                };
                pages::row(ui, zone, row, &name, false);
                let confirming = confirm == Some(rule.id);
                let mut remainder = row.inset(space::M, 0.0);
                let mut slots = None;
                if confirming {
                    let (answer, left) = confirm_delete(ui, row, zone, "Delete rule");
                    match answer {
                        Some(true) => remove = Some(rule.id),
                        Some(false) => confirm_next = None,
                        None => {}
                    }
                    remainder = left;
                } else {
                    let delete = remainder.take_right(30.0).centered(30.0, 30.0);
                    remainder.take_right(4.0);
                    let copy = remainder.take_right(30.0).centered(30.0, 30.0);
                    remainder.take_right(4.0);
                    let change = remainder.take_right(30.0).centered(30.0, 30.0);
                    remainder.take_right(space::S);
                    let switch = remainder.take_right(40.0);
                    remainder.take_right(space::S);
                    slots = Some((switch, change, copy, delete));
                }
                let status = statuses.get(&rule.id);
                let t = ui.theme;
                let (words, color) = match status {
                    None => ("Off".to_owned(), t.text3),
                    Some(Status::Starting) => ("Starting\u{2026}".to_owned(), t.warn),
                    Some(Status::Active(n)) => (format!("On \u{b7} {n} open"), t.ok),
                    Some(Status::Reconnecting(_)) => ("Reconnecting\u{2026}".to_owned(), t.warn),
                    Some(Status::Failed(why)) => (why.clone(), t.error),
                };
                let words_w = ui.gfx.measure(&words, SMALL).width.min(220.0);
                let state = remainder.take_right(words_w + 14.0);
                ui.gfx.fill_circle(
                    annulus_ui::gfx::Point::new(state.x + 4.0, state.center().y),
                    3.5,
                    color,
                );
                ui.gfx.text(
                    Rect::new(state.x + 12.0, state.y, state.w - 12.0, state.h),
                    &words,
                    SMALL,
                    t.text2,
                    Align::Start,
                );
                if exposed {
                    const OPEN: &str = "Open to your network";
                    let width = ui.gfx.measure(OPEN, SMALL).width + 20.0;
                    remainder.take_right(space::M);
                    let mark = remainder.take_right(width);
                    ui.gfx.icon(
                        icons::WARNING,
                        Rect::new(mark.x, mark.center().y - 7.0, 14.0, 14.0),
                        t.warn,
                    );
                    ui.gfx.text(
                        Rect::new(mark.x + 20.0, mark.y, mark.w - 20.0, mark.h),
                        OPEN,
                        SMALL,
                        t.warn,
                        Align::Start,
                    );
                }
                let inner = pages::row_icon(ui, remainder, icons::FORWARD);
                pages::two_lines(ui, inner, &title, &forward_detail(rule, host), false);
                let Some((switch, change, copy, delete)) = slots else {
                    return;
                };
                let mut on = status.is_some();
                if ui
                    .switch(zone.with("switch"), switch, &mut on, "Run this rule")
                    .clicked
                {
                    toggles.push((rule.id, on));
                }
                // As in other lists: they come up with the pointer or the keyboard.
                let focused = ui.state.focus();
                let near = ui.hover_within(row)
                    || ["edit", "copy", "delete"]
                        .into_iter()
                        .any(|name| focused == Some(zone.with(name)));
                let shown = ui.ease(zone.with("near"), if near { 1.0 } else { 0.0 });
                ui.push_layer(Layer::fade(shown));
                if ui
                    .icon_button(zone.with("edit"), change, icons::EDIT, "Edit rule", false)
                    .clicked
                {
                    edit = Some(rule.clone());
                }
                if ui
                    .icon_button(
                        zone.with("copy"),
                        copy,
                        icons::COPY,
                        "Duplicate rule",
                        false,
                    )
                    .clicked
                {
                    copied = Some(rule.clone());
                }
                if ui
                    .icon_button(
                        zone.with("delete"),
                        delete,
                        icons::TRASH,
                        "Delete rule",
                        false,
                    )
                    .clicked
                {
                    confirm_next = Some(rule.id);
                }
                ui.pop_layer();
            },
        );
        ui.pop_layer();
        self.confirm = confirm_next;
        if let Some(rule) = edit {
            self.draft.open(Draft::new(rule, false));
        }
        // A copy opens as a new rule for its own port (F-FWD-005).
        if let Some(rule) = copied {
            let copy = duplicate(&rule, vault.draft_forward(rule.host_id).id);
            self.draft.open(Draft::new(copy, true));
        }
        for (id, on) in toggles {
            if on {
                self.start_rule(id, vault, files, waker);
            } else {
                self.running.remove(&id);
            }
        }
        if let Some(id) = remove {
            self.confirm = None;
            self.running.remove(&id);
            let _ = vault.remove_forward(id);
        }
        self.ask_card(ui, rect);
    }

    /// The wizard in `area`; the rule it makes is saved and, when asked, started.
    fn wizard_page(
        &mut self,
        ui: &mut Ui<'_>,
        area: Rect,
        vault: &mut Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        let hosts: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let cancellable = !vault.forwards().is_empty();
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        match wizard.draw(ui, area, &hosts, cancellable) {
            Some(Finish::Cancel) => self.wizard = None,
            Some(Finish::Save(rule, start)) => match vault.upsert_forward(rule) {
                // Started by the id it was saved under: a rule synced meanwhile may hold
                // the one it was drafted with.
                Ok(id) => {
                    self.wizard = None;
                    if start {
                        self.start_rule(id, vault, files, waker);
                    }
                }
                Err(error) => wizard.problem = Some(error.to_string()),
            },
            None => {}
        }
    }

    /// A question a running rule's connection asks, over the screen.
    fn ask_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        if let Some((&id, ask)) = self.asks.iter_mut().next()
            && let Some(running) = self.running.get(&id)
            && ask::card(
                ui,
                Id::new(("forward.ask", id)),
                rect,
                ask,
                running.answers(),
            )
        {
            self.asks.remove(&id);
        }
    }
}

/// The snippet panel beside a terminal (Ctrl+Shift+.): search, then Paste types a snippet
/// into the focused pane and Run types it with Enter. Returns the bytes to type. `rect` is the
/// inside of the panel's card.
pub fn snippet_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    search: &mut String,
    snippets: &[Snippet],
) -> Option<Vec<u8>> {
    let t = ui.theme;
    let mut column = rect.inset(space::M, space::M);
    ui.section_label(column.take_top(18.0), "Snippets");
    column.take_top(space::S);
    ui.text_field(
        Id::new("panel.search"),
        column.take_top(CONTROL),
        search,
        FieldStyle {
            placeholder: "Search snippets",
            icon: Some(icons::SEARCH),
            ..FieldStyle::default()
        },
    );
    column.take_top(space::S);
    let query = search.trim().to_lowercase();
    let shown: Vec<&Snippet> = snippets
        .iter()
        .filter(|s| {
            query.is_empty()
                || s.label.to_lowercase().contains(&query)
                || s.script.to_lowercase().contains(&query)
        })
        .collect();
    if shown.is_empty() {
        ui.gfx.text(
            column.take_top(20.0),
            "No snippets match.",
            SMALL,
            t.text2,
            Align::Start,
        );
        return None;
    }
    let mut out = None;
    for snippet in shown {
        if column.h < 52.0 {
            break;
        }
        let card = column.take_top(52.0);
        column.take_top(space::XS);
        let id = Id::new(("panel.snippet", snippet.id));
        pages::row(ui, id, card, &snippet.label, false);
        let mut inner = card.inset(space::M, 0.0);
        let run = inner.take_right(28.0).centered(28.0, 28.0);
        inner.take_right(2.0);
        let paste = inner.take_right(28.0).centered(28.0, 28.0);
        let first = snippet.script.lines().next().unwrap_or_default();
        pages::two_lines(ui, inner, &snippet.label, first, true);
        if ui
            .icon_button(
                id.with("paste"),
                paste,
                icons::CODE,
                "Type into the terminal",
                false,
            )
            .clicked
        {
            out = Some(snippet.script.clone().into_bytes());
        }
        if ui
            .icon_button(
                id.with("run"),
                run,
                icons::PLAY,
                "Run in the terminal",
                false,
            )
            .clicked
        {
            let mut text = snippet.script.replace('\n', "\r");
            text.push('\r');
            out = Some(text.into_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

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
