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

    fn apply(&mut self, id: &str, seq: u64, item: Option<Item>) {
        self.0
            .apply(id, seq, item.map(|item| (item.kind, item.hlc, item.body)));
    }

    fn rebase(&mut self, id: &str, seq: u64) {
        self.0.rebase(id, seq);
    }
}
