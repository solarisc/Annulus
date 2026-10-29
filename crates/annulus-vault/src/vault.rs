//! The encrypted vault file (ADR-0007): the key file and one encrypted record per entity.
//! Nothing secret is on disk in the clear, not even host names. Writes are atomic, and a file
//! that cannot be read is reported and never overwritten.
//!
//! This single-file container is the step before SQLite storage; the key hierarchy
//! and record format carry over unchanged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, KdfParams, KeyFile, Keys, Sealed};
use crate::forwards::PortForward;
use crate::hosts::{DEFAULT_PORT, Host};
use crate::keychain::SshKey;
use crate::logs::SessionLog;
use crate::snippets::Snippet;
use crate::sync::{SyncAccount, SyncCopy, SyncState, kind};

/// Record kind of a saved host.
const KIND_HOST: u8 = 1;
/// Record kind of a port-forwarding rule.
const KIND_FORWARD: u8 = 2;
/// Record kind of the sync bookkeeping (one record).
const KIND_SYNC_STATE: u8 = 3;
/// Record kind of a keychain key.
const KIND_KEY: u8 = 5;
/// Record kind of a snippet.
const KIND_SNIPPET: u8 = 6;
/// Record kind of a session log.
const KIND_LOG: u8 = 7;

/// Why the vault could not be used.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// The password or recovery key does not open the vault.
    #[error("wrong password")]
    WrongPassword,
    /// The file or a record in it is damaged; it is left untouched.
    #[error("the vault file is damaged: {0}")]
    Damaged(String),
    /// The file system refused.
    #[error("vault file: {0}")]
    Io(#[from] std::io::Error),
    /// The OS could not supply randomness.
    #[error("no secure randomness available")]
    Randomness,
}

impl From<CryptoError> for VaultError {
    fn from(error: CryptoError) -> Self {
        match error {
            CryptoError::WrongPassword => VaultError::WrongPassword,
            CryptoError::Randomness => VaultError::Randomness,
            CryptoError::Corrupt | CryptoError::Parameters => {
                VaultError::Damaged(error.to_string())
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    id: String,
    kind: u8,
    sealed: Sealed,
}

#[derive(Debug, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    keys: KeyFile,
    records: Vec<Record>,
}

/// The keys and key file of a vault, for changing its password on another thread.
pub struct PasswordKit {
    keys: Keys,
    key_file: KeyFile,
}

impl PasswordKit {
    /// Checks `current` and returns the key file re-wrapped for `new` (records and the
    /// recovery key are untouched). Runs Argon2id twice.
    ///
    /// # Errors
    /// [`VaultError::WrongPassword`] if `current` is wrong, or no randomness.
    pub fn rewrap(&self, current: &str, new: &str) -> Result<Vec<u8>, VaultError> {
        let opened = Keys::unlock(&self.key_file, current)?;
        if !opened.same_account(&self.keys) {
            return Err(VaultError::WrongPassword);
        }
        let file = self.keys.change_password(&self.key_file, new)?;
        serde_json::to_vec(&file).map_err(|error| VaultError::Damaged(error.to_string()))
    }
}

/// An unlocked vault.
pub struct Vault {
    path: PathBuf,
    key_file: KeyFile,
    keys: Keys,
    hosts: Vec<Host>,
    forwards: Vec<PortForward>,
    ssh_keys: Vec<SshKey>,
    snippets: Vec<Snippet>,
    logs: Vec<SessionLog>,
    sync: SyncState,
    /// Counts saved changes, so a sync result made from an older snapshot is not applied.
    generation: u64,
}

impl Vault {
    /// Whether a vault exists at `path`.
    #[must_use]
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    /// Creates a new vault at `path` protected by `password`, and returns it with the
    /// recovery key to show the user once.
    ///
    /// # Errors
    /// Writing the file, or no randomness.
    pub fn create(
        path: impl Into<PathBuf>,
        password: &str,
        kdf: KdfParams,
    ) -> Result<(Self, Zeroizing<String>), VaultError> {
        let (keys, key_file, recovery) = Keys::create(password, kdf)?;
        let vault = Self {
            path: path.into(),
            key_file,
            keys,
            hosts: Vec::new(),
            forwards: Vec::new(),
            ssh_keys: Vec::new(),
            snippets: Vec::new(),
            logs: Vec::new(),
            sync: SyncState::new()?,
            generation: 0,
        };
        vault.save(&vault.hosts, &vault.forwards, &vault.sync)?;
        Ok((vault, recovery))
    }

    /// Unlocks the vault at `path` with the master password.
    ///
    /// # Errors
    /// [`VaultError::WrongPassword`], or the file is missing or damaged.
    pub fn unlock(path: impl Into<PathBuf>, password: &str) -> Result<Self, VaultError> {
        let path = path.into();
        let file = read(&path)?;
        let keys = Keys::unlock(&file.keys, password)?;
        Self::open_records(path, file, keys)
    }

    /// Unlocks with the recovery key, for when the password is forgotten.
    ///
    /// # Errors
    /// As [`Self::unlock`].
    pub fn recover(path: impl Into<PathBuf>, recovery_key: &str) -> Result<Self, VaultError> {
        let path = path.into();
        let file = read(&path)?;
        let keys = Keys::recover(&file.keys, recovery_key)?;
        Self::open_records(path, file, keys)
    }

    fn open_records(path: PathBuf, file: VaultFile, keys: Keys) -> Result<Self, VaultError> {
        let mut hosts = Vec::new();
        let mut forwards = Vec::new();
        let mut ssh_keys = Vec::new();
        let mut snippets = Vec::new();
        let mut logs = Vec::new();
        let mut sync = None;
        for record in &file.records {
            let damaged = |error: serde_json::Error| VaultError::Damaged(error.to_string());
            match record.kind {
                KIND_HOST => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    hosts.push(serde_json::from_slice::<Host>(&plaintext).map_err(damaged)?);
                }
                KIND_FORWARD => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    forwards
                        .push(serde_json::from_slice::<PortForward>(&plaintext).map_err(damaged)?);
                }
                KIND_KEY => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    ssh_keys.push(serde_json::from_slice::<SshKey>(&plaintext).map_err(damaged)?);
                }
                KIND_SNIPPET => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    snippets.push(serde_json::from_slice::<Snippet>(&plaintext).map_err(damaged)?);
                }
                KIND_LOG => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    logs.push(serde_json::from_slice::<SessionLog>(&plaintext).map_err(damaged)?);
                }
                KIND_SYNC_STATE => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    sync = Some(serde_json::from_slice::<SyncState>(&plaintext).map_err(damaged)?);
                }
                // A newer version wrote kinds this one doesn't know; saving would drop them, so
                // refuse to open rather than lose data silently.
                other => {
                    return Err(VaultError::Damaged(format!("unknown record kind {other}")));
                }
            }
        }
        hosts.sort_by_key(|host| host.label.to_lowercase());
        forwards.sort_by_key(|rule| rule.label.to_lowercase());
        let mut sync = match sync {
            Some(sync) => sync,
            None => SyncState::new()?,
        };
        // Entities from before sync existed get sync ids now, as unpushed changes.
        for host in &hosts {
            if sync.id_of(kind::HOST, host.id).is_none() {
                sync.touch(kind::HOST, host.id, false)?;
            }
        }
        for rule in &forwards {
            if sync.id_of(kind::FORWARD, rule.id).is_none() {
                sync.touch(kind::FORWARD, rule.id, false)?;
            }
        }
        ssh_keys.sort_by_key(|key: &SshKey| key.label.to_lowercase());
        snippets.sort_by_key(|snippet: &Snippet| snippet.label.to_lowercase());
        Ok(Self {
            path,
            key_file: file.keys,
            keys,
            hosts,
            forwards,
            ssh_keys,
            snippets,
            logs,
            sync,
            generation: 0,
        })
    }

    /// Changes the master password; records are untouched and the recovery key still works.
    ///
    /// # Errors
    /// Writing the file, or no randomness.
    pub fn change_password(&mut self, password: &str) -> Result<(), VaultError> {
        let key_file = self.keys.change_password(&self.key_file, password)?;
        let previous = std::mem::replace(&mut self.key_file, key_file);
        if let Err(error) = self.save(&self.hosts, &self.forwards, &self.sync) {
            self.key_file = previous;
            return Err(error);
        }
        Ok(())
    }

    /// What a password change needs, to run its Argon2id work off the UI thread.
    #[must_use]
    pub fn password_kit(&self) -> PasswordKit {
        PasswordKit {
            keys: self.keys.clone(),
            key_file: self.key_file.clone(),
        }
    }

    /// Takes a key file made by [`PasswordKit::rewrap`] and saves.
    ///
    /// # Errors
    /// The bytes are not a key file, or writing failed (the vault keeps the old one).
    pub fn set_key_file(&mut self, key_file: &[u8]) -> Result<(), VaultError> {
        let file: KeyFile = serde_json::from_slice(key_file)
            .map_err(|error| VaultError::Damaged(error.to_string()))?;
        let previous = std::mem::replace(&mut self.key_file, file);
        if let Err(error) = self.save(&self.hosts, &self.forwards, &self.sync) {
            self.key_file = previous;
            return Err(error);
        }
        Ok(())
    }

    /// All hosts, sorted by label.
    #[must_use]
    pub fn hosts(&self) -> &[Host] {
        &self.hosts
    }

    /// The hosts matching `query`.
    pub fn search<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a Host> + 'a {
        self.hosts.iter().filter(move |host| host.matches(query))
    }

    /// A new, unsaved host with a fresh id.
    #[must_use]
    pub fn draft_host(&self) -> Host {
        let id = self
            .hosts
            .iter()
            .map(|host| host.id)
            .max()
            .map_or(1, |max| max + 1);
        Host {
            id,
            label: String::new(),
            address: String::new(),
            port: DEFAULT_PORT,
            username: String::new(),
            tags: Vec::new(),
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
        }
    }

    /// Adds or replaces a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_host(&mut self, host: Host) -> Result<(), VaultError> {
        let id = host.id;
        let mut next = self.hosts.clone();
        match next.iter_mut().find(|existing| existing.id == host.id) {
            Some(existing) => *existing = host,
            None => next.push(host),
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::HOST, id, false)?;
        self.save(&next, &self.forwards, &sync)?;
        self.hosts = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_host(&mut self, id: u64) -> Result<(), VaultError> {
        let next: Vec<Host> = self
            .hosts
            .iter()
            .filter(|host| host.id != id)
            .cloned()
            .collect();
        let mut sync = self.sync.clone();
        sync.touch(kind::HOST, id, true)?;
        self.save(&next, &self.forwards, &sync)?;
        self.hosts = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Moves hosts from an old plain host file into the vault, then deletes the plain file
    /// (only after the vault was written). Returns how many were imported.
    ///
    /// # Errors
    /// The plain file is damaged (it is kept), or writing failed.
    pub fn import_plain_hosts(&mut self, plain: &Path) -> Result<usize, VaultError> {
        let imported = crate::hosts::load_plain(plain)
            .map_err(|error| VaultError::Damaged(error.to_string()))?;
        if imported.is_empty() {
            return Ok(0);
        }
        let mut next = self.hosts.clone();
        let mut sync = self.sync.clone();
        let mut id = next.iter().map(|host| host.id).max().unwrap_or(0);
        for mut host in imported.iter().cloned() {
            id += 1;
            host.id = id;
            sync.touch(kind::HOST, id, false)?;
            next.push(host);
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        self.save(&next, &self.forwards, &sync)?;
        self.hosts = next;
        self.sync = sync;
        self.generation += 1;
        std::fs::remove_file(plain)?;
        Ok(imported.len())
    }

    /// The port-forwarding rules, sorted by label.
    #[must_use]
    pub fn forwards(&self) -> &[PortForward] {
        &self.forwards
    }

    /// A new local rule through `host_id` with a fresh id; not saved until upserted.
    #[must_use]
    pub fn draft_forward(&self, host_id: u64) -> PortForward {
        let id = self.forwards.iter().map(|rule| rule.id).max().unwrap_or(0) + 1;
        PortForward::new(id, host_id)
    }

    /// Adds or replaces a rule, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_forward(&mut self, rule: PortForward) -> Result<(), VaultError> {
        let id = rule.id;
        let mut next = self.forwards.clone();
        match next.iter_mut().find(|existing| existing.id == rule.id) {
            Some(existing) => *existing = rule,
            None => next.push(rule),
        }
        next.sort_by_key(|rule| rule.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::FORWARD, id, false)?;
        self.save(&self.hosts, &next, &sync)?;
        self.forwards = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a rule, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_forward(&mut self, id: u64) -> Result<(), VaultError> {
        let next: Vec<PortForward> = self
            .forwards
            .iter()
            .filter(|rule| rule.id != id)
            .cloned()
            .collect();
        let mut sync = self.sync.clone();
        sync.touch(kind::FORWARD, id, true)?;
        self.save(&self.hosts, &next, &sync)?;
        self.forwards = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Counts saved changes; equal values mean nothing changed in between.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Where the vault file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Moves the vault file to `path`, replacing what is there.
    ///
    /// # Errors
    /// The file system refused; the vault stays where it was.
    pub fn move_to(&mut self, path: &Path) -> Result<(), VaultError> {
        std::fs::rename(&self.path, path)?;
        self.path = path.to_path_buf();
        Ok(())
    }

    /// The sync bookkeeping.
    #[must_use]
    pub fn sync_state(&self) -> &SyncState {
        &self.sync
    }

    /// A snapshot for a sync round on another thread.
    #[must_use]
    pub fn sync_copy(&self) -> SyncCopy {
        SyncCopy {
            keys: self.keys.clone(),
            hosts: self.hosts.clone(),
            forwards: self.forwards.clone(),
            ssh_keys: self.ssh_keys.clone(),
            snippets: self.snippets.clone(),
            logs: self.logs.clone(),
            state: self.sync.clone(),
            generation: self.generation,
        }
    }

    /// Takes the result of a sync round and saves it. Returns false, changing nothing, if
    /// the vault was edited since the snapshot; the next round redoes the work (the server
    /// keeps everything, and the edit is pushed then).
    ///
    /// # Errors
    /// Writing the file; the vault stays as it was.
    pub fn commit_sync(&mut self, copy: SyncCopy) -> Result<bool, VaultError> {
        if copy.generation != self.generation {
            return Ok(false);
        }
        self.save_all(
            &copy.hosts,
            &copy.forwards,
            &copy.ssh_keys,
            &copy.snippets,
            &copy.logs,
            &copy.state,
        )?;
        self.hosts = copy.hosts;
        self.forwards = copy.forwards;
        self.ssh_keys = copy.ssh_keys;
        self.snippets = copy.snippets;
        self.logs = copy.logs;
        self.sync = copy.state;
        self.generation += 1;
        Ok(true)
    }

    /// Signs the vault in to (or, with `None`, out of) a sync account. Signing out keeps
    /// every local record (ADR-0008 data-loss rule 2); signing in to another account later
    /// pushes them there as new records.
    ///
    /// # Errors
    /// Writing the file.
    pub fn set_sync_account(&mut self, account: Option<SyncAccount>) -> Result<(), VaultError> {
        let mut sync = self.sync.clone();
        let switching = sync.account.as_ref().map(|a| (&a.server, &a.name))
            != account.as_ref().map(|a| (&a.server, &a.name));
        if switching {
            // Another account: everything local is new there.
            sync.restart();
        }
        sync.account = account;
        self.save(&self.hosts, &self.forwards, &sync)?;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Adds `hosts` and `forwards` from another vault as new entities (fresh local ids,
    /// rules relinked to their hosts' new ids), so joining an account keeps what was on this
    /// device (ADR-0008 data-loss rule 4). They are pushed at the next sync.
    ///
    /// # Errors
    /// Writing the file; nothing is added then.
    pub fn adopt(
        &mut self,
        hosts: &[Host],
        forwards: &[PortForward],
        ssh_keys: &[SshKey],
        snippets: &[Snippet],
    ) -> Result<(), VaultError> {
        let mut next_snippets = self.snippets.clone();
        let mut snippet_id = next_snippets.iter().map(|s| s.id).max().unwrap_or(0);
        let mut sync = self.sync.clone();
        for snippet in snippets {
            snippet_id += 1;
            sync.touch(kind::SNIPPET, snippet_id, false)?;
            next_snippets.push(Snippet {
                id: snippet_id,
                ..snippet.clone()
            });
        }
        let mut next_hosts = self.hosts.clone();
        let mut next_forwards = self.forwards.clone();
        let mut next_keys = self.ssh_keys.clone();
        let mut key_id = next_keys.iter().map(|key| key.id).max().unwrap_or(0);
        let mut new_key_ids = std::collections::HashMap::new();
        for key in ssh_keys {
            key_id += 1;
            new_key_ids.insert(key.id, key_id);
            sync.touch(kind::KEY, key_id, false)?;
            next_keys.push(SshKey {
                id: key_id,
                ..key.clone()
            });
        }
        let mut host_id = next_hosts.iter().map(|host| host.id).max().unwrap_or(0);
        let mut new_ids = std::collections::HashMap::new();
        for host in hosts {
            host_id += 1;
            new_ids.insert(host.id, host_id);
            sync.touch(kind::HOST, host_id, false)?;
            next_hosts.push(Host {
                id: host_id,
                key_id: host.key_id.and_then(|key| new_key_ids.get(&key).copied()),
                ..host.clone()
            });
        }
        // Jump hosts point at hosts adopted in the same batch.
        for host in next_hosts.iter_mut().rev().take(hosts.len()) {
            host.jump_id = host.jump_id.and_then(|jump| new_ids.get(&jump).copied());
        }
        let mut rule_id = next_forwards.iter().map(|rule| rule.id).max().unwrap_or(0);
        for rule in forwards {
            rule_id += 1;
            sync.touch(kind::FORWARD, rule_id, false)?;
            next_forwards.push(PortForward {
                id: rule_id,
                host_id: new_ids.get(&rule.host_id).copied().unwrap_or(0),
                ..rule.clone()
            });
        }
        next_hosts.sort_by_key(|host| host.label.to_lowercase());
        next_forwards.sort_by_key(|rule| rule.label.to_lowercase());
        next_keys.sort_by_key(|key| key.label.to_lowercase());
        next_snippets.sort_by_key(|snippet| snippet.label.to_lowercase());
        self.save_all(
            &next_hosts,
            &next_forwards,
            &next_keys,
            &next_snippets,
            &self.logs,
            &sync,
        )?;
        self.hosts = next_hosts;
        self.forwards = next_forwards;
        self.ssh_keys = next_keys;
        self.snippets = next_snippets;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Signs in again to the account this vault already belongs to: takes the account's
    /// key file (it may be wrapped by a newer password) and the new device token, keeping
    /// every record and its sync state. Check with `SyncCopy::same_account` first.
    ///
    /// # Errors
    /// The key file is damaged, or writing the file.
    pub fn relink(&mut self, key_file: &[u8], account: SyncAccount) -> Result<(), VaultError> {
        let file: KeyFile = serde_json::from_slice(key_file)
            .map_err(|error| VaultError::Damaged(error.to_string()))?;
        let previous = std::mem::replace(&mut self.key_file, file);
        let mut sync = self.sync.clone();
        sync.account = Some(account);
        if let Err(error) = self.save(&self.hosts, &self.forwards, &sync) {
            self.key_file = previous;
            return Err(error);
        }
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// The key file as uploaded at account registration: the account key wrapped by the
    /// password and by the recovery key, never the key itself.
    #[must_use]
    pub fn key_file_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.key_file).unwrap_or_default()
    }

    /// Whether `password` opens `key_file` (as from [`Self::key_file_bytes`]). Runs
    /// Argon2id, so call it off the UI thread.
    #[must_use]
    pub fn password_opens(key_file: &[u8], password: &str) -> bool {
        serde_json::from_slice::<KeyFile>(key_file)
            .is_ok_and(|file| Keys::unlock(&file, password).is_ok())
    }

    /// Creates a vault at `path` from an account's key file (downloaded at sign-in) and
    /// the password, empty until the first sync fills it. The caller moves any existing
    /// vault aside first (ADR-0008 data-loss rule 4).
    ///
    /// # Errors
    /// [`VaultError::WrongPassword`], a damaged key file, or writing the file.
    pub fn join(
        path: impl Into<PathBuf>,
        key_file: &[u8],
        password: &str,
        account: SyncAccount,
    ) -> Result<Self, VaultError> {
        let key_file: KeyFile = serde_json::from_slice(key_file)
            .map_err(|error| VaultError::Damaged(error.to_string()))?;
        let keys = Keys::unlock(&key_file, password)?;
        let mut sync = SyncState::new()?;
        sync.account = Some(account);
        let vault = Self {
            path: path.into(),
            key_file,
            keys,
            hosts: Vec::new(),
            forwards: Vec::new(),
            ssh_keys: Vec::new(),
            snippets: Vec::new(),
            logs: Vec::new(),
            sync,
            generation: 0,
        };
        vault.save(&vault.hosts, &vault.forwards, &vault.sync)?;
        Ok(vault)
    }

    /// The keychain, sorted by label.
    #[must_use]
    pub fn ssh_keys(&self) -> &[SshKey] {
        &self.ssh_keys
    }

    /// The key with local id `id`.
    #[must_use]
    pub fn ssh_key(&self, id: u64) -> Option<&SshKey> {
        self.ssh_keys.iter().find(|key| key.id == id)
    }

    /// A fresh key id.
    #[must_use]
    pub fn next_key_id(&self) -> u64 {
        self.ssh_keys.iter().map(|key| key.id).max().unwrap_or(0) + 1
    }

    /// Adds or replaces a key, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_key(&mut self, key: SshKey) -> Result<(), VaultError> {
        let id = key.id;
        let mut next = self.ssh_keys.clone();
        match next.iter_mut().find(|existing| existing.id == id) {
            Some(existing) => *existing = key,
            None => next.push(key),
        }
        next.sort_by_key(|key| key.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::KEY, id, false)?;
        self.save_all(
            &self.hosts,
            &self.forwards,
            &next,
            &self.snippets,
            &self.logs,
            &sync,
        )?;
        self.ssh_keys = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a key; hosts that used it fall back to the default keys and passwords.
    ///
    /// # Errors
    /// Writing the file; nothing changes then.
    pub fn remove_key(&mut self, id: u64) -> Result<(), VaultError> {
        let keys: Vec<SshKey> = self
            .ssh_keys
            .iter()
            .filter(|key| key.id != id)
            .cloned()
            .collect();
        let mut hosts = self.hosts.clone();
        let mut sync = self.sync.clone();
        for host in hosts.iter_mut().filter(|host| host.key_id == Some(id)) {
            host.key_id = None;
            sync.touch(kind::HOST, host.id, false)?;
        }
        sync.touch(kind::KEY, id, true)?;
        self.save_all(
            &hosts,
            &self.forwards,
            &keys,
            &self.snippets,
            &self.logs,
            &sync,
        )?;
        self.hosts = hosts;
        self.ssh_keys = keys;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    fn save(
        &self,
        hosts: &[Host],
        forwards: &[PortForward],
        sync: &SyncState,
    ) -> Result<(), VaultError> {
        self.save_all(
            hosts,
            forwards,
            &self.ssh_keys,
            &self.snippets,
            &self.logs,
            sync,
        )
    }

    /// The session logs, newest first.
    #[must_use]
    pub fn logs(&self) -> &[SessionLog] {
        &self.logs
    }

    /// Saves a finished session's text as a log (capped at 1 MiB), dropping the oldest logs
    /// of the same title beyond five.
    ///
    /// # Errors
    /// Writing the file; nothing changes then.
    pub fn add_log(&mut self, title: &str, text: &str, saved_at: u64) -> Result<(), VaultError> {
        let id = self.logs.iter().map(|log| log.id).max().unwrap_or(0) + 1;
        let mut next = self.logs.clone();
        next.push(SessionLog {
            id,
            title: title.to_owned(),
            saved_at,
            text: crate::logs::capped(text),
        });
        let mut sync = self.sync.clone();
        sync.touch(kind::LOG, id, false)?;
        for old in crate::logs::over_retention(&next) {
            sync.touch(kind::LOG, old, true)?;
            next.retain(|log| log.id != old);
        }
        next.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(b.id.cmp(&a.id)));
        self.save_all(
            &self.hosts,
            &self.forwards,
            &self.ssh_keys,
            &self.snippets,
            &next,
            &sync,
        )?;
        self.logs = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a log.
    ///
    /// # Errors
    /// Writing the file; nothing changes then.
    pub fn remove_log(&mut self, id: u64) -> Result<(), VaultError> {
        let next: Vec<SessionLog> = self
            .logs
            .iter()
            .filter(|log| log.id != id)
            .cloned()
            .collect();
        let mut sync = self.sync.clone();
        sync.touch(kind::LOG, id, true)?;
        self.save_all(
            &self.hosts,
            &self.forwards,
            &self.ssh_keys,
            &self.snippets,
            &next,
            &sync,
        )?;
        self.logs = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// The snippets, sorted by label.
    #[must_use]
    pub fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    /// A new, unsaved snippet with a fresh id.
    #[must_use]
    pub fn draft_snippet(&self) -> Snippet {
        Snippet {
            id: self.snippets.iter().map(|s| s.id).max().unwrap_or(0) + 1,
            label: String::new(),
            script: String::new(),
            package: String::new(),
        }
    }

    /// Adds or replaces a snippet, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_snippet(&mut self, snippet: Snippet) -> Result<(), VaultError> {
        let id = snippet.id;
        let mut next = self.snippets.clone();
        match next.iter_mut().find(|existing| existing.id == id) {
            Some(existing) => *existing = snippet,
            None => next.push(snippet),
        }
        next.sort_by_key(|snippet| snippet.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::SNIPPET, id, false)?;
        self.save_all(
            &self.hosts,
            &self.forwards,
            &self.ssh_keys,
            &next,
            &self.logs,
            &sync,
        )?;
        self.snippets = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a snippet, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_snippet(&mut self, id: u64) -> Result<(), VaultError> {
        let next: Vec<Snippet> = self
            .snippets
            .iter()
            .filter(|s| s.id != id)
            .cloned()
            .collect();
        let mut sync = self.sync.clone();
        sync.touch(kind::SNIPPET, id, true)?;
        self.save_all(
            &self.hosts,
            &self.forwards,
            &self.ssh_keys,
            &next,
            &self.logs,
            &sync,
        )?;
        self.snippets = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    fn save_all(
        &self,
        hosts: &[Host],
        forwards: &[PortForward],
        ssh_keys: &[SshKey],
        snippets: &[Snippet],
        logs: &[SessionLog],
        sync: &SyncState,
    ) -> Result<(), VaultError> {
        let mut records = Vec::with_capacity(
            hosts.len() + forwards.len() + ssh_keys.len() + snippets.len() + logs.len() + 1,
        );
        let damaged = |error: serde_json::Error| VaultError::Damaged(error.to_string());
        for host in hosts {
            let id = format!("host-{}", host.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(host).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_HOST, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_HOST,
                sealed,
            });
        }
        for rule in forwards {
            let id = format!("forward-{}", rule.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(rule).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_FORWARD, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_FORWARD,
                sealed,
            });
        }
        for key in ssh_keys {
            let id = format!("key-{}", key.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(key).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_KEY, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_KEY,
                sealed,
            });
        }
        for snippet in snippets {
            let id = format!("snippet-{}", snippet.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(snippet).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_SNIPPET, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_SNIPPET,
                sealed,
            });
        }
        for log in logs {
            let id = format!("log-{}", log.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(log).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_LOG, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_LOG,
                sealed,
            });
        }
        let plaintext = Zeroizing::new(serde_json::to_vec(sync).map_err(damaged)?);
        records.push(Record {
            id: "sync".into(),
            kind: KIND_SYNC_STATE,
            sealed: self.keys.seal_record("sync", KIND_SYNC_STATE, &plaintext)?,
        });
        let file = VaultFile {
            version: 1,
            keys: self.key_file.clone(),
            records,
        };
        let bytes =
            serde_json::to_vec(&file).map_err(|error| VaultError::Damaged(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

fn read(path: &Path) -> Result<VaultFile, VaultError> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|error| VaultError::Damaged(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("annulus-vault-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("vault.anv")
    }

    fn host(vault: &Vault, label: &str) -> Host {
        let mut host = vault.draft_host();
        host.label = label.into();
        host.address = format!("{label}.example.com");
        host
    }

    #[test]
    fn forwarding_rules_survive_lock_and_unlock_beside_the_hosts() {
        let path = temp("forwards");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        vault.upsert_host(host(&vault, "bastion")).unwrap();
        let host_id = vault.hosts()[0].id;
        let mut rule = vault.draft_forward(host_id);
        rule.label = "postgres".into();
        rule.bind_port = 15432;
        rule.destination_host = "db.internal".into();
        rule.destination_port = 5432;
        vault.upsert_forward(rule.clone()).unwrap();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("db.internal")
        );

        let mut reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.forwards(), [rule.clone()]);
        assert_eq!(reopened.hosts()[0].label, "bastion");
        // Editing a host keeps the rules, and removing a rule keeps the host.
        reopened.upsert_host(host(&reopened, "second")).unwrap();
        assert_eq!(reopened.forwards().len(), 1);
        reopened.remove_forward(rule.id).unwrap();
        let reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.forwards(), []);
        assert_eq!(reopened.hosts().len(), 2);
    }

    #[test]
    fn adopted_entities_get_new_ids_and_rules_follow_their_hosts() {
        let path = temp("adopt");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        vault.upsert_host(host(&vault, "already-here")).unwrap();
        let mut old = host(&vault, "from-old-vault");
        old.id = 1;
        let mut rule = PortForward::new(9, 1);
        rule.label = "db".into();
        vault.adopt(&[old], &[rule], &[], &[]).unwrap();

        let reopened = Vault::unlock(&path, "pw").unwrap();
        let adopted = reopened
            .hosts()
            .iter()
            .find(|host| host.label == "from-old-vault")
            .unwrap();
        assert_eq!(adopted.id, 2, "a fresh id, not the colliding 1");
        assert_eq!(reopened.forwards()[0].host_id, adopted.id);
        assert_eq!(reopened.sync_state().pending_count(), 3);
    }

    #[test]
    fn a_joined_vault_moves_over_the_old_file_and_opens_with_the_account_password() {
        let live = temp("move-live");
        let (old, _) = Vault::create(&live, "local pw", KdfParams::TEST).unwrap();
        let (account_vault, _) =
            Vault::create(temp("move-account"), "account pw", KdfParams::TEST).unwrap();
        let account = crate::sync::SyncAccount {
            server: "https://sync.example.com".into(),
            name: "ana".into(),
            device: 1,
            token: crate::hosts::Secret::new("t"),
        };
        let joining = live.with_extension("joining.anv");
        let mut joined = Vault::join(
            &joining,
            &account_vault.key_file_bytes(),
            "account pw",
            account,
        )
        .unwrap();
        assert!(matches!(
            Vault::join(
                temp("move-wrong"),
                &account_vault.key_file_bytes(),
                "local pw",
                joined.sync_state().account.clone().unwrap()
            ),
            Err(VaultError::WrongPassword)
        ));
        drop(old);
        joined.move_to(&live).unwrap();
        assert!(!joining.exists());
        assert!(Vault::unlock(&live, "account pw").is_ok());
        assert!(matches!(
            Vault::unlock(&live, "local pw"),
            Err(VaultError::WrongPassword)
        ));
    }

    #[test]
    fn keys_are_stored_encrypted_and_deleting_one_detaches_its_hosts() {
        let path = temp("keys");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        let key = SshKey {
            id: vault.next_key_id(),
            label: "work".into(),
            private: crate::hosts::Secret::new("-----BEGIN OPENSSH PRIVATE KEY----- secret"),
            public: "ssh-ed25519 AAAA work".into(),
            fingerprint: "SHA256:x".into(),
        };
        vault.upsert_key(key.clone()).unwrap();
        let mut host = host(&vault, "db");
        host.key_id = Some(key.id);
        vault.upsert_host(host).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret"));

        let mut reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.ssh_keys(), std::slice::from_ref(&key));
        assert_eq!(reopened.hosts()[0].key_id, Some(key.id));
        reopened.remove_key(key.id).unwrap();
        let reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.ssh_keys(), []);
        assert_eq!(reopened.hosts()[0].key_id, None);
    }

    #[test]
    fn a_password_change_off_the_ui_thread_needs_the_current_password() {
        let path = temp("rewrap");
        let (mut vault, _) = Vault::create(&path, "old pw", KdfParams::TEST).unwrap();
        vault.upsert_host(host(&vault, "db")).unwrap();
        let kit = vault.password_kit();
        assert!(matches!(
            kit.rewrap("wrong", "new pw"),
            Err(VaultError::WrongPassword)
        ));
        let file = kit.rewrap("old pw", "new pw").unwrap();
        vault.set_key_file(&file).unwrap();
        assert!(matches!(
            Vault::unlock(&path, "old pw"),
            Err(VaultError::WrongPassword)
        ));
        assert_eq!(
            Vault::unlock(&path, "new pw").unwrap().hosts()[0].label,
            "db"
        );
    }

    #[test]
    fn hosts_survive_lock_and_unlock_and_never_appear_in_the_clear() {
        let path = temp("roundtrip");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        let prod = host(&vault, "prod-db");
        vault.upsert_host(prod).unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("prod-db"),
            "host names must not be readable on disk"
        );

        let reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.hosts()[0].label, "prod-db");
        assert!(matches!(
            Vault::unlock(&path, "nope"),
            Err(VaultError::WrongPassword)
        ));
    }

    #[test]
    fn the_recovery_key_opens_the_vault_and_a_new_password_replaces_the_old() {
        let path = temp("recover");
        let (mut vault, recovery) = Vault::create(&path, "forgotten", KdfParams::TEST).unwrap();
        let web = host(&vault, "web");
        vault.upsert_host(web).unwrap();
        let mut recovered = Vault::recover(&path, &recovery).unwrap();
        recovered.change_password("new").unwrap();
        assert_eq!(Vault::unlock(&path, "new").unwrap().hosts().len(), 1);
        assert!(matches!(
            Vault::unlock(&path, "forgotten"),
            Err(VaultError::WrongPassword)
        ));
    }

    #[test]
    fn a_damaged_vault_is_reported_and_left_alone() {
        let path = temp("damaged");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        let web = host(&vault, "web");
        vault.upsert_host(web).unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        // Flip one digit of the last ciphertext: the host record, not the wrapped key (that
        // would read as a wrong password, since the key can no longer be unwrapped).
        let at = text.rfind("\"ciphertext\":[").unwrap() + 14;
        let digit = text.as_bytes()[at];
        text.replace_range(at..=at, if digit == b'1' { "2" } else { "1" });
        std::fs::write(&path, &text).unwrap();
        assert!(matches!(
            Vault::unlock(&path, "pw"),
            Err(VaultError::Damaged(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn plain_hosts_are_imported_and_the_plain_file_removed_only_after_saving() {
        let path = temp("import");
        let plain = path.with_file_name("hosts.json");
        std::fs::create_dir_all(plain.parent().unwrap()).unwrap();
        std::fs::write(
            &plain,
            br#"{"version":1,"hosts":[{"id":7,"label":"old","address":"old.example"}]}"#,
        )
        .unwrap();
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        assert_eq!(vault.import_plain_hosts(&plain).unwrap(), 1);
        assert!(!plain.exists());
        assert_eq!(Vault::unlock(&path, "pw").unwrap().hosts()[0].label, "old");
    }
}
