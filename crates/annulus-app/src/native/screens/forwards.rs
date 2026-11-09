//! The Port Forwarding screen, rule editor, and running-rule state.

use std::collections::HashMap;

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::SshFiles;
use annulus_ui::gfx::{Align, Canvas as _, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL};
use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::vault::Vault;

use super::super::ask::{self, Ask};
use super::super::forward_wizard::{Finish, Wizard};
use super::super::pages::{self, Fold, ListMotion, RowLook};
use super::{confirm_delete, rows, segmented};
use crate::connect;
use crate::forwards_screen::{Draft, Running, Status, start};

/// The Port Forwarding screen and the rules it runs.
#[derive(Default)]
pub struct ForwardsView {
    pub(super) running: HashMap<u64, Running>,
    draft: Fold<Draft>,
    pub(super) rows: ListMotion,
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
pub(super) fn duplicate(rule: &PortForward, id: u64) -> PortForward {
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
    pub(super) fn start_rule(
        &mut self,
        id: u64,
        vault: &Vault,
        files: &SshFiles,
        waker: Option<&Waker>,
    ) {
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
