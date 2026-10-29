//! The questions an SSH connection asks while it connects: trust an unknown host key, and
//! the password. Terminal and SFTP tabs show them the same way.

use eframe::egui::{self, Key, Vec2};
use annulus_session::{Answers, HostKeyQuestion, SessionEvent};
use annulus_ui::secret::{SecretText, secret_field};
use zeroize::Zeroizing;

/// The question on screen, if any.
enum Question {
    HostKey(HostKeyQuestion),
    Password { user: String, host: String },
}

/// Shows one SSH question at a time and sends the answer back.
#[derive(Default)]
pub struct SshDialogs {
    question: Option<Question>,
    password: SecretText,
}

impl SshDialogs {
    /// Takes the event if it is a question; returns it otherwise.
    pub fn take(&mut self, event: SessionEvent) -> Option<SessionEvent> {
        match event {
            SessionEvent::HostKey(question) => self.question = Some(Question::HostKey(question)),
            SessionEvent::Password { user, host } => {
                self.question = Some(Question::Password { user, host });
            }
            other => return Some(other),
        }
        None
    }

    /// Draws the open question and sends its answer to `answers`. True while a question is
    /// open, so the tab can hold back its own keyboard handling.
    pub fn show(&mut self, ctx: &egui::Context, answers: &Answers) -> bool {
        match &self.question {
            None => false,
            Some(Question::HostKey(question)) => {
                if let Some(trust) = host_key(ctx, question) {
                    answers.host_key(trust);
                    self.question = None;
                }
                true
            }
            Some(Question::Password { user, host }) => {
                if let Some(typed) = password(ctx, user, host, &mut self.password) {
                    answers.password(match typed {
                        Typed::Connect(password) => Some(password),
                        Typed::Cancel => None,
                    });
                    self.question = None;
                }
                true
            }
        }
    }
}

fn dialog(title: &str) -> egui::Window<'_> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
}

fn host_key(ctx: &egui::Context, question: &HostKeyQuestion) -> Option<bool> {
    let mut answer = None;
    dialog("Unknown host").show(ctx, |ui| {
        ui.label(format!(
            "This is the first connection to {}.",
            question.host
        ));
        ui.label("Check that the fingerprint matches the server before trusting it:");
        ui.add_space(6.0);
        ui.monospace(format!("{}  {}", question.algorithm, question.fingerprint));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Trust and connect").clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|input| input.key_pressed(Key::Escape)) {
                answer = Some(false);
            }
        });
    });
    answer
}

/// What the password dialog decided.
enum Typed {
    Connect(Zeroizing<String>),
    Cancel,
}

/// The user's decision, or `None` while the dialog stays open.
fn password(ctx: &egui::Context, user: &str, host: &str, field: &mut SecretText) -> Option<Typed> {
    let mut submit = None;
    dialog("Password").show(ctx, |ui| {
        ui.label(format!("Password for {user}@{host}"));
        let response = secret_field(ui, field, "Password");
        response.request_focus();
        ui.horizontal(|ui| {
            let enter = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if ui.button("Connect").clicked() || enter {
                submit = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|input| input.key_pressed(Key::Escape)) {
                submit = Some(false);
            }
        });
    });
    let connect = submit?;
    let typed = if connect {
        Typed::Connect(Zeroizing::new(field.expose().to_owned()))
    } else {
        Typed::Cancel
    };
    // The field's copy is wiped either way; the connection gets its own zeroizing copy.
    egui::TextBuffer::clear(field);
    Some(typed)
}
