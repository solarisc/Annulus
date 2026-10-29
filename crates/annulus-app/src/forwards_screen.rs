//! The Port Forwarding screen (F-FWD-001…005): rules saved in the vault, each started and
//! stopped on its own. A running rule keeps its own SSH connection on a background thread,
//! so it needs no terminal tab.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::{Answers, Program, SessionEvent};
use annulus_ssh::forward::{self, Rule};
use annulus_ui::theme;
use annulus_vault::forwards::{ForwardKind, PortForward};

use crate::hosts_screen::HostsScreen;
use crate::ssh_dialogs::SshDialogs;

/// Where a running rule is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Starting,
    Active(usize),
    /// The connection dropped; the reason, while retrying.
    Reconnecting(String),
    Failed(String),
}

/// A rule that was started.
struct Running {
    status: Arc<Mutex<Status>>,
    events: Receiver<SessionEvent>,
    answers: Answers,
    dialogs: SshDialogs,
    /// Dropping it stops the rule and closes its connection.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Running {
    fn status(&self) -> Status {
        self.status.lock().map_or_else(
            |_| Status::Failed("stopped".into()),
            |status| status.clone(),
        )
    }
}

/// Sets a rule's status and asks for a repaint.
type SetStatus = Arc<dyn Fn(Status) + Send + Sync>;

fn start(rule: &PortForward, program: Program, wake: impl Fn() + Send + Sync + 'static) -> Running {
    let status = Arc::new(Mutex::new(Status::Starting));
    let (events_tx, events) = channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let answers = Answers::default();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    let set: SetStatus = {
        let status = status.clone();
        let wake = wake.clone();
        Arc::new(move |next: Status| {
            if let Ok(mut status) = status.lock() {
                *status = next;
            }
            wake();
        })
    };
    let running = |status| Running {
        status,
        events,
        answers: answers.clone(),
        dialogs: SshDialogs::default(),
        _stop: stop,
    };
    let (
        Program::Ssh {
            target,
            files,
            password,
            identity,
            via,
            proxy,
            ..
        },
        Some(forward_rule),
    ) = (program, engine_rule(rule))
    else {
        set(Status::Failed(
            "The rule is not valid, or its host is not one Annulus can connect to itself.".into(),
        ));
        return running(status);
    };
    let notify: Arc<dyn Fn(SessionEvent) + Send + Sync> = Arc::new(move |event| {
        let _ = events_tx.send(event);
        wake();
    });
    let connect = Connect {
        last: annulus_session::Hop {
            target,
            password,
            identity,
            proxy,
        },
        files,
        via,
        answers: answers.clone(),
        notify,
    };
    let thread_set = set.clone();
    let thread = std::thread::Builder::new()
        .name("port forward".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(run_rule(connect, forward_rule, thread_set, stopped));
                }
                Err(_) => thread_set(Status::Failed(
                    "Could not start the forwarding thread.".into(),
                )),
            }
        });
    if thread.is_err() {
        set(Status::Failed(
            "Could not start the forwarding thread.".into(),
        ));
    }
    running(status)
}

/// What a rule needs to (re)connect.
struct Connect {
    last: annulus_session::Hop,
    files: annulus_session::SshFiles,
    via: Vec<annulus_session::Hop>,
    answers: Answers,
    notify: Arc<dyn Fn(SessionEvent) + Send + Sync>,
}

impl Connect {
    async fn connect(&self) -> Result<Arc<annulus_ssh::Connection>, String> {
        annulus_session::connect_chain(
            &self.via,
            &self.last,
            &self.files,
            &self.answers,
            self.notify.clone(),
        )
        .await
        .map(Arc::new)
    }
}

/// Seconds to wait before reconnect attempt `attempt` (1, 2, 4, … up to 30).
fn backoff(attempt: u32) -> u64 {
    1u64.checked_shl(attempt.saturating_sub(1))
        .unwrap_or(u64::MAX)
        .min(30)
}

/// Connects and runs the rule until `stopped`. A first failure ends the rule with its
/// reason; a connection lost later is retried with growing pauses.
async fn run_rule(
    connect: Connect,
    rule: Rule,
    set: SetStatus,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) {
    let mut attempt = 0u32;
    loop {
        let connection = tokio::select! {
            connected = connect.connect() => connected,
            _ = &mut stopped => return,
        };
        let connection = match connection {
            Ok(connection) => connection,
            Err(reason) if attempt == 0 => return set(Status::Failed(reason)),
            Err(reason) => {
                attempt += 1;
                set(Status::Reconnecting(reason));
                tokio::select! {
                    _ = &mut stopped => return,
                    () = tokio::time::sleep(Duration::from_secs(backoff(attempt))) => continue,
                }
            }
        };
        let running = match forward::start(connection.clone(), rule.clone()).await {
            Ok(running) => running,
            Err(error) => return set(Status::Failed(error.to_string())),
        };
        attempt = 0;
        while !connection.is_closed() {
            set(Status::Active(running.connections()));
            tokio::select! {
                _ = &mut stopped => return,
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
        drop(running);
        attempt += 1;
        set(Status::Reconnecting("the connection was lost".into()));
        tokio::select! {
            _ = &mut stopped => return,
            () = tokio::time::sleep(Duration::from_secs(backoff(attempt))) => {}
        }
    }
}

/// Whether `rule` starts when a terminal to `host_id` opens.
fn starts_with(rule: &PortForward, host_id: u64) -> bool {
    rule.with_host && rule.host_id == host_id
}

/// The saved rule as the forwarding engine runs it; `None` if it is not valid.
fn engine_rule(rule: &PortForward) -> Option<Rule> {
    match (rule.kind, rule.bind()) {
        (ForwardKind::Local | ForwardKind::Dynamic, None) => None,
        (ForwardKind::Local, Some(bind)) => Some(Rule::Local {
            bind,
            host: rule.destination_host.trim().to_owned(),
            port: rule.destination_port,
        }),
        (ForwardKind::Dynamic, Some(bind)) => Some(Rule::Dynamic { bind }),
        (ForwardKind::Remote, _) => Some(Rule::Remote {
            bind_host: rule.bind_address.trim().to_owned(),
            bind_port: rule.bind_port,
            host: rule.destination_host.trim().to_owned(),
            port: rule.destination_port,
        }),
    }
}

/// The rule being edited, with its ports as text until saved.
struct Draft {
    rule: PortForward,
    bind_port: String,
    destination_port: String,
    problem: Option<String>,
    is_new: bool,
}

impl Draft {
    fn new(rule: PortForward, is_new: bool) -> Self {
        let port = |port: u16| {
            if port == 0 {
                String::new()
            } else {
                port.to_string()
            }
        };
        Self {
            bind_port: port(rule.bind_port),
            destination_port: port(rule.destination_port),
            rule,
            problem: None,
            is_new,
        }
    }

    /// The rule with its ports parsed, or why it cannot be saved.
    fn finish(&self) -> Result<PortForward, String> {
        let mut rule = self.rule.clone();
        self.rule.label.trim().clone_into(&mut rule.label);
        rule.bind_port = self.bind_port.trim().parse().unwrap_or(0);
        rule.destination_port = if rule.has_destination() {
            self.destination_port.trim().parse().unwrap_or(0)
        } else {
            0
        };
        match rule.problem() {
            Some(problem) => Err(problem.to_owned()),
            None => Ok(rule),
        }
    }
}

/// The Port Forwarding screen's state while the vault is unlocked.
#[derive(Default)]
pub struct ForwardsScreen {
    running: HashMap<u64, Running>,
    editor: Option<Draft>,
    confirm_delete: Option<u64>,
    error: Option<String>,
    auto_started: bool,
}

impl ForwardsScreen {
    fn start_rule(&mut self, ctx: &egui::Context, rule: &PortForward, hosts: &HostsScreen) {
        let Some(host) = hosts
            .vault()
            .hosts()
            .iter()
            .find(|host| host.id == rule.host_id)
        else {
            self.error = Some(format!(
                "{}: its host was deleted. Edit the rule to pick another.",
                rule.label
            ));
            return;
        };
        let ctx = ctx.clone();
        let running = start(rule, hosts.connect_host(host), move || {
            ctx.request_repaint();
        });
        self.running.insert(rule.id, running);
    }

    /// Starts the rules marked to start with the vault (F-FWD-004), once.
    pub fn auto_start(&mut self, ctx: &egui::Context, hosts: &HostsScreen) {
        if std::mem::replace(&mut self.auto_started, true) {
            return;
        }
        let rules: Vec<PortForward> = hosts
            .vault()
            .forwards()
            .iter()
            .filter(|rule| rule.auto_start)
            .cloned()
            .collect();
        for rule in &rules {
            self.start_rule(ctx, rule, hosts);
        }
    }

    /// Starts the rules marked to start with `host_id`'s terminal (F-FWD-004) that are not
    /// running yet.
    pub fn host_opened(&mut self, ctx: &egui::Context, host_id: u64, hosts: &HostsScreen) {
        let rules: Vec<PortForward> = hosts
            .vault()
            .forwards()
            .iter()
            .filter(|rule| starts_with(rule, host_id) && !self.running.contains_key(&rule.id))
            .cloned()
            .collect();
        for rule in &rules {
            self.start_rule(ctx, rule, hosts);
        }
    }

    /// Shows SSH questions from running rules; they can arrive on any screen.
    pub fn prompts(&mut self, ctx: &egui::Context) {
        for running in self.running.values_mut() {
            for event in running.events.try_iter().collect::<Vec<_>>() {
                running.dialogs.take(event);
            }
            if running.dialogs.show(ctx, &running.answers) {
                // One dialog at a time.
                break;
            }
        }
    }

    /// Draws the screen.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, hosts: &mut HostsScreen) {
        if self.editor.is_some() {
            egui::Panel::right("forward-editor")
                .resizable(false)
                .exact_size(360.0)
                .show(ui, |ui| self.editor_ui(ui, palette, hosts));
        }
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            let first_host = hosts.vault().hosts().first().map(|host| host.id);
            let new = ui.add_enabled(first_host.is_some(), egui::Button::new("New rule"));
            if let Some(host_id) = first_host
                && new.clicked()
            {
                self.editor = Some(Draft::new(hosts.vault().draft_forward(host_id), true));
            }
            if first_host.is_none() {
                ui.label("Save a host first; rules forward through a saved host.");
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(theme::color(palette.status_error), error);
        }
        ui.add_space(space::S);
        let rules = hosts.vault().forwards().to_vec();
        if rules.is_empty() {
            ui.label(
                RichText::new("No rules yet. A local rule makes a remote port reachable on this computer; a SOCKS rule routes any program that supports a SOCKS proxy through the host.")
                    .color(theme::color(palette.text_secondary)),
            );
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for rule in &rules {
                self.row(ui, palette, rule, hosts);
                ui.separator();
            }
        });
    }

    fn row(&mut self, ui: &mut Ui, palette: &Palette, rule: &PortForward, hosts: &mut HostsScreen) {
        let host_label = hosts
            .vault()
            .hosts()
            .iter()
            .find(|host| host.id == rule.host_id)
            .map_or_else(|| "(host deleted)".to_owned(), |host| host.label.clone());
        let status = self.running.get(&rule.id).map(Running::status);
        let running = matches!(
            status,
            Some(Status::Starting | Status::Active(_) | Status::Reconnecting(_))
        );
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let name = ui
                    .add(
                        egui::Label::new(
                            RichText::new(&rule.label).size(text::BODY + 1.0).strong(),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Double-click to start or stop");
                toggle = name.double_clicked();
                ui.label(
                    RichText::new(format!("{}  \u{b7}  via {host_label}", rule.summary()))
                        .size(text::SECONDARY)
                        .color(theme::color(palette.text_secondary)),
                );
                if rule.is_exposed() {
                    ui.colored_label(
                        theme::color(palette.status_warn),
                        "Listens beyond this computer: other devices on the network can use it.",
                    );
                }
                match &status {
                    None => ui.label("Stopped"),
                    Some(Status::Starting) => ui.label("Starting\u{2026}"),
                    Some(Status::Reconnecting(reason)) => ui.colored_label(
                        theme::color(palette.status_warn),
                        format!("Reconnecting\u{2026} ({reason})"),
                    ),
                    Some(Status::Active(1)) => {
                        ui.colored_label(theme::color(palette.status_ok), "Active, 1 connection")
                    }
                    Some(Status::Active(n)) => ui.colored_label(
                        theme::color(palette.status_ok),
                        format!("Active, {n} connections"),
                    ),
                    Some(Status::Failed(reason)) => {
                        ui.colored_label(theme::color(palette.status_error), reason)
                    }
                };
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.confirm_delete == Some(rule.id) {
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                    if ui.button("Delete rule").clicked() {
                        self.running.remove(&rule.id);
                        self.confirm_delete = None;
                        self.error = hosts
                            .vault_mut()
                            .remove_forward(rule.id)
                            .err()
                            .map(|error| error.to_string());
                    }
                    return;
                }
                if ui.button("Delete").clicked() {
                    self.confirm_delete = Some(rule.id);
                }
                if ui.button("Duplicate").clicked() {
                    let mut copy = hosts.vault().draft_forward(rule.host_id);
                    let id = copy.id;
                    copy = PortForward {
                        id,
                        label: format!("{} copy", rule.label),
                        ..rule.clone()
                    };
                    self.editor = Some(Draft::new(copy, true));
                }
                if ui.button("Edit").clicked() {
                    self.editor = Some(Draft::new(rule.clone(), false));
                }
                let label = if running { "Stop" } else { "Start" };
                toggle |= ui.button(label).clicked();
            });
        });
        if toggle && running {
            self.running.remove(&rule.id);
        } else if toggle {
            self.error = None;
            self.start_rule(ui.ctx(), rule, hosts);
        }
    }

    fn editor_ui(&mut self, ui: &mut Ui, palette: &Palette, hosts: &mut HostsScreen) {
        let Some(draft) = &mut self.editor else {
            return;
        };
        ui.add_space(space::M);
        ui.heading(if draft.is_new {
            "New rule"
        } else {
            "Edit rule"
        });
        ui.add_space(space::M);
        let host_names: Vec<(u64, String)> = hosts
            .vault()
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        fields(ui, draft, &host_names);
        ui.label(
            RichText::new("Keep 127.0.0.1 unless other devices should use the forward.")
                .size(text::SECONDARY)
                .color(theme::color(palette.text_secondary)),
        );
        if let Some(problem) = &draft.problem {
            ui.colored_label(theme::color(palette.status_error), problem);
        }
        ui.add_space(space::M);
        let mut close = false;
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                match draft.finish() {
                    Ok(rule) => match hosts.vault_mut().upsert_forward(rule) {
                        Ok(()) => close = true,
                        Err(error) => draft.problem = Some(format!("Could not save: {error}")),
                    },
                    Err(problem) => draft.problem = Some(problem),
                }
            }
            if ui.button("Cancel").clicked()
                || ui.input(|input| input.key_pressed(egui::Key::Escape))
            {
                close = true;
            }
        });
        if close {
            self.editor = None;
        }
    }
}

/// The rule editor's fields.
fn fields(ui: &mut Ui, draft: &mut Draft, host_names: &[(u64, String)]) {
    egui::Grid::new("forward-fields")
        .num_columns(2)
        .spacing([space::M, space::S])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut draft.rule.label).hint_text("postgres"));
            ui.end_row();
            ui.label("Type");
            ui.horizontal(|ui| {
                ui.radio_value(&mut draft.rule.kind, ForwardKind::Local, "Local");
                ui.radio_value(&mut draft.rule.kind, ForwardKind::Dynamic, "SOCKS");
                ui.radio_value(&mut draft.rule.kind, ForwardKind::Remote, "Remote");
            });
            ui.end_row();
            ui.label("Through host");
            let selected = host_names
                .iter()
                .find(|(id, _)| *id == draft.rule.host_id)
                .map_or("(pick a host)", |(_, label)| label.as_str());
            egui::ComboBox::from_id_salt("forward-host")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (id, label) in host_names {
                        ui.selectable_value(&mut draft.rule.host_id, *id, label);
                    }
                });
            ui.end_row();
            ui.label(if draft.rule.kind == ForwardKind::Remote {
                "Server listens on"
            } else {
                "Listen on"
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut draft.rule.bind_address).desired_width(110.0),
                );
                ui.label(":");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.bind_port)
                        .desired_width(60.0)
                        .hint_text("8080"),
                );
            });
            ui.end_row();
            if draft.rule.has_destination() {
                ui.label("Destination");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.rule.destination_host)
                            .desired_width(110.0)
                            .hint_text("localhost"),
                    );
                    ui.label(":");
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.destination_port)
                            .desired_width(60.0)
                            .hint_text("5432"),
                    );
                });
                ui.end_row();
            }
            ui.label("");
            ui.checkbox(&mut draft.rule.auto_start, "Start when the vault unlocks");
            ui.checkbox(
                &mut draft.rule.with_host,
                "Start when I open a terminal to its host",
            );
            ui.end_row();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the SSH banner a forwarded port leads to, or `None` if nothing answers.
    fn banner_through(port: u16) -> Option<String> {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let mut buffer = [0u8; 64];
        let read = stream.read(&mut buffer).ok()?;
        Some(String::from_utf8_lossy(&buffer[..read]).into_owned())
    }

    /// Answers host-key questions and waits until `running` reports `wanted`.
    fn wait_for(running: &mut Running, wanted: impl Fn(&Status) -> bool) -> Status {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            while let Ok(event) = running.events.try_recv() {
                if matches!(event, SessionEvent::HostKey(_)) {
                    running.answers.host_key(true);
                }
            }
            let status = running.status();
            if wanted(&status) {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "stuck at {status:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// End to end on test VM 1 (docs/TESTING.md §Test VMs): a local rule to the VM's own
    /// sshd keeps working after its connection is killed on the server.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_a_local_rule_comes_back_after_its_connection_is_killed() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let (vm1, user) = (var("ANNULUS_E2E_VM1"), var("ANNULUS_E2E_USER"));
        let key = std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap();
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let mut rule = PortForward::new(1, 1);
        (rule.label, rule.bind_port) = ("sshd".into(), port);
        (rule.destination_host, rule.destination_port) = ("127.0.0.1".into(), 22);
        let dir = std::env::temp_dir().join(format!("annulus-fwd-e2e-{}", std::process::id()));
        let program = Program::Ssh {
            target: annulus_session::Target {
                host: vm1.clone(),
                port: 22,
                user: user.clone(),
            },
            files: annulus_session::SshFiles {
                known_hosts: dir.join("known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: Some(annulus_session::Password::new(&key)),
            via: Vec::new(),
            proxy: None,
            reconnect: true,
            env: Vec::new(),
            startup: None,
        };
        let mut running = start(&rule, program, || {});
        wait_for(&mut running, |status| matches!(status, Status::Active(_)));
        assert!(banner_through(port).is_some_and(|b| b.starts_with("SSH-2.0-OpenSSH")));

        // Kill every forward-only login of the test user on the VM (this rule's, and any
        // tunnel); the exec session doing it is "sshd: user@notty" and survives.
        let killed = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", &format!("{user}@{vm1}")])
            .arg(format!("pkill -9 -f '^sshd: {user}$'"))
            .status()
            .unwrap();
        assert!(killed.success());
        wait_for(&mut running, |status| {
            matches!(status, Status::Reconnecting(_))
        });
        wait_for(&mut running, |status| matches!(status, Status::Active(_)));
        assert!(banner_through(port).is_some_and(|b| b.starts_with("SSH-2.0-OpenSSH")));
    }

    #[test]
    fn a_rule_starts_with_its_own_hosts_terminal_only_when_asked() {
        let mut rule = PortForward::new(1, 7);
        assert!(!starts_with(&rule, 7), "off unless chosen");
        rule.with_host = true;
        assert!(starts_with(&rule, 7));
        assert!(!starts_with(&rule, 8), "another host's terminal");
    }

    #[test]
    fn reconnects_back_off_up_to_thirty_seconds() {
        let waits: Vec<u64> = (1..=7).map(backoff).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(backoff(200), 30);
    }

    #[test]
    fn the_draft_parses_ports_and_reports_the_first_problem() {
        let mut draft = Draft::new(PortForward::new(1, 1), true);
        draft.rule.label = "db".into();
        draft.bind_port = "15432".into();
        draft.rule.destination_host = "localhost".into();
        draft.destination_port = "nope".into();
        assert_eq!(
            draft.finish().unwrap_err(),
            "Choose a destination port from 1 to 65535."
        );
        draft.destination_port = " 5432 ".into();
        let rule = draft.finish().unwrap();
        assert_eq!((rule.bind_port, rule.destination_port), (15432, 5432));

        // A SOCKS rule drops the destination port it does not use.
        draft.rule.kind = ForwardKind::Dynamic;
        assert_eq!(draft.finish().unwrap().destination_port, 0);
    }
}
