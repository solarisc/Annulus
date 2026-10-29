//! SSH keys for the keychain (F-KEY-001…003): generate Ed25519 keys, import OpenSSH and
//! PEM private keys (decrypting them once with their passphrase, since the vault encrypts
//! everything it stores), and show public keys and fingerprints.

use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PrivateKey};
use zeroize::Zeroizing;

/// A private key in OpenSSH format, unencrypted (the vault encrypts it), and its public
/// key line.
pub struct KeyPair {
    /// `-----BEGIN OPENSSH PRIVATE KEY-----…`, wiped on drop.
    pub private: Zeroizing<String>,
    /// `ssh-ed25519 AAAA… comment`, for `authorized_keys`.
    pub public: String,
    /// `SHA256:…`.
    pub fingerprint: String,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// Why a key could not be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The key is encrypted and no passphrase (or a wrong one) was given.
    #[error("the key is protected by a passphrase; enter it to import the key")]
    NeedsPassphrase,
    /// Not a private key this app can read.
    #[error("not a private key in OpenSSH or PEM format")]
    Invalid,
    /// No randomness to generate a key.
    #[error("could not generate a key")]
    Generate,
}

fn pair(key: &PrivateKey) -> Result<KeyPair, KeyError> {
    let private = key
        .to_openssh(LineEnding::LF)
        .map_err(|_| KeyError::Invalid)?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|_| KeyError::Invalid)?;
    Ok(KeyPair {
        private: Zeroizing::new(private.to_string()),
        public,
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// The kinds of key the keychain can generate (F-KEY-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// Ed25519: small and fast; the default.
    Ed25519,
    /// ECDSA on NIST P-256, for servers that need a NIST curve.
    EcdsaP256,
    /// RSA, 3072 bits (signing with SHA-512), for old servers.
    Rsa3072,
}

impl KeyKind {
    /// Every kind, default first.
    pub const ALL: [KeyKind; 3] = [KeyKind::Ed25519, KeyKind::EcdsaP256, KeyKind::Rsa3072];

    /// The name shown in the keychain.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            KeyKind::Ed25519 => "Ed25519",
            KeyKind::EcdsaP256 => "ECDSA P-256",
            KeyKind::Rsa3072 => "RSA 3072",
        }
    }
}

/// A new key of `kind` with `comment` (often `user@device`). RSA takes a few seconds.
///
/// # Errors
/// No randomness.
pub fn generate(kind: KeyKind, comment: &str) -> Result<KeyPair, KeyError> {
    let mut rng = rand::rng();
    let mut key = match kind {
        KeyKind::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519),
        KeyKind::EcdsaP256 => PrivateKey::random(
            &mut rng,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
        ),
        KeyKind::Rsa3072 => russh::keys::ssh_key::private::RsaKeypair::random(&mut rng, 3072)
            .and_then(|pair| PrivateKey::new(pair.into(), comment)),
    }
    .map_err(|_| KeyError::Generate)?;
    key.set_comment(comment);
    pair(&key)
}

/// Reads a private key (OpenSSH, or PEM such as PKCS#1/PKCS#8), decrypting it with
/// `passphrase` if it is protected.
///
/// # Errors
/// [`KeyError::NeedsPassphrase`] when it is encrypted and the passphrase is missing or
/// wrong; [`KeyError::Invalid`] otherwise.
pub fn import(text: &str, passphrase: Option<&str>) -> Result<KeyPair, KeyError> {
    let passphrase = passphrase.filter(|value| !value.is_empty());
    if let Ok(key) = russh::keys::decode_secret_key(text.trim(), passphrase) {
        return pair(&key);
    }
    // Without a passphrase, an encrypted key fails to decode: tell that apart from text
    // that is not a key at all.
    let encrypted = text.contains("ENCRYPTED")
        || PrivateKey::from_openssh(text.trim()).is_ok_and(|key| key.is_encrypted());
    Err(if encrypted {
        KeyError::NeedsPassphrase
    } else {
        KeyError::Invalid
    })
}

/// Decodes a stored private key for signing.
pub(crate) fn decode(private: &str) -> Option<PrivateKey> {
    russh::keys::decode_secret_key(private, None).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_imports_back_to_the_same_public_key() {
        let generated = generate(KeyKind::Ed25519, "me@laptop").unwrap();
        assert!(generated.public.starts_with("ssh-ed25519 "));
        assert!(generated.public.ends_with(" me@laptop"));
        assert!(generated.fingerprint.starts_with("SHA256:"));
        let again = import(&generated.private, None).unwrap();
        assert_eq!(again.public, generated.public);
        assert!(!format!("{generated:?}").contains("PRIVATE"));
    }

    #[test]
    fn ecdsa_and_rsa_keys_generate_and_decode_for_signing() {
        for (kind, prefix) in [
            (KeyKind::EcdsaP256, "ecdsa-sha2-nistp256 "),
            (KeyKind::Rsa3072, "ssh-rsa "),
        ] {
            let generated = generate(kind, "me@laptop").unwrap();
            assert!(generated.public.starts_with(prefix), "{}", generated.public);
            assert!(decode(&generated.private).is_some(), "{}", kind.name());
            assert_eq!(
                import(&generated.private, None).unwrap().public,
                generated.public
            );
        }
    }

    #[test]
    fn an_encrypted_key_needs_its_passphrase_and_is_stored_decrypted() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let encrypted = key
            .encrypt(&mut rand::rng(), "s3cret")
            .unwrap()
            .to_openssh(LineEnding::LF)
            .unwrap();
        assert_eq!(
            import(&encrypted, None).unwrap_err(),
            KeyError::NeedsPassphrase
        );
        assert_eq!(
            import(&encrypted, Some("wrong")).unwrap_err(),
            KeyError::NeedsPassphrase
        );
        let imported = import(&encrypted, Some("s3cret")).unwrap();
        assert!(
            decode(&imported.private).is_some(),
            "usable without the passphrase"
        );
        assert_eq!(import("hello", None).unwrap_err(), KeyError::Invalid);
    }
}
