//! The keychain (F-KEY-001…003): SSH private keys kept in the vault, encrypted with
//! everything else and synced with the account. `annulus-ssh` generates and reads them.

use serde::{Deserialize, Serialize};

use crate::hosts::Secret;

/// One SSH key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshKey {
    /// Stable identifier within the vault.
    pub id: u64,
    /// The name shown in the keychain.
    pub label: String,
    /// The private key in OpenSSH format, unencrypted inside the encrypted vault.
    pub private: Secret,
    /// The public key line for `authorized_keys`.
    pub public: String,
    /// `SHA256:…`, to compare with what a server shows.
    pub fingerprint: String,
}
