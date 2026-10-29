//! What the vault keeps for account sync (ADR-0008): a random sync id per entity, the server
//! version each copy is based on, which entities changed since the last sync, and the
//! hybrid-logical-clock time of each change. Stored encrypted like every other record.
//!
//! A [`SyncCopy`] is a snapshot the sync thread works on without holding the vault; the vault
//! takes its result back with [`crate::vault::Vault::commit_sync`].

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, Keys, Sealed};
use crate::forwards::PortForward;
use crate::groups::Group;
use crate::hosts::{Host, Secret};
use crate::keychain::SshKey;
use crate::logs::SessionLog;
use crate::snippets::Snippet;

/// Entity kinds inside synced records.
pub mod kind {
    /// A saved host.
    pub const HOST: u8 = 1;
    /// A port-forwarding rule.
    pub const FORWARD: u8 = 2;
    /// A keychain key.
    pub const KEY: u8 = 3;
    /// A snippet.
    pub const SNIPPET: u8 = 4;
    /// A session log.
    pub const LOG: u8 = 5;
    /// A host group.
    pub const GROUP: u8 = 6;
}

/// The record kind (bound into the encryption) of envelopes sent to the server.
const KIND_SYNCED: u8 = 4;

/// The signed-in account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncAccount {
    /// Base URL of the server.
    pub server: String,
    /// The account name.
    pub name: String,
    /// This device's id on the server.
    pub device: u64,
    /// This device's token.
    pub token: Secret,
}

/// Sync bookkeeping for one entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// [`kind::HOST`] or [`kind::FORWARD`].
    pub kind: u8,
    /// The entity's local id.
    pub local: u64,
    /// The server version this copy is based on; 0 before the first push.
    pub base: u64,
    /// Changed locally since `base`.
    pub dirty: bool,
    /// Deleted (a tombstone until the deletion is pushed, and after).
    pub deleted: bool,
    /// When it last changed.
    pub hlc: u64,
    /// A rule whose host has not arrived yet: the host's sync id, to link once it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaits: Option<String>,
    /// A host whose jump host has not arrived yet: that host's sync id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaits_jump: Option<String>,
}

/// Everything the vault stores for sync.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// The account, while signed in.
    pub account: Option<SyncAccount>,
    /// The newest server sequence number seen.
    pub cursor: u64,
    /// This device's number in clock times.
    pub node: u16,
    /// The newest clock time seen.
    pub clock: u64,
    /// Entries by sync id.
    pub entries: BTreeMap<String, Entry>,
    /// When the last sync round finished, seconds since the Unix epoch.
    #[serde(default)]
    pub last_synced: u64,
    /// Per kind, the highest local id among entries `restart` forgot, so it is not reused.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub retired: BTreeMap<u8, u64>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// A random 128-bit id as hex.
pub(crate) fn random_id() -> Result<String, CryptoError> {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| CryptoError::Randomness)?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

impl SyncState {
    /// A fresh state for a new device.
    ///
    /// # Errors
    /// No randomness.
    pub(crate) fn new() -> Result<Self, CryptoError> {
        let mut node = [0u8; 2];
        getrandom::fill(&mut node).map_err(|_| CryptoError::Randomness)?;
        Ok(Self {
            node: u16::from_le_bytes(node),
            ..Self::default()
        })
    }

    /// A new clock time, after every time seen so far.
    fn tick(&mut self) -> u64 {
        let wall = (now_millis() << 16) | u64::from(self.node);
        self.clock = if wall > self.clock {
            wall
        } else {
            (((self.clock >> 16) + 1) << 16) | u64::from(self.node)
        };
        self.clock
    }

    /// The sync id of local entity (`kind`, `local`).
    #[must_use]
    pub fn id_of(&self, kind: u8, local: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.kind == kind && entry.local == local && !entry.deleted)
            .map(|(id, _)| id.as_str())
    }

    /// A local id for a new entity of `kind` that no entity, live or deleted, has had. Ids are
    /// never reused, so a link left pointing at a deleted entity (here, or on a device that
    /// has not synced the deletion yet) can never land on a new one.
    #[must_use]
    pub fn fresh_id(&self, kind: u8, live: impl Iterator<Item = u64>) -> u64 {
        let known = self
            .entries
            .values()
            .filter(|entry| entry.kind == kind)
            .map(|entry| entry.local);
        live.chain(known)
            .chain(self.retired.get(&kind).copied())
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Records a local change of (`kind`, `local`): a new entity gets a sync id.
    pub(crate) fn touch(&mut self, kind: u8, local: u64, deleted: bool) -> Result<(), CryptoError> {
        let hlc = self.tick();
        let id = match self.id_of(kind, local) {
            Some(id) => id.to_owned(),
            None if deleted => return Ok(()),
            None => random_id()?,
        };
        let entry = self.entries.entry(id).or_insert(Entry {
            kind,
            local,
            base: 0,
            dirty: true,
            deleted: false,
            hlc,
            awaits: None,
            awaits_jump: None,
        });
        entry.dirty = true;
        entry.deleted = deleted;
        entry.hlc = hlc;
        Ok(())
    }

    /// Makes everything held pending again with no server version, for a new account or
    /// a server that lost changes. Deletions never pushed are forgotten.
    pub(crate) fn restart(&mut self) {
        self.cursor = 0;
        for entry in self.entries.values() {
            if entry.deleted && entry.base == 0 {
                let high = self.retired.entry(entry.kind).or_default();
                *high = (*high).max(entry.local);
            }
        }
        self.entries
            .retain(|_, entry| !(entry.deleted && entry.base == 0));
        for entry in self.entries.values_mut() {
            entry.base = 0;
            entry.dirty = true;
        }
    }

    /// Changes not pushed yet.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.dirty && !(entry.deleted && entry.base == 0))
            .count()
    }
}

/// A host as synced: it names its key by sync id, since local ids differ per device.
/// Older clients sent the host alone, which still reads (with no key).
#[derive(Serialize, Deserialize)]
struct SyncedHost {
    #[serde(flatten)]
    host: Host,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jump: Option<String>,
}

/// A group as synced: its default key and jump host named by sync id (local ids differ
/// per device).
#[derive(Serialize, Deserialize)]
struct SyncedGroup {
    #[serde(flatten)]
    group: Group,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jump: Option<String>,
}

/// A forward rule as synced: it names its host by sync id, since local ids differ per
/// device.
#[derive(Serialize, Deserialize)]
struct SyncedForward {
    rule: PortForward,
    host: Option<String>,
}

/// A local change for the sync engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// Sync id.
    pub id: String,
    /// Server version it was made from.
    pub base: u64,
    /// When it was made.
    pub hlc: u64,
    /// `(kind, body)`, or `None` for a deletion.
    pub content: Option<(u8, Vec<u8>)>,
}

/// A snapshot of the vault for one sync round.
pub struct SyncCopy {
    pub(crate) keys: Keys,
    pub(crate) hosts: Vec<Host>,
    pub(crate) forwards: Vec<PortForward>,
    pub(crate) ssh_keys: Vec<SshKey>,
    pub(crate) snippets: Vec<Snippet>,
    pub(crate) logs: Vec<SessionLog>,
    pub(crate) groups: Vec<Group>,
    pub(crate) state: SyncState,
    pub(crate) generation: u64,
}

impl SyncCopy {
    /// The sync bookkeeping.
    #[must_use]
    pub fn state(&self) -> &SyncState {
        &self.state
    }

    /// The newest server sequence number seen.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.state.cursor
    }

    /// Remembers the newest server sequence number.
    pub fn set_cursor(&mut self, cursor: u64) {
        self.state.cursor = cursor;
    }

    /// Notes when this round finished.
    pub fn mark_synced(&mut self) {
        self.state.last_synced = now_millis() / 1000;
    }

    /// The server version of `id` held here.
    #[must_use]
    pub fn version(&self, id: &str) -> u64 {
        self.state.entries.get(id).map_or(0, |entry| entry.base)
    }

    /// Local changes to push. A record created and deleted before its first push is not
    /// sent at all.
    #[must_use]
    pub fn pending(&self) -> Vec<Pending> {
        let mut dirty: Vec<(&String, &Entry)> = self
            .state
            .entries
            .iter()
            .filter(|(_, entry)| entry.dirty && !(entry.deleted && entry.base == 0))
            .collect();
        // Keys, then hosts, then rules, so other devices usually see what each points at
        // first.
        dirty.sort_by_key(|(_, entry)| match entry.kind {
            kind::KEY => 0,
            kind::HOST => 1,
            _ => 2,
        });
        dirty
            .into_iter()
            .filter_map(|(id, entry)| {
                let content = if entry.deleted {
                    None
                } else {
                    Some((entry.kind, self.body(entry)?))
                };
                Some(Pending {
                    id: id.clone(),
                    base: entry.base,
                    hlc: entry.hlc,
                    content,
                })
            })
            .collect()
    }

    fn body(&self, entry: &Entry) -> Option<Vec<u8>> {
        match entry.kind {
            kind::HOST => {
                let host = self.hosts.iter().find(|host| host.id == entry.local)?;
                let synced = SyncedHost {
                    key: host
                        .key_id
                        .and_then(|key| self.state.id_of(kind::KEY, key))
                        .map(str::to_owned),
                    jump: host
                        .jump_id
                        .and_then(|jump| self.state.id_of(kind::HOST, jump))
                        .map(str::to_owned),
                    host: Host {
                        key_id: None,
                        jump_id: None,
                        ..host.clone()
                    },
                };
                serde_json::to_vec(&synced).ok()
            }
            kind::KEY => {
                let key = self.ssh_keys.iter().find(|key| key.id == entry.local)?;
                serde_json::to_vec(key).ok()
            }
            kind::SNIPPET => {
                let snippet = self.snippets.iter().find(|s| s.id == entry.local)?;
                serde_json::to_vec(snippet).ok()
            }
            kind::LOG => {
                let log = self.logs.iter().find(|log| log.id == entry.local)?;
                serde_json::to_vec(log).ok()
            }
            kind::GROUP => {
                let group = self.groups.iter().find(|group| group.id == entry.local)?;
                let mut plain = group.clone();
                (plain.defaults.key_id, plain.defaults.jump_id) = (None, None);
                let synced = SyncedGroup {
                    key: group
                        .defaults
                        .key_id
                        .and_then(|key| self.state.id_of(kind::KEY, key))
                        .map(str::to_owned),
                    jump: group
                        .defaults
                        .jump_id
                        .and_then(|jump| self.state.id_of(kind::HOST, jump))
                        .map(str::to_owned),
                    group: plain,
                };
                serde_json::to_vec(&synced).ok()
            }
            kind::FORWARD => {
                let rule = self.forwards.iter().find(|rule| rule.id == entry.local)?;
                let synced = SyncedForward {
                    // Starting on its own is this device's choice (a rule listening on
                    // 0.0.0.0 should not start everywhere), so it never syncs.
                    rule: PortForward {
                        auto_start: false,
                        with_host: false,
                        ..rule.clone()
                    },
                    host: self
                        .state
                        .id_of(kind::HOST, rule.host_id)
                        .map(str::to_owned),
                };
                serde_json::to_vec(&synced).ok()
            }
            _ => None,
        }
    }

    /// Encrypts `(kind, hlc, body)` for record `id` with the account key.
    ///
    /// # Errors
    /// No randomness.
    pub fn seal(&self, id: &str, kind: u8, hlc: u64, body: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut envelope = Zeroizing::new(Vec::with_capacity(body.len() + 9));
        envelope.push(kind);
        envelope.extend_from_slice(&hlc.to_le_bytes());
        envelope.extend_from_slice(body);
        let sealed = self.keys.seal_record(id, KIND_SYNCED, &envelope)?;
        serde_json::to_vec(&sealed).map_err(|_| CryptoError::Corrupt)
    }

    /// Decrypts record `id` into `(kind, hlc, body)`.
    ///
    /// # Errors
    /// Not made with this account's key, or altered.
    pub fn open(&self, id: &str, ciphertext: &[u8]) -> Result<(u8, u64, Vec<u8>), CryptoError> {
        let sealed: Sealed =
            serde_json::from_slice(ciphertext).map_err(|_| CryptoError::Corrupt)?;
        let envelope = self.keys.open_record(id, KIND_SYNCED, &sealed)?;
        let (kind, rest) = envelope.split_first().ok_or(CryptoError::Corrupt)?;
        if rest.len() < 8 {
            return Err(CryptoError::Corrupt);
        }
        let (hlc, body) = rest.split_at(8);
        let hlc = u64::from_le_bytes(hlc.try_into().map_err(|_| CryptoError::Corrupt)?);
        Ok((*kind, hlc, body.to_vec()))
    }

    /// The pending change of `id` was accepted as version `seq` (or the copy now holds
    /// version `seq`).
    pub fn pushed(&mut self, id: &str, seq: u64) {
        if let Some(entry) = self.state.entries.get_mut(id) {
            entry.base = seq;
            entry.dirty = false;
        }
    }

    /// Whether `key_file` (an account's, from sign-in) opens with `password` to this vault's
    /// own account key: then signing in only re-links, with nothing to merge. Runs
    /// Argon2id; call it off the UI thread.
    #[must_use]
    pub fn same_account(&self, key_file: &[u8], password: &str) -> bool {
        serde_json::from_slice::<crate::crypto::KeyFile>(key_file)
            .ok()
            .and_then(|file| Keys::unlock(&file, password).ok())
            .is_some_and(|keys| keys.same_account(&self.keys))
    }

    /// Records deleted after reaching the server: their sync ids and kinds, for the trash.
    #[must_use]
    pub fn deleted(&self) -> Vec<(String, u8)> {
        self.state
            .entries
            .iter()
            .filter(|(_, entry)| entry.deleted && entry.base > 0)
            .map(|(id, entry)| (id.clone(), entry.kind))
            .collect()
    }

    /// A short name for an entity body of `kind` (its label), for lists such as the trash.
    #[must_use]
    pub fn describe(kind: u8, body: &[u8]) -> String {
        #[derive(Deserialize)]
        struct Labelled {
            #[serde(default)]
            label: String,
            #[serde(default)]
            name: String,
            #[serde(default)]
            rule: Option<PortForward>,
        }
        let name = serde_json::from_slice::<Labelled>(body)
            .map(|labelled| match labelled.rule {
                Some(rule) => rule.label,
                None if labelled.label.is_empty() => labelled.name,
                None => labelled.label,
            })
            .unwrap_or_default();
        let what = match kind {
            kind::HOST => "Host",
            kind::FORWARD => "Port forward",
            kind::KEY => "Key",
            kind::SNIPPET => "Snippet",
            kind::LOG => "Session log",
            kind::GROUP => "Group",
            _ => "Item",
        };
        format!("{what} {name}")
    }

    /// Whether clock time `hlc` was made on this device.
    #[must_use]
    pub fn own_time(&self, hlc: u64) -> bool {
        hlc & 0xFFFF == u64::from(self.state.node)
    }

    /// Starts over against a server that lost changes.
    pub fn restart(&mut self) {
        self.state.restart();
    }

    /// Keeps the pending change of `id`, rebased on server version `seq`.
    pub fn rebase(&mut self, id: &str, seq: u64) {
        if let Some(entry) = self.state.entries.get_mut(id) {
            entry.base = seq;
        }
    }

    /// Takes the server's version `seq` of `id`: `content` is `(kind, hlc, body)`, or
    /// `None` when it was deleted. A body this version cannot read is skipped (kept on the
    /// server) rather than guessed at.
    pub fn apply(&mut self, id: &str, seq: u64, content: Option<(u8, u64, Vec<u8>)>) {
        let Some((kind, hlc, body)) = content else {
            self.apply_deletion(id, seq);
            return;
        };
        self.state.clock = self.state.clock.max(hlc);
        let existing = self.state.entries.get(id).map(|entry| entry.local);
        let mut awaits = None;
        let mut awaits_jump = None;
        let local = match kind {
            kind::HOST => {
                let Ok(synced) = serde_json::from_slice::<SyncedHost>(&body) else {
                    return;
                };
                let mut host = synced.host;
                host.id = existing.unwrap_or_else(|| {
                    self.state
                        .fresh_id(kind::HOST, self.hosts.iter().map(|h| h.id))
                });
                host.key_id = synced
                    .key
                    .as_ref()
                    .and_then(|key| self.state.entries.get(key))
                    .filter(|entry| entry.kind == kind::KEY && !entry.deleted)
                    .map(|entry| entry.local);
                awaits = synced.key.filter(|_| host.key_id.is_none());
                host.jump_id = synced
                    .jump
                    .as_ref()
                    .and_then(|jump| self.state.entries.get(jump))
                    .filter(|entry| entry.kind == kind::HOST && !entry.deleted)
                    .map(|entry| entry.local);
                awaits_jump = synced.jump.filter(|_| host.jump_id.is_none());
                let local = host.id;
                upsert(&mut self.hosts, host, |h| h.id, |h| h.label.to_lowercase());
                local
            }
            kind::FORWARD => {
                let Ok(synced) = serde_json::from_slice::<SyncedForward>(&body) else {
                    return;
                };
                let mut rule = synced.rule;
                rule.id = existing.unwrap_or_else(|| {
                    self.state
                        .fresh_id(kind::FORWARD, self.forwards.iter().map(|r| r.id))
                });
                let local = self.forwards.iter().find(|local| local.id == rule.id);
                rule.auto_start = local.is_some_and(|local| local.auto_start);
                rule.with_host = local.is_some_and(|local| local.with_host);
                // The host's local id here; 0 shows as "(host deleted)" until it arrives.
                rule.host_id = synced
                    .host
                    .as_ref()
                    .and_then(|host| self.state.entries.get(host))
                    .filter(|entry| entry.kind == kind::HOST && !entry.deleted)
                    .map_or(0, |entry| entry.local);
                awaits = synced.host.filter(|_| rule.host_id == 0);
                let local = rule.id;
                upsert(
                    &mut self.forwards,
                    rule,
                    |r| r.id,
                    |r| r.label.to_lowercase(),
                );
                local
            }
            kind::GROUP => match self.take_group(existing, &body) {
                Some((local, key, jump)) => {
                    (awaits, awaits_jump) = (key, jump);
                    local
                }
                None => return,
            },
            kind::KEY | kind::SNIPPET | kind::LOG => match self.take_plain(kind, existing, &body) {
                Some(local) => local,
                None => return,
            },
            _ => return,
        };
        self.state.entries.insert(
            id.to_owned(),
            Entry {
                kind,
                local,
                base: seq,
                dirty: false,
                deleted: false,
                hlc,
                awaits,
                awaits_jump,
            },
        );
        if kind == kind::HOST || kind == kind::KEY {
            self.link_waiting(id, local);
        }
    }

    /// Stores a group from `body` under its local id `existing` or a new one. Returns the
    /// local id and the sync ids of a default key and jump host that have not arrived yet,
    /// or `None` if `body` is unreadable.
    fn take_group(
        &mut self,
        existing: Option<u64>,
        body: &[u8],
    ) -> Option<(u64, Option<String>, Option<String>)> {
        let synced = serde_json::from_slice::<SyncedGroup>(body).ok()?;
        let mut group = synced.group;
        group.id = existing.unwrap_or_else(|| {
            self.state
                .fresh_id(kind::GROUP, self.groups.iter().map(|g| g.id))
        });
        let local_of = |id: &Option<String>, want: u8| {
            id.as_ref()
                .and_then(|id| self.state.entries.get(id))
                .filter(|entry| entry.kind == want && !entry.deleted)
                .map(|entry| entry.local)
        };
        group.defaults.key_id = local_of(&synced.key, kind::KEY);
        group.defaults.jump_id = local_of(&synced.jump, kind::HOST);
        let awaits = synced.key.filter(|_| group.defaults.key_id.is_none());
        let awaits_jump = synced.jump.filter(|_| group.defaults.jump_id.is_none());
        let local = group.id;
        upsert(&mut self.groups, group, |g| g.id, |g| g.name.to_lowercase());
        Some((local, awaits, awaits_jump))
    }

    /// Removes the local copy of `id`, deleted on the server as version `seq`.
    fn apply_deletion(&mut self, id: &str, seq: u64) {
        if let Some(entry) = self.state.entries.get_mut(id) {
            match entry.kind {
                kind::HOST => self.hosts.retain(|host| host.id != entry.local),
                kind::FORWARD => self.forwards.retain(|rule| rule.id != entry.local),
                kind::KEY => self.ssh_keys.retain(|key| key.id != entry.local),
                kind::SNIPPET => self.snippets.retain(|s| s.id != entry.local),
                kind::LOG => self.logs.retain(|log| log.id != entry.local),
                kind::GROUP => self.groups.retain(|group| group.id != entry.local),
                _ => {}
            }
            entry.deleted = true;
            entry.dirty = false;
            entry.base = seq;
        }
    }

    /// Stores a key or snippet (entities that point at nothing) from `body`, under its local
    /// id `existing` or a new one; returns the local id, or `None` if `body` is unreadable.
    fn take_plain(&mut self, kind: u8, existing: Option<u64>, body: &[u8]) -> Option<u64> {
        match kind {
            kind::KEY => {
                let mut key = serde_json::from_slice::<SshKey>(body).ok()?;
                key.id = existing.unwrap_or_else(|| {
                    self.state
                        .fresh_id(kind::KEY, self.ssh_keys.iter().map(|k| k.id))
                });
                let local = key.id;
                upsert(
                    &mut self.ssh_keys,
                    key,
                    |k| k.id,
                    |k| k.label.to_lowercase(),
                );
                Some(local)
            }
            kind::SNIPPET => {
                let mut snippet = serde_json::from_slice::<Snippet>(body).ok()?;
                snippet.id = existing.unwrap_or_else(|| {
                    self.state
                        .fresh_id(kind::SNIPPET, self.snippets.iter().map(|s| s.id))
                });
                let local = snippet.id;
                upsert(
                    &mut self.snippets,
                    snippet,
                    |s| s.id,
                    |s| s.label.to_lowercase(),
                );
                Some(local)
            }
            kind::LOG => {
                let mut log = serde_json::from_slice::<SessionLog>(body).ok()?;
                log.id = existing.unwrap_or_else(|| {
                    self.state
                        .fresh_id(kind::LOG, self.logs.iter().map(|l| l.id))
                });
                let local = log.id;
                match self.logs.iter_mut().find(|l| l.id == local) {
                    Some(slot) => *slot = log,
                    None => self.logs.push(log),
                }
                self.logs
                    .sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(b.id.cmp(&a.id)));
                Some(local)
            }
            _ => None,
        }
    }

    /// Links what arrived before the entity it points at (sync id `target`, local id
    /// `local`): rules waiting for their host, hosts waiting for their key.
    fn link_waiting(&mut self, target: &str, local: u64) {
        for entry in self.state.entries.values_mut() {
            if entry.awaits_jump.as_deref() == Some(target) {
                entry.awaits_jump = None;
                if entry.kind == kind::GROUP {
                    if let Some(group) =
                        self.groups.iter_mut().find(|group| group.id == entry.local)
                    {
                        group.defaults.jump_id = Some(local);
                    }
                } else if let Some(host) = self.hosts.iter_mut().find(|host| host.id == entry.local)
                {
                    host.jump_id = Some(local);
                }
            }
            if entry.awaits.as_deref() != Some(target) {
                continue;
            }
            entry.awaits = None;
            match entry.kind {
                kind::FORWARD => {
                    if let Some(rule) = self.forwards.iter_mut().find(|rule| rule.id == entry.local)
                    {
                        rule.host_id = local;
                    }
                }
                kind::HOST => {
                    if let Some(host) = self.hosts.iter_mut().find(|host| host.id == entry.local) {
                        host.key_id = Some(local);
                    }
                }
                kind::GROUP => {
                    if let Some(group) =
                        self.groups.iter_mut().find(|group| group.id == entry.local)
                    {
                        group.defaults.key_id = Some(local);
                    }
                }
                _ => {}
            }
        }
    }
}

fn upsert<T, K: Ord>(list: &mut Vec<T>, item: T, id: impl Fn(&T) -> u64, sort: impl Fn(&T) -> K) {
    let key = id(&item);
    match list.iter_mut().find(|existing| id(existing) == key) {
        Some(existing) => *existing = item,
        None => list.push(item),
    }
    list.sort_by_key(sort);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfParams;

    fn copy() -> SyncCopy {
        let (keys, _, _) = Keys::create("pw", KdfParams::TEST).unwrap();
        SyncCopy {
            keys,
            hosts: Vec::new(),
            forwards: Vec::new(),
            ssh_keys: Vec::new(),
            snippets: Vec::new(),
            logs: Vec::new(),
            groups: Vec::new(),
            state: SyncState::new().unwrap(),
            generation: 0,
        }
    }

    #[test]
    fn fresh_ids_stay_above_deleted_and_forgotten_entries() {
        let mut state = SyncState::new().unwrap();
        state.touch(kind::HOST, 7, false).unwrap();
        state.touch(kind::HOST, 7, true).unwrap();
        assert_eq!(state.fresh_id(kind::HOST, [1, 2].into_iter()), 8);
        assert_eq!(state.fresh_id(kind::KEY, std::iter::empty()), 1, "per kind");
        // A deletion never pushed is forgotten by a restart, but its id stays retired.
        state.restart();
        assert!(state.entries.is_empty());
        assert_eq!(state.fresh_id(kind::HOST, std::iter::empty()), 8);
    }

    #[test]
    fn a_rule_that_arrives_before_its_host_is_linked_when_the_host_comes() {
        let mut copy = copy();
        let rule = SyncedForward {
            rule: PortForward::new(5, 99),
            host: Some("host-sync-id".into()),
        };
        copy.apply(
            "rule-sync-id",
            1,
            Some((kind::FORWARD, 10, serde_json::to_vec(&rule).unwrap())),
        );
        assert_eq!(copy.forwards[0].host_id, 0, "no host yet");

        let host = Host {
            id: 42,
            label: "bastion".into(),
            address: "10.0.0.1".into(),
            port: Some(22),
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
        };
        copy.apply(
            "host-sync-id",
            2,
            Some((kind::HOST, 11, serde_json::to_vec(&host).unwrap())),
        );
        assert_eq!(copy.forwards[0].host_id, copy.hosts[0].id);
        assert_eq!(copy.state.entries["rule-sync-id"].awaits, None);
    }

    #[test]
    fn auto_start_stays_on_the_device_that_set_it() {
        let mut copy = copy();
        let mut mine = PortForward::new(1, 0);
        mine.auto_start = true;
        mine.with_host = true;
        copy.forwards.push(mine);
        copy.state.touch(kind::FORWARD, 1, false).unwrap();
        let (_, body) = copy.pending()[0].content.clone().unwrap();
        let sent: SyncedForward = serde_json::from_slice(&body).unwrap();
        assert!(!sent.rule.auto_start, "never sent");
        assert!(!sent.rule.with_host, "never sent");

        // An edit from elsewhere keeps this device's choice.
        let id = copy.pending()[0].id.clone();
        let mut incoming = sent;
        incoming.rule.label = "renamed".into();
        copy.apply(
            &id,
            5,
            Some((kind::FORWARD, 99, serde_json::to_vec(&incoming).unwrap())),
        );
        assert_eq!(copy.forwards[0].label, "renamed");
        assert!(copy.forwards[0].auto_start);
        assert!(copy.forwards[0].with_host);
    }

    #[test]
    fn hosts_are_pushed_before_rules() {
        let mut copy = copy();
        copy.forwards.push(PortForward::new(1, 1));
        copy.hosts.push(Host {
            id: 1,
            label: "h".into(),
            address: "h".into(),
            port: Some(22),
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
        });
        // Many entries, so random ids would put some rule before some host by chance.
        for _ in 0..20 {
            copy.state.touch(kind::FORWARD, 1, false).unwrap();
            copy.state
                .entries
                .values_mut()
                .for_each(|entry| entry.local = 1);
            let id = random_id().unwrap();
            copy.state.entries.insert(
                id,
                Entry {
                    kind: kind::HOST,
                    local: 1,
                    base: 0,
                    dirty: true,
                    deleted: false,
                    hlc: 1,
                    awaits: None,
                    awaits_jump: None,
                },
            );
        }
        let kinds: Vec<u8> = copy
            .pending()
            .iter()
            .filter_map(|p| p.content.as_ref().map(|(k, _)| *k))
            .collect();
        let first_rule = kinds.iter().position(|k| *k == kind::FORWARD).unwrap();
        assert!(kinds[first_rule..].iter().all(|k| *k == kind::FORWARD));
    }
}
