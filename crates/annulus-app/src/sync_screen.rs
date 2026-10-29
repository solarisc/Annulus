//! Account sync in Settings (F-SYNC-002…004, 008, 010): create an account on a self-hosted
//! server or sign in to one, sync in the background, see and sign out devices, and sign
//! out of the account. Network work and Argon2id run on worker threads; the vault is only
//! touched on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_proto::Device;
use annulus_sync::account;
use annulus_sync::engine::{Report, SyncError, sync};
use annulus_sync::http::HttpServer;
use annulus_sync::replica::VaultReplica;
use annulus_ui::secret::{SecretText, secret_field};
use annulus_ui::theme;
use annulus_vault::hosts::Secret;
use annulus_vault::sync::{SyncAccount, SyncCopy};
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use crate::hosts_screen::HostsScreen;

/// Seconds between background syncs while nothing changes locally.
const PERIOD: f64 = 60.0;

/// Seconds after a local edit before it is pushed.
const AFTER_EDIT: f64 = 3.0;

/// What a worker thread sends back.
enum Done {
    /// An account was created; the vault signs in to it.
    Registered(SyncAccount),
    /// Signed in; the new vault replaces the local one (whose records it adopted).
    /// The joined vault (written next to the live one), the backup, and the generation of
    /// the local vault it merged.
    Joined(Box<Vault>, PathBuf, u64),
    /// A sync round finished.
    Synced(Box<SyncCopy>, Result<Report, SyncError>),
    /// The device list.
    Devices(Result<Vec<Device>, SyncError>),
    /// A step failed before anything changed.
    Failed(String),
}

/// The account part of Settings, kept while the vault is unlocked.
pub struct SyncScreen {
    server: String,
    account: String,
    device: String,
    password: SecretText,
    work: Option<(String, Receiver<Done>)>,
    message: Option<(bool, String)>,
    devices: Vec<Device>,
    /// When the last round started, in the clock of `Pace::now`.
    last_round: f64,
    /// When a local change was first seen unpushed.
    dirty_since: Option<f64>,
}

impl Default for SyncScreen {
    fn default() -> Self {
        let device = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "This computer".to_owned());
        Self {
            server: String::new(),
            account: String::new(),
            device,
            password: SecretText::new(),
            work: None,
            message: None,
            devices: Vec::new(),
            last_round: f64::NEG_INFINITY,
            dirty_since: None,
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
fn ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

impl SyncScreen {
    fn start(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        job: impl FnOnce() -> Done + Send + 'static,
    ) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.work = Some((label.to_owned(), rx));
    }

    /// Runs background syncs and applies finished work; call every frame while unlocked.
    /// Returns true when the vault was replaced (after signing in), so running port
    /// forwards, whose ids changed, are stopped.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        hosts: &mut HostsScreen,
        vault_path: &Path,
    ) -> bool {
        let mut replaced = false;
        if let Some((_, rx)) = &self.work
            && let Ok(done) = rx.try_recv()
        {
            self.work = None;
            replaced = self.finish(done, hosts, vault_path);
        }
        let state = hosts.vault().sync_state();
        let Some(signed_in) = state.account.clone() else {
            return replaced;
        };
        let now = ctx.input(|input| input.time);
        if state.pending_count() == 0 {
            self.dirty_since = None;
        } else if self.dirty_since.is_none() {
            self.dirty_since = Some(now);
        }
        let due = now - self.last_round >= PERIOD
            || self
                .dirty_since
                .is_some_and(|since| now - since >= AFTER_EDIT);
        if due && self.work.is_none() {
            self.last_round = now;
            self.dirty_since = None;
            let copy = hosts.vault().sync_copy();
            self.start(ctx, "Syncing", move || run_round(&signed_in, copy));
        }
        // Wake up for the next round only (idle wakeups, P-06).
        let next = self.dirty_since.map_or(self.last_round + PERIOD, |since| {
            (since + AFTER_EDIT).min(self.last_round + PERIOD)
        });
        ctx.request_repaint_after(Duration::from_secs_f64((next - now).clamp(0.1, PERIOD)));
        replaced
    }

    /// Applies finished work; true when the vault was replaced.
    fn finish(&mut self, done: Done, hosts: &mut HostsScreen, vault_path: &Path) -> bool {
        match done {
            Done::Registered(account) => {
                self.message = Some(match hosts.vault_mut().set_sync_account(Some(account)) {
                    Ok(()) => (false, "Account created. This device is signed in.".into()),
                    Err(error) => (true, format!("Could not save the account: {error}")),
                });
                self.last_round = f64::NEG_INFINITY;
            }
            Done::Joined(vault, backup, generation) => {
                let mut vault = *vault;
                if hosts.vault().generation() != generation {
                    // Edited while signing in: the merge would miss that edit.
                    let _ = std::fs::remove_file(vault.path());
                    self.message = Some((
                        true,
                        "Hosts changed while signing in, so nothing was replaced. Sign in again."
                            .into(),
                    ));
                    return false;
                }
                if let Err(error) = vault.move_to(vault_path) {
                    self.message = Some((
                        true,
                        format!("Could not put the account's vault in place: {error}"),
                    ));
                    return false;
                }
                hosts.replace_vault(vault);
                self.message = Some((
                    false,
                    format!(
                        "Signed in, and this device's hosts are merged into the account. From now on the vault opens with the account's master password and recovery key. The old vault file is kept at {}.",
                        backup.display()
                    ),
                ));
                self.last_round = f64::NEG_INFINITY;
                return true;
            }
            Done::Synced(copy, result) => match result {
                Ok(report) => {
                    let mut copy = *copy;
                    copy.mark_synced();
                    // A local edit since the snapshot: drop this result, the next round redoes it.
                    let _ = hosts.vault_mut().commit_sync(copy);
                    self.message = (!report.conflicts.is_empty()).then(|| {
                        (
                            false,
                            format!(
                                "{} item(s) were edited on two devices; the newer edit was kept and the other is in the server's history.",
                                report.conflicts.len()
                            ),
                        )
                    });
                }
                Err(SyncError::Unauthorized) => {
                    let _ = hosts.vault_mut().set_sync_account(None);
                    self.message = Some((
                        true,
                        "This device was signed out of the account. Everything on it is kept."
                            .into(),
                    ));
                }
                Err(error) => {
                    self.message = Some((
                        true,
                        format!("Sync failed: {error}. It retries automatically."),
                    ));
                }
            },
            Done::Devices(result) => match result {
                Ok(devices) => self.devices = devices,
                Err(error) => self.message = Some((true, error.to_string())),
            },
            Done::Failed(message) => self.message = Some((true, message)),
        }
        false
    }

    /// Draws the section.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        hosts: &mut HostsScreen,
        vault_path: &Path,
    ) {
        ui.heading("Sync");
        ui.add_space(space::S);
        let state = hosts.vault().sync_state().clone();
        if let Some(account) = &state.account {
            self.signed_in_ui(
                ui,
                palette,
                hosts,
                account,
                state.last_synced,
                state.pending_count(),
            );
        } else {
            self.sign_in_ui(ui, palette, hosts, vault_path);
        }
        if let Some((label, _)) = &self.work {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{label}\u{2026}"));
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

    fn sign_in_ui(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        hosts: &mut HostsScreen,
        vault_path: &Path,
    ) {
        ui.label(
            RichText::new("Keep hosts and rules in sync through a server you run yourself. Everything is encrypted on this device with your master password; the server cannot read it.")
                .size(text::SECONDARY)
                .color(theme::color(palette.text_secondary)),
        );
        ui.add_space(space::S);
        egui::Grid::new("sync-fields")
            .num_columns(2)
            .spacing([space::M, space::S])
            .show(ui, |ui| {
                ui.label("Server");
                ui.add(
                    egui::TextEdit::singleline(&mut self.server)
                        .hint_text("https://sync.example.com")
                        .desired_width(280.0),
                );
                ui.end_row();
                ui.label("Account");
                ui.add(
                    egui::TextEdit::singleline(&mut self.account)
                        .hint_text("you@example.com")
                        .desired_width(280.0),
                );
                ui.end_row();
                ui.label("This device");
                ui.add(egui::TextEdit::singleline(&mut self.device).desired_width(280.0));
                ui.end_row();
                ui.label("Master password");
                secret_field(
                    ui,
                    &mut self.password,
                    "The password that unlocks this vault",
                );
                ui.end_row();
            });
        ui.add_space(space::S);
        let ready = self.work.is_none()
            && !self.server.trim().is_empty()
            && !self.account.trim().is_empty()
            && !self.password.expose().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("Create account"))
                .clicked()
            {
                self.create(ui.ctx(), hosts);
            }
            if ui
                .add_enabled(ready, egui::Button::new("Sign in"))
                .clicked()
            {
                self.join(ui.ctx(), hosts, vault_path);
            }
        });
        ui.label(
            RichText::new("Signing in on a device that already has hosts merges them into the account; the old vault file is kept as a backup.")
                .size(text::SECONDARY)
                .color(theme::color(palette.text_secondary)),
        );
    }

    fn take_password(&mut self) -> Zeroizing<String> {
        let password = Zeroizing::new(self.password.expose().to_owned());
        egui::TextBuffer::clear(&mut self.password);
        password
    }

    fn create(&mut self, ctx: &egui::Context, hosts: &HostsScreen) {
        let password = self.take_password();
        let (server, name, device) = (
            self.server.trim().to_owned(),
            self.account.trim().to_owned(),
            self.device.trim().to_owned(),
        );
        let key_file = hosts.vault().key_file_bytes();
        self.message = None;
        self.start(ctx, "Creating the account", move || {
            // The account password must be the vault's, so other devices can unwrap the key.
            if !Vault::password_opens(&key_file, &password) {
                return Done::Failed("That is not this vault's master password.".into());
            }
            let result = HttpServer::new(&server, None).and_then(|mut http| {
                account::register(
                    &mut http,
                    &account::default_ksf(),
                    &name,
                    &password,
                    key_file,
                    &device,
                )
            });
            match result {
                Ok(session) => Done::Registered(SyncAccount {
                    server,
                    name,
                    device: session.device,
                    token: Secret::new(session.token),
                }),
                Err(error) => Done::Failed(format!("Could not create the account: {error}")),
            }
        });
    }

    fn join(&mut self, ctx: &egui::Context, hosts: &HostsScreen, vault_path: &Path) {
        let password = self.take_password();
        let (server, name, device) = (
            self.server.trim().to_owned(),
            self.account.trim().to_owned(),
            self.device.trim().to_owned(),
        );
        let local_hosts = hosts.vault().hosts().to_vec();
        let local_forwards = hosts.vault().forwards().to_vec();
        let generation = hosts.vault().generation();
        let path = vault_path.to_path_buf();
        self.message = None;
        self.start(ctx, "Signing in", move || {
            let session = match HttpServer::new(&server, None).and_then(|mut http| {
                account::login(
                    &mut http,
                    &account::default_ksf(),
                    &name,
                    &password,
                    &device,
                )
            }) {
                Ok(session) => session,
                Err(error) => return Done::Failed(format!("Could not sign in: {error}")),
            };
            // Keep the current vault file before anything replaces it.
            let backup = path.with_extension(format!("before-sync-{}.anv", now_secs()));
            if let Err(error) = std::fs::copy(&path, &backup) {
                return Done::Failed(format!(
                    "Could not back up the vault, so nothing was changed: {error}"
                ));
            }
            let account = SyncAccount {
                server,
                name,
                device: session.device,
                token: Secret::new(session.token),
            };
            // Built beside the live vault; the UI thread moves it in place.
            let joining = path.with_extension("joining.anv");
            let mut vault = match Vault::join(&joining, &session.wrapped_keys, &password, account) {
                Ok(vault) => vault,
                Err(error) => {
                    return Done::Failed(format!("Could not open the account's vault: {error}"));
                }
            };
            if let Err(error) = vault.adopt(&local_hosts, &local_forwards) {
                let _ = std::fs::remove_file(&joining);
                return Done::Failed(format!("Could not merge this device's hosts: {error}"));
            }
            Done::Joined(Box::new(vault), backup, generation)
        });
    }

    fn signed_in_ui(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        hosts: &mut HostsScreen,
        account: &SyncAccount,
        last_synced: u64,
        pending: usize,
    ) {
        ui.label(format!(
            "Signed in as {} on {}",
            account.name, account.server
        ));
        let status = match (last_synced, pending) {
            (0, _) => "Not synced yet".to_owned(),
            (then, 0) => format!("Up to date, synced {}", ago(then, now_secs())),
            (then, n) => format!(
                "{n} change(s) waiting, last synced {}",
                ago(then, now_secs())
            ),
        };
        ui.label(RichText::new(status).color(theme::color(palette.text_secondary)));
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.work.is_none(), egui::Button::new("Sync now"))
                .clicked()
            {
                self.last_round = f64::NEG_INFINITY;
            }
            if ui
                .add_enabled(self.work.is_none(), egui::Button::new("Show devices"))
                .clicked()
            {
                let (server, token) = (account.server.clone(), account.token.expose().to_owned());
                self.start(ui.ctx(), "Loading devices", move || {
                    Done::Devices(
                        HttpServer::new(&server, Some(token))
                            .and_then(|http| http.devices())
                            .map(|list| list.devices),
                    )
                });
            }
            if ui.button("Sign out").clicked() {
                self.devices.clear();
                self.message = Some(match hosts.vault_mut().set_sync_account(None) {
                    Ok(()) => (
                        false,
                        "Signed out. Everything on this device is kept.".into(),
                    ),
                    Err(error) => (true, error.to_string()),
                });
            }
        });
        let mut revoke = None;
        for device in &self.devices {
            ui.horizontal(|ui| {
                let label = if device.current {
                    format!("{} (this device)", device.name)
                } else {
                    device.name.clone()
                };
                ui.label(label);
                ui.label(
                    RichText::new(format!("seen {}", ago(device.last_seen, now_secs())))
                        .size(text::SECONDARY)
                        .color(theme::color(palette.text_secondary)),
                );
                if !device.current && ui.small_button("Sign out").clicked() {
                    revoke = Some(device.id);
                }
            });
        }
        if let Some(id) = revoke {
            let (server, token) = (account.server.clone(), account.token.expose().to_owned());
            self.start(ui.ctx(), "Signing the device out", move || {
                let http = match HttpServer::new(&server, Some(token)) {
                    Ok(http) => http,
                    Err(error) => return Done::Devices(Err(error)),
                };
                Done::Devices(
                    http.revoke(id)
                        .and_then(|()| http.devices())
                        .map(|list| list.devices),
                )
            });
        }
    }
}

/// One sync round on a worker thread.
fn run_round(account: &SyncAccount, copy: SyncCopy) -> Done {
    let mut replica = VaultReplica(copy);
    let result = HttpServer::new(&account.server, Some(account.token.expose().to_owned()))
        .and_then(|mut http| sync(&mut http, &mut replica));
    Done::Synced(Box::new(replica.0), result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_synced_reads_in_plain_words() {
        assert_eq!(ago(100, 130), "just now");
        assert_eq!(ago(0, 5 * 60), "5 min ago");
        assert_eq!(ago(0, 3 * 3600), "3 h ago");
        assert_eq!(ago(0, 2 * 86_400), "2 days ago");
        assert_eq!(
            ago(200, 100),
            "just now",
            "a clock behind the server is not negative"
        );
    }
}
