//! One sync round (ADR-0008): pull what changed, resolve conflicts, push local changes.
//!
//! The rules, all chosen so that no version is lost silently:
//! - A record changed only on one side takes that side's version.
//! - Edited on both sides: the newer hybrid-logical-clock time wins. When the server's
//!   version wins over an unpushed local edit, the local edit is pushed first and then
//!   replaced, so it stays in the server's history.
//! - Edited on one side and deleted on the other: the edit wins; a deletion never removes
//!   an edit nobody has seen. The edit is made newer than the deletion, so the next rule
//!   holds for it too.
//! - Never back in time: a version older than the one held, with nothing pending here, is the
//!   server serving an old copy again (a rollback). It is not applied, and the copy held here
//!   is pushed back over it.
//! - A deletion is a sealed record like any other (a tombstone with an empty body). One the
//!   server made up, with nothing sealed, is passed over.

use annulus_proto::{Change, Changes, PushResult};

/// The kind inside a sealed deletion; its empty body is what marks it.
pub const TOMBSTONE: u8 = 0;

/// How often a round retries after another device pushed in between.
const ROUNDS: usize = 3;

/// Ciphertext bytes sent in one push request at most (a record bigger than this goes
/// alone); the server accepts requests up to `annulus_proto::MAX_BODY`.
pub const PUSH_BATCH_BYTES: usize = 1024 * 1024;

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
    /// The server's certificate is not one this device trusts for it: not from a public
    /// authority and not pinned, or not the one pinned (ADR-0018).
    #[error("the server's certificate is not trusted ({0})")]
    Untrusted(String),
}

/// The plaintext inside a record's ciphertext. An empty body is a deletion.
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
    /// Takes the server's version `seq` of `id`, made at clock time `hlc` (`None`: it was
    /// deleted), dropping any pending local change of it.
    fn apply(&mut self, id: &str, seq: u64, hlc: u64, item: Option<Item>);
    /// Keeps the pending local change of `id` but makes it build on server version `seq`.
    fn rebase(&mut self, id: &str, seq: u64);
    /// Makes the pending local change of `id` newer than clock time `after` (an edit that
    /// beats a newer deletion).
    fn renew(&mut self, id: &str, after: u64);
    /// When the copy of `id` held here (a record or its deletion) was made; `None` if none.
    fn held_time(&self, id: &str) -> Option<u64>;
    /// The clock time of the last version of `id` taken from the server or pushed to it;
    /// `None` (or 0) if none.
    fn seen_time(&self, id: &str) -> Option<u64>;
    /// A clock time of this device, newer than `after`.
    fn next_time(&mut self, after: u64) -> u64;
    /// The server's version `seq` of `id` is older than the copy held here: that copy is
    /// pushed again, over it.
    fn reassert(&mut self, id: &str, seq: u64);
    /// Whether clock time `hlc` was made on this device (an edit of ours coming back).
    fn own_time(&self, hlc: u64) -> bool;
    /// Starts over against a server that lost changes (restored from a backup): the cursor
    /// goes to 0 and everything held is pending again, so it is pushed or reconciled.
    fn restart(&mut self);
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
    let mut changes = transport.pull(replica.cursor())?;
    if changes.latest < replica.cursor() {
        // The server is behind what this device saw: it was restored from a backup.
        replica.restart();
        changes = transport.pull(0)?;
    }
    let pending = replica.pending();
    let mut preserve = Vec::new();
    for record in changes.records {
        // Already held, typically this device's own push coming back.
        if record.seq <= replica.version(&record.id) {
            continue;
        }
        // What a record is comes from its sealed content, never from the server's flag. An
        // unsealed one is passed over, and the copy held here goes back over it, so the
        // record does not stay stuck behind a version this device will never take.
        if record.ciphertext.is_empty() {
            if replica.held_time(&record.id).is_some() {
                replica.reassert(&record.id, record.seq);
            }
            continue;
        }
        let item = replica.open(&record.id, &record.ciphertext)?;
        let hlc = item.hlc;
        let remote = (!item.body.is_empty()).then_some(item);
        let local = pending.iter().find(|change| change.id == record.id);
        if local.is_none() && replica.held_time(&record.id).is_some_and(|held| hlc < held) {
            replica.reassert(&record.id, record.seq);
            continue;
        }
        // A version this device already took or pushed, served again while an edit (or a
        // deletion) is pending here: not news, so the pending change goes over it.
        if local.is_some()
            && replica
                .seen_time(&record.id)
                .is_some_and(|seen| hlc <= seen)
        {
            replica.rebase(&record.id, record.seq);
            report.pulled += 1;
            continue;
        }
        match (local, remote) {
            (None, remote) => replica.apply(&record.id, record.seq, hlc, remote),
            // Edit beats deletion, either way round.
            (Some(local), None) if local.item.is_some() => {
                replica.rebase(&record.id, record.seq);
                replica.renew(&record.id, hlc);
            }
            (Some(_), None) => replica.apply(&record.id, record.seq, hlc, None),
            (Some(local), Some(remote)) => match &local.item {
                None => replica.apply(&record.id, record.seq, hlc, Some(remote)),
                // The same edit: ours, pushed in a round whose result was not kept.
                Some(_) if local.hlc == remote.hlc => replica.pushed(&record.id, record.seq),
                Some(_) if local.hlc > remote.hlc => {
                    // An older edit of our own is not a conflict.
                    if !replica.own_time(remote.hlc) {
                        report.conflicts.push(record.id.clone());
                    }
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
                    replica.apply(&record.id, record.seq, hlc, Some(remote));
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
                // A deletion is sealed too, so no one else can make one up.
                None => replica.seal(
                    &change.id,
                    &Item {
                        kind: TOMBSTONE,
                        hlc: change.hlc,
                        body: Vec::new(),
                    },
                )?,
            },
            deleted: change.item.is_none(),
        });
    }
    let mut clean = true;
    for batch in batches(&changes, PUSH_BATCH_BYTES) {
        let result = transport.push(batch)?;
        for (id, seq) in &result.accepted {
            replica.pushed(id, *seq);
            report.pushed += 1;
        }
        clean &= result.conflicts.is_empty();
    }
    Ok(clean)
}

/// `changes` cut into runs of at most `budget` ciphertext bytes; each run holds at least
/// one change, so a single large record still goes.
fn batches(changes: &[Change], budget: usize) -> Vec<&[Change]> {
    let mut runs = Vec::new();
    let (mut start, mut size) = (0, 0);
    for (index, change) in changes.iter().enumerate() {
        let bytes = change.ciphertext.len() + change.id.len();
        if index > start && size + bytes > budget {
            runs.push(&changes[start..index]);
            (start, size) = (index, 0);
        }
        size += bytes;
    }
    if start < changes.len() {
        runs.push(&changes[start..]);
    }
    runs
}

/// Brings back an earlier `version` of record `id` (from its history, e.g. a deleted host):
/// it is pushed as the newest version, so every device gets it back, and applied here.
///
/// # Errors
/// Unreachable, the version cannot be decrypted, or the record changed meanwhile (pull and
/// try again).
pub fn restore(
    transport: &mut impl Transport,
    replica: &mut impl Replica,
    id: &str,
    version: &annulus_proto::Record,
) -> Result<(), SyncError> {
    let mut item = replica.open(id, &version.ciphertext)?;
    if item.body.is_empty() {
        return Err(SyncError::Transport(
            "that version is a deletion; there is nothing in it to bring back".into(),
        ));
    }
    // Bringing it back is a new edit: newer than the deletion, or other devices would take
    // it for an old copy served again.
    item.hlc = replica.next_time(replica.held_time(id).unwrap_or(0));
    let change = Change {
        id: id.to_owned(),
        base: replica.version(id),
        ciphertext: replica.seal(id, &item)?,
        deleted: false,
    };
    let result = transport.push(&[change])?;
    let Some(&(_, seq)) = result.accepted.first() else {
        return Err(SyncError::Transport(
            "it changed on another device meanwhile; sync and try again".into(),
        ));
    };
    let hlc = item.hlc;
    replica.apply(id, seq, hlc, Some(item));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(id: &str, bytes: usize) -> Change {
        Change {
            id: id.into(),
            base: 0,
            ciphertext: vec![0; bytes],
            deleted: false,
        }
    }

    #[test]
    fn pushes_are_cut_into_batches_and_a_big_record_goes_alone() {
        let changes = [
            change("a", 40),
            change("b", 40),
            change("c", 300),
            change("d", 10),
        ];
        let sizes: Vec<Vec<&str>> = batches(&changes, 100)
            .into_iter()
            .map(|run| run.iter().map(|c| c.id.as_str()).collect())
            .collect();
        assert_eq!(sizes, [vec!["a", "b"], vec!["c"], vec!["d"]]);
        assert_eq!(batches(&[], 100), Vec::<&[Change]>::new());
    }
}
