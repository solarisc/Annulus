//! The vault as the sync engine's local copy.

use annulus_vault::sync::SyncCopy;

use crate::engine::{Item, Outgoing, Replica, SyncError};

/// A vault snapshot taking part in a sync round; give it back with
/// `Vault::commit_sync(replica.0)`.
pub struct VaultReplica(pub SyncCopy);

impl Replica for VaultReplica {
    fn cursor(&self) -> u64 {
        self.0.cursor()
    }

    fn set_cursor(&mut self, cursor: u64) {
        self.0.set_cursor(cursor);
    }

    fn pending(&self) -> Vec<Outgoing> {
        self.0
            .pending()
            .into_iter()
            .map(|pending| Outgoing {
                item: pending.content.map(|(kind, body)| Item {
                    kind,
                    hlc: pending.hlc,
                    body,
                }),
                id: pending.id,
                base: pending.base,
                hlc: pending.hlc,
            })
            .collect()
    }

    fn version(&self, id: &str) -> u64 {
        self.0.version(id)
    }

    fn seal(&self, id: &str, item: &Item) -> Result<Vec<u8>, SyncError> {
        self.0
            .seal(id, item.kind, item.hlc, &item.body)
            .map_err(|error| SyncError::Transport(format!("could not encrypt: {error}")))
    }

    fn open(&self, id: &str, ciphertext: &[u8]) -> Result<Item, SyncError> {
        let (kind, hlc, body) = self
            .0
            .open(id, ciphertext)
            .map_err(|_| SyncError::Undecryptable(id.to_owned()))?;
        Ok(Item { kind, hlc, body })
    }

    fn pushed(&mut self, id: &str, seq: u64) {
        self.0.pushed(id, seq);
    }

    fn apply(&mut self, id: &str, seq: u64, hlc: u64, item: Option<Item>) {
        match item {
            Some(item) => self
                .0
                .apply(id, seq, Some((item.kind, item.hlc, item.body))),
            None => self.0.deleted_on_server(id, seq, hlc),
        }
    }

    fn rebase(&mut self, id: &str, seq: u64) {
        self.0.rebase(id, seq);
    }

    fn renew(&mut self, id: &str, after: u64) {
        self.0.renew(id, after);
    }

    fn held_time(&self, id: &str) -> Option<u64> {
        self.0.held_time(id)
    }

    fn next_time(&mut self, after: u64) -> u64 {
        self.0.next_time(after)
    }

    fn reassert(&mut self, id: &str, seq: u64) {
        self.0.reassert(id, seq);
    }

    fn own_time(&self, hlc: u64) -> bool {
        self.0.own_time(hlc)
    }

    fn restart(&mut self) {
        self.0.restart();
    }
}
