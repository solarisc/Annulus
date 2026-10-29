//! A one-off command on a saved host, run in the background: connect (asking for a host
//! key or password through the usual dialogs), run it, and hand back how it ended. Used for
//! actions the user starts, such as installing a key (F-KEY-004).

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use eframe::egui;
use annulus_session::{Answers, Hop, Program, SessionEvent};
use annulus_ssh::Exec;

use crate::ssh_dialogs::SshDialogs;

/// How the command ended: its exit status and output, or why it could not run.
pub type Outcome = Result<Exec, String>;

/// A command running on a host.
pub struct SshTask {
    events: Receiver<SessionEvent>,
    answers: Answers,
    dialogs: SshDialogs,
    outcome: Arc<Mutex<Option<Outcome>>>,
}

impl SshTask {
    /// Connects with `program` (an SSH program from the hosts screen) and runs `command`.
    /// `wake` is called from the background thread whenever there is news.
    pub fn exec(
        program: Program,
        command: String,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (events_tx, events) = channel();
        let answers = Answers::default();
        let outcome = Arc::new(Mutex::new(None));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let finish = {
            let outcome = outcome.clone();
            let wake = wake.clone();
            move |result: Outcome| {
                if let Ok(mut slot) = outcome.lock() {
                    *slot = Some(result);
                }
                wake();
            }
        };
        let task = Self {
            events,
            answers: answers.clone(),
            dialogs: SshDialogs::default(),
            outcome,
        };
        let Program::Ssh {
            target,
            files,
            password,
            identity,
            via,
            proxy,
            ..
        } = program
        else {
            finish(Err("Annulus cannot connect to this host itself.".into()));
            return task;
        };
        let notify: Arc<dyn Fn(SessionEvent) + Send + Sync> = Arc::new(move |event| {
            let _ = events_tx.send(event);
            wake();
        });
        let last = Hop {
            target,
            password,
            identity,
            proxy,
        };
        let finish_thread = finish.clone();
        let spawned = std::thread::Builder::new()
            .name("ssh task".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return finish_thread(Err("Could not start the connection.".into()));
                };
                finish_thread(runtime.block_on(async {
                    let connection =
                        annulus_session::connect_chain(&via, &last, &files, &answers, notify)
                            .await?;
                    connection
                        .exec(&command)
                        .await
                        .map_err(|error| error.to_string())
                }));
            });
        if spawned.is_err() {
            finish(Err("Could not start the connection.".into()));
        }
        task
    }

    /// The next question the connection asks, for the new UI to show.
    pub(crate) fn question(&mut self) -> Option<annulus_session::SessionEvent> {
        self.events.try_recv().ok()
    }

    /// The connection's answers.
    pub(crate) fn answers(&self) -> &annulus_session::Answers {
        &self.answers
    }

    /// The outcome, once the command finished (the new UI's poll).
    pub(crate) fn outcome(&self) -> Option<Outcome> {
        self.outcome.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Shows any question the connection asks, and returns the outcome once it is done.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        while let Ok(event) = self.events.try_recv() {
            self.dialogs.take(event);
        }
        self.dialogs.show(ctx, &self.answers);
        self.outcome.lock().ok().and_then(|mut slot| slot.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polls `task` until it is done (at most 20 s).
    fn finish(task: &mut SshTask) -> Outcome {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let mut found = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                found = task.poll(ui.ctx());
            });
            output.textures_delta.clear();
            if let Some(outcome) = found {
                return outcome;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the task never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_host_annulus_cannot_reach_itself_fails_at_once() {
        let mut task = SshTask::exec(Program::system_ssh("web"), "true".into(), || {});
        assert_eq!(
            finish(&mut task),
            Err("Annulus cannot connect to this host itself.".into())
        );
    }

    #[test]
    fn a_refused_connection_comes_back_as_an_error() {
        // A port that was just free: nothing listens there.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = std::env::temp_dir().join(format!("annulus-task-{}", std::process::id()));
        let program = Program::Ssh {
            target: annulus_session::Target {
                host: "127.0.0.1".into(),
                port,
                user: "nobody".into(),
            },
            files: annulus_session::SshFiles {
                known_hosts: dir.join("known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: false,
            env: Vec::new(),
            startup: None,
            term: String::new(),
        };
        let mut task = SshTask::exec(program, "true".into(), || {});
        assert!(finish(&mut task).is_err());
    }
}
