//! The vault's key hierarchy and record encryption (ADR-0007):
//!
//! ```text
//! master password ──Argon2id(salt, params)──► master key ──HKDF("annulus v1 kek")──► KEK
//! account key (random 256-bit) ──XChaCha20-Poly1305 under KEK──► wrapped account key
//! account key ──XChaCha20-Poly1305 under the recovery key──► second wrapped copy
//! account key ──HKDF("annulus v1 record")──► record key
//! record ──XChaCha20-Poly1305(random nonce, AAD = id ‖ kind)──► ciphertext
//! ```
//!
//! A password change re-wraps the account key and leaves every record untouched; the
//! recovery key unlocks when the password is forgotten. All keys are zeroized on drop.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Why a vault operation failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// The password (or recovery key) does not open this vault. Nothing is damaged.
    #[error("wrong password")]
    WrongPassword,
    /// A record failed authentication: it was altered, moved or damaged.
    #[error("a record is damaged or was tampered with")]
    Corrupt,
    /// The OS random number generator failed.
    #[error("no secure randomness available")]
    Randomness,
    /// The key derivation parameters are not usable.
    #[error("invalid key derivation parameters")]
    Parameters,
}

/// Argon2id cost parameters, stored with the vault so they can be raised later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory in KiB.
    pub memory_kib: u32,
    /// Passes.
    pub iterations: u32,
    /// Lanes.
    pub parallelism: u32,
}

impl KdfParams {
    /// The defaults for new vaults: 64 MiB, 3 passes, 1 lane (about a second on a
    /// low-end laptop).
    pub const DEFAULT: Self = Self {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// Cheap parameters for tests only.
    #[cfg(test)]
    pub const TEST: Self = Self {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
}

/// A 256-bit secret key, wiped on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key([u8; 32]);

impl Key {
    /// A fresh random key.
    ///
    /// # Errors
    /// [`CryptoError::Randomness`].
    pub fn random() -> Result<Self, CryptoError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|_| CryptoError::Randomness)?;
        Ok(Self(key))
    }

    fn derive(&self, info: &[u8]) -> Key {
        let mut out = [0u8; 32];
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(None, &self.0);
        // 32 bytes is always a valid HKDF-SHA256 output length.
        let _ = hkdf.expand(info, &mut out);
        Key(out)
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&self.0.into())
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key([redacted])")
    }
}

/// Ciphertext with its nonce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// The 192-bit random nonce.
    pub nonce: Vec<u8>,
    /// Ciphertext and tag.
    pub ciphertext: Vec<u8>,
}

fn seal(key: &Key, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, CryptoError> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(|_| CryptoError::Randomness)?;
    let ciphertext = key
        .cipher()
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Corrupt)?;
    Ok(Sealed {
        nonce: nonce.to_vec(),
        ciphertext,
    })
}

fn open(key: &Key, sealed: &Sealed, aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let nonce: [u8; 24] = sealed
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::Corrupt)?;
    key.cipher()
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Corrupt)
}

/// The master key from `password`.
fn master_key(password: &str, salt: &[u8], params: KdfParams) -> Result<Key, CryptoError> {
    let argon_params = argon2::Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|_| CryptoError::Parameters)?;
    let argon = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon_params,
    );
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|_| CryptoError::Parameters)?;
    Ok(Key(out))
}

const WRAP_AAD: &[u8] = b"annulus v1 account key";

/// What is stored to unlock a vault: no secret in the clear.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    /// Argon2id salt.
    pub salt: Vec<u8>,
    /// Argon2id costs.
    pub kdf: KdfParams,
    /// The account key wrapped by the password-derived KEK.
    pub by_password: Sealed,
    /// The account key wrapped by the recovery key.
    pub by_recovery: Sealed,
}

/// An unlocked vault's keys.
#[derive(Clone)]
pub struct Keys {
    account: Key,
    record: Key,
}

impl Keys {
    /// Whether both unlock the same account key (the same vault, perhaps wrapped by a
    /// different password).
    #[must_use]
    pub fn same_account(&self, other: &Keys) -> bool {
        // Not secret-dependent timing worth guarding: both sides are already unlocked.
        self.account.0 == other.account.0
    }

    /// Creates a new account key protected by `password` and by a new recovery key, which is
    /// returned for the user to write down (shown once).
    ///
    /// # Errors
    /// Randomness or parameter failures.
    pub fn create(
        password: &str,
        kdf: KdfParams,
    ) -> Result<(Self, KeyFile, Zeroizing<String>), CryptoError> {
        let account = Key::random()?;
        let recovery = Key::random()?;
        let mut salt = vec![0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| CryptoError::Randomness)?;
        let kek = master_key(password, &salt, kdf)?.derive(b"annulus v1 kek");
        let file = KeyFile {
            salt,
            kdf,
            by_password: seal(&kek, &account.0, WRAP_AAD)?,
            by_recovery: seal(&recovery, &account.0, WRAP_AAD)?,
        };
        let recovery_text = encode_recovery(&recovery.0);
        Ok((Self::from_account(account), file, recovery_text))
    }

    /// Unlocks with the master password.
    ///
    /// # Errors
    /// [`CryptoError::WrongPassword`] if it does not open the vault.
    pub fn unlock(file: &KeyFile, password: &str) -> Result<Self, CryptoError> {
        let kek = master_key(password, &file.salt, file.kdf)?.derive(b"annulus v1 kek");
        let account =
            open(&kek, &file.by_password, WRAP_AAD).map_err(|_| CryptoError::WrongPassword)?;
        Self::from_bytes(&account)
    }

    /// Unlocks with the recovery key (spaces and case are ignored).
    ///
    /// # Errors
    /// [`CryptoError::WrongPassword`] if it does not open the vault.
    pub fn recover(file: &KeyFile, recovery: &str) -> Result<Self, CryptoError> {
        let bytes = decode_recovery(recovery).ok_or(CryptoError::WrongPassword)?;
        let account = open(&Key(*bytes), &file.by_recovery, WRAP_AAD)
            .map_err(|_| CryptoError::WrongPassword)?;
        Self::from_bytes(&account)
    }

    /// The key file for a new `password`: the account key is re-wrapped, so every record
    /// stays as it is.
    ///
    /// # Errors
    /// Randomness or parameter failures.
    pub fn change_password(&self, file: &KeyFile, password: &str) -> Result<KeyFile, CryptoError> {
        let mut salt = vec![0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| CryptoError::Randomness)?;
        let kek = master_key(password, &salt, file.kdf)?.derive(b"annulus v1 kek");
        Ok(KeyFile {
            salt,
            kdf: file.kdf,
            by_password: seal(&kek, &self.account.0, WRAP_AAD)?,
            by_recovery: file.by_recovery.clone(),
        })
    }

    /// Encrypts a record, binding it to its `id` and `kind`.
    ///
    /// # Errors
    /// Randomness failure.
    pub fn seal_record(&self, id: &str, kind: u8, plaintext: &[u8]) -> Result<Sealed, CryptoError> {
        seal(&self.record, plaintext, &record_aad(id, kind))
    }

    /// Decrypts a record; fails if it was changed or moved to another id or kind.
    ///
    /// # Errors
    /// [`CryptoError::Corrupt`].
    pub fn open_record(
        &self,
        id: &str,
        kind: u8,
        sealed: &Sealed,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        open(&self.record, sealed, &record_aad(id, kind))
    }

    fn from_account(account: Key) -> Self {
        let record = account.derive(b"annulus v1 record");
        Self { account, record }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let account: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::Corrupt)?;
        Ok(Self::from_account(Key(account)))
    }
}

fn record_aad(id: &str, kind: u8) -> Vec<u8> {
    let mut aad = Vec::with_capacity(id.len() + 2);
    aad.extend_from_slice(id.as_bytes());
    aad.push(0);
    aad.push(kind);
    aad
}

const BASE32: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 32 bytes as 52 base32 characters (an alphabet without 0/O, 1/I), grouped by 4.
fn encode_recovery(bytes: &[u8; 32]) -> Zeroizing<String> {
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Zeroizing::new(String::new());
    for byte in bytes {
        bits = (bits << 8) | u32::from(*byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            out.push(char::from(BASE32[((bits >> count) & 31) as usize]));
        }
    }
    if count > 0 {
        out.push(char::from(BASE32[((bits << (5 - count)) & 31) as usize]));
    }
    let grouped: String = out
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-");
    Zeroizing::new(grouped)
}

fn decode_recovery(text: &str) -> Option<Zeroizing<[u8; 32]>> {
    let mut out = Zeroizing::new([0u8; 32]);
    let mut bits = 0u32;
    let mut count = 0;
    let mut index = 0;
    for c in text.chars().filter(char::is_ascii_alphanumeric) {
        let value = BASE32
            .iter()
            .position(|b| char::from(*b) == c.to_ascii_uppercase())?;
        bits = (bits << 5) | u32::try_from(value).ok()?;
        count += 5;
        if count >= 8 {
            count -= 8;
            *out.get_mut(index)? = u8::try_from((bits >> count) & 0xff).ok()?;
            index += 1;
        }
    }
    (index == 32).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (Keys, KeyFile, Zeroizing<String>) {
        Keys::create("correct horse", KdfParams::TEST).unwrap()
    }

    #[test]
    fn the_password_unlocks_and_a_wrong_one_is_reported_as_such() {
        let (keys, file, _) = vault();
        let sealed = keys.seal_record("host-1", 1, b"prod-db").unwrap();
        let unlocked = Keys::unlock(&file, "correct horse").unwrap();
        assert_eq!(
            unlocked
                .open_record("host-1", 1, &sealed)
                .unwrap()
                .as_slice(),
            b"prod-db"
        );
        assert_eq!(
            Keys::unlock(&file, "wrong").err(),
            Some(CryptoError::WrongPassword)
        );
    }

    #[test]
    fn a_flipped_byte_or_a_moved_record_fails() {
        let (keys, _, _) = vault();
        let sealed = keys.seal_record("host-1", 1, b"prod-db").unwrap();
        let mut flipped = sealed.clone();
        flipped.ciphertext[0] ^= 1;
        assert_eq!(
            keys.open_record("host-1", 1, &flipped).err(),
            Some(CryptoError::Corrupt)
        );
        assert_eq!(
            keys.open_record("host-2", 1, &sealed).err(),
            Some(CryptoError::Corrupt)
        );
        assert_eq!(
            keys.open_record("host-1", 2, &sealed).err(),
            Some(CryptoError::Corrupt)
        );
    }

    #[test]
    fn the_recovery_key_unlocks_after_a_forgotten_password() {
        let (keys, file, recovery) = vault();
        let sealed = keys.seal_record("host-1", 1, b"prod-db").unwrap();
        let typed = recovery.to_lowercase().replace('-', " ");
        let recovered = Keys::recover(&file, &typed).unwrap();
        assert_eq!(
            recovered
                .open_record("host-1", 1, &sealed)
                .unwrap()
                .as_slice(),
            b"prod-db"
        );
        assert_eq!(
            Keys::recover(&file, "AAAA-BBBB").err(),
            Some(CryptoError::WrongPassword)
        );
    }

    #[test]
    fn a_password_change_rewraps_the_key_and_leaves_records_untouched() {
        let (keys, file, recovery) = vault();
        let sealed = keys.seal_record("host-1", 1, b"prod-db").unwrap();
        let new_file = keys.change_password(&file, "new secret").unwrap();
        assert_eq!(
            Keys::unlock(&new_file, "correct horse").err(),
            Some(CryptoError::WrongPassword)
        );
        let unlocked = Keys::unlock(&new_file, "new secret").unwrap();
        assert_eq!(
            unlocked
                .open_record("host-1", 1, &sealed)
                .unwrap()
                .as_slice(),
            b"prod-db"
        );
        assert!(
            Keys::recover(&new_file, &recovery).is_ok(),
            "the recovery key still works"
        );
    }

    #[test]
    fn recovery_keys_round_trip_and_look_readable() {
        let bytes = [0xA5; 32];
        let text = encode_recovery(&bytes);
        assert_eq!(text.chars().filter(char::is_ascii_alphanumeric).count(), 52);
        assert!(!text.contains(['0', 'O', '1', 'I']));
        assert_eq!(*decode_recovery(&text).unwrap(), bytes);
    }
}
