//! The encrypted vault file (ADR-0007): the key file and one encrypted record per entity.
//! Nothing secret is on disk in the clear, not even host names. Writes are atomic, and a file
//! that cannot be read is reported and never overwritten.
//!
//! This single-file container is the step before SQLite storage; the key hierarchy
//! and record format carry over unchanged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, KdfParams, KeyFile, Keys, Sealed};
use crate::hosts::{DEFAULT_PORT, Host};

/// Record kind of a saved host.
const KIND_HOST: u8 = 1;

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

/// An unlocked vault.
pub struct Vault {
    path: PathBuf,
    key_file: KeyFile,
    keys: Keys,
    hosts: Vec<Host>,
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
        };
        vault.save(&vault.hosts)?;
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
        for record in &file.records {
            if record.kind != KIND_HOST {
                // A newer version wrote kinds this one doesn't know; saving would drop them, so
                // refuse to open rather than lose data silently.
                return Err(VaultError::Damaged(format!(
                    "unknown record kind {}",
                    record.kind
                )));
            }
            let plaintext = keys.open_record(&record.id, record.kind, &record.sealed)?;
            let host: Host = serde_json::from_slice(&plaintext)
                .map_err(|error| VaultError::Damaged(error.to_string()))?;
            hosts.push(host);
        }
        hosts.sort_by_key(|host| host.label.to_lowercase());
        Ok(Self {
            path,
            key_file: file.keys,
            keys,
            hosts,
        })
    }

    /// Changes the master password; records are untouched and the recovery key still works.
    ///
    /// # Errors
    /// Writing the file, or no randomness.
    pub fn change_password(&mut self, password: &str) -> Result<(), VaultError> {
        let key_file = self.keys.change_password(&self.key_file, password)?;
        let previous = std::mem::replace(&mut self.key_file, key_file);
        if let Err(error) = self.save(&self.hosts) {
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

    /// A new, unsaved host with a fresh id.
    #[must_use]
    pub fn draft_host(&self) -> Host {
        let id = self
            .hosts
            .iter()
            .map(|host| host.id)
            .max()
            .map_or(1, |max| max + 1);
        Host {
            id,
            label: String::new(),
            address: String::new(),
            port: DEFAULT_PORT,
            username: String::new(),
            tags: Vec::new(),
            password: None,
        }
    }

    /// Adds or replaces a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn upsert_host(&mut self, host: Host) -> Result<(), VaultError> {
        let mut next = self.hosts.clone();
        match next.iter_mut().find(|existing| existing.id == host.id) {
            Some(existing) => *existing = host,
            None => next.push(host),
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        self.save(&next)?;
        self.hosts = next;
        Ok(())
    }

    /// Deletes a host, then saves. On failure nothing changes.
    ///
    /// # Errors
    /// Writing the file.
    pub fn remove_host(&mut self, id: u64) -> Result<(), VaultError> {
        let next: Vec<Host> = self
            .hosts
            .iter()
            .filter(|host| host.id != id)
            .cloned()
            .collect();
        self.save(&next)?;
        self.hosts = next;
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
        let mut id = next.iter().map(|host| host.id).max().unwrap_or(0);
        for mut host in imported.iter().cloned() {
            id += 1;
            host.id = id;
            next.push(host);
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        self.save(&next)?;
        self.hosts = next;
        std::fs::remove_file(plain)?;
        Ok(imported.len())
    }

    fn save(&self, hosts: &[Host]) -> Result<(), VaultError> {
        let mut records = Vec::with_capacity(hosts.len());
        for host in hosts {
            let id = format!("host-{}", host.id);
            let plaintext = Zeroizing::new(
                serde_json::to_vec(host).map_err(|error| VaultError::Damaged(error.to_string()))?,
            );
            let sealed = self.keys.seal_record(&id, KIND_HOST, &plaintext)?;
            records.push(Record {
                id,
                kind: KIND_HOST,
                sealed,
            });
        }
        let file = VaultFile {
            version: 1,
            keys: self.key_file.clone(),
            records,
        };
        let bytes =
            serde_json::to_vec(&file).map_err(|error| VaultError::Damaged(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, &self.path)?;
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
        // Flip one ciphertext digit.
        let at = text.find("\"ciphertext\":[").unwrap() + 14;
        let digit = text.as_bytes()[at];
        text.replace_range(at..=at, if digit == b'1' { "2" } else { "1" });
        std::fs::write(&path, &text).unwrap();
        assert!(matches!(
            Vault::unlock(&path, "pw"),
            Err(VaultError::Damaged(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
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
