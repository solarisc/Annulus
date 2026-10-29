//! Changing the master password in Settings (F-SYNC-009). The Argon2id work runs on a
//! worker thread. When the vault is signed in to an account, the account is updated first
//! (other devices are signed out and sign in again with the new password), and only then
//! the local vault, so the two never disagree.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Ui};
use annulus_config::tokens::{Palette, space};
use annulus_sync::account;
use annulus_sync::http::HttpServer;
use annulus_ui::secret::{SecretText, secret_field};
use annulus_ui::theme;

use crate::hosts_screen::HostsScreen;

/// Shortest master password accepted (as at setup).
const MIN_PASSWORD: usize = 10;

/// The password form's state.
#[derive(Default)]
pub struct PasswordScreen {
    current: SecretText,
    new: SecretText,
    confirm: SecretText,
    work: Option<Receiver<Result<Vec<u8>, String>>>,
    message: Option<(bool, String)>,
}

/// Why the new password cannot be used, if it cannot.
fn problem(new: &str, confirm: &str) -> Option<&'static str> {
    if new.chars().count() < MIN_PASSWORD {
        Some("Use at least 10 characters.")
    } else if new != confirm {
        Some("The two new passwords differ.")
    } else {
        None
    }
}

impl PasswordScreen {
    /// Draws the form.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, hosts: &mut HostsScreen) {
        if let Some(rx) = &self.work
            && let Ok(result) = rx.try_recv()
        {
            self.work = None;
            self.message = Some(match result {
                Ok(key_file) => match hosts.vault_mut().set_key_file(&key_file) {
                    Ok(()) => (
                        false,
                        "Master password changed. The recovery key still works.".into(),
                    ),
                    Err(error) => (
                        true,
                        format!(
                            "The account has the new password, but this vault could not be saved: {error}. Sign in again with the new password."
                        ),
                    ),
                },
                Err(message) => (true, message),
            });
        }
        ui.heading("Master password");
        ui.add_space(space::S);
        egui::Grid::new("password-fields")
            .num_columns(2)
            .spacing([space::M, space::S])
            .show(ui, |ui| {
                ui.label("Current");
                secret_field(ui, &mut self.current, "Current master password");
                ui.end_row();
                ui.label("New");
                secret_field(ui, &mut self.new, "10+ characters");
                ui.end_row();
                ui.label("New again");
                secret_field(ui, &mut self.confirm, "Type it again");
                ui.end_row();
            });
        let ready = self.work.is_none() && !self.current.is_empty() && !self.new.is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("Change password"))
            .clicked()
        {
            match problem(self.new.expose(), self.confirm.expose()) {
                Some(problem) => self.message = Some((true, problem.into())),
                None => self.start(ui.ctx(), hosts),
            }
        }
        if self.work.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Changing the password\u{2026}");
            });
        }
        if let Some((error, message)) = &self.message {
            let color = if *error {
                palette.status_error
            } else {
                palette.text_secondary
            };
            ui.colored_label(theme::color(color), message);
        }
    }

    fn start(&mut self, ctx: &egui::Context, hosts: &HostsScreen) {
        let current = zeroize::Zeroizing::new(self.current.expose().to_owned());
        let new = zeroize::Zeroizing::new(self.new.expose().to_owned());
        for field in [&mut self.current, &mut self.new, &mut self.confirm] {
            egui::TextBuffer::clear(field);
        }
        let kit = hosts.vault().password_kit();
        let account = hosts.vault().sync_state().account.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = kit
                .rewrap(&current, &new)
                .map_err(|error| match error {
                    annulus_vault::vault::VaultError::WrongPassword => {
                        "The current password is wrong.".to_owned()
                    }
                    other => other.to_string(),
                })
                .and_then(|key_file| {
                    if let Some(account) = account {
                        let mut http = HttpServer::new(
                            &account.server,
                            Some(account.token.expose().to_owned()),
                        )
                        .map_err(|error| error.to_string())?;
                        account::change_password(
                            &mut http,
                            &account::default_ksf(),
                            &new,
                            key_file.clone(),
                        )
                        .map_err(|error| {
                            format!("The account could not be updated, so nothing changed: {error}")
                        })?;
                    }
                    Ok(key_file)
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.work = Some(rx);
        self.message = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_password_must_be_long_and_typed_twice() {
        assert!(problem("short", "short").is_some());
        assert!(problem("long enough pw", "long enough pX").is_some());
        assert_eq!(problem("long enough pw", "long enough pw"), None);
    }
}
