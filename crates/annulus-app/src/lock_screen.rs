//! Creating and unlocking the vault (F-SYNC-001, F-SEC-001): the master password, the
//! recovery key shown once at setup, and unlocking with either.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::secret::{SecretText, secret_field};
use annulus_ui::theme;
use annulus_vault::crypto::KdfParams;
use annulus_vault::vault::{Vault, VaultError};
use zeroize::Zeroizing;

/// Shortest master password accepted.
const MIN_PASSWORD: usize = 10;

/// Where the lock screen is.
pub enum LockScreen {
    /// No vault yet: choose a master password.
    Setup {
        /// The new password.
        password: SecretText,
        /// Typed again.
        confirm: SecretText,
        /// Why the last attempt failed.
        error: Option<String>,
    },
    /// The vault exists and is created or unlocked once the recovery key is confirmed.
    Recovery {
        /// The new vault, handed over once the user confirms.
        vault: Option<Vault>,
        /// The recovery key, shown this once.
        key: Zeroizing<String>,
        /// The user's re-typed first group, proving it was written down.
        typed: String,
        /// Why confirming failed.
        error: Option<String>,
    },
    /// A vault exists: unlock it.
    Locked {
        /// The master password, or the recovery key when `with_recovery` is set.
        secret: SecretText,
        /// Unlocking with the recovery key instead of the password.
        with_recovery: bool,
        /// Why the last attempt failed.
        error: Option<String>,
    },
}

impl LockScreen {
    /// The screen for the vault at `path`: setup if there is none, unlock otherwise.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        if Vault::exists(path) {
            LockScreen::Locked {
                secret: SecretText::new(),
                with_recovery: false,
                error: None,
            }
        } else {
            LockScreen::Setup {
                password: SecretText::new(),
                confirm: SecretText::new(),
                error: None,
            }
        }
    }

    /// Draws the screen; returns the vault once it is created or unlocked.
    pub fn ui(&mut self, ui: &mut Ui, path: &PathBuf, palette: &Palette) -> Option<Vault> {
        let mut opened = None;
        let mut next = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 6.0);
            ui.set_max_width(420.0);
            match self {
                LockScreen::Setup { password, confirm, error } => {
                    heading(ui, palette, "Create your vault", "Hosts and keys are encrypted with a master password. It never leaves this device.");
                    secret_field(ui, password, "Master password (10+ characters)");
                    let confirmed = secret_field(ui, confirm, "Type it again");
                    show_error(ui, palette, error.as_deref());
                    let enter = confirmed.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.button("Create vault").clicked() || enter {
                        if password.expose().chars().count() < MIN_PASSWORD {
                            *error = Some(format!("Use at least {MIN_PASSWORD} characters."));
                        } else if password.expose() != confirm.expose() {
                            *error = Some("The two passwords differ.".into());
                        } else {
                            match Vault::create(path.clone(), password.expose(), KdfParams::DEFAULT) {
                                Ok((vault, key)) => next = Some(LockScreen::Recovery { vault: Some(vault), key, typed: String::new(), error: None }),
                                Err(failure) => *error = Some(format!("Could not create the vault: {failure}")),
                            }
                        }
                    }
                }
                LockScreen::Recovery { vault, key, typed, error } => {
                    heading(ui, palette, "Save your recovery key", "If you forget the master password, this key is the only way back in. Write it down or store it in a password manager. It is shown only now.");
                    ui.add_space(space::S);
                    ui.label(RichText::new(key.as_str()).monospace().size(text::HEADING));
                    ui.add_space(space::M);
                    ui.label("Type its first group to confirm you saved it:");
                    ui.add(egui::TextEdit::singleline(typed).desired_width(120.0));
                    show_error(ui, palette, error.as_deref());
                    if ui.button("I saved it, open the vault").clicked() {
                        let first = key.split('-').next().unwrap_or_default();
                        if typed.trim().eq_ignore_ascii_case(first) {
                            opened = vault.take();
                        } else {
                            *error = Some("That does not match the first group.".into());
                        }
                    }
                }
                LockScreen::Locked { secret, with_recovery, error } => {
                    let (title, hint) = if *with_recovery {
                        ("Unlock with your recovery key", "Recovery key")
                    } else {
                        ("Unlock your vault", "Master password")
                    };
                    heading(ui, palette, title, "Local terminals work while the vault is locked.");
                    let field = secret_field(ui, secret, hint);
                    show_error(ui, palette, error.as_deref());
                    let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.button("Unlock").clicked() || enter {
                        let result = if *with_recovery { Vault::recover(path.clone(), secret.expose()) } else { Vault::unlock(path.clone(), secret.expose()) };
                        egui::TextBuffer::clear(secret);
                        match result {
                            Ok(vault) => opened = Some(vault),
                            Err(VaultError::WrongPassword) => {
                                *error = Some(if *with_recovery { "That recovery key does not open this vault." } else { "Wrong password." }.into());
                            }
                            Err(failure) => *error = Some(format!("{failure}. The vault file was not changed.")),
                        }
                    }
                    let switch = if *with_recovery { "Use the master password" } else { "Forgot the password? Use the recovery key" };
                    if ui.link(switch).clicked() {
                        *with_recovery = !*with_recovery;
                        *error = None;
                    }
                }
            }
        });
        if let Some(next) = next {
            *self = next;
        }
        opened
    }
}

fn heading(ui: &mut Ui, palette: &Palette, title: &str, detail: &str) {
    ui.label(
        RichText::new(title)
            .size(text::TITLE)
            .color(theme::color(palette.text_primary)),
    );
    ui.add_space(space::S);
    ui.label(RichText::new(detail).color(theme::color(palette.text_secondary)));
    ui.add_space(space::L);
}

fn show_error(ui: &mut Ui, palette: &Palette, error: Option<&str>) {
    if let Some(error) = error {
        ui.colored_label(theme::color(palette.status_error), error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_vault_starts_with_setup_and_an_existing_one_with_unlock() {
        let dir = std::env::temp_dir().join(format!("annulus-lock-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("vault.anv");
        assert!(matches!(LockScreen::new(&path), LockScreen::Setup { .. }));
        let cheap = KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        Vault::create(path.clone(), "long enough pw", cheap).unwrap();
        assert!(matches!(
            LockScreen::new(&path),
            LockScreen::Locked {
                with_recovery: false,
                ..
            }
        ));
    }
}
