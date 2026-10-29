//! Saved hosts (F-HOST-001, F-HOST-005): the connection details of each SSH target, a
//! search across them, and their file on disk. No secrets live here: passwords and keys
//! belong to the keychain and the encrypted vault.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The default SSH port.
pub const DEFAULT_PORT: u16 = 22;

/// One SSH target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Stable identifier within the vault.
    pub id: u64,
    /// The name shown in the list, e.g. "prod-db".
    pub label: String,
    /// Hostname or IP address.
    pub address: String,
    /// SSH port.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Login name; empty means the SSH client's default.
    #[serde(default)]
    pub username: String,
    /// Free-form tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Host {
    /// The `ssh` destination: `user@address:port`, leaving out a default port and an empty
    /// user. IPv6 addresses are bracketed when a port follows.
    #[must_use]
    pub fn target(&self) -> String {
        let user = if self.username.trim().is_empty() {
            String::new()
        } else {
            format!("{}@", self.username.trim())
        };
        let address = self.address.trim();
        if self.port == DEFAULT_PORT {
            format!("{user}{address}")
        } else if address.contains(':') {
            format!("{user}[{address}]:{}", self.port)
        } else {
            format!("{user}{address}:{}", self.port)
        }
    }

    /// Whether every word of `query` appears in the label, address, user or a tag
    /// (case-insensitive). An empty query matches everything.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let haystack = format!(
            "{} {} {} {}",
            self.label,
            self.address,
            self.username,
            self.tags.join(" ")
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }

    /// Why the host can't be saved, or `None` if it can.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let address = self.address.trim();
        if address.is_empty() {
            Some("Enter a hostname or IP address.")
        } else if address.chars().any(char::is_whitespace) {
            Some("The address can't contain spaces.")
        } else if self.port == 0 {
            Some("The port must be between 1 and 65535.")
        } else if self
            .username
            .trim()
            .chars()
            .any(|c| c.is_whitespace() || c == '@')
        {
            Some("The username can't contain spaces or @.")
        } else {
            None
        }
    }
}

/// Why the host file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The file system refused.
    #[error("host file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a host list (it is kept, never overwritten, so nothing is lost).
    #[error("host file is damaged: {0}")]
    Damaged(#[from] serde_json::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostFile {
    version: u32,
    hosts: Vec<Host>,
}

/// The saved hosts and where they are kept.
#[derive(Debug)]
pub struct HostStore {
    path: PathBuf,
    hosts: Vec<Host>,
}

impl HostStore {
    /// Loads the hosts saved at `path`; a missing file is an empty list.
    ///
    /// # Errors
    /// [`StoreError::Damaged`] if the file exists but can't be read as a host list; it is
    /// left untouched so the user's data is never silently lost.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let hosts = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<HostFile>(&bytes)?.hosts,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, hosts })
    }

    /// All hosts, sorted by label.
    #[must_use]
    pub fn hosts(&self) -> &[Host] {
        &self.hosts
    }

    /// The hosts matching `query`, sorted by label.
    pub fn search<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a Host> + 'a {
        self.hosts.iter().filter(move |host| host.matches(query))
    }

    /// The host with `id`.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Host> {
        self.hosts.iter().find(|host| host.id == id)
    }

    /// A new, unsaved host with a fresh id.
    #[must_use]
    pub fn draft(&self) -> Host {
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
        }
    }

    /// Adds `host` or replaces the one with its id, then saves.
    ///
    /// # Errors
    /// Writing the file failed; the list in memory is unchanged.
    pub fn upsert(&mut self, host: Host) -> Result<(), StoreError> {
        let mut next = self.hosts.clone();
        match next.iter_mut().find(|existing| existing.id == host.id) {
            Some(existing) => *existing = host,
            None => next.push(host),
        }
        next.sort_by_key(|host| host.label.to_lowercase());
        save(&self.path, &next)?;
        self.hosts = next;
        Ok(())
    }

    /// Deletes the host with `id`, then saves.
    ///
    /// # Errors
    /// Writing the file failed; the list in memory is unchanged.
    pub fn remove(&mut self, id: u64) -> Result<(), StoreError> {
        let next: Vec<Host> = self
            .hosts
            .iter()
            .filter(|host| host.id != id)
            .cloned()
            .collect();
        save(&self.path, &next)?;
        self.hosts = next;
        Ok(())
    }
}

/// Writes the list to a temporary file and renames it over the old one, so a crash mid-write
/// never leaves a half-written host file.
fn save(path: &Path, hosts: &[Host]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = HostFile {
        version: 1,
        hosts: hosts.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&file)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: u64, label: &str, address: &str, port: u16, username: &str) -> Host {
        Host {
            id,
            label: label.into(),
            address: address.into(),
            port,
            username: username.into(),
            tags: Vec::new(),
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("annulus-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("hosts.json")
    }

    #[test]
    fn targets_leave_out_defaults_and_bracket_ipv6() {
        assert_eq!(
            host(1, "a", "prod-db", 22, "admin").target(),
            "admin@prod-db"
        );
        assert_eq!(
            host(1, "a", "10.0.4.12", 2222, "").target(),
            "10.0.4.12:2222"
        );
        assert_eq!(
            host(1, "a", "fe80::1", 2222, "root").target(),
            "root@[fe80::1]:2222"
        );
    }

    #[test]
    fn search_needs_every_word_somewhere() {
        let mut prod = host(1, "prod-db", "10.0.4.12", 22, "admin");
        prod.tags = vec!["postgres".into()];
        assert!(prod.matches(""));
        assert!(prod.matches("PROD postgres"));
        assert!(prod.matches("admin 10.0"));
        assert!(!prod.matches("prod staging"));
    }

    #[test]
    fn invalid_hosts_are_explained() {
        assert!(host(1, "a", " ", 22, "").problem().is_some());
        assert!(host(1, "a", "x y", 22, "").problem().is_some());
        assert!(host(1, "a", "x", 0, "").problem().is_some());
        assert!(host(1, "a", "x", 22, "me@x").problem().is_some());
        assert_eq!(host(1, "a", "x", 22, "me").problem(), None);
    }

    #[test]
    fn hosts_survive_a_restart_sorted_and_deletable() {
        let path = temp_path("roundtrip");
        let mut store = HostStore::open(&path).unwrap();
        let mut web = store.draft();
        web.label = "web".into();
        web.address = "web.example".into();
        store.upsert(web).unwrap();
        let mut db = store.draft();
        db.label = "db".into();
        db.address = "db.example".into();
        let db_id = db.id;
        store.upsert(db).unwrap();

        let reopened = HostStore::open(&path).unwrap();
        let labels: Vec<_> = reopened
            .hosts()
            .iter()
            .map(|host| host.label.as_str())
            .collect();
        assert_eq!(labels, ["db", "web"]);

        let mut store = reopened;
        store.remove(db_id).unwrap();
        assert_eq!(HostStore::open(&path).unwrap().hosts().len(), 1);
    }

    #[test]
    fn a_damaged_file_is_reported_and_left_alone() {
        let path = temp_path("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(
            HostStore::open(&path),
            Err(StoreError::Damaged(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }
}
