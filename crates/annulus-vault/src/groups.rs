//! Host groups (F-HOST-003): a tree of folders. A group sets defaults (user, port, key,
//! jump host, proxy) that its hosts and subgroups inherit field by field; a host's own
//! value always wins, and an unset field (empty user, no port, key, jump host or proxy) is
//! what inherits.
//!
//! Hosts and subgroups name their group by its `uid`, which is the same on every device,
//! so sync needs no translation for these links.

use serde::{Deserialize, Serialize};

use crate::crypto::CryptoError;
use crate::hosts::{Host, Secret};

/// One group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Local id within the vault.
    pub id: u64,
    /// Global id: random, never reused, the same on every synced device.
    pub uid: String,
    /// The name shown in the list.
    pub name: String,
    /// The parent group's `uid`; `None` is the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What its hosts inherit.
    #[serde(default)]
    pub defaults: Defaults,
}

/// The fields a group passes down. `None` (or an empty user) passes on what its own parent
/// sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    /// Login name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// SSH port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// A keychain key (local id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<u64>,
    /// A saved host to connect through (local id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_id: Option<u64>,
    /// A proxy URL (may hold a login).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Secret>,
}

/// Where each inherited value of a resolved host came from: the group's name, or `None`
/// when the host sets it itself (or nothing does).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// Login name.
    pub username: Option<String>,
    /// Port.
    pub port: Option<String>,
    /// Key.
    pub key_id: Option<String>,
    /// Jump host.
    pub jump_id: Option<String>,
    /// Proxy.
    pub proxy: Option<String>,
}

/// A new random group uid.
///
/// # Errors
/// No randomness.
pub fn new_uid() -> Result<String, CryptoError> {
    crate::sync::random_id()
}

/// The chain of groups from `uid` up to the top, nearest first. Stops at a missing group or
/// a loop (a synced edit can briefly make one).
#[must_use]
pub fn chain<'a>(groups: &'a [Group], uid: Option<&str>) -> Vec<&'a Group> {
    let mut chain: Vec<&Group> = Vec::new();
    let mut next = uid;
    while let Some(uid) = next {
        let Some(group) = groups.iter().find(|group| group.uid == uid) else {
            break;
        };
        if chain.iter().any(|seen| seen.uid == group.uid) {
            break;
        }
        chain.push(group);
        next = group.parent.as_deref();
    }
    chain
}

/// `host` with every unset field filled from its groups, nearest first, and where each
/// filled value came from.
#[must_use]
pub fn resolve(host: &Host, groups: &[Group]) -> (Host, Sources) {
    let mut resolved = host.clone();
    let mut sources = Sources::default();
    for group in chain(groups, host.group.as_deref()) {
        let defaults = &group.defaults;
        if resolved.username.trim().is_empty() && !defaults.username.trim().is_empty() {
            resolved.username.clone_from(&defaults.username);
            sources.username = Some(group.name.clone());
        }
        if resolved.port.is_none() && defaults.port.is_some() {
            resolved.port = defaults.port;
            sources.port = Some(group.name.clone());
        }
        if resolved.key_id.is_none() && defaults.key_id.is_some() {
            resolved.key_id = defaults.key_id;
            sources.key_id = Some(group.name.clone());
        }
        // A jump host inherits too, except onto itself (it would jump through itself).
        if resolved.jump_id.is_none()
            && defaults.jump_id.is_some()
            && defaults.jump_id != Some(host.id)
        {
            resolved.jump_id = defaults.jump_id;
            sources.jump_id = Some(group.name.clone());
        }
        if resolved.proxy.is_none() && defaults.proxy.is_some() {
            resolved.proxy.clone_from(&defaults.proxy);
            sources.proxy = Some(group.name.clone());
        }
    }
    (resolved, sources)
}

/// Whether `uid` is `ancestor` or lies below it (moving `ancestor` under `uid` would make a
/// loop).
#[must_use]
pub fn is_within(groups: &[Group], uid: &str, ancestor: &str) -> bool {
    chain(groups, Some(uid))
        .iter()
        .any(|group| group.uid == ancestor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(uid: &str, parent: Option<&str>, defaults: Defaults) -> Group {
        Group {
            id: 0,
            uid: uid.into(),
            name: uid.to_uppercase(),
            parent: parent.map(Into::into),
            defaults,
        }
    }

    fn host_in(group: &str) -> Host {
        Host {
            id: 1,
            label: "web".into(),
            address: "10.0.0.1".into(),
            port: None,
            username: String::new(),
            tags: Vec::new(),
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
            no_reconnect: false,
            group: Some(group.into()),
        }
    }

    fn tree() -> Vec<Group> {
        vec![
            group(
                "prod",
                None,
                Defaults {
                    username: "admin".into(),
                    port: Some(2222),
                    key_id: Some(7),
                    jump_id: Some(1),
                    proxy: None,
                },
            ),
            group(
                "db",
                Some("prod"),
                Defaults {
                    username: "postgres".into(),
                    ..Defaults::default()
                },
            ),
        ]
    }

    #[test]
    fn unset_fields_inherit_from_the_nearest_group_that_sets_them() {
        let (resolved, sources) = resolve(&host_in("db"), &tree());
        assert_eq!(resolved.username, "postgres");
        assert_eq!(sources.username.as_deref(), Some("DB"));
        assert_eq!(resolved.port, Some(2222));
        assert_eq!(sources.port.as_deref(), Some("PROD"));
        assert_eq!(resolved.key_id, Some(7));
        assert_eq!(resolved.jump_id, None, "host 1 is the jump host itself");
        let mut other = host_in("db");
        other.id = 2;
        assert_eq!(resolve(&other, &tree()).0.jump_id, Some(1));
        assert_eq!(resolved.proxy, None);
        assert_eq!(sources.proxy, None);
    }

    #[test]
    fn a_hosts_own_value_wins_even_when_it_is_the_default_port() {
        let mut host = host_in("db");
        host.port = Some(22);
        host.username = "me".into();
        let (resolved, sources) = resolve(&host, &tree());
        assert_eq!(
            (resolved.port, resolved.username.as_str()),
            (Some(22), "me")
        );
        assert_eq!((sources.port, sources.username), (None, None));
    }

    #[test]
    fn a_loop_or_a_missing_group_ends_the_chain() {
        let mut groups = tree();
        groups[0].parent = Some("db".into());
        assert_eq!(chain(&groups, Some("db")).len(), 2);
        assert_eq!(chain(&groups, Some("gone")), Vec::<&Group>::new());
        assert!(is_within(&groups, "db", "prod"));
        assert!(!is_within(&tree(), "prod", "db"));
        assert_eq!(new_uid().unwrap().len(), 32);
        assert_ne!(new_uid().unwrap(), new_uid().unwrap());
    }
}
