//! The vault's other screens in the new UI: Keychain, Port Forwarding, Snippets,
//! Known Hosts, Logs and Settings, built on [`super::pages`].

use std::collections::HashMap;

use annulus_config::settings::Settings;
use annulus_config::tokens::{ThemeMode, space};
use annulus_platform::window::Waker;
use annulus_session::profiles::Profile;
use annulus_session::{Program, SshFiles};
use annulus_ssh::keys::{self, KeyKind, KeyPair};
use annulus_ssh::known::{self, KnownHost};
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL, button_width};
use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::keychain::SshKey;
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use super::pages::{self, ROW_H};
use crate::forwards_screen::{Draft, Running, Status, start};
use crate::job::{Job, Poll};
use crate::ssh_task::SshTask;
use crate::{connect, keychain_screen, logs_screen, snippets_screen};

/// What a screen asks the window to do.
pub enum ScreenAction {
    /// Open a terminal tab running a program.
    Terminal(String, Box<Program>),
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
    let delete_w = button_width(ui.gfx, delete, None);
    let delete_rect = remainder.take_right(delete_w).centered(delete_w, 30.0);
    remainder.take_right(space::S);
    let cancel_w = button_width(ui.gfx, "Cancel", None);
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

/// One key as the list shows it.
struct KeyRow {
    id: u64,
    label: String,
    detail: String,
    public: String,
}

/// The Keychain screen.
#[derive(Default)]
pub struct KeychainView {
    form: KeyForm,
    /// The last result, and whether it is a problem.
    message: Option<(bool, String)>,
    confirm_delete: Option<u64>,
    install: Option<Install>,
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
        let generate = actions.take_right(button_width(ui.gfx, "Generate key", Some(icons::PLUS)));
        actions.take_right(space::S);
        let import = actions.take_right(button_width(ui.gfx, "Import key", Some(icons::DOWNLOAD)));
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
            && ask::card(ui, rect, ask_now, task.answers())
        {
            install.ask = None;
        }
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
                ui.text_field(
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
                let go = buttons.take_right(button_width(ui.gfx, &go_label, None).max(96.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(ui.gfx, "Cancel", None));
                close |= ui
                    .button(Id::new("key.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked;
                let clicked = ui
                    .button(Id::new("key.go"), go, &go_label, None, Kind::Primary)
                    .clicked;
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
                let go = line.take_right(button_width(ui.gfx, "Import", None).max(96.0));
                line.take_right(space::S);
                let cancel = line.take_right(button_width(ui.gfx, "Cancel", None));
                line.take_right(space::M);
                ui.text_field(
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
                    .clicked;
                let clicked = ui
                    .button(Id::new("import.go"), go, "Import", None, Kind::Primary)
                    .clicked;
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
        let go = line.take_right(button_width(ui.gfx, "Installing\u{2026}", None));
        line.take_right(space::S);
        let cancel = line.take_right(button_width(ui.gfx, "Cancel", None));
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
            .clicked;
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
        let list: Vec<KeyRow> = vault
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
                }
            })
            .collect();
        let confirm = self.confirm_delete;
        let mut confirm_next = confirm;
        let (mut remove, mut install, mut copied) = (None, None, None);
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
                    Some(0) => confirm_next = Some(key.id),
                    _ => {}
                }
                remainder
            };
            let inner = pages::row_icon(ui, remainder, icons::KEY);
            pages::two_lines(ui, inner, &key.label, &key.detail, true);
        });
        self.confirm_delete = confirm_next;
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
}

impl SnippetsView {
    /// Draws the screen; running a snippet opens a terminal.
    #[allow(clippy::too_many_lines)] // Header, editor and list, top to bottom.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) -> Option<ScreenAction> {
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
                .clicked;
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
        if count == 0 {
            pages::empty(
                ui,
                area,
                icons::CODE,
                "No snippets yet",
                "Save a command once, run it anywhere.",
            );
            return None;
        }
        let list: Vec<Snippet> = vault.snippets().to_vec();
        let mut action = None;
        let mut remove = None;
        rows(ui, "snippets.list", area, list.len(), |ui, index, row| {
            let snippet = &list[index];
            let zone = Id::new(("snippet", snippet.id));
            pages::row(ui, zone, row, &snippet.label, false);
            let (clicked, remainder) = row_buttons(
                ui,
                row,
                zone,
                &[
                    (icons::TRASH, "Delete"),
                    (icons::PLAY, "Run in a local terminal"),
                ],
            );
            let inner = pages::row_icon(ui, remainder, icons::CODE);
            let first = snippet.script.lines().next().unwrap_or_default();
            pages::two_lines(ui, inner, &snippet.label, first, true);
            match clicked {
                Some(1) => {
                    action = Some(ScreenAction::Terminal(
                        snippet.label.clone(),
                        Box::new(snippets_screen::with_script(
                            Program::DefaultShell,
                            &snippet.script,
                        )),
                    ));
                }
                Some(0) => remove = Some(snippet.id),
                _ => {}
            }
        });
        if let Some(id) = remove {
            let _ = vault.remove_snippet(id);
        }
        action
    }
}

// ---------------------------------------------------------------------------------------
// Logs

/// The Logs screen.
#[derive(Default)]
pub struct LogsView {
    message: Option<String>,
}

impl LogsView {
    /// Draws the screen; opening a log shows it in a tab.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &Vault,
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
        if let Some(message) = &self.message {
            let t = ui.theme;
            ui.gfx
                .text(area.take_top(20.0), message, SMALL, t.text2, Align::Start);
            area.take_top(space::S);
        }
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
        rows(ui, "logs.list", area, list.len(), |ui, index, row| {
            let (id, title, saved_at, text) = &list[index];
            let zone = Id::new(("log", *id));
            let response = pages::row(ui, zone, row, title, false);
            let (clicked, remainder) =
                row_buttons(ui, row, zone, &[(icons::DOWNLOAD, "Export to Downloads")]);
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
            if clicked == Some(0) {
                export = Some(*id);
            } else if response.clicked {
                action = Some(ScreenAction::Log(title.clone(), text.clone()));
            }
        });
        if let Some(id) = export
            && let Some(log) = vault.logs().iter().find(|log| log.id == id)
        {
            self.message = Some(match logs_screen::export(log, &logs_screen::downloads()) {
                Ok(path) => format!("Saved to {}", path.display()),
                Err(error) => format!("Could not export: {error}"),
            });
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
                forget = Some(entry.line);
            }
        });
        if let Some(line) = forget {
            self.message = Some(match known::forget(path, line) {
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
                rule.with_host && rule.host_id == host_id && !self.running.contains_key(&rule.id)
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
        if let Some(host) = first_host
            && pages::action(
                ui,
                &mut actions,
                "forwards.new",
                "New rule",
                Some(icons::PLUS),
                Kind::Primary,
            )
        {
            self.draft = Some(Draft::new(vault.draft_forward(host), true));
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
                "New rule",
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
                .clicked;
            if ui
                .button(Id::new("forward.save"), save, "Save", None, Kind::Primary)
                .clicked
            {
                match draft.finish() {
                    Ok(rule) => {
                        let _ = vault.upsert_forward(rule);
                        self.draft = None;
                    }
                    Err(problem) => draft.problem = Some(problem),
                }
            } else if cancelled {
                self.draft = None;
            }
        }
        if count == 0 {
            let line = if first_host.is_some() {
                "Add a rule to tunnel a port through one of your hosts."
            } else {
                "Save a host first: rules run through a host."
            };
            pages::empty(ui, area, icons::FORWARD, "No rules yet", line);
            return;
        }
        let rules: Vec<PortForward> = vault.forwards().to_vec();
        let host_names: HashMap<u64, String> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let mut toggles = Vec::new();
        let mut remove = None;
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
            let mut remainder = row.inset(space::M, 0.0);
            let delete = remainder.take_right(30.0).centered(30.0, 30.0);
            remainder.take_right(space::S);
            let switch = remainder.take_right(40.0);
            remainder.take_right(space::S);
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
            let mut on = status.is_some();
            if ui
                .switch(zone.with("switch"), switch, &mut on, "Run this rule")
                .clicked
            {
                toggles.push((rule.id, on));
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
                remove = Some(rule.id);
            }
        });
        for (id, on) in toggles {
            if on {
                self.start_rule(id, vault, files, waker);
            } else {
                self.running.remove(&id);
            }
        }
        if let Some(id) = remove {
            self.running.remove(&id);
            let _ = vault.remove_forward(id);
        }
        if let Some((&id, ask)) = self.asks.iter_mut().next()
            && let Some(running) = self.running.get(&id)
            && ask::card(ui, rect, ask, running.answers())
        {
            self.asks.remove(&id);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Settings

const LOCK_MINUTES: [(u32, &str); 5] = [
    (0, "Never"),
    (5, "After 5 minutes"),
    (15, "After 15 minutes"),
    (30, "After 30 minutes"),
    (60, "After 1 hour"),
];

/// Draws the account card into a rectangle and returns the height it took.
pub type AccountCard<'a> = &'a mut dyn FnMut(&mut Ui<'_>, Rect) -> f32;

/// Draws Settings, with the account card (`account`, which returns the height it took)
/// first; `account_h` is that height from the frame before. Returns whether a setting
/// changed and the account card's height now.
#[allow(clippy::too_many_lines)] // One card per section, top to bottom.
pub fn settings(
    ui: &mut Ui<'_>,
    rect: Rect,
    (settings, profiles): (&mut Settings, &[Profile]),
    account: Option<AccountCard<'_>>,
    account_h: f32,
) -> (bool, f32) {
    let mut area = pages::page(rect);
    pages::header(
        ui,
        &mut area,
        "Settings",
        None,
        "Saved on this device; your vault syncs separately.",
    );
    let row_h = 58.0;
    let content =
        5.0 * (18.0 + space::S + space::XL + 2.0 * space::S) + 7.0 * row_h + account_h + space::XL;
    let offset = ui.scroll_view(Id::new("settings.scroll"), area, content);
    ui.push_clip(area);
    let mut column = Rect::new(area.x, area.y - offset, area.w - 12.0, content);
    let mut changed = false;
    let mut drawn_h = 0.0;
    if let Some(account) = account {
        drawn_h = account(ui, column);
        column.take_top(drawn_h + space::XL);
    }

    let mut card = pages::card(ui, &mut column, "Appearance", row_h + 2.0 * space::S);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Theme",
        "Follow the system, or always light or dark.",
        300.0,
    );
    let modes = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];
    let at = modes
        .iter()
        .position(|mode| *mode == settings.theme)
        .unwrap_or(0);
    if let Some(chosen) = segmented(
        ui,
        "settings.theme",
        control,
        &["System", "Light", "Dark"],
        at,
    ) {
        settings.theme = modes[chosen];
        changed = true;
    }

    let mut card = pages::card(ui, &mut column, "Terminal", 2.0 * row_h + 2.0 * space::S);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Colors",
        "The palette programs draw with.",
        260.0,
    );
    let themes: Vec<String> = annulus_term::themes::THEMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let at = themes
        .iter()
        .position(|name| *name == settings.terminal_theme);
    if let Some(chosen) = ui.select(
        Id::new("settings.colors"),
        control,
        &themes,
        at,
        "Annulus Dark",
    ) {
        settings.terminal_theme.clone_from(&themes[chosen]);
        changed = true;
    }
    pages::divider(ui, card.y, card);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Font size",
        "Ctrl+Shift+= and Ctrl+Shift+- change it too.",
        260.0,
    );
    let sizes: Vec<String> = (11..=20).map(|size| format!("{size} pt")).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 8 to 32.
    let at = (settings.font_size.round() as usize).checked_sub(11);
    if let Some(chosen) = ui.select(Id::new("settings.font"), control, &sizes, at, "14 pt") {
        #[allow(clippy::cast_precision_loss)] // Small sizes.
        let size = (chosen + 11) as f32;
        settings.font_size = size;
        changed = true;
    }

    let mut card = pages::card(ui, &mut column, "Security", row_h + 2.0 * space::S);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Lock the vault",
        "When you have been away from the keyboard.",
        260.0,
    );
    let items: Vec<String> = LOCK_MINUTES
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let at = LOCK_MINUTES
        .iter()
        .position(|(minutes, _)| *minutes == settings.auto_lock_minutes);
    if let Some(chosen) = ui.select(Id::new("settings.lock"), control, &items, at, "Custom") {
        settings.auto_lock_minutes = LOCK_MINUTES[chosen].0;
        changed = true;
    }

    let mut card = pages::card(ui, &mut column, "Sessions", 2.0 * row_h + 2.0 * space::S);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Reopen my tabs",
        "Local shells and saved hosts, at the next start.",
        60.0,
    );
    if ui
        .switch(
            Id::new("settings.restore"),
            control,
            &mut settings.restore_tabs,
            "Reopen my tabs",
        )
        .clicked
    {
        changed = true;
    }
    pages::divider(ui, card.y, card);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "Save session logs",
        "Output can hold secrets; logs stay encrypted.",
        60.0,
    );
    if ui
        .switch(
            Id::new("settings.logs"),
            control,
            &mut settings.record_logs,
            "Save session logs",
        )
        .clicked
    {
        changed = true;
    }

    let mut card = pages::card(ui, &mut column, "Local shells", row_h + 2.0 * space::S);
    let control = pages::setting(
        ui,
        card.take_top(row_h),
        "New terminals open",
        "Ctrl+Shift+T and the + button.",
        260.0,
    );
    let names: Vec<String> = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    let at = settings
        .default_profile
        .as_ref()
        .and_then(|name| names.iter().position(|known| known == name));
    if let Some(chosen) = ui.select(
        Id::new("settings.profile"),
        control,
        &names,
        at.or(Some(0)),
        "System shell",
    ) {
        settings.default_profile = Some(names[chosen].clone());
        changed = true;
    }
    ui.pop_clip();
    (changed, drawn_h)
}

/// The snippet panel beside a terminal (Ctrl+Shift+.): search, then Paste types a snippet
/// into the focused pane and Run types it with Enter. Returns the bytes to type.
pub fn snippet_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    search: &mut String,
    snippets: &[Snippet],
) -> Option<Vec<u8>> {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 0.0, t.chrome);
    ui.gfx
        .fill_rect(Rect::new(rect.x, rect.y, 1.0, rect.h), 0.0, t.line);
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
