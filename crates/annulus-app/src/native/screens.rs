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
use annulus_ui::kit::{Id, Key, Modifiers, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL, Tone, button_width};
use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::keychain::SshKey;
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use super::forward_wizard::{Finish, Wizard};
use super::pages::{self, ROW_H};
use crate::forwards_screen::{Draft, Running, Status, start};
use crate::job::{Job, Poll};
use crate::ssh_task::SshTask;
use crate::{connect, keychain_screen, logs_screen, snippets_screen};

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
/// Height of the script editor: about five lines.
const SCRIPT_H: f32 = 104.0;

/// Draws `count` rows of `ROW_H` in a scroll view filling `rect`.
fn rows(
    ui: &mut Ui<'_>,
    id: &str,
    rect: Rect,
    count: usize,
    mut each: impl FnMut(&mut Ui<'_>, usize, Rect),
) {
    #[allow(clippy::cast_precision_loss)] // Lists are short.
    let height = count as f32 * (ROW_H + GAP);
    let offset = ui.scroll_view(Id::new(id), rect, height);
    ui.push_clip(rect);
    for index in 0..count {
        #[allow(clippy::cast_precision_loss)] // Lists are short.
        let y = rect.y - offset + index as f32 * (ROW_H + GAP);
        if y + ROW_H < rect.y || y > rect.b() {
            continue;
        }
        each(ui, index, Rect::new(rect.x, y, rect.w - 10.0, ROW_H));
    }
    ui.pop_clip();
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
#[derive(Default)]
enum KeyForm {
    #[default]
    Closed,
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
}

/// Installing a key's public half on a saved host (F-KEY-004).
struct Install {
    key_id: u64,
    host_id: Option<u64>,
    task: Option<SshTask>,
    ask: Option<Ask>,
}

/// Deleting a key that hosts log in with (F-KEY-004): it comes off those servers'
/// `authorized_keys` first, while it can still log in, then out of the vault.
struct Removal {
    key_id: u64,
    /// Take it off the servers too; on unless unticked.
    from_servers: bool,
    /// Each server's run, once Delete was clicked.
    runs: Vec<ServerRun>,
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
            let users = vault
                .hosts()
                .iter()
                .filter(|host| host.key_id == Some(key.id))
                .count();
            let algorithm = key.public.split_whitespace().next().unwrap_or_default();
            KeyRow {
                id: key.id,
                label: key.label.clone(),
                detail: format!(
                    "{algorithm}  {}  \u{b7}  used by {users} host(s)",
                    key.fingerprint
                ),
                public: key.public.clone(),
                users,
            }
        })
        .collect()
}

/// The Keychain screen.
#[derive(Default)]
pub struct KeychainView {
    form: KeyForm,
    /// The last result, and whether it is a problem.
    message: Option<(bool, String)>,
    confirm_delete: Option<u64>,
    install: Option<Install>,
    removal: Option<Removal>,
}

impl KeychainView {
    /// Draws the screen.
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
            self.form = KeyForm::Import {
                label: String::new(),
                private: Zeroizing::new(String::new()),
                passphrase: Zeroizing::new(String::new()),
            };
            self.message = None;
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
            self.form = KeyForm::Generate {
                label: String::new(),
                kind: KeyKind::Ed25519,
                working: None,
            };
            self.message = None;
        }
        self.form_card(ui, &mut area, vault, waker);
        self.install_card(ui, &mut area, vault, files, waker);
        self.removal_card(ui, &mut area, vault, files, waker);
        if let Some((problem, message)) = &self.message {
            let t = ui.theme;
            let color = if *problem { t.error } else { t.text2 };
            ui.gfx
                .text(area.take_top(20.0), message, SMALL, color, Align::Start);
            area.take_top(space::S);
        }
        if count == 0 {
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
        if let Some(install) = &mut self.install
            && let (Some(task), Some(ask_now)) = (&install.task, &mut install.ask)
            && ask::card(ui, Id::new("install.ask"), rect, ask_now, task.answers())
        {
            install.ask = None;
        }
        // One at a time, a question a server the key comes off asks.
        if let Some(removal) = &mut self.removal
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

    /// Deleting a key that hosts log in with: the hosts it comes off, ticked, then each
    /// server's progress. Once every server has answered, the key leaves the vault, unless a
    /// server still has it: then it stays, so no way in is left behind unseen.
    #[allow(clippy::too_many_lines)] // The card, then what its runs came to.
    fn removal_card(
        &mut self,
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &mut Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        let Some(removal) = &mut self.removal else {
            return;
        };
        let Some(key) = vault
            .ssh_keys()
            .iter()
            .find(|key| key.id == removal.key_id)
            .cloned()
        else {
            self.removal = None;
            return;
        };
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
        if busy && removal.runs.iter().all(|run| run.ended.is_some()) {
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
            self.message = Some((problem, message));
            self.removal = None;
            return;
        }
        let users: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .filter(|host| host.key_id == Some(key.id))
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let names: Vec<&str> = users.iter().map(|(_, label)| label.as_str()).collect();
        let t = ui.theme;
        let mut card = pages::card(
            ui,
            area,
            &format!("Delete {}", key.label),
            2.0 * CONTROL + 20.0 + 5.0 * space::S,
        );
        card.take_top(space::S);
        let about = if busy {
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
        } else {
            format!(
                "It logs in to {}. Taken off there, it no longer opens them.",
                names.join(", ")
            )
        };
        ui.gfx
            .text(card.take_top(20.0), &about, SMALL, t.text2, Align::Start);
        card.take_top(space::S);
        let tick = card.take_top(CONTROL);
        if !busy {
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
        if cancelled {
            // Dropping the runs stops them; a server may have taken the key off already.
            if busy {
                self.message = Some((
                    true,
                    format!(
                        "Stopped. {} is kept; some servers may no longer have it.",
                        key.label
                    ),
                ));
            }
            self.removal = None;
            return;
        }
        if !start {
            return;
        }
        if !removal.from_servers || users.is_empty() {
            let kept: Vec<String> = if users.is_empty() {
                Vec::new()
            } else {
                users.iter().map(|(_, label)| label.clone()).collect()
            };
            let (_, message) = keychain_screen::removal_summary(&key.label, &[], &kept);
            self.message = Some(match vault.remove_key(key.id) {
                Ok(()) => (false, message),
                Err(error) => (true, error.to_string()),
            });
            self.removal = None;
            return;
        }
        let Some(command) = keys::remove_command(&key.public) else {
            self.message = Some((
                true,
                "This key's public half cannot be read, so it cannot be taken off servers. \
                 Untick them to delete it here only."
                    .into(),
            ));
            return;
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
                    command.clone(),
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
        self.message = None;
    }

    /// The Generate or Import card, while one is open.
    #[allow(clippy::too_many_lines)] // Both forms, each top to bottom.
    fn form_card(
        &mut self,
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &mut Vault,
        waker: Option<&Waker>,
    ) {
        let mut done: Option<Result<SshKey, keys::KeyError>> = None;
        let mut close = false;
        match &mut self.form {
            KeyForm::Closed => return,
            KeyForm::Generate {
                label,
                kind,
                working,
            } => {
                if let Some(job) = working {
                    match job.poll() {
                        Poll::Running => {}
                        Poll::Done(result) => {
                            done = Some(
                                result.map(|pair| keychain_screen::stored(vault, label, &pair)),
                            );
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
                let go = buttons.take_right(button_width(&mut ui.gfx, &go_label, None).max(96.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                close |= ui
                    .button(Id::new("key.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE);
                let clicked = ui
                    .button(Id::new("key.go"), go, &go_label, None, Kind::Primary)
                    .clicked
                    || name_field.submitted;
                if clicked && !busy {
                    if label.trim().is_empty() {
                        *label = format!("{} key {}", kind.name(), vault.ssh_keys().len() + 1);
                    }
                    let (kind, comment) = (*kind, keychain_screen::comment());
                    let pace = super::FramePace::new(ui, waker.cloned());
                    *working = Some(Job::start(&pace, move || keys::generate(kind, &comment)));
                }
            }
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
        if close {
            // Dropping the form wipes a pasted key and passphrase.
            self.form = KeyForm::Closed;
            self.message = None;
        }
        if let Some(result) = done {
            match result {
                Ok(key) => {
                    let label = key.label.clone();
                    self.message = Some(match vault.upsert_key(key) {
                        Ok(()) => (
                            false,
                            format!(
                                "Saved {label}. Copy its public key to a server, or install it on one."
                            ),
                        ),
                        Err(error) => (true, format!("Could not save the key: {error}")),
                    });
                    // Dropping the form wipes the pasted key and passphrase.
                    self.form = KeyForm::Closed;
                }
                Err(error) => self.message = Some((true, error.to_string())),
            }
        }
    }

    /// Installing a key on a host: pick the host, then connect and add the key to its
    /// `authorized_keys`. Once it is there, the host logs in with the key.
    fn install_card(
        &mut self,
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &mut Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
        let Some(install) = &mut self.install else {
            return;
        };
        let Some(key) = vault
            .ssh_keys()
            .iter()
            .find(|key| key.id == install.key_id)
            .cloned()
        else {
            self.install = None;
            return;
        };
        if let Some(task) = &mut install.task {
            while let Some(event) = task.question() {
                if let Some(question) = Ask::from_event(event) {
                    install.ask = Some(question);
                }
            }
            if let Some(outcome) = task.outcome() {
                let host_id = install.host_id;
                self.install = None;
                self.message = Some(match keychain_screen::install_result(outcome) {
                    Ok(()) => (false, keychain_screen::link_key(vault, host_id, &key)),
                    Err(reason) => (true, reason),
                });
                return;
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
        if cancelled {
            self.install = None;
            return;
        }
        if !start {
            return;
        }
        let host = install
            .host_id
            .and_then(|id| vault.hosts().iter().find(|host| host.id == id));
        let Some(host) = host else {
            self.message = Some((true, "Choose the host to install the key on.".into()));
            return;
        };
        let waker = waker.cloned();
        install.task = Some(SshTask::exec(
            connect::host(vault, files, host),
            keys::install_command(&key.public),
            move || {
                if let Some(waker) = &waker {
                    waker();
                }
            },
        ));
        self.message = None;
    }

    /// The keys, each with Copy, Install and Delete; Delete asks once more.
    fn list(&mut self, ui: &mut Ui<'_>, area: Rect, vault: &mut Vault) {
        let list = key_rows(vault);
        let confirm = self.confirm_delete;
        let mut confirm_next = confirm;
        let (mut remove, mut install, mut copied, mut unkey) = (None, None, None, None);
        rows(ui, "keys.list", area, list.len(), |ui, index, row| {
            let key = &list[index];
            let zone = Id::new(("key", key.id));
            pages::row(ui, zone, row, &key.label, false);
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
                    ],
                );
                match clicked {
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
        });
        self.confirm_delete = confirm_next;
        if let Some(key_id) = unkey
            && self
                .removal
                .as_ref()
                .is_none_or(|removal| removal.runs.is_empty())
        {
            self.removal = Some(Removal {
                key_id,
                from_servers: true,
                runs: Vec::new(),
            });
            (self.install, self.message) = (None, None);
        }
        if let Some(label) = copied {
            self.message = Some((false, format!("Copied the public key of {label}.")));
        }
        if let Some(key_id) = install {
            if vault.hosts().is_empty() {
                self.message = Some((
                    true,
                    "Save a host first, then install the key on it.".into(),
                ));
            } else {
                self.install = Some(Install {
                    key_id,
                    host_id: None,
                    task: None,
                    ask: None,
                });
                self.message = None;
            }
        }
        if let Some(id) = remove {
            self.confirm_delete = None;
            self.message = Some(match vault.remove_key(id) {
                Ok(()) => (false, "Key deleted.".into()),
                Err(error) => (true, error.to_string()),
            });
        }
    }
}

// ---------------------------------------------------------------------------------------
// Snippets

/// The Snippets screen.
#[derive(Default)]
pub struct SnippetsView {
    draft: Option<(Snippet, String, String)>,
    /// The snippet whose Delete asks once more.
    confirm: Option<u64>,
    /// Running a snippet on hosts: the snippet, and the hosts ticked.
    targets: Option<(u64, Vec<u64>)>,
}

impl SnippetsView {
    /// The hosts to run a snippet on (F-SNIP-004): tick up to [`snippets_screen::MAX_TARGETS`],
    /// then each gets a terminal that runs it after the host's own startup command.
    #[allow(clippy::too_many_lines)] // The list, then its buttons.
    fn targets_card(
        &mut self,
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &Vault,
        files: &SshFiles,
    ) -> Option<ScreenAction> {
        let (snippet_id, ticked) = self.targets.as_mut()?;
        let Some(snippet) = vault
            .snippets()
            .iter()
            .find(|s| s.id == *snippet_id)
            .cloned()
        else {
            self.targets = None;
            return None;
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
        let offset = ui.scroll_view(Id::new("snippet.targets"), list, hosts.len() as f32 * LINE);
        ui.push_clip(list);
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
            self.targets = None;
            return None;
        }
        if !run {
            return None;
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
        self.targets = None;
        let title = match tabs.len() {
            1 => snippet.label.clone(),
            n => format!("{} on {n} hosts", snippet.label),
        };
        Some(ScreenAction::Terminals(title, tabs))
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
            self.draft = Some((vault.draft_snippet(), String::new(), String::new()));
        }
        if let Some((snippet, label, script)) = &mut self.draft {
            let mut card = pages::card(
                ui,
                &mut area,
                "New snippet",
                2.0 * CONTROL + SCRIPT_H + 4.0 * space::M,
            );
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
                self.draft = None;
            } else if cancelled {
                self.draft = None;
            }
        }
        let mut action = self.targets_card(ui, &mut area, vault, files);
        if count == 0 {
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
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        rows(ui, "snippets.list", area, list.len(), |ui, index, row| {
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
        });
        self.confirm = confirm_next;
        if let Some(id) = pick {
            self.targets = Some((id, Vec::new()));
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
        rows(ui, "logs.list", area, list.len(), |ui, index, row| {
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
            #[allow(clippy::cast_precision_loss)] // Logs are a few MiB at most.
            let size = format!("{:.1} KB", text.len() as f64 / 1024.0);
            pages::two_lines(
                ui,
                inner,
                title,
                &format!("{} \u{b7} {size}", logs_screen::date(*saved_at)),
                false,
            );
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
        });
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
    message: Option<String>,
}

impl KnownView {
    /// Draws the screen over the known-hosts file at `path`.
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
            self.message = Some(match known::import(path, openssh) {
                Ok(_) => "Imported the keys OpenSSH trusts.".into(),
                Err(error) => format!("Could not import: {error}"),
            });
            self.entries = None;
        }
        if let Some(message) = &self.message {
            let t = ui.theme;
            ui.gfx
                .text(area.take_top(20.0), message, SMALL, t.text2, Align::Start);
            area.take_top(space::S);
        }
        if entries.is_empty() {
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
        rows(ui, "known.list", area, entries.len(), |ui, index, row| {
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
        });
        if let Some(entry) = forget {
            self.message = Some(match known::forget(path, &entry) {
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
    draft: Option<Draft>,
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
            self.draft = None;
        }
        // With no rules the wizard is the screen, so a first rule explains how tunnels work.
        if count == 0 && self.wizard.is_none() {
            self.wizard = Some(Wizard::new(vault.draft_forward(first_host.unwrap_or(0))));
        }
        if self.wizard.is_some() {
            self.wizard_page(ui, area, vault, files, waker);
            self.ask_card(ui, rect);
            return;
        }
        if let Some(draft) = &mut self.draft {
            let hosts: Vec<(u64, String)> = vault
                .hosts()
                .iter()
                .map(|host| (host.id, host.label.clone()))
                .collect();
            let mut card = pages::card(
                ui,
                &mut area,
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
            if let Some(chosen) = ui.select(Id::new("forward.host"), line, &items, host_at, "Host")
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
                    Ok(rule) => {
                        // A running rule starts again with what was changed.
                        let id = rule.id;
                        let was_running = self.running.remove(&id).is_some();
                        let _ = vault.upsert_forward(rule);
                        self.draft = None;
                        if was_running {
                            self.start_rule(id, vault, files, waker);
                        }
                    }
                    Err(problem) => draft.problem = Some(problem),
                }
            } else if cancelled {
                self.draft = None;
            }
        }
        let rules: Vec<PortForward> = vault.forwards().to_vec();
        let host_names: HashMap<u64, String> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let mut toggles = Vec::new();
        let mut remove = None;
        let mut edit = None;
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        let statuses: HashMap<u64, Status> = self
            .running
            .iter()
            .map(|(id, running)| (*id, running.status()))
            .collect();
        rows(ui, "forwards.list", area, rules.len(), |ui, index, row| {
            let rule = &rules[index];
            let host = host_names.get(&rule.host_id).map_or("?", String::as_str);
            let zone = Id::new(("forward", rule.id));
            let title = if rule.label.trim().is_empty() {
                describe(rule, host)
            } else {
                rule.label.clone()
            };
            pages::row(ui, zone, row, &title, false);
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
                let change = remainder.take_right(30.0).centered(30.0, 30.0);
                remainder.take_right(space::S);
                let switch = remainder.take_right(40.0);
                remainder.take_right(space::S);
                slots = Some((switch, change, delete));
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
            let inner = pages::row_icon(ui, remainder, icons::FORWARD);
            let kind = KINDS
                .iter()
                .find(|(_, kind)| *kind == rule.kind)
                .map_or("", |(name, _)| *name);
            pages::two_lines(
                ui,
                inner,
                &title,
                &if rule.label.trim().is_empty() {
                    format!("{kind} forward")
                } else {
                    format!("{kind} \u{b7} {}", describe(rule, host))
                },
                false,
            );
            let Some((switch, change, delete)) = slots else {
                return;
            };
            let mut on = status.is_some();
            if ui
                .switch(zone.with("switch"), switch, &mut on, "Run this rule")
                .clicked
            {
                toggles.push((rule.id, on));
            }
            if ui
                .icon_button(zone.with("edit"), change, icons::EDIT, "Edit rule", false)
                .clicked
            {
                edit = Some(rule.clone());
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
        });
        self.confirm = confirm_next;
        if let Some(rule) = edit {
            self.draft = Some(Draft::new(rule, false));
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
            Some(Finish::Save(rule, start)) => {
                let id = rule.id;
                match vault.upsert_forward(rule) {
                    Ok(()) => {
                        self.wizard = None;
                        if start {
                            self.start_rule(id, vault, files, waker);
                        }
                    }
                    Err(error) => wizard.problem = Some(error.to_string()),
                }
            }
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
