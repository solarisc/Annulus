//! Creating and unlocking the vault (F-SYNC-001, F-SEC-001): the master password, the
//! recovery key shown once at setup, and unlocking with either. Argon2id takes a moment,
//! so creating and unlocking run on a background thread while the screen shows a spinner.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_ui::secret::{SecretText, secret_field};
use annulus_ui::theme;
use annulus_vault::crypto::KdfParams;
use annulus_vault::vault::{Vault, VaultError};
use zeroize::Zeroizing;

use crate::job::{Job, Poll};

/// Shortest master password accepted.
const MIN_PASSWORD: usize = 10;

/// A new vault and its recovery key, or why not.
type Created = Result<(Vault, Zeroizing<String>), VaultError>;

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
        /// Creating the vault.
        creating: Option<Job<Created>>,
    },
    /// The vault exists and is created or unlocked once the recovery key is confirmed.
    Recovery {
        /// The new vault, handed over once the user confirms.
        vault: Option<Box<Vault>>,
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
        /// Unlocking.
        opening: Option<Job<Result<Vault, VaultError>>>,
    },
}

/// Opens the vault at `path` with the password, or with the recovery key when
/// `with_recovery`.
fn open(path: PathBuf, secret: &str, with_recovery: bool) -> Result<Vault, VaultError> {
    if with_recovery {
        Vault::recover(path, secret)
    } else {
        Vault::unlock(path, secret)
    }
}

/// The message for a failed unlock.
fn unlock_error(failure: &VaultError, with_recovery: bool) -> String {
    match failure {
        VaultError::WrongPassword if with_recovery => {
            "That recovery key does not open this vault.".into()
        }
        VaultError::WrongPassword => "Wrong password.".into(),
        other => format!("{other}. The vault file was not changed."),
    }
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
                opening: None,
            }
        } else {
            LockScreen::Setup {
                password: SecretText::new(),
                confirm: SecretText::new(),
                error: None,
                creating: None,
            }
        }
    }

    /// Draws the screen; returns the vault once it is created or unlocked.
    pub fn ui(&mut self, ui: &mut Ui, path: &Path, palette: &Palette) -> Option<Vault> {
        let mut opened = None;
        let mut next = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 6.0);
            ui.set_max_width(420.0);
            match self {
                LockScreen::Setup { .. } => next = self.setup_ui(ui, path, palette),
                LockScreen::Recovery { .. } => opened = self.recovery_ui(ui, palette),
                LockScreen::Locked { .. } => opened = self.locked_ui(ui, path, palette),
            }
        });
        if let Some(next) = next {
            *self = next;
        }
        opened
    }

    fn setup_ui(&mut self, ui: &mut Ui, path: &Path, palette: &Palette) -> Option<LockScreen> {
        let LockScreen::Setup {
            password,
            confirm,
            error,
            creating,
        } = self
        else {
            return None;
        };
        heading(
            ui,
            palette,
            "Create your vault",
            "Hosts and keys are encrypted with a master password. It never leaves this device.",
        );
        if let Some(job) = creating {
            match job.poll() {
                Poll::Running => {
                    busy(ui, "Creating the vault\u{2026}");
                    return None;
                }
                Poll::Done(Ok((vault, key))) => {
                    return Some(LockScreen::Recovery {
                        vault: Some(Box::new(vault)),
                        key,
                        typed: String::new(),
                        error: None,
                    });
                }
                Poll::Done(Err(failure)) => {
                    *error = Some(format!("Could not create the vault: {failure}"));
                }
                Poll::Lost => *error = Some("Could not create the vault.".into()),
            }
            *creating = None;
        }
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
                let chosen = Zeroizing::new(password.expose().to_owned());
                let path = path.to_path_buf();
                *error = None;
                *creating = Some(Job::start(ui.ctx(), move || {
                    Vault::create(path, &chosen, KdfParams::DEFAULT)
                }));
            }
        }
        None
    }

    fn recovery_ui(&mut self, ui: &mut Ui, palette: &Palette) -> Option<Vault> {
        let LockScreen::Recovery {
            vault,
            key,
            typed,
            error,
        } = self
        else {
            return None;
        };
        heading(
            ui,
            palette,
            "Save your recovery key",
            "If you forget the master password, this key is the only way back in. Write it \
             down or store it in a password manager. It is shown only now.",
        );
        ui.add_space(space::S);
        ui.label(RichText::new(key.as_str()).monospace().size(text::HEADING));
        ui.add_space(space::M);
        ui.label("Type its first group to confirm you saved it:");
        ui.add(egui::TextEdit::singleline(typed).desired_width(120.0));
        show_error(ui, palette, error.as_deref());
        if ui.button("I saved it, open the vault").clicked() {
            let first = key.split('-').next().unwrap_or_default();
            if typed.trim().eq_ignore_ascii_case(first) {
                return vault.take().map(|vault| *vault);
            }
            *error = Some("That does not match the first group.".into());
        }
        None
    }

    fn locked_ui(&mut self, ui: &mut Ui, path: &Path, palette: &Palette) -> Option<Vault> {
        let LockScreen::Locked {
            secret,
            with_recovery,
            error,
            opening,
        } = self
        else {
            return None;
        };
        let (title, hint) = if *with_recovery {
            ("Unlock with your recovery key", "Recovery key")
        } else {
            ("Unlock your vault", "Master password")
        };
        heading(
            ui,
            palette,
            title,
            "Local terminals work while the vault is locked.",
        );
        if let Some(job) = opening {
            match job.poll() {
                Poll::Running => {
                    busy(ui, "Unlocking\u{2026}");
                    return None;
                }
                Poll::Done(Ok(vault)) => return Some(vault),
                Poll::Done(Err(failure)) => *error = Some(unlock_error(&failure, *with_recovery)),
                Poll::Lost => *error = Some("Could not unlock the vault.".into()),
            }
            *opening = None;
        }
        let field = secret_field(ui, secret, hint);
        show_error(ui, palette, error.as_deref());
        let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui.button("Unlock").clicked() || enter {
            let typed = Zeroizing::new(secret.expose().to_owned());
            egui::TextBuffer::clear(secret);
            let (path, recovery) = (path.to_path_buf(), *with_recovery);
            *error = None;
            *opening = Some(Job::start(ui.ctx(), move || open(path, &typed, recovery)));
        }
        let switch = if *with_recovery {
            "Use the master password"
        } else {
            "Forgot the password? Use the recovery key"
        };
        if ui.link(switch).clicked() {
            *with_recovery = !*with_recovery;
            *error = None;
        }
        None
    }
}

/// A spinner and what is happening, while a job runs.
fn busy(ui: &mut Ui, what: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(what);
    });
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

    #[test]
    fn unlocking_runs_on_another_thread_and_reports_a_wrong_password() {
        let dir = std::env::temp_dir().join(format!("annulus-lock-job-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("vault.anv");
        let cheap = KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        Vault::create(path.clone(), "long enough pw", cheap).unwrap();
        let ctx = egui::Context::default();
        let wait = |job: Job<Result<Vault, VaultError>>| loop {
            match job.poll() {
                Poll::Running => std::thread::sleep(std::time::Duration::from_millis(5)),
                Poll::Done(result) => return result,
                Poll::Lost => panic!("the thread died"),
            }
        };
        let (good, bad) = (path.clone(), path.clone());
        assert!(
            wait(Job::start(&ctx, move || open(
                good,
                "long enough pw",
                false
            )))
            .is_ok()
        );
        let Err(failure) = wait(Job::start(&ctx, move || open(bad, "nope", false))) else {
            panic!("a wrong password opened the vault");
        };
        assert_eq!(unlock_error(&failure, false), "Wrong password.");
        assert_eq!(
            unlock_error(&failure, true),
            "That recovery key does not open this vault."
        );
        let lost: Job<u8> = Job::start(&ctx, || panic!("a crash in the job"));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(matches!(lost.poll(), Poll::Lost));
    }
}
