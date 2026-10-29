//! Saved hosts (F-HOST-001, F-HOST-005): the connection details of each SSH target and the
//! search across them. They are stored in the encrypted vault ([`crate::vault`]).

use std::path::Path;

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
    /// SSH port; `None` inherits from the group, else 22 (F-HOST-003).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Login name; empty inherits from the group, else the SSH client's default.
    #[serde(default)]
    pub username: String,
    /// Free-form tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// A saved password, tried before asking (encrypted with the rest of the vault record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
    /// The keychain key to log in with, tried before the default key files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<u64>,
    /// The saved host to connect through (`ProxyJump`); it may have its own jump host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_id: Option<u64>,
    /// A proxy to reach the host through: `socks5://…` or `http://…` (may hold a login,
    /// which is why it lives in the encrypted record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Secret>,
    /// Never reconnect on its own when the connection drops (F-SSH-009).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_reconnect: bool,
    /// The group it is in ([`crate::groups::Group::uid`]); `None` is the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Environment variables sent before the shell starts (F-SSH-012); its groups add the
    /// names it does not set. The server accepts only those its `AcceptEnv` allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    /// A command typed into the shell once it opens (F-SSH-013); empty inherits.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub startup: String,
}

/// A secret string: wiped from memory when dropped, and never shown by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(zeroize::Zeroizing<String>);

impl Secret {
    /// Wraps `value`.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(zeroize::Zeroizing::new(value.into()))
    }

    /// The secret itself. Hand it only to the code that needs it, and never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl Host {
    /// The port to connect to: its own, else 22.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }

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
        let port = self.port();
        if port == DEFAULT_PORT {
            format!("{user}{address}")
        } else if address.contains(':') {
            format!("{user}[{address}]:{port}")
        } else {
            format!("{user}{address}:{port}")
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
        } else if self.port == Some(0) {
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

/// Why an old plain host file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum PlainFileError {
    /// The file system refused.
    #[error("host file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a host list (it is kept, never overwritten).
    #[error("host file is damaged: {0}")]
    Damaged(#[from] serde_json::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostFile {
    version: u32,
    hosts: Vec<Host>,
}

/// Reads the plain host file of earlier builds (before the encrypted vault) for import; a
/// missing file is an empty list.
///
/// # Errors
/// The file exists but cannot be read as a host list.
pub fn load_plain(path: &Path) -> Result<Vec<Host>, PlainFileError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<HostFile>(&bytes)?.hosts),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: u64, label: &str, address: &str, port: u16, username: &str) -> Host {
        Host {
            id,
            label: label.into(),
            address: address.into(),
            port: Some(port),
            username: username.into(),
            tags: Vec::new(),
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
            no_reconnect: false,
            group: None,
            env: Vec::new(),
            startup: String::new(),
        }
    }

    #[test]
    fn an_older_record_keeps_its_port_and_a_missing_port_is_unset() {
        let old: Host =
            serde_json::from_str(r#"{"id":1,"label":"a","address":"x","port":22}"#).unwrap();
        assert_eq!(old.port, Some(22), "an explicit 22 does not inherit");
        let new: Host = serde_json::from_str(r#"{"id":1,"label":"a","address":"x"}"#).unwrap();
        assert_eq!((new.port, new.port()), (None, 22));
        assert!(!serde_json::to_string(&new).unwrap().contains("port"));
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
    fn a_saved_password_never_shows_in_debug_output() {
        let mut host = host(1, "a", "x", 22, "me");
        host.password = Some(Secret::new("hunter2"));
        assert!(!format!("{host:?}").contains("hunter2"));
        assert!(
            !host.matches("hunter2"),
            "search never looks at the password"
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
    fn a_missing_plain_file_is_an_empty_list() {
        let hosts = load_plain(Path::new("does-not-exist/hosts.json")).unwrap();
        assert_eq!(hosts, Vec::<Host>::new());
    }
}
