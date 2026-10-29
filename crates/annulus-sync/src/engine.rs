//! One sync round (ADR-0008): pull what changed, resolve conflicts, push local changes.
//!
//! The rules, all chosen so that no version is lost silently:
//! - A record changed only on one side takes that side's version.
//! - Edited on both sides: the newer hybrid-logical-clock time wins. When the server's
//!   version wins over an unpushed local edit, the local edit is pushed first and then
//!   replaced, so it stays in the server's history.
//! - Edited on one side and deleted on the other: the edit wins; a deletion never removes
//!   an edit nobody has seen.

use annulus_proto::{Change, Changes, PushResult};

/// How often a round retries after another device pushed in between.
const ROUNDS: usize = 3;

/// Why a sync round failed. Nothing local is lost; the next round retries.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The server could not be reached or answered with an error.
    #[error("{0}")]
    Transport(String),
    /// The device was signed out (revoked, or the account was removed).
    #[error("this device is no longer signed in")]
    Unauthorized,
    /// A record could not be decrypted with the account key.
    #[error("record {0} could not be decrypted")]
    Undecryptable(String),
}

/// The plaintext inside a record's ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What the body is (a host, a rule, …), known only to clients.
    pub kind: u8,
    /// When it was last edited, as a hybrid-logical-clock time.
    pub hlc: u64,
    /// The serialized entity.
    pub body: Vec<u8>,
}

/// A local change waiting to be pushed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The record id.
    pub id: String,
    /// The server version it was made from; 0 for a new record.
    pub base: u64,
    /// The new content, or `None` for a deletion.
    pub item: Option<Item>,
    /// When the change was made.
    pub hlc: u64,
}

/// The server, as the engine sees it.
pub trait Transport {
    /// Sends changes.
    ///
    /// # Errors
    /// The server is unreachable or refused.
    fn push(&mut self, changes: &[Change]) -> Result<PushResult, SyncError>;
    /// Fetches what changed after `since`.
    ///
    /// # Errors
    /// The server is unreachable or refused.
    fn pull(&mut self, since: u64) -> Result<Changes, SyncError>;
}

/// The local copy, as the engine sees it.
pub trait Replica {
    /// The server sequence number this copy has seen.
    fn cursor(&self) -> u64;
    /// Remembers the new cursor.
    fn set_cursor(&mut self, cursor: u64);
    /// Local changes not pushed yet.
    fn pending(&self) -> Vec<Outgoing>;
    /// The server version of `id` this copy holds; 0 if none.
    fn version(&self, id: &str) -> u64;
    /// Encrypts an item for record `id`.
    ///
    /// # Errors
    /// No randomness for the nonce.
    fn seal(&self, id: &str, item: &Item) -> Result<Vec<u8>, SyncError>;
    /// Decrypts record `id`.
    ///
    /// # Errors
    /// The ciphertext was not made with this account's key, or was altered.
    fn open(&self, id: &str, ciphertext: &[u8]) -> Result<Item, SyncError>;
    /// The server accepted the pending change of `id` as version `seq`.
    fn pushed(&mut self, id: &str, seq: u64);
    /// Takes the server's version `seq` of `id` (`None`: it was deleted), dropping any
    /// pending local change of it.
    fn apply(&mut self, id: &str, seq: u64, item: Option<Item>);
    /// Keeps the pending local change of `id` but makes it build on server version `seq`.
    fn rebase(&mut self, id: &str, seq: u64);
}

/// What a round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Records taken from the server.
    pub pulled: usize,
    /// Local changes the server accepted.
    pub pushed: usize,
    /// Records edited on two devices; the newer edit was kept and the other is in history.
    pub conflicts: Vec<String>,
}

/// A local edit that lost to a newer server version: it goes into history first.
struct Preserve {
    id: String,
    loser: Item,
    base: u64,
    winner: Vec<u8>,
}

/// Runs one sync round.
///
/// # Errors
/// The server could not be reached or a record could not be decrypted; what was applied
/// before the error stays applied and the rest is retried next round.
pub fn sync(
    transport: &mut impl Transport,
    replica: &mut impl Replica,
) -> Result<Report, SyncError> {
    let mut report = Report::default();
    for _ in 0..ROUNDS {
        let preserve = pull(transport, replica, &mut report)?;
        for entry in preserve {
            keep_in_history(transport, replica, entry)?;
        }
        if push(transport, replica, &mut report)? {
            return Ok(report);
        }
    }
    Ok(report)
}

fn pull(
    transport: &mut impl Transport,
    replica: &mut impl Replica,
    report: &mut Report,
) -> Result<Vec<Preserve>, SyncError> {
    let changes = transport.pull(replica.cursor())?;
    let pending = replica.pending();
    let mut preserve = Vec::new();
    for record in changes.records {
        // Already held, typically this device's own push coming back.
        if record.seq <= replica.version(&record.id) {
            continue;
        }
        let remote = if record.deleted {
            None
        } else {
            Some(replica.open(&record.id, &record.ciphertext)?)
        };
        let local = pending.iter().find(|change| change.id == record.id);
        match (local, remote) {
            (None, remote) => replica.apply(&record.id, record.seq, remote),
            // Edit beats deletion, either way round.
            (Some(local), None) if local.item.is_some() => replica.rebase(&record.id, record.seq),
            (Some(_), None) => replica.apply(&record.id, record.seq, None),
            (Some(local), Some(remote)) => match &local.item {
                None => replica.apply(&record.id, record.seq, Some(remote)),
                Some(_) if local.hlc > remote.hlc => {
                    report.conflicts.push(record.id.clone());
                    replica.rebase(&record.id, record.seq);
                }
                Some(loser) => {
                    report.conflicts.push(record.id.clone());
                    preserve.push(Preserve {
                        id: record.id.clone(),
                        loser: loser.clone(),
                        base: record.seq,
                        winner: record.ciphertext.clone(),
                    });
                    replica.apply(&record.id, record.seq, Some(remote));
                }
            },
        }
        report.pulled += 1;
    }
    replica.set_cursor(changes.latest);
    Ok(preserve)
}

/// Pushes the losing local edit, then the winning version on top of it, so the server's
/// history holds both and its current version is the winner.
fn keep_in_history(
    transport: &mut impl Transport,
    replica: &mut impl Replica,
    entry: Preserve,
) -> Result<(), SyncError> {
    let loser = Change {
        id: entry.id.clone(),
        base: entry.base,
        ciphertext: replica.seal(&entry.id, &entry.loser)?,
        deleted: false,
    };
    let Some(&(_, loser_seq)) = transport.push(&[loser])?.accepted.first() else {
        // Someone pushed in between; the next pull brings their version.
        return Ok(());
    };
    let winner = Change {
        id: entry.id.clone(),
        base: loser_seq,
        ciphertext: entry.winner,
        deleted: false,
    };
    if let Some(&(_, seq)) = transport.push(&[winner])?.accepted.first() {
        replica.pushed(&entry.id, seq);
    }
    Ok(())
}

/// Pushes pending changes; false when some came back as conflicts and need another round.
fn push(
    transport: &mut impl Transport,
    replica: &mut impl Replica,
    report: &mut Report,
) -> Result<bool, SyncError> {
    let pending = replica.pending();
    if pending.is_empty() {
        return Ok(true);
    }
    let mut changes = Vec::with_capacity(pending.len());
    for change in &pending {
        changes.push(Change {
            id: change.id.clone(),
            base: change.base,
            ciphertext: match &change.item {
                Some(item) => replica.seal(&change.id, item)?,
                None => Vec::new(),
            },
            deleted: change.item.is_none(),
        });
    }
    let result = transport.push(&changes)?;
    for (id, seq) in &result.accepted {
        replica.pushed(id, *seq);
        report.pushed += 1;
    }
    Ok(result.conflicts.is_empty())
}
