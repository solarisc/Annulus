//! The Keychain screen (F-KEY-001…003): SSH keys kept in the vault. Generate an Ed25519
//! key, import one by pasting it (with its passphrase if it has one), copy a public key for
//! `authorized_keys`, and delete keys. Hosts pick a key in the host editor.

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ssh::keys::{self, KeyPair};
use annulus_ui::secret::{SecretText, secret_area, secret_field};
use annulus_ui::theme;
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::vault::Vault;

/// What the form at the top is doing.
#[derive(Default)]
enum Form {
    #[default]
    Closed,
    Generate {
        label: String,
        kind: keys::KeyKind,
    },
    Import {
        label: String,
        private: SecretText,
        passphrase: SecretText,
    },
}

/// The Keychain screen's state.
#[derive(Default)]
pub struct KeychainScreen {
    form: Form,
    message: Option<(bool, String)>,
    confirm_delete: Option<u64>,
}

/// The key as stored, from a generated or imported pair.
fn stored(vault: &Vault, label: &str, pair: &KeyPair) -> SshKey {
    SshKey {
        id: vault.next_key_id(),
        label: label.trim().to_owned(),
        private: Secret::new(pair.private.as_str()),
        public: pair.public.clone(),
        fingerprint: pair.fingerprint.clone(),
    }
}

/// The default comment of a generated key: `user@computer`.
fn comment() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "annulus".into());
    let computer = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "device".into());
    format!("{user}@{computer}")
}

impl KeychainScreen {
    /// Draws the screen.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault) {
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            if ui.button("Generate key").clicked() {
                self.form = Form::Generate {
                    label: String::new(),
                    kind: keys::KeyKind::Ed25519,
                };
                self.message = None;
            }
            if ui.button("Import key").clicked() {
                self.form = Form::Import {
                    label: String::new(),
                    private: SecretText::new(),
                    passphrase: SecretText::new(),
                };
                self.message = None;
            }
        });
        self.form_ui(ui, vault);
        if let Some((error, message)) = &self.message {
            let color = if *error {
                palette.status_error
            } else {
                palette.text_secondary
            };
            ui.colored_label(theme::color(color), message);
        }
        ui.add_space(space::M);
        if vault.ssh_keys().is_empty() {
            ui.label(
                RichText::new("No keys yet. Generate one, or import a key you already use; it is encrypted in the vault and synced with your account.")
                    .color(theme::color(palette.text_secondary)),
            );
            return;
        }
        self.list(ui, palette, vault);
    }

    fn form_ui(&mut self, ui: &mut Ui, vault: &mut Vault) {
        let mut done = None;
        let mut cancel = false;
        match &mut self.form {
            Form::Closed => return,
            Form::Generate { label, kind } => {
                ui.add_space(space::S);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(label).hint_text("work laptop"));
                    egui::ComboBox::from_label("Type")
                        .selected_text(kind.name())
                        .show_ui(ui, |ui| {
                            for choice in keys::KeyKind::ALL {
                                ui.selectable_value(kind, choice, choice.name());
                            }
                        });
                    let ready = !label.trim().is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Generate"))
                        .clicked()
                    {
                        done = Some(
                            keys::generate(*kind, &comment())
                                .map(|pair| stored(vault, label, &pair)),
                        );
                    }
                    cancel |= ui.button("Cancel").clicked();
                });
            }
            Form::Import {
                label,
                private,
                passphrase,
            } => {
                ui.add_space(space::S);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(label).hint_text("id_ed25519"));
                });
                secret_area(
                    ui,
                    private,
                    "Paste the private key (-----BEGIN … PRIVATE KEY-----)",
                    6,
                );
                ui.horizontal(|ui| {
                    ui.label("Passphrase");
                    secret_field(ui, passphrase, "Only if the key has one");
                });
                let ready = !label.trim().is_empty() && !private.expose().trim().is_empty();
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new("Import")).clicked() {
                        let result = keys::import(private.expose(), Some(passphrase.expose()));
                        done = Some(result.map(|pair| stored(vault, label, &pair)));
                    }
                    cancel |= ui.button("Cancel").clicked();
                });
            }
        }
        if cancel {
            // Dropping the form wipes a pasted key and passphrase.
            self.form = Form::Closed;
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
                                "Saved {label}. Copy its public key to the servers' authorized_keys."
                            ),
                        ),
                        Err(error) => (true, format!("Could not save the key: {error}")),
                    });
                    // Dropping the form wipes the pasted key and passphrase.
                    self.form = Form::Closed;
                }
                Err(error) => self.message = Some((true, error.to_string())),
            }
        }
    }

    fn list(&mut self, ui: &mut Ui, palette: &Palette, vault: &mut Vault) {
        let keys: Vec<SshKey> = vault.ssh_keys().to_vec();
        for key in &keys {
            let users = vault
                .hosts()
                .iter()
                .filter(|host| host.key_id == Some(key.id))
                .count();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(&key.label).size(text::BODY + 1.0).strong());
                    let algorithm = key.public.split_whitespace().next().unwrap_or_default();
                    ui.label(
                        RichText::new(format!(
                            "{algorithm}  {}  \u{b7}  used by {users} host(s)",
                            key.fingerprint
                        ))
                        .size(text::SECONDARY)
                        .color(theme::color(palette.text_secondary)),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.confirm_delete == Some(key.id) {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete = None;
                        }
                        if ui.button("Delete key").clicked() {
                            self.confirm_delete = None;
                            self.message = vault
                                .remove_key(key.id)
                                .err()
                                .map(|error| (true, error.to_string()));
                        }
                        return;
                    }
                    if ui.button("Delete").clicked() {
                        self.confirm_delete = Some(key.id);
                    }
                    if ui.button("Copy public key").clicked() {
                        ui.ctx().copy_text(key.public.clone());
                        self.message =
                            Some((false, format!("Copied the public key of {}.", key.label)));
                    }
                });
            });
            ui.separator();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_is_stored_with_its_public_half_and_a_fresh_id() {
        let dir = std::env::temp_dir().join(format!("annulus-keychain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut vault, _) = Vault::create(
            dir.join("vault.anv"),
            "pw",
            annulus_vault::crypto::KdfParams {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
        )
        .unwrap();
        let pair = keys::generate(keys::KeyKind::Ed25519, "me@test").unwrap();
        let key = stored(&vault, "  work  ", &pair);
        assert_eq!(key.label, "work");
        assert_eq!(key.public, pair.public);
        vault.upsert_key(key).unwrap();
        assert_eq!(stored(&vault, "next", &pair).id, 2);
    }
}
