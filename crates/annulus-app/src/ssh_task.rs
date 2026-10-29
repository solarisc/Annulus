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

    /// Shows any question the connection asks, and returns the outcome once it is done.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        while let Ok(event) = self.events.try_recv() {
            self.dialogs.take(event);
        }
        self.dialogs.show(ctx, &self.answers);
        self.outcome.lock().ok().and_then(|mut slot| slot.take())
    }
}
