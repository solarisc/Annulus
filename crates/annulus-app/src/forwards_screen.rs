//! The Port Forwarding screen (F-FWD-001…005): rules saved in the vault, each started and
//! stopped on its own. A running rule keeps its own SSH connection on a background thread,
//! so it needs no terminal tab.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use annulus_session::{Answers, Program, SessionEvent};
use annulus_ssh::forward::{self, Rule};
use annulus_vault::forwards::{ForwardKind, PortForward};

/// Where a running rule is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Starting,
    Active(usize),
    /// The connection dropped; the reason, while retrying.
    Reconnecting(String),
    Failed(String),
}

/// A rule that was started.
pub(crate) struct Running {
    status: Arc<Mutex<Status>>,
    events: Receiver<SessionEvent>,
    answers: Answers,
    /// Dropping it stops the rule and closes its connection.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Running {
    /// Questions the rule's connection asks (a new host key, a password).
    pub(crate) fn take_events(&self) -> Vec<SessionEvent> {
        self.events.try_iter().collect()
    }

    /// The rule's connection, for answering its questions.
    pub(crate) fn answers(&self) -> &Answers {
        &self.answers
    }

    pub(crate) fn status(&self) -> Status {
        self.status.lock().map_or_else(
            |_| Status::Failed("stopped".into()),
            |status| status.clone(),
        )
    }
}

/// Sets a rule's status and asks for a repaint.
type SetStatus = Arc<dyn Fn(Status) + Send + Sync>;

pub(crate) fn start(
    rule: &PortForward,
    program: Program,
    wake: impl Fn() + Send + Sync + 'static,
) -> Running {
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
            // The first try, or a failure no retry fixes (a changed or refused host key, a
            // refused login): it stops and says why.
            Err(reason) if attempt == 0 || is_final(&reason) => {
                return set(Status::Failed(reason));
            }
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

/// Whether a connection failure is one that trying again cannot fix: the host's key changed
/// or was not trusted, or the login was refused. Those stop a rule instead of retrying it.
pub(crate) fn is_final(reason: &str) -> bool {
    let reason = reason.to_lowercase();
    ["has changed", "was not trusted", "authentication failed"]
        .iter()
        .any(|final_reason| reason.contains(final_reason))
}

/// Whether `rule` starts when a terminal to `host_id` opens.
pub(crate) fn starts_with(rule: &PortForward, host_id: u64) -> bool {
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
pub(crate) struct Draft {
    pub(crate) rule: PortForward,
    pub(crate) bind_port: String,
    pub(crate) destination_port: String,
    pub(crate) problem: Option<String>,
    pub(crate) is_new: bool,
}

impl Draft {
    pub(crate) fn new(rule: PortForward, is_new: bool) -> Self {
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
    pub(crate) fn finish(&self) -> Result<PortForward, String> {
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
            term: String::new(),
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
    fn a_changed_key_or_a_refused_login_stops_a_rule_instead_of_retrying() {
        assert!(is_final(
            "web: the host key of web has changed (SHA256:x); the connection was stopped"
        ));
        assert!(is_final(
            "authentication failed (tried: publickey; the server accepts: password)"
        ));
        assert!(is_final("the host key was not trusted"));
        assert!(
            !is_final("could not reach web:22: timed out"),
            "the network comes back"
        );
        assert!(!is_final("the connection was lost"));
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
