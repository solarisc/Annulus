//! The Keychain screen (F-KEY-001…003): SSH keys kept in the vault. Generate an Ed25519
//! key, import one by pasting it (with its passphrase if it has one), copy a public key for
//! `authorized_keys`, install it on a saved host (F-KEY-004), and delete keys. Hosts pick
//! a key in the host editor.

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ssh::keys::{self, KeyPair};
use annulus_ui::secret::{SecretText, secret_area, secret_field};
use annulus_ui::theme;
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::vault::Vault;

use crate::hosts_screen::HostsScreen;
use crate::job::{Job, Poll};
use crate::ssh_task::{Outcome, SshTask};

/// What the form at the top is doing.
#[derive(Default)]
enum Form {
    #[default]
    Closed,
    Generate {
        label: String,
        kind: keys::KeyKind,
        /// Making the key (RSA takes seconds).
        working: Option<Job<Result<KeyPair, keys::KeyError>>>,
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
    install: Option<Install>,
}

/// Installing a key's public half on a saved host (F-KEY-004).
struct Install {
    key_id: u64,
    host_id: Option<u64>,
    task: Option<SshTask>,
}

/// What an install's outcome means: done, or why not.
fn install_result(outcome: Outcome) -> Result<(), String> {
    let exec = outcome?;
    if exec.status == Some(0) {
        return Ok(());
    }
    let output = String::from_utf8_lossy(&exec.output).trim().to_owned();
    Err(if output.is_empty() {
        format!(
            "The host could not add the key (exit status {}).",
            exec.status
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        )
    } else {
        format!("The host could not add the key: {output}")
    })
}

/// The key as stored, from a generated or imported pair.
pub(crate) fn stored(vault: &Vault, label: &str, pair: &KeyPair) -> SshKey {
    SshKey {
        id: vault.next_key_id(),
        label: label.trim().to_owned(),
        private: Secret::new(pair.private.as_str()),
        public: pair.public.clone(),
        fingerprint: pair.fingerprint.clone(),
    }
}

/// The default comment of a generated key: `user@computer`.
pub(crate) fn comment() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "annulus".into());
    let computer = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "device".into());
    format!("{user}@{computer}")
}

/// Sets the host to log in with `key` after it was installed there; the message to show.
fn link_key(vault: &mut Vault, host_id: Option<u64>, key: &SshKey) -> String {
    let Some(mut host) =
        host_id.and_then(|id| vault.hosts().iter().find(|host| host.id == id).cloned())
    else {
        return format!("Installed {}.", key.label);
    };
    host.key_id = Some(key.id);
    let label = host.label.clone();
    match vault.upsert_host(host) {
        Ok(()) => format!(
            "Installed {} on {label}; it now logs in with this key.",
            key.label
        ),
        Err(error) => format!(
            "Installed {} on {label}, but could not save the host: {error}",
            key.label
        ),
    }
}

impl KeychainScreen {
    /// Draws the screen.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette, hosts: &mut HostsScreen) {
        self.install_ui(ui, hosts);
        let vault = hosts.vault_mut();
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            if ui.button("Generate key").clicked() {
                self.form = Form::Generate {
                    label: String::new(),
                    kind: keys::KeyKind::Ed25519,
                    working: None,
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

    /// The "Install on host" row: pick a host, then connect and add the key there. On
    /// success the host is set to log in with the key.
    fn install_ui(&mut self, ui: &mut Ui, hosts: &mut HostsScreen) {
        let Some(install) = &mut self.install else {
            return;
        };
        let Some(key) = hosts
            .vault()
            .ssh_keys()
            .iter()
            .find(|key| key.id == install.key_id)
            .cloned()
        else {
            self.install = None;
            return;
        };
        if let Some(task) = &mut install.task {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Installing {}\u{2026}", key.label));
            });
            if let Some(outcome) = task.poll(ui.ctx()) {
                let host_id = install.host_id;
                self.install = None;
                self.message = Some(match install_result(outcome) {
                    Ok(()) => (false, link_key(hosts.vault_mut(), host_id, &key)),
                    Err(reason) => (true, reason),
                });
            }
            return;
        }
        let choices: Vec<(u64, String)> = hosts
            .vault()
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let (mut start, mut cancel) = (false, false);
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.label(format!("Install {} on", key.label));
            let shown = install
                .host_id
                .and_then(|id| choices.iter().find(|(choice, _)| *choice == id))
                .map_or("Choose a host", |(_, label)| label.as_str());
            egui::ComboBox::from_id_salt("install-host")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (id, label) in &choices {
                        ui.selectable_value(&mut install.host_id, Some(*id), label);
                    }
                });
            start = ui
                .add_enabled(install.host_id.is_some(), egui::Button::new("Install"))
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
        let host = install
            .host_id
            .and_then(|id| hosts.vault().hosts().iter().find(|host| host.id == id))
            .cloned();
        if cancel {
            self.install = None;
        } else if start && let Some(host) = host {
            let ctx = ui.ctx().clone();
            install.task = Some(SshTask::exec(
                hosts.connect_host(&host),
                keys::install_command(&key.public),
                move || ctx.request_repaint(),
            ));
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, vault: &mut Vault) {
        let mut done = None;
        let mut cancel = false;
        match &mut self.form {
            Form::Closed => return,
            Form::Generate {
                label,
                kind,
                working,
            } => {
                ui.add_space(space::S);
                if let Some(job) = working {
                    match job.poll() {
                        Poll::Running => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("Making a {} key\u{2026}", kind.name()));
                            });
                            return;
                        }
                        Poll::Done(result) => {
                            done = Some(result.map(|pair| stored(vault, label, &pair)));
                        }
                        Poll::Lost => done = Some(Err(keys::KeyError::Generate)),
                    }
                    *working = None;
                }
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
                        let (kind, comment) = (*kind, comment());
                        *working =
                            Some(Job::start(ui.ctx(), move || keys::generate(kind, &comment)));
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
                    if ui
                        .button("Install on host\u{2026}")
                        .on_hover_text("Add the public key to a saved host's authorized_keys")
                        .clicked()
                    {
                        self.install = Some(Install {
                            key_id: key.id,
                            host_id: None,
                            task: None,
                        });
                        self.message = None;
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
    fn an_install_succeeds_only_on_exit_status_zero() {
        let exec = |status, output: &[u8]| {
            Ok(annulus_ssh::Exec {
                status,
                output: output.to_vec(),
            })
        };
        assert_eq!(install_result(exec(Some(0), b"")), Ok(()));
        assert_eq!(
            install_result(exec(Some(1), b"touch: Read-only file system\n")),
            Err("The host could not add the key: touch: Read-only file system".into())
        );
        assert_eq!(
            install_result(exec(None, b"")),
            Err("The host could not add the key (exit status unknown).".into())
        );
        assert_eq!(install_result(Err("refused".into())), Err("refused".into()));
    }

    fn vault(name: &str) -> Vault {
        let dir =
            std::env::temp_dir().join(format!("annulus-keychain-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Vault::create(
            dir.join("vault.anv"),
            "pw",
            annulus_vault::crypto::KdfParams {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
        )
        .unwrap()
        .0
    }

    #[test]
    fn an_installed_key_becomes_the_hosts_key() {
        let mut vault = vault("link");
        let pair = keys::generate(keys::KeyKind::Ed25519, "me@test").unwrap();
        let key = stored(&vault, "work", &pair);
        vault.upsert_key(key.clone()).unwrap();
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.1".into());
        vault.upsert_host(host.clone()).unwrap();
        assert_eq!(
            link_key(&mut vault, Some(host.id), &key),
            "Installed work on web; it now logs in with this key."
        );
        assert_eq!(vault.hosts()[0].key_id, Some(key.id));
        assert_eq!(link_key(&mut vault, Some(999), &key), "Installed work.");
    }

    #[test]
    fn a_generated_key_is_stored_with_its_public_half_and_a_fresh_id() {
        let mut vault = vault("generate");
        let pair = keys::generate(keys::KeyKind::Ed25519, "me@test").unwrap();
        let key = stored(&vault, "  work  ", &pair);
        assert_eq!(key.label, "work");
        assert_eq!(key.public, pair.public);
        vault.upsert_key(key).unwrap();
        assert_eq!(stored(&vault, "next", &pair).id, 2);
    }
}
