//! The encrypted vault file (ADR-0007): the key file and one encrypted record per entity.
//! Nothing secret is on disk in the clear, not even host names. Writes are atomic, and a file
//! that cannot be read is reported and never overwritten.
//!
//! This single-file container is the step before SQLite storage; the key hierarchy
//! and record format carry over unchanged.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, KdfParams, KeyFile, Keys, Sealed};
use crate::forwards::PortForward;
use crate::groups::{self, Group, Sources};
use crate::hosts::Host;
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
/// Record kind of a host group.
const KIND_GROUP: u8 = 8;

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
    /// The change is not allowed; nothing was saved.
    #[error("{0}")]
    Refused(&'static str),
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

/// Everything the vault file holds, borrowed: what `save_all` writes.
#[derive(Clone, Copy)]
struct Parts<'a> {
    hosts: &'a [Host],
    forwards: &'a [PortForward],
    ssh_keys: &'a [SshKey],
    snippets: &'a [Snippet],
    logs: &'a [SessionLog],
    groups: &'a [Group],
    sync: &'a SyncState,
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
    groups: Vec<Group>,
    sync: SyncState,
    /// Counts saved changes, so a sync result made from an older snapshot is not applied.
    generation: u64,
    /// Ids handed out by the `draft_*` calls and not saved yet, by entity kind. A sync can
    /// give one of them to an arriving entity while the editor is open; saving the draft
    /// then takes a new id instead of overwriting the arrival (ADR-0008 data-loss rules).
    drafts: Mutex<HashSet<(u8, u64)>>,
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
            groups: Vec::new(),
            sync: SyncState::new()?,
            generation: 0,
            drafts: Mutex::default(),
        };
        vault.save_all(&vault.parts())?;
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
        let mut groups = Vec::new();
        let mut sync = None;
        for record in &file.records {
            // serde's message can quote the decrypted text (a password, a private key), so
            // only where reading stopped is kept.
            let damaged = |error: serde_json::Error| {
                VaultError::Damaged(format!(
                    "a record of kind {} does not read (line {}, column {})",
                    record.kind,
                    error.line(),
                    error.column()
                ))
            };
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
                KIND_GROUP => {
                    let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
                    groups.push(serde_json::from_slice::<Group>(&plaintext).map_err(damaged)?);
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
        groups.sort_by_key(|group: &Group| group.name.to_lowercase());
        Ok(Self {
            path,
            key_file: file.keys,
            keys,
            hosts,
            forwards,
            ssh_keys,
            snippets,
            logs,
            groups,
            sync,
            generation: 0,
            drafts: Mutex::default(),
        })
    }

    /// Changes the master password; records are untouched and the recovery key still works.
    ///
    /// # Errors
    /// Writing the file, or no randomness.
    pub fn change_password(&mut self, password: &str) -> Result<(), VaultError> {
        let key_file = self.keys.change_password(&self.key_file, password)?;
        let previous = std::mem::replace(&mut self.key_file, key_file);
        if let Err(error) = self.save_all(&self.parts()) {
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
        if let Err(error) = self.save_all(&self.parts()) {
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

    /// An id for a new draft of `kind`: above `fresh` (what the vault and sync state know)
    /// and above every other draft still open, noted as handed out.
    fn issue(&self, kind: u8, fresh: u64) -> u64 {
        let Ok(mut drafts) = self.drafts.lock() else {
            return fresh;
        };
        let open = drafts
            .iter()
            .filter(|(of, _)| *of == kind)
            .map(|(_, id)| id + 1)
            .max()
            .unwrap_or(0);
        let id = fresh.max(open);
        drafts.insert((kind, id));
        id
    }

    /// The id a saved entity of `kind` gets: its own, unless it is a draft whose id was
    /// meanwhile taken (by an entity a sync brought in), then a new one.
    fn settle_id(&self, kind: u8, id: u64, taken: impl Iterator<Item = u64> + Clone) -> u64 {
        let was_draft = self
            .drafts
            .lock()
            .is_ok_and(|mut drafts| drafts.remove(&(kind, id)));
        if was_draft && taken.clone().any(|existing| existing == id) {
            let fresh = self.sync.fresh_id(kind, taken);
            let id = self.issue(kind, fresh);
            // Saved now, so no longer a draft.
            if let Ok(mut drafts) = self.drafts.lock() {
                drafts.remove(&(kind, id));
            }
            id
        } else {
            id
        }
    }

    /// A new, unsaved host with a fresh id.
    #[must_use]
    pub fn draft_host(&self) -> Host {
        let fresh = self
            .sync
            .fresh_id(kind::HOST, self.hosts.iter().map(|host| host.id));
        let id = self.issue(kind::HOST, fresh);
        Host {
            id,
            label: String::new(),
            address: String::new(),
            port: None,
            username: String::new(),
            tags: Vec::new(),
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
            no_reconnect: false,
            group: None,
            env: Vec::new(),
            startup: String::new(),
            term: String::new(),
            os: String::new(),
            os_by_hand: false,
            notes: String::new(),
        }
    }

    /// Adds or replaces a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_host(&mut self, mut host: Host) -> Result<(), VaultError> {
        host.id = self.settle_id(kind::HOST, host.id, self.hosts.iter().map(|h| h.id));
        let id = host.id;
        let mut next = self.hosts.clone();
        match next.iter_mut().find(|existing| existing.id == host.id) {
            Some(existing) => *existing = host,
            None => next.push(host),
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::HOST, id, false)?;
        self.save_all(&Parts {
            hosts: &next,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Takes `tag` off every host that has it, in one save; returns how many changed
    /// (F-HOST-004).
    ///
    /// # Errors
    /// Writing the file; nothing changes then.
    pub fn remove_tag(&mut self, tag: &str) -> Result<usize, VaultError> {
        let mut next = self.hosts.clone();
        let mut sync = self.sync.clone();
        let mut changed = 0;
        for host in next
            .iter_mut()
            .filter(|host| host.tags.iter().any(|t| t == tag))
        {
            host.tags.retain(|t| t != tag);
            sync.touch(kind::HOST, host.id, false)?;
            changed += 1;
        }
        if changed == 0 {
            return Ok(0);
        }
        self.save_all(&Parts {
            hosts: &next,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = next;
        self.sync = sync;
        self.generation += 1;
        Ok(changed)
    }

    /// Deletes a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_host(&mut self, id: u64) -> Result<(), VaultError> {
        self.remove_hosts(&[id])
    }

    /// Deletes several hosts in one save; nothing keeps pointing at them (jump links, group
    /// defaults and forward rules are cleared). On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_hosts(&mut self, ids: &[u64]) -> Result<(), VaultError> {
        let gone = |id: Option<u64>| id.is_some_and(|id| ids.contains(&id));
        let mut next: Vec<Host> = self
            .hosts
            .iter()
            .filter(|host| !ids.contains(&host.id))
            .cloned()
            .collect();
        let mut sync = self.sync.clone();
        // Nothing may keep pointing at the ids: a new host could get one and be used instead.
        for host in next.iter_mut().filter(|host| gone(host.jump_id)) {
            host.jump_id = None;
            sync.touch(kind::HOST, host.id, false)?;
        }
        let mut groups = self.groups.clone();
        for group in groups
            .iter_mut()
            .filter(|group| gone(group.defaults.jump_id))
        {
            group.defaults.jump_id = None;
            sync.touch(kind::GROUP, group.id, false)?;
        }
        let mut forwards = self.forwards.clone();
        for rule in forwards.iter_mut().filter(|rule| gone(Some(rule.host_id))) {
            rule.host_id = 0;
            sync.touch(kind::FORWARD, rule.id, false)?;
        }
        for id in ids {
            sync.touch(kind::HOST, *id, true)?;
        }
        self.save_all(&Parts {
            hosts: &next,
            forwards: &forwards,
            groups: &groups,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = next;
        self.forwards = forwards;
        self.groups = groups;
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
        let mut id = sync.fresh_id(kind::HOST, next.iter().map(|host| host.id)) - 1;
        for mut host in imported.iter().cloned() {
            id += 1;
            host.id = id;
            sync.touch(kind::HOST, id, false)?;
            next.push(host);
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        self.save_all(&Parts {
            hosts: &next,
            sync: &sync,
            ..self.parts()
        })?;
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
        let fresh = self
            .sync
            .fresh_id(kind::FORWARD, self.forwards.iter().map(|rule| rule.id));
        let id = self.issue(kind::FORWARD, fresh);
        PortForward::new(id, host_id)
    }

    /// Adds or replaces a rule, then saves; returns the id it is saved under, which differs
    /// from a draft's when a rule synced meanwhile took that id. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_forward(&mut self, mut rule: PortForward) -> Result<u64, VaultError> {
        rule.id = self.settle_id(kind::FORWARD, rule.id, self.forwards.iter().map(|r| r.id));
        let id = rule.id;
        let mut next = self.forwards.clone();
        match next.iter_mut().find(|existing| existing.id == rule.id) {
            Some(existing) => *existing = rule,
            None => next.push(rule),
        }
        next.sort_by_key(|rule| rule.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::FORWARD, id, false)?;
        self.save_all(&Parts {
            forwards: &next,
            sync: &sync,
            ..self.parts()
        })?;
        self.forwards = next;
        self.sync = sync;
        self.generation += 1;
        Ok(id)
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
        self.save_all(&Parts {
            forwards: &next,
            sync: &sync,
            ..self.parts()
        })?;
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

    /// Moves the vault file to `path`, replacing what is there. That is kept as
    /// `<name>.bak` first, so a replaced vault can still be opened.
    ///
    /// # Errors
    /// The file system refused; the vault stays where it was.
    pub fn move_to(&mut self, path: &Path) -> Result<(), VaultError> {
        if path.exists() {
            std::fs::copy(path, crate::files::backup_path(path))?;
        }
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
            groups: self.groups.clone(),
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
        self.save_all(&Parts {
            hosts: &copy.hosts,
            forwards: &copy.forwards,
            ssh_keys: &copy.ssh_keys,
            snippets: &copy.snippets,
            logs: &copy.logs,
            groups: &copy.groups,
            sync: &copy.state,
        })?;
        self.hosts = copy.hosts;
        self.forwards = copy.forwards;
        self.ssh_keys = copy.ssh_keys;
        self.snippets = copy.snippets;
        self.logs = copy.logs;
        self.groups = copy.groups;
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
        self.save_all(&Parts {
            sync: &sync,
            ..self.parts()
        })?;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Adds `hosts`, `forwards`, keys, snippets and groups from another vault as new
    /// entities (fresh local ids, links remapped to them), so joining an account keeps what
    /// was on this device (ADR-0008 data-loss rule 4). They are pushed at the next sync.
    /// Groups keep their uids, which are unique everywhere.
    ///
    /// # Errors
    /// Writing the file; nothing is added then.
    pub fn adopt(
        &mut self,
        hosts: &[Host],
        forwards: &[PortForward],
        ssh_keys: &[SshKey],
        snippets: &[Snippet],
        groups: &[Group],
    ) -> Result<(), VaultError> {
        let mut next_snippets = self.snippets.clone();
        let mut snippet_id = self
            .sync
            .fresh_id(kind::SNIPPET, next_snippets.iter().map(|s| s.id))
            - 1;
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
        let mut key_id = sync.fresh_id(kind::KEY, next_keys.iter().map(|key| key.id)) - 1;
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
        let mut next_groups = self.groups.clone();
        let mut group_id = sync.fresh_id(kind::GROUP, next_groups.iter().map(|group| group.id)) - 1;
        for group in groups {
            group_id += 1;
            sync.touch(kind::GROUP, group_id, false)?;
            let mut adopted = group.clone();
            adopted.id = group_id;
            adopted.defaults.key_id = group
                .defaults
                .key_id
                .and_then(|key| new_key_ids.get(&key).copied());
            next_groups.push(adopted);
        }
        next_groups.sort_by_key(|group| group.name.to_lowercase());
        let mut host_id = sync.fresh_id(kind::HOST, next_hosts.iter().map(|host| host.id)) - 1;
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
        for group in next_groups
            .iter_mut()
            .filter(|g| groups.iter().any(|a| a.uid == g.uid))
        {
            group.defaults.jump_id = group
                .defaults
                .jump_id
                .and_then(|jump| new_ids.get(&jump).copied());
        }
        let mut rule_id =
            sync.fresh_id(kind::FORWARD, next_forwards.iter().map(|rule| rule.id)) - 1;
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
        self.save_all(&Parts {
            hosts: &next_hosts,
            forwards: &next_forwards,
            ssh_keys: &next_keys,
            snippets: &next_snippets,
            groups: &next_groups,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = next_hosts;
        self.forwards = next_forwards;
        self.ssh_keys = next_keys;
        self.snippets = next_snippets;
        self.groups = next_groups;
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
        if let Err(error) = self.save_all(&Parts {
            sync: &sync,
            ..self.parts()
        }) {
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
            groups: Vec::new(),
            sync,
            generation: 0,
            drafts: Mutex::default(),
        };
        vault.save_all(&vault.parts())?;
        Ok(vault)
    }

    /// The host groups, sorted by name (F-HOST-003).
    #[must_use]
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// A new, unsaved group inside `parent` (a group uid, or `None` for the top level).
    ///
    /// # Errors
    /// No randomness for its uid.
    pub fn draft_group(&self, parent: Option<String>) -> Result<Group, VaultError> {
        let fresh = self
            .sync
            .fresh_id(kind::GROUP, self.groups.iter().map(|group| group.id));
        let id = self.issue(kind::GROUP, fresh);
        Ok(Group {
            id,
            uid: groups::new_uid()?,
            name: String::new(),
            parent,
            defaults: groups::Defaults::default(),
        })
    }

    /// Adds or replaces a group, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// [`VaultError::Refused`] for an empty name or a parent inside the group itself;
    /// writing the file.
    pub fn upsert_group(&mut self, mut group: Group) -> Result<(), VaultError> {
        if group.name.trim().is_empty() {
            return Err(VaultError::Refused("Give the group a name."));
        }
        if let Some(parent) = &group.parent
            && groups::is_within(&self.groups, parent, &group.uid)
        {
            return Err(VaultError::Refused("A group can't go inside itself."));
        }
        group.id = self.settle_id(kind::GROUP, group.id, self.groups.iter().map(|g| g.id));
        let id = group.id;
        let mut next = self.groups.clone();
        match next.iter_mut().find(|existing| existing.id == id) {
            Some(existing) => *existing = group,
            None => next.push(group),
        }
        next.sort_by_key(|group| group.name.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::GROUP, id, false)?;
        self.save_all(&Parts {
            groups: &next,
            sync: &sync,
            ..self.parts()
        })?;
        self.groups = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Deletes a group; its hosts and subgroups move up into its parent, so nothing else is
    /// lost.
    ///
    /// # Errors
    /// Writing the file; nothing changes then.
    pub fn remove_group(&mut self, id: u64) -> Result<(), VaultError> {
        let Some(gone) = self.groups.iter().find(|group| group.id == id).cloned() else {
            return Ok(());
        };
        let mut sync = self.sync.clone();
        let mut groups: Vec<Group> = self
            .groups
            .iter()
            .filter(|group| group.id != id)
            .cloned()
            .collect();
        for group in groups
            .iter_mut()
            .filter(|group| group.parent.as_deref() == Some(gone.uid.as_str()))
        {
            group.parent.clone_from(&gone.parent);
            sync.touch(kind::GROUP, group.id, false)?;
        }
        let mut hosts = self.hosts.clone();
        for host in hosts
            .iter_mut()
            .filter(|host| host.group.as_deref() == Some(gone.uid.as_str()))
        {
            host.group.clone_from(&gone.parent);
            sync.touch(kind::HOST, host.id, false)?;
        }
        sync.touch(kind::GROUP, id, true)?;
        self.save_all(&Parts {
            hosts: &hosts,
            groups: &groups,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = hosts;
        self.groups = groups;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// `host` with what it inherits from its groups filled in, and where each inherited
    /// value came from. Connect with the resolved host.
    #[must_use]
    pub fn resolve(&self, host: &Host) -> (Host, Sources) {
        groups::resolve(host, &self.groups)
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
        self.sync
            .fresh_id(kind::KEY, self.ssh_keys.iter().map(|key| key.id))
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
        self.save_all(&Parts {
            ssh_keys: &next,
            sync: &sync,
            ..self.parts()
        })?;
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
        let mut groups = self.groups.clone();
        for group in groups
            .iter_mut()
            .filter(|group| group.defaults.key_id == Some(id))
        {
            group.defaults.key_id = None;
            sync.touch(kind::GROUP, group.id, false)?;
        }
        sync.touch(kind::KEY, id, true)?;
        self.save_all(&Parts {
            hosts: &hosts,
            ssh_keys: &keys,
            groups: &groups,
            sync: &sync,
            ..self.parts()
        })?;
        self.hosts = hosts;
        self.ssh_keys = keys;
        self.groups = groups;
        self.sync = sync;
        self.generation += 1;
        Ok(())
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
        let id = self
            .sync
            .fresh_id(kind::LOG, self.logs.iter().map(|log| log.id));
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
        self.save_all(&Parts {
            logs: &next,
            sync: &sync,
            ..self.parts()
        })?;
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
        self.save_all(&Parts {
            logs: &next,
            sync: &sync,
            ..self.parts()
        })?;
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
        let fresh = self
            .sync
            .fresh_id(kind::SNIPPET, self.snippets.iter().map(|s| s.id));
        let id = self.issue(kind::SNIPPET, fresh);
        Snippet {
            id,
            label: String::new(),
            script: String::new(),
            package: String::new(),
        }
    }

    /// Adds or replaces a snippet, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_snippet(&mut self, mut snippet: Snippet) -> Result<(), VaultError> {
        snippet.id = self.settle_id(
            kind::SNIPPET,
            snippet.id,
            self.snippets.iter().map(|s| s.id),
        );
        let id = snippet.id;
        let mut next = self.snippets.clone();
        match next.iter_mut().find(|existing| existing.id == id) {
            Some(existing) => *existing = snippet,
            None => next.push(snippet),
        }
        next.sort_by_key(|snippet| snippet.label.to_lowercase());
        let mut sync = self.sync.clone();
        sync.touch(kind::SNIPPET, id, false)?;
        self.save_all(&Parts {
            snippets: &next,
            sync: &sync,
            ..self.parts()
        })?;
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
        self.save_all(&Parts {
            snippets: &next,
            sync: &sync,
            ..self.parts()
        })?;
        self.snippets = next;
        self.sync = sync;
        self.generation += 1;
        Ok(())
    }

    /// Everything the file holds, as it is now; change a field to save something new.
    fn parts(&self) -> Parts<'_> {
        Parts {
            hosts: &self.hosts,
            forwards: &self.forwards,
            ssh_keys: &self.ssh_keys,
            snippets: &self.snippets,
            logs: &self.logs,
            groups: &self.groups,
            sync: &self.sync,
        }
    }

    fn save_all(&self, parts: &Parts<'_>) -> Result<(), VaultError> {
        let Parts {
            hosts,
            forwards,
            ssh_keys,
            snippets,
            logs,
            groups,
            sync,
        } = *parts;
        let mut records = Vec::with_capacity(
            hosts.len()
                + forwards.len()
                + ssh_keys.len()
                + snippets.len()
                + logs.len()
                + groups.len()
                + 1,
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
        for group in groups {
            let id = format!("group-{}", group.id);
            let plaintext = Zeroizing::new(serde_json::to_vec(group).map_err(damaged)?);
            let sealed = self.keys.seal_record(&id, KIND_GROUP, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_GROUP,
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
        // Flushed, owner-only, the previous version kept beside it.
        crate::files::write_private(&self.path, &bytes, true)?;
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
    fn two_drafts_open_at_once_never_share_an_id() {
        let (mut vault, _) = Vault::create(temp("two-drafts"), "pw", KdfParams::TEST).unwrap();
        let mut first = vault.draft_host();
        let mut second = vault.draft_host();
        assert_ne!(first.id, second.id);
        (second.label, second.address) = ("second".into(), "b".into());
        vault.upsert_host(second).unwrap();
        (first.label, first.address) = ("first".into(), "a".into());
        vault.upsert_host(first).unwrap();
        let labels: Vec<&str> = vault.hosts().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["first", "second"]);
        // Saving an existing host again is an edit, not a new one.
        let mut edited = vault.hosts()[0].clone();
        edited.address = "a2".into();
        vault.upsert_host(edited).unwrap();
        assert_eq!(vault.hosts().len(), 2);
        assert_eq!(vault.hosts()[0].address, "a2");
    }

    #[test]
    fn a_tag_comes_off_every_host_in_one_go() {
        let (mut vault, _) = Vault::create(temp("tags"), "pw", KdfParams::TEST).unwrap();
        for (label, tags) in [
            ("a", vec!["prod", "db"]),
            ("b", vec!["prod"]),
            ("c", vec!["dev"]),
        ] {
            let mut host = vault.draft_host();
            (host.label, host.address) = (label.into(), "x".into());
            host.tags = tags.into_iter().map(Into::into).collect();
            vault.upsert_host(host).unwrap();
        }
        assert_eq!(vault.remove_tag("prod").unwrap(), 2);
        let tags: Vec<Vec<String>> = vault.hosts().iter().map(|h| h.tags.clone()).collect();
        assert_eq!(
            tags,
            [vec!["db".to_owned()], vec![], vec!["dev".to_owned()]]
        );
        assert_eq!(vault.remove_tag("missing").unwrap(), 0);
    }

    #[test]
    fn deleting_a_host_leaves_nothing_pointing_at_its_id() {
        let (mut vault, _) = Vault::create(temp("dangling"), "pw", KdfParams::TEST).unwrap();
        let mut bastion = vault.draft_host();
        (bastion.label, bastion.address) = ("bastion".into(), "b".into());
        vault.upsert_host(bastion.clone()).unwrap();
        let mut web = vault.draft_host();
        (web.label, web.address, web.jump_id) = ("web".into(), "w".into(), Some(bastion.id));
        vault.upsert_host(web).unwrap();
        let mut group = vault.draft_group(None).unwrap();
        group.name = "g".into();
        group.defaults.jump_id = Some(bastion.id);
        vault.upsert_group(group).unwrap();
        let mut rule = vault.draft_forward(bastion.id);
        rule.label = "r".into();
        vault.upsert_forward(rule).unwrap();

        vault.remove_host(bastion.id).unwrap();
        assert_eq!(vault.hosts()[0].jump_id, None);
        assert_eq!(vault.groups()[0].defaults.jump_id, None);
        assert_eq!(vault.forwards()[0].host_id, 0, "shows as (host deleted)");
        // A new host may get the old id; nothing routes through it by accident.
        let mut fresh = vault.draft_host();
        (fresh.label, fresh.address) = ("new".into(), "n".into());
        vault.upsert_host(fresh).unwrap();
        assert!(vault.hosts().iter().all(|host| host.jump_id.is_none()));
    }

    #[test]
    fn deleting_a_group_moves_its_hosts_and_subgroups_up_and_loops_are_refused() {
        let path = temp("groups");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        let mut top = vault.draft_group(None).unwrap();
        top.name = "top".into();
        vault.upsert_group(top.clone()).unwrap();
        let mut mid = vault.draft_group(Some(top.uid.clone())).unwrap();
        mid.name = "mid".into();
        vault.upsert_group(mid.clone()).unwrap();
        let mut leaf = vault.draft_group(Some(mid.uid.clone())).unwrap();
        leaf.name = "leaf".into();
        vault.upsert_group(leaf.clone()).unwrap();
        let mut host = vault.draft_host();
        host.label = "h".into();
        host.address = "x".into();
        host.group = Some(mid.uid.clone());
        vault.upsert_host(host).unwrap();

        let mut looped = top.clone();
        looped.parent = Some(leaf.uid.clone());
        assert!(matches!(
            vault.upsert_group(looped),
            Err(VaultError::Refused(_))
        ));
        let mut unnamed = vault.draft_group(None).unwrap();
        unnamed.name = " ".into();
        assert!(vault.upsert_group(unnamed).is_err());

        vault.remove_group(mid.id).unwrap();
        let leaf_now = vault.groups().iter().find(|g| g.uid == leaf.uid).unwrap();
        assert_eq!(leaf_now.parent.as_deref(), Some(top.uid.as_str()));
        assert_eq!(vault.hosts()[0].group.as_deref(), Some(top.uid.as_str()));
        let reopened = Vault::unlock(&path, "pw").unwrap();
        assert_eq!(reopened.groups().len(), 2);
    }

    #[test]
    fn a_draft_rule_whose_id_was_taken_meanwhile_is_saved_under_the_id_it_answers() {
        let (mut vault, _) = Vault::create(temp("forward-id"), "pw", KdfParams::TEST).unwrap();
        vault.upsert_host(host(&vault, "bastion")).unwrap();
        let mut mine = vault.draft_forward(vault.hosts()[0].id);
        mine.label = "mine".into();
        // While it is open, a rule synced from another device arrives with the same id.
        let mut theirs = mine.clone();
        theirs.label = "theirs".into();
        vault.forwards.push(theirs);

        let id = vault.upsert_forward(mine.clone()).unwrap();
        assert_ne!(id, mine.id);
        let label = |id: u64| {
            vault
                .forwards()
                .iter()
                .find(|r| r.id == id)
                .map(|r| &*r.label)
        };
        assert_eq!((label(id), label(mine.id)), (Some("mine"), Some("theirs")));
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
        vault.adopt(&[old], &[rule], &[], &[], &[]).unwrap();

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
            pin: None,
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
        assert!(
            Vault::unlock(crate::files::backup_path(&live), "local pw").is_ok(),
            "the replaced vault is kept"
        );
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
        // Change one character of the last ciphertext: a record, not the wrapped key (that
        // would read as a wrong password, since the key can no longer be unwrapped).
        let at = text.rfind("\"ciphertext\":\"").unwrap() + 14;
        let first = text.as_bytes()[at];
        text.replace_range(at..=at, if first == b'A' { "B" } else { "A" });
        std::fs::write(&path, &text).unwrap();
        assert!(matches!(
            Vault::unlock(&path, "pw"),
            Err(VaultError::Damaged(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn a_record_that_does_not_read_is_reported_without_its_text() {
        let path = temp("unreadable");
        let (mut vault, _) = Vault::create(&path, "pw", KdfParams::TEST).unwrap();
        let web = host(&vault, "web");
        vault.upsert_host(web).unwrap();
        let mut file = read(&path).unwrap();
        let record = file
            .records
            .iter_mut()
            .find(|record| record.kind == KIND_HOST)
            .unwrap();
        record.sealed = vault
            .keys
            .seal_record(&record.id, KIND_HOST, br#"{"id":"hunter2-secret"}"#)
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let Err(VaultError::Damaged(why)) = Vault::unlock(&path, "pw") else {
            panic!("a record that does not read is damage");
        };
        assert!(!why.contains("hunter2"), "{why}");
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
