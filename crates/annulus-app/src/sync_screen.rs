//! Account sync in Settings (F-SYNC-002…004, 008, 010): create an account on a self-hosted
//! server or sign in to one, sync in the background, see and sign out devices, and sign
//! out of the account. Network work and Argon2id run on worker threads; the vault is only
//! touched on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use annulus_proto::Device;
use annulus_sync::account;
use annulus_sync::engine::{Report, SyncError, restore as restore_item, sync};
use annulus_sync::http::HttpServer;
use annulus_sync::replica::VaultReplica;
use annulus_vault::hosts::Secret;
use annulus_vault::sync::{SyncAccount, SyncCopy};
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use crate::job::Pace;

/// Seconds between background syncs while nothing changes locally.
const PERIOD: f64 = 60.0;

/// Seconds after a local edit before it is pushed.
const AFTER_EDIT: f64 = 3.0;

/// Seconds after a failed round before the next try (the server may be back).
const RETRY: f64 = 15.0;

/// When the next round is due, in the clock of `Pace::now`: a minute after the last, a few
/// seconds after a local edit, and after a failure (`last` is then when it failed) 15 s
/// whatever is waiting, so an unreachable server is not tried back to back.
fn next_round(last: f64, dirty_since: Option<f64>, failed: bool) -> f64 {
    if failed {
        return last + RETRY;
    }
    dirty_since.map_or(last + PERIOD, |since| {
        (since + AFTER_EDIT).min(last + PERIOD)
    })
}

/// What a worker thread sends back.
pub(crate) enum Done {
    /// An account was created; the vault signs in to it.
    Registered(SyncAccount),
    /// Signed in; the new vault replaces the local one (whose records it adopted).
    /// The joined vault (written next to the live one), the backup, and the generation of
    /// the local vault it merged.
    Joined(Box<Vault>, PathBuf, u64),
    /// Signed in again to the account this vault already belongs to: its key file and the
    /// new device.
    Relinked(Vec<u8>, SyncAccount),
    /// A sync round finished.
    Synced(Box<SyncCopy>, Result<Report, SyncError>),
    /// The device list.
    Devices(Result<Vec<Device>, SyncError>),
    /// Deleted items that can be restored.
    Trash(Result<Vec<TrashItem>, SyncError>),
    /// A restore finished; the copy holds the restored item.
    Restored(Box<SyncCopy>, Result<(), SyncError>),
    /// A step failed before anything changed.
    Failed(String),
    /// The server's certificate is not from a public authority: this one (DER) waits for
    /// the user to compare its fingerprint and trust it (ADR-0018).
    Untrusted(Vec<u8>),
}

/// The account part of Settings, kept while the vault is unlocked.
pub struct SyncScreen {
    pub(crate) server: String,
    pub(crate) account: String,
    pub(crate) device: String,
    pub(crate) password: Zeroizing<String>,
    pub(crate) work: Option<(String, Receiver<Done>)>,
    pub(crate) message: Option<(bool, String)>,
    pub(crate) devices: Vec<Device>,
    pub(crate) trash: Vec<TrashItem>,
    /// When the last round started, in the clock of `Pace::now`.
    pub(crate) last_round: f64,
    /// When a local change was first seen unpushed.
    pub(crate) dirty_since: Option<f64>,
    /// The thread that waits for changes from other devices (live sync).
    pub(crate) watcher: Option<Watcher>,
    /// The last sync round failed (it retries); shown in the status pill.
    pub(crate) round_failed: bool,
    /// The certificate the user trusted for `server` (DER), pinned to the account made or
    /// signed in to with it.
    pub(crate) pin: Option<Vec<u8>>,
    /// A certificate shown by `server` that waits for a yes.
    pub(crate) untrusted: Option<Vec<u8>>,
}

/// How sync is doing, for the pill in the tab strip (F-SYNC-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pill {
    /// Everything is on the server.
    Synced,
    /// Changes are waiting to be pushed, or nothing synced yet.
    Waiting,
    /// The last round failed.
    Failed,
}

/// The pill's state and words for a signed-in vault: when it last synced, how many
/// changes wait, and whether the last round failed. `now` is seconds since the epoch.
#[must_use]
pub fn pill(last_synced: u64, pending: usize, failed: bool, now: u64) -> (Pill, String) {
    if failed {
        return (Pill::Failed, "Sync failed, retrying".into());
    }
    match (last_synced, pending) {
        (0, _) => (Pill::Waiting, "Not synced yet".into()),
        (then, 0) => (Pill::Synced, format!("Synced {}", ago(then, now))),
        (_, n) => (Pill::Waiting, format!("{n} to sync")),
    }
}

/// Waits on the server for changes from other devices and flags them, so a round runs at
/// once instead of at the next minute.
pub(crate) struct Watcher {
    /// Which account it watches: server, name, device.
    account: (String, String, u64),
    stop: Arc<AtomicBool>,
    news: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Watcher {
    fn start(pace: &dyn Pace, account: &SyncAccount, since: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let news = Arc::new(AtomicBool::new(false));
        let watched = account.clone();
        let (thread_stop, thread_news, wake) = (stop.clone(), news.clone(), pace.waker());
        std::thread::spawn(move || {
            let Ok(http) = HttpServer::for_account(&watched) else {
                return;
            };
            let mut since = since;
            let mut offline = false;
            while !thread_stop.load(Ordering::Relaxed) {
                match http.wait(since) {
                    // Another device pushed, or the server is back after an outage: what
                    // it holds now comes in at once.
                    Ok(latest) if latest > since || offline => {
                        since = since.max(latest);
                        offline = false;
                        thread_news.store(true, Ordering::Relaxed);
                        wake();
                    }
                    Ok(_) => {}
                    Err(SyncError::Unauthorized) => return,
                    // Offline or the server is down: try again in a while.
                    Err(_) => {
                        offline = true;
                        std::thread::sleep(Duration::from_secs(15));
                    }
                }
            }
        });
        Self {
            account: (account.server.clone(), account.name.clone(), account.device),
            stop,
            news,
        }
    }
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
            password: Zeroizing::new(String::new()),
            work: None,
            message: None,
            devices: Vec::new(),
            trash: Vec::new(),
            last_round: f64::NEG_INFINITY,
            dirty_since: None,
            watcher: None,
            round_failed: false,
            pin: None,
            untrusted: None,
        }
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub(crate) fn ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

impl SyncScreen {
    /// Whether the last sync round failed.
    #[must_use]
    pub fn round_failed(&self) -> bool {
        self.round_failed
    }

    fn start(&mut self, pace: &dyn Pace, label: &str, job: impl FnOnce() -> Done + Send + 'static) {
        let (tx, rx) = channel();
        let wake = pace.waker();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            wake();
        });
        self.work = Some((label.to_owned(), rx));
    }

    /// Runs background syncs and applies finished work; call every frame while unlocked.
    /// Returns true when the vault was replaced (after signing in), so running port
    /// forwards, whose ids changed, are stopped.
    pub fn tick(&mut self, ctx: &dyn Pace, hosts: &mut Vault, vault_path: &Path) -> bool {
        let mut replaced = false;
        if let Some((_, rx)) = &self.work {
            match rx.try_recv() {
                Ok(done) => {
                    self.work = None;
                    replaced = self.finish(done, hosts, vault_path);
                    // A retry counts from the failure, not from when the round began.
                    if self.round_failed {
                        self.last_round = ctx.now();
                    }
                }
                // The work thread ended without an answer (it panicked): sync goes on.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.work = None;
                    self.round_failed = true;
                    self.last_round = ctx.now();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let state = hosts.sync_state();
        let Some(signed_in) = state.account.clone() else {
            self.watcher = None;
            return replaced;
        };
        let watched = (
            signed_in.server.clone(),
            signed_in.name.clone(),
            signed_in.device,
        );
        if self
            .watcher
            .as_ref()
            .is_none_or(|watcher| watcher.account != watched)
        {
            self.watcher = Some(Watcher::start(ctx, &signed_in, state.cursor));
        }
        if self
            .watcher
            .as_ref()
            .is_some_and(|watcher| watcher.news.swap(false, Ordering::Relaxed))
        {
            // Another device pushed: sync now.
            self.last_round = f64::NEG_INFINITY;
        }
        let now = ctx.now();
        if state.pending_count() == 0 {
            self.dirty_since = None;
        } else if self.dirty_since.is_none() {
            self.dirty_since = Some(now);
        }
        let due = now >= next_round(self.last_round, self.dirty_since, self.round_failed);
        if due && self.work.is_none() {
            self.last_round = now;
            self.dirty_since = None;
            let copy = hosts.sync_copy();
            self.start(ctx, "Syncing", move || run_round(&signed_in, copy));
        }
        // Wake up for the next round only (idle wakeups, P-06); a round in flight wakes the
        // window itself when it ends.
        if self.work.is_none() {
            let next = next_round(self.last_round, self.dirty_since, self.round_failed);
            ctx.later(Duration::from_secs_f64((next - now).clamp(0.1, PERIOD)));
        }
        replaced
    }

    /// Asks for a round as soon as possible: a host is being connected to, so its details
    /// should be the latest the account has.
    pub(crate) fn sync_soon(&mut self) {
        self.last_round = f64::NEG_INFINITY;
    }

    /// Applies finished work; true when the vault was replaced.
    fn finish(&mut self, done: Done, hosts: &mut Vault, vault_path: &Path) -> bool {
        match done {
            Done::Registered(account) => {
                self.message = Some(match hosts.set_sync_account(Some(account)) {
                    Ok(()) => (false, "Account created. This device is signed in.".into()),
                    Err(error) => (true, format!("Could not save the account: {error}")),
                });
                self.last_round = f64::NEG_INFINITY;
            }
            Done::Relinked(key_file, account) => {
                self.message = Some(match hosts.relink(&key_file, account) {
                    Ok(()) => (false, "Signed in again. Nothing was merged: this vault already belongs to the account.".into()),
                    Err(error) => (true, format!("Could not sign in: {error}")),
                });
                self.last_round = f64::NEG_INFINITY;
            }
            Done::Joined(vault, backup, generation) => {
                return self.finish_join(*vault, &backup, generation, hosts, vault_path);
            }
            Done::Synced(copy, result) => match result {
                Ok(report) => {
                    self.round_failed = false;
                    let mut copy = *copy;
                    copy.mark_synced();
                    // A local edit since the snapshot: drop this result, the next round redoes it.
                    let _ = hosts.commit_sync(copy);
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
                    let _ = hosts.set_sync_account(None);
                    self.message = Some((
                        true,
                        "This device was signed out of the account. Everything on it is kept."
                            .into(),
                    ));
                }
                Err(error) => {
                    self.round_failed = true;
                    self.message = Some((
                        true,
                        format!("Sync failed: {error}. It retries automatically."),
                    ));
                }
            },
            Done::Trash(result) => match result {
                Ok(items) => {
                    self.message = items
                        .is_empty()
                        .then(|| (false, "Nothing deleted to restore.".to_owned()));
                    self.trash = items;
                }
                Err(error) => self.message = Some((true, error.to_string())),
            },
            Done::Restored(copy, result) => match result {
                Ok(()) => {
                    self.message = Some(match hosts.commit_sync(*copy) {
                        Ok(true) => (
                            false,
                            "Restored. Other devices get it at their next sync.".into(),
                        ),
                        Ok(false) => (
                            false,
                            "Restored on the server; it arrives here at the next sync.".into(),
                        ),
                        Err(error) => (true, error.to_string()),
                    });
                }
                Err(error) => self.message = Some((true, format!("Could not restore: {error}"))),
            },
            Done::Devices(result) => match result {
                Ok(devices) => self.devices = devices,
                Err(error) => self.message = Some((true, error.to_string())),
            },
            Done::Failed(message) => self.message = Some((true, message)),
            // The card shows the certificate and the question.
            Done::Untrusted(certificate) => {
                self.message = None;
                self.untrusted = Some(certificate);
            }
        }
        false
    }

    /// Puts a joined vault in place; true when it replaced the local one.
    fn finish_join(
        &mut self,
        mut vault: Vault,
        backup: &Path,
        generation: u64,
        hosts: &mut Vault,
        vault_path: &Path,
    ) -> bool {
        if hosts.generation() != generation {
            // Edited while signing in: the merge would miss that edit.
            let _ = std::fs::remove_file(vault.path());
            self.message = Some((
                true,
                "Hosts changed while signing in, so nothing was replaced. Sign in again.".into(),
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
        *hosts = vault;
        self.message = Some((
            false,
            format!(
                "Signed in, and this device's hosts are merged into the account. From now on the vault opens with the account's master password and recovery key. The old vault file is kept at {}.",
                backup.display()
            ),
        ));
        self.last_round = f64::NEG_INFINITY;
        true
    }

    /// Starts a sync round at the next tick.
    pub(crate) fn sync_now(&mut self) {
        self.last_round = f64::NEG_INFINITY;
    }

    /// Loads the account's devices.
    pub(crate) fn load_devices(&mut self, pace: &dyn Pace, account: &SyncAccount) {
        let account = account.clone();
        self.start(pace, "Loading devices", move || {
            Done::Devices(
                HttpServer::for_account(&account)
                    .and_then(|http| http.devices())
                    .map(|list| list.devices),
            )
        });
    }

    /// Loads what was deleted on any device and can be restored.
    pub(crate) fn load_deleted(&mut self, pace: &dyn Pace, account: &SyncAccount, vault: &Vault) {
        let account = account.clone();
        let copy = vault.sync_copy();
        self.start(pace, "Loading deleted items", move || {
            Done::Trash(load_trash(&account, &copy))
        });
    }

    /// Restores deleted item `index` of the loaded list.
    pub(crate) fn restore(
        &mut self,
        pace: &dyn Pace,
        account: &SyncAccount,
        vault: &Vault,
        index: usize,
    ) {
        if index >= self.trash.len() {
            return;
        }
        let item = self.trash.remove(index);
        let account = account.clone();
        let copy = vault.sync_copy();
        self.start(pace, "Restoring", move || {
            let mut replica = VaultReplica(copy);
            let result = HttpServer::for_account(&account).and_then(|mut http| {
                restore_item(&mut http, &mut replica, &item.id, &item.version)
            });
            Done::Restored(Box::new(replica.0), result)
        });
    }

    /// Signs this device out; everything on it is kept.
    pub(crate) fn sign_out(&mut self, vault: &mut Vault) {
        self.devices.clear();
        self.trash.clear();
        self.message = Some(match vault.set_sync_account(None) {
            Ok(()) => (
                false,
                "Signed out. Everything on this device is kept.".into(),
            ),
            Err(error) => (true, error.to_string()),
        });
    }

    /// Signs another device out of the account.
    pub(crate) fn revoke(&mut self, pace: &dyn Pace, account: &SyncAccount, id: u64) {
        let account = account.clone();
        self.start(pace, "Signing the device out", move || {
            let http = match HttpServer::for_account(&account) {
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

    /// The name of deleted item `index`.
    pub(crate) fn deleted_name(&self, index: usize) -> Option<&str> {
        self.trash.get(index).map(|item| item.name.as_str())
    }

    /// The typed master password, taken once (the field empties).
    pub(crate) fn take_password(&mut self) -> Zeroizing<String> {
        std::mem::replace(&mut self.password, Zeroizing::new(String::new()))
    }

    /// Sets the master password typed in the new UI's field, for `create` and `join`.
    pub(crate) fn set_password(&mut self, password: &str) {
        self.password = Zeroizing::new(password.to_owned());
    }

    /// Creates the account; `invite` is the code a server with closed sign-ups needs
    /// (empty for none).
    pub(crate) fn create(&mut self, ctx: &dyn Pace, hosts: &Vault, invite: &str) {
        let password = self.take_password();
        let (server, name, device, invite) = (
            self.server.trim().to_owned(),
            self.account.trim().to_owned(),
            self.device.trim().to_owned(),
            invite.trim().to_owned(),
        );
        let key_file = hosts.key_file_bytes();
        let pin = self.pin.clone();
        self.message = None;
        self.start(ctx, "Creating the account", move || {
            // The account password must be the vault's, so other devices can unwrap the key.
            if !Vault::password_opens(&key_file, &password) {
                return Done::Failed("That is not this vault's master password.".into());
            }
            // The server will hold a record of it that a thief could guess against offline.
            if !annulus_vault::strength::estimate(&password).is_strong() {
                return Done::Failed(
                    "This master password is too easy to guess to put an account on a server. Change it under Master password first."
                        .into(),
                );
            }
            let connect = HttpServer::new(&server, None).map(|http| http.pinned(pin.as_deref()));
            let result = connect.and_then(|mut http| {
                account::register_with_invite(
                    &mut http,
                    &account::default_ksf(),
                    &name,
                    &password,
                    key_file,
                    &device,
                    Some(&invite),
                )
            });
            match result {
                Ok(session) => Done::Registered(SyncAccount {
                    server,
                    name,
                    device: session.device,
                    token: Secret::new(session.token),
                    pin,
                }),
                Err(SyncError::Untrusted(_)) if pin.is_none() => untrusted(&server),
                Err(error) => Done::Failed(format!("Could not create the account: {error}")),
            }
        });
    }

    pub(crate) fn join(&mut self, ctx: &dyn Pace, hosts: &Vault, vault_path: &Path) {
        let password = self.take_password();
        let (server, name, device) = (
            self.server.trim().to_owned(),
            self.account.trim().to_owned(),
            self.device.trim().to_owned(),
        );
        let local_hosts = hosts.hosts().to_vec();
        let local_forwards = hosts.forwards().to_vec();
        let local_keys = hosts.ssh_keys().to_vec();
        let local_snippets = hosts.snippets().to_vec();
        let local_groups = hosts.groups().to_vec();
        let generation = hosts.generation();
        let copy = hosts.sync_copy();
        let path = vault_path.to_path_buf();
        let pin = self.pin.clone();
        self.message = None;
        self.start(ctx, "Signing in", move || {
            let connect = HttpServer::new(&server, None).map(|http| http.pinned(pin.as_deref()));
            let session = match connect.and_then(|mut http| {
                account::login(
                    &mut http,
                    &account::default_ksf(),
                    &name,
                    &password,
                    &device,
                )
            }) {
                Ok(session) => session,
                Err(SyncError::Untrusted(_)) if pin.is_none() => return untrusted(&server),
                Err(error) => return Done::Failed(format!("Could not sign in: {error}")),
            };
            let account = SyncAccount {
                server: server.clone(),
                name: name.clone(),
                device: session.device,
                token: Secret::new(session.token.clone()),
                pin: pin.clone(),
            };
            // This vault already belongs to the account (signed out earlier, or signed out
            // by another device): just link it again, nothing to merge.
            if copy.same_account(&session.wrapped_keys, &password) {
                return Done::Relinked(session.wrapped_keys, account);
            }
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
                pin,
            };
            // Built beside the live vault; the UI thread moves it in place.
            let joining = path.with_extension("joining.anv");
            let mut vault = match Vault::join(&joining, &session.wrapped_keys, &password, account) {
                Ok(vault) => vault,
                Err(error) => {
                    return Done::Failed(format!("Could not open the account's vault: {error}"));
                }
            };
            if let Err(error) = vault.adopt(
                &local_hosts,
                &local_forwards,
                &local_keys,
                &local_snippets,
                &local_groups,
            ) {
                let _ = std::fs::remove_file(&joining);
                return Done::Failed(format!("Could not merge this device's hosts: {error}"));
            }
            Done::Joined(Box::new(vault), backup, generation)
        });
    }
}

/// A deleted item and the version to bring back.
pub(crate) struct TrashItem {
    id: String,
    name: String,
    version: annulus_proto::Record,
}

/// The deleted items of `copy`, each with its last version from the server's history.
fn load_trash(account: &SyncAccount, copy: &SyncCopy) -> Result<Vec<TrashItem>, SyncError> {
    let http = HttpServer::for_account(account)?;
    let mut items = Vec::new();
    for (id, _) in copy.deleted() {
        let Some(version) = http
            .history(&id)?
            .into_iter()
            .find(|version| !version.deleted)
        else {
            continue;
        };
        let Ok((kind, _, body)) = copy.open(&id, &version.ciphertext) else {
            continue;
        };
        // A deletion flagged as a version by the server holds nothing to bring back.
        if body.is_empty() {
            continue;
        }
        items.push(TrashItem {
            name: SyncCopy::describe(kind, &body),
            id,
            version,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

/// The certificate `server` shows, for the user to compare and trust, or why it could not
/// be read.
fn untrusted(server: &str) -> Done {
    match annulus_sync::pin::certificate_of(server) {
        Ok(certificate) => Done::Untrusted(certificate),
        Err(error) => Done::Failed(format!(
            "The server's certificate is not trusted, and {error}"
        )),
    }
}

/// One sync round on a worker thread.
fn run_round(account: &SyncAccount, copy: SyncCopy) -> Done {
    let mut replica = VaultReplica(copy);
    let result =
        HttpServer::for_account(account).and_then(|mut http| sync(&mut http, &mut replica));
    Done::Synced(Box::new(replica.0), result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_come_a_minute_apart_sooner_after_an_edit_or_a_failure() {
        assert_eq!(next_round(100.0, None, false), 160.0);
        assert_eq!(next_round(100.0, Some(120.0), false), 123.0);
        assert_eq!(
            next_round(100.0, None, true),
            115.0,
            "a failed round retries soon"
        );
        assert_eq!(
            next_round(100.0, Some(101.0), true),
            115.0,
            "an edit waiting does not make an unreachable server be tried back to back"
        );
        assert!(
            next_round(f64::NEG_INFINITY, None, false) < 0.0,
            "sync soon"
        );
    }

    #[test]
    fn the_pill_says_synced_waiting_or_failed() {
        let now = 10_000;
        assert_eq!(
            pill(now - 120, 0, false, now),
            (Pill::Synced, "Synced 2 min ago".into())
        );
        assert_eq!(
            pill(now - 120, 3, false, now),
            (Pill::Waiting, "3 to sync".into())
        );
        assert_eq!(
            pill(0, 0, false, now),
            (Pill::Waiting, "Not synced yet".into())
        );
        assert_eq!(pill(now, 0, true, now).0, Pill::Failed);
    }

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
