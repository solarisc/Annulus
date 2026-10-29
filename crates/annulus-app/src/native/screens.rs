//! The vault's other screens in the new UI: Keychain, Port Forwarding, Snippets,
//! Known Hosts, Logs and Settings, built on [`super::pages`].

use std::collections::HashMap;

use annulus_config::settings::Settings;
use annulus_config::tokens::{ThemeMode, space};
use annulus_platform::window::Waker;
use annulus_session::profiles::Profile;
use annulus_session::{Program, SessionEvent, SshFiles};
use annulus_ssh::keys::{self, KeyKind};
use annulus_ssh::known::{self, KnownHost};
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL};
use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;

use super::pages::{self, ROW_H};
use crate::forwards_screen::{Draft, Running, Status, start};
use crate::{connect, keychain_screen, logs_screen, snippets_screen};

/// What a screen asks the window to do.
pub enum ScreenAction {
    /// Open a terminal tab running a program.
    Terminal(String, Box<Program>),
    /// Open a log, read-only, in a tab.
    Log(String, String),
}

const GAP: f32 = 8.0;

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

/// The Keychain screen.
#[derive(Default)]
pub struct KeychainView {
    message: Option<String>,
}

impl KeychainView {
    /// Draws the screen.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) {
        let mut area = pages::page(rect);
        let count = vault.ssh_keys().len();
        let mut actions = pages::header(
            ui,
            &mut area,
            "Keychain",
            Some(count),
            "Keys sync with your vault, encrypted. Hosts without one use ~/.ssh.",
        );
        if pages::action(
            ui,
            &mut actions,
            "keys.generate",
            "Generate key",
            Some(icons::PLUS),
            Kind::Primary,
        ) {
            self.message = Some(
                match keys::generate(KeyKind::Ed25519, &keychain_screen::comment()) {
                    Ok(pair) => {
                        let label = format!("Ed25519 key {}", count + 1);
                        let key = keychain_screen::stored(vault, &label, &pair);
                        match vault.upsert_key(key) {
                            Ok(()) => format!("Generated {label}."),
                            Err(error) => format!("Could not save the key: {error}"),
                        }
                    }
                    Err(error) => format!("Could not generate a key: {error}"),
                },
            );
        }
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
                icons::KEY,
                "No keys yet",
                "Generate one here, or import yours from ~/.ssh.",
            );
            return;
        }
        let list: Vec<(u64, String, String, String)> = vault
            .ssh_keys()
            .iter()
            .map(|key| {
                (
                    key.id,
                    key.label.clone(),
                    key.fingerprint.clone(),
                    key.public.clone(),
                )
            })
            .collect();
        let mut remove = None;
        rows(ui, "keys.list", area, list.len(), |ui, index, row| {
            let (id, label, fingerprint, public) = &list[index];
            let zone = Id::new(("key", *id));
            pages::row(ui, zone, row, label, false);
            let (clicked, remainder) = row_buttons(
                ui,
                row,
                zone,
                &[(icons::TRASH, "Delete"), (icons::COPY, "Copy public key")],
            );
            let inner = pages::row_icon(ui, remainder, icons::KEY);
            pages::two_lines(ui, inner, label, fingerprint, true);
            match clicked {
                Some(1) => ui.copy(public.clone()),
                Some(0) => remove = Some(*id),
                _ => {}
            }
        });
        if let Some(id) = remove
            && vault.remove_key(id).is_ok()
        {
            self.message = Some("Key deleted.".into());
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
                2.0 * CONTROL + 3.0 * space::M + 4.0,
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
            let mut line = card.take_top(CONTROL);
            let save = line.take_right(88.0);
            line.take_right(space::S);
            let cancel = line.take_right(80.0);
            line.take_right(space::S);
            let field = ui.text_field(
                Id::new("snippet.script"),
                line,
                script,
                FieldStyle {
                    placeholder: "sudo systemctl restart nginx",
                    mono: true,
                    ..FieldStyle::default()
                },
            );
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
                .clicked
                || field.submitted;
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
        // Running rules report questions they cannot ask here; they fail with a hint.
        for running in self.running.values() {
            for event in running.take_events() {
                match event {
                    SessionEvent::HostKey(_) => running.answers().host_key(false),
                    SessionEvent::Password { .. } => running.answers().password(None),
                    _ => {}
                }
            }
        }
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
                3.0 * CONTROL + 4.0 * space::M + 4.0,
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
            if !on {
                self.running.remove(&id);
                continue;
            }
            let Some(rule) = vault.forwards().iter().find(|rule| rule.id == id).cloned() else {
                continue;
            };
            let Some(host) = vault.hosts().iter().find(|host| host.id == rule.host_id) else {
                continue;
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
        if let Some(id) = remove {
            self.running.remove(&id);
            let _ = vault.remove_forward(id);
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

/// Draws Settings; returns whether anything changed.
#[allow(clippy::too_many_lines)] // One card per section, top to bottom.
pub fn settings(
    ui: &mut Ui<'_>,
    rect: Rect,
    settings: &mut Settings,
    profiles: &[Profile],
) -> bool {
    let mut area = pages::page(rect);
    pages::header(
        ui,
        &mut area,
        "Settings",
        None,
        "Saved on this device; your vault syncs separately.",
    );
    let row_h = 58.0;
    let content = 5.0 * (18.0 + space::S + space::XL + 2.0 * space::S) + 7.0 * row_h;
    let offset = ui.scroll_view(Id::new("settings.scroll"), area, content);
    ui.push_clip(area);
    let mut column = Rect::new(area.x, area.y - offset, area.w - 12.0, content);
    let mut changed = false;

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
    changed
}
