//! Versioned wire types shared by the Annulus client and the sync server (ADR-0008,
//! F-SRV-009). Bodies are `postcard`; every request names the protocol version in the
//! [`VERSION_HEADER`] header.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The request header that carries [`PROTOCOL_VERSION`].
pub const VERSION_HEADER: &str = "Annulus-Protocol";

/// The OPAQUE cipher suite (RFC 9807): ristretto255, 3DH with SHA-512, and Argon2id as the
/// key-stretching function, which only the client runs.
pub struct Suite;

impl opaque_ke::CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, opaque_sha2::Sha512>;
    type Ksf = opaque_ke::argon2::Argon2<'static>;
}

/// Request paths, all under `/v1`.
pub mod path {
    /// Liveness check, unauthenticated.
    pub const HEALTH: &str = "/healthz";
    /// Starts registration ([`super::RegisterStart`]).
    pub const REGISTER_START: &str = "/v1/register/start";
    /// Finishes registration ([`super::RegisterFinish`]).
    pub const REGISTER_FINISH: &str = "/v1/register/finish";
    /// Starts login ([`super::LoginStart`]).
    pub const LOGIN_START: &str = "/v1/login/start";
    /// Finishes login ([`super::LoginFinish`]).
    pub const LOGIN_FINISH: &str = "/v1/login/finish";
    /// Pull with `GET ?since=<seq>`, push with `POST` ([`super::Push`]).
    pub const RECORDS: &str = "/v1/records";
    /// The account's devices ([`super::Devices`]).
    pub const DEVICES: &str = "/v1/devices";
    /// Revokes a device ([`super::Revoke`]).
    pub const REVOKE: &str = "/v1/devices/revoke";
}

/// Why a body could not be read.
#[derive(Debug, thiserror::Error)]
#[error("malformed message: {0}")]
pub struct DecodeError(String);

/// Encodes a message body.
#[must_use]
pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    // Serializing our own plain structs into a Vec cannot fail.
    postcard::to_stdvec(message).unwrap_or_default()
}

/// Decodes a message body.
///
/// # Errors
/// The bytes are not a valid `T`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    postcard::from_bytes(bytes).map_err(|error| DecodeError(error.to_string()))
}

/// First registration step: the account name and the OPAQUE registration request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterStart {
    /// The account name (a username or e-mail address).
    pub account: String,
    /// `RegistrationRequest`, serialized by opaque-ke.
    pub request: Vec<u8>,
}

/// The server's OPAQUE registration response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterStartResponse {
    /// `RegistrationResponse`, serialized by opaque-ke.
    pub response: Vec<u8>,
}

/// Last registration step: the OPAQUE record and the wrapped account key. The server keeps
/// both and can open neither.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterFinish {
    /// The account name again.
    pub account: String,
    /// `RegistrationUpload`, serialized by opaque-ke.
    pub upload: Vec<u8>,
    /// The vault key file (the account key wrapped by the password and by the recovery
    /// key, ADR-0007), as the client serialized it.
    pub wrapped_keys: Vec<u8>,
    /// A name for this device, chosen by the user.
    pub device: String,
}

/// First login step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStart {
    /// The account name.
    pub account: String,
    /// `CredentialRequest`, serialized by opaque-ke.
    pub request: Vec<u8>,
}

/// The server's OPAQUE credential response. Unknown accounts get a fake one, so the reply
/// never says whether an account exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStartResponse {
    /// Names this login attempt in [`LoginFinish`].
    pub login: u64,
    /// `CredentialResponse`, serialized by opaque-ke.
    pub response: Vec<u8>,
}

/// Last login step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginFinish {
    /// From [`LoginStartResponse`].
    pub login: u64,
    /// `CredentialFinalization`, serialized by opaque-ke.
    pub finalization: Vec<u8>,
    /// A name for this device.
    pub device: String,
}

/// A signed-in device: its token and the account's wrapped key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Bearer token for this device; the server keeps only its hash.
    pub token: String,
    /// This device's id in [`Devices`].
    pub device: u64,
    /// The account key wrapped as at registration; the client unwraps it with the password.
    pub wrapped_keys: Vec<u8>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("token", &"[redacted]")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

/// One record as the server stores it: an id, a sequence number and ciphertext it cannot
/// read. The record's kind and contents live inside the ciphertext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Random id chosen by the client.
    pub id: String,
    /// The server sequence number of this version.
    pub seq: u64,
    /// Encrypted by the client; empty for a deletion.
    pub ciphertext: Vec<u8>,
    /// A tombstone: the record was deleted on some device.
    pub deleted: bool,
}

/// A change a client sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The record id.
    pub id: String,
    /// The sequence number of the version this change was made from; 0 for a new record.
    pub base: u64,
    /// The new ciphertext; empty for a deletion.
    pub ciphertext: Vec<u8>,
    /// This change deletes the record.
    pub deleted: bool,
}

/// `POST /v1/records`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Push {
    /// The changes, applied in order.
    pub changes: Vec<Change>,
}

/// What became of a push.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResult {
    /// Accepted changes: record id and its new sequence number.
    pub accepted: Vec<(String, u64)>,
    /// Changes made from an outdated version: the server's current version of each, for
    /// the client to resolve and push again.
    pub conflicts: Vec<Record>,
}

/// `GET /v1/records?since=<seq>`: everything newer than the cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    /// Current versions of records changed after the cursor, in sequence order.
    pub records: Vec<Record>,
    /// The newest sequence number on the server; the client's next cursor.
    pub latest: u64,
}

/// One device of the account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Device id.
    pub id: u64,
    /// The name the user gave it.
    pub name: String,
    /// Last request, seconds since the Unix epoch.
    pub last_seen: u64,
    /// The device whose token made this request.
    pub current: bool,
}

/// `GET /v1/devices`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Devices {
    /// Devices that are signed in.
    pub devices: Vec<Device>,
}

/// `POST /v1/devices/revoke`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoke {
    /// The device to sign out.
    pub device: u64,
}

/// An error body. Login and registration errors are generic on purpose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// What went wrong, safe to show to the user.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_survive_encoding() {
        let push = Push {
            changes: vec![Change {
                id: "r1".into(),
                base: 7,
                ciphertext: vec![1, 2, 3],
                deleted: false,
            }],
        };
        assert_eq!(decode::<Push>(&encode(&push)).unwrap(), push);
        let changes = Changes {
            records: vec![Record {
                id: "r2".into(),
                seq: 9,
                ciphertext: Vec::new(),
                deleted: true,
            }],
            latest: 9,
        };
        assert_eq!(decode::<Changes>(&encode(&changes)).unwrap(), changes);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode::<Push>(&[0xFF, 0xFF, 0xFF]).is_err());
        assert!(decode::<LoginFinish>(&[]).is_err());
    }

    #[test]
    fn session_debug_hides_the_token() {
        let session = Session {
            token: "secret-token".into(),
            device: 1,
            wrapped_keys: vec![1],
        };
        assert!(!format!("{session:?}").contains("secret-token"));
    }
}
