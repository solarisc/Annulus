//! Import from `~/.ssh/config` with a preview (F-IMP-001): every host found is listed, those
//! whose name is already saved start unticked, and nothing is saved until Import.

use annulus_import::ssh_config::{self, Found};
use annulus_vault::forwards::PortForward;
use annulus_vault::hosts::Host;
use annulus_vault::vault::Vault;

/// The preview window.
pub struct ImportPreview {
    pub(crate) rows: Vec<(Found, bool)>,
    pub(crate) existing: Vec<String>,
}

/// The hosts and rules to import, with temporary ids that `Vault::adopt` renumbers.
#[must_use]
pub fn entities(found: &[&Found]) -> (Vec<Host>, Vec<PortForward>) {
    let mut hosts = Vec::new();
    let mut rules = Vec::new();
    for (host_id, entry) in (1..).zip(found) {
        hosts.push(entry.to_host(host_id));
        let first = u64::try_from(rules.len()).unwrap_or(u64::MAX) + 1;
        rules.extend(entry.to_rules(host_id, first));
    }
    // ProxyJump a,b means a, then b, then the host: the host's own jump is the last hop,
    // when that hop is imported too (its own ProxyJump continues the chain).
    for (index, entry) in found.iter().enumerate() {
        let nearest = entry
            .proxy_jump
            .as_deref()
            .and_then(|jumps| jumps.rsplit(',').next())
            .map(str::trim);
        let jump = nearest.and_then(|alias| hosts.iter().find(|host| host.label == alias));
        let jump_id = jump.map(|host| host.id);
        if let Some(host) = hosts.get_mut(index) {
            host.jump_id = jump_id;
        }
    }
    (hosts, rules)
}

impl ImportPreview {
    /// Reads `~/.ssh/config` (and what it includes).
    ///
    /// # Errors
    /// No home folder, or no config file.
    pub fn load(vault: &Vault) -> Result<Self, String> {
        let home = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or("No home folder found.")?;
        let ssh_dir = home.join(".ssh");
        let text = std::fs::read_to_string(ssh_dir.join("config"))
            .map_err(|_| format!("No OpenSSH config at {}.", ssh_dir.join("config").display()))?;
        let found = ssh_config::parse(
            &text,
            &ssh_dir,
            &home,
            &|path| std::fs::read_to_string(path).ok(),
            &|dir| {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(|entry| entry.ok().map(|e| e.path()))
                            .collect()
                    })
                    .unwrap_or_default()
            },
        );
        if found.is_empty() {
            return Err("The OpenSSH config names no hosts.".into());
        }
        let existing: Vec<String> = vault
            .hosts()
            .iter()
            .map(|host| host.label.to_lowercase())
            .collect();
        let rows = found
            .into_iter()
            .map(|entry| {
                let new = !existing.contains(&entry.alias.to_lowercase());
                (entry, new)
            })
            .collect();
        Ok(Self { rows, existing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_point_at_their_own_host_before_renumbering() {
        let text =
            "Host a\n  LocalForward 1 x:1\nHost b\n  LocalForward 2 y:2\n  DynamicForward 3\n";
        let found = ssh_config::parse(
            text,
            std::path::Path::new("/s"),
            std::path::Path::new("/h"),
            &|_| None,
            &|_| Vec::new(),
        );
        let chosen: Vec<&Found> = found.iter().collect();
        let (hosts, rules) = entities(&chosen);
        assert_eq!(hosts.len(), 2);
        let ids: Vec<(u64, u64)> = rules.iter().map(|rule| (rule.id, rule.host_id)).collect();
        assert_eq!(ids, [(1, 1), (2, 2), (3, 2)]);
    }

    #[test]
    fn proxy_jump_links_to_an_imported_host() {
        let text = "Host bastion
  HostName b.example.com
Host app
  ProxyJump outside,bastion
Host lone
  ProxyJump elsewhere
";
        let found = ssh_config::parse(
            text,
            std::path::Path::new("/s"),
            std::path::Path::new("/h"),
            &|_| None,
            &|_| Vec::new(),
        );
        let chosen: Vec<&Found> = found.iter().collect();
        let (hosts, _) = entities(&chosen);
        let jump_of = |label: &str| hosts.iter().find(|h| h.label == label).unwrap().jump_id;
        assert_eq!(jump_of("app"), Some(hosts[0].id));
        assert_eq!(jump_of("lone"), None, "not imported, so no link");
    }
}
