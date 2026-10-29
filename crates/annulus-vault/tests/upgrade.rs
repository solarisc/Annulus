//! Upgrades never lose a vault: `fixtures/vault-2026-10-29.anv` was written by the
//! build of 2026-10-29, and every later build must open it with everything in it. A format
//! change that breaks this needs a migration, not a new fixture.
//!
//! The fixture was made with `ANNULUS_WRITE_FIXTURE=1 cargo test -p annulus-vault --test
//! upgrade`; it uses cheap Argon2id costs so the test stays fast.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use annulus_vault::crypto::KdfParams;
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::vault::Vault;

const PASSWORD: &str = "upgrade-fixture password";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vault-2026-10-29.anv")
}

/// What the fixture holds: a group with defaults, a host in it with a password, tags and
/// variables, a key, a snippet, a forwarding rule and a session log.
fn write(path: &std::path::Path) {
    let cheap = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let (mut vault, _) = Vault::create(path, PASSWORD, cheap).unwrap();
    let mut group = vault.draft_group(None).unwrap();
    group.name = "Production".into();
    group.defaults.username = "deploy".into();
    group.defaults.port = Some(2222);
    let uid = group.uid.clone();
    vault.upsert_group(group).unwrap();
    let key = SshKey {
        id: vault.next_key_id(),
        label: "laptop".into(),
        private: Secret::new(
            "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n-----END OPENSSH PRIVATE KEY-----\n",
        ),
        public: "ssh-ed25519 AAAAfixture laptop".into(),
        fingerprint: "SHA256:fixture".into(),
    };
    let key_id = key.id;
    vault.upsert_key(key).unwrap();
    let mut host = vault.draft_host();
    host.label = "prod-db".into();
    host.address = "10.0.4.12".into();
    host.group = Some(uid);
    host.tags = vec!["db".into(), "eu".into()];
    host.password = Some(Secret::new("fixture host password"));
    host.key_id = Some(key_id);
    host.env = vec![("LANG".into(), "C.UTF-8".into())];
    host.startup = "uptime".into();
    let host_id = host.id;
    vault.upsert_host(host).unwrap();
    let mut snippet = vault.draft_snippet();
    snippet.label = "Restart nginx".into();
    snippet.script = "sudo systemctl restart nginx".into();
    vault.upsert_snippet(snippet).unwrap();
    let mut rule = vault.draft_forward(host_id);
    rule.label = "postgres".into();
    rule.bind_port = 15432;
    rule.destination_host = "127.0.0.1".into();
    rule.destination_port = 5432;
    vault.upsert_forward(rule).unwrap();
    vault
        .add_log("deploy@prod-db", "$ uptime\n up 3 days\n", 1_793_000_000)
        .unwrap();
}

#[test]
fn a_vault_from_an_earlier_build_opens_with_everything_in_it() {
    let path = fixture();
    if std::env::var_os("ANNULUS_WRITE_FIXTURE").is_some() {
        let _ = std::fs::remove_file(&path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&path);
    }
    // Opened from a copy: opening must never need to change the fixture.
    let copy = std::env::temp_dir().join(format!("annulus-upgrade-{}.anv", std::process::id()));
    std::fs::copy(&path, &copy).unwrap();
    let vault = Vault::unlock(&copy, PASSWORD).unwrap();
    let group = &vault.groups()[0];
    assert_eq!(
        (group.name.as_str(), group.defaults.username.as_str()),
        ("Production", "deploy")
    );
    assert_eq!(group.defaults.port, Some(2222));
    let host = &vault.hosts()[0];
    assert_eq!(
        (host.label.as_str(), host.address.as_str()),
        ("prod-db", "10.0.4.12")
    );
    assert_eq!(host.group.as_deref(), Some(group.uid.as_str()));
    assert_eq!(host.tags, ["db", "eu"]);
    assert_eq!(
        host.password.as_ref().map(Secret::expose),
        Some("fixture host password")
    );
    assert_eq!(host.env, [("LANG".to_owned(), "C.UTF-8".to_owned())]);
    assert_eq!(host.startup, "uptime");
    let key = &vault.ssh_keys()[0];
    assert_eq!(host.key_id, Some(key.id));
    assert!(key.private.expose().contains("fixture"));
    assert_eq!(vault.snippets()[0].script, "sudo systemctl restart nginx");
    let rule = &vault.forwards()[0];
    assert_eq!(
        (rule.host_id, rule.bind_port, rule.destination_port),
        (host.id, 15432, 5432)
    );
    assert_eq!(vault.logs()[0].text, "$ uptime\n up 3 days\n");
    // The inherited port comes from the group.
    let (resolved, _) = vault.resolve(host);
    assert_eq!(resolved.port, Some(2222));
    // Every entity has a sync id waiting to be pushed, as a vault that never synced does.
    assert!(vault.sync_state().pending_count() >= 6);
    let _ = std::fs::remove_file(&copy);
}
