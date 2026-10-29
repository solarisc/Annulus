//! A throwaway vault with made-up hosts: for `annulus-app --preview` and the scene
//! catalogue, so the new UI can be looked at without touching anyone's real vault.

#![allow(clippy::expect_used)] // A preview that cannot write its temporary folder cannot run.

use annulus_vault::crypto::KdfParams;
use annulus_vault::vault::Vault;

/// A made-up host: label, address, user, port, tags and the index of its group.
type Fixture = (
    &'static str,
    &'static str,
    &'static str,
    Option<u16>,
    &'static [&'static str],
    Option<usize>,
);

/// A vault in a fresh temporary folder, with made-up hosts and groups when `populated`. Its
/// key derivation is cheap: it holds nothing real.
///
/// # Panics
/// When the temporary folder cannot be written.
#[must_use]
pub fn fixture_vault(name: &str, populated: bool) -> Vault {
    let dir = std::env::temp_dir().join(format!("annulus-scenes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cheap = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let (mut vault, _) =
        Vault::create(dir.join("vault.anv"), "pw", cheap).expect("a temporary vault");
    if !populated {
        return vault;
    }
    let mut group_uids = Vec::new();
    for name in ["Production", "Staging", "Homelab"] {
        let mut group = vault.draft_group(None).expect("a temporary vault");
        group.name = name.into();
        group_uids.push(group.uid.clone());
        vault.upsert_group(group).expect("a temporary vault");
    }
    let hosts: [Fixture; 14] = [
        (
            "prod-db",
            "10.0.4.12",
            "postgres",
            None,
            &["postgres", "prod"],
            None,
        ),
        (
            "web-1",
            "web-1.example.com",
            "deploy",
            None,
            &["nginx"],
            None,
        ),
        (
            "bastion",
            "jump.example.com",
            "admin",
            Some(2222),
            &["jump"],
            None,
        ),
        (
            "cache-eu-1",
            "10.0.7.31",
            "ubuntu",
            None,
            &["redis", "eu"],
            None,
        ),
        (
            "grafana",
            "grafana.internal",
            "ops",
            None,
            &["monitoring"],
            None,
        ),
        ("build-runner", "10.1.1.40", "ci", None, &[], None),
        (
            "nas",
            "nas.local",
            "admin",
            Some(2200),
            &["storage", "home"],
            None,
        ),
        ("pi-hole", "10.1.1.2", "pi", None, &["dns"], None),
        ("api-1", "10.0.5.21", "deploy", None, &["api"], Some(0)),
        ("api-2", "10.0.5.22", "deploy", None, &["api"], Some(0)),
        ("worker-1", "10.0.6.11", "deploy", None, &[], Some(0)),
        (
            "staging-web",
            "staging.example.com",
            "deploy",
            None,
            &["nginx"],
            Some(1),
        ),
        ("staging-db", "10.1.4.12", "postgres", None, &[], Some(1)),
        ("router", "10.1.1.1", "root", None, &["network"], Some(2)),
    ];
    for (label, address, user, port, tags, group) in hosts {
        let mut host = vault.draft_host();
        host.label = label.into();
        host.address = address.into();
        host.username = user.into();
        host.port = port;
        host.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
        host.group = group.map(|at| group_uids[at].clone());
        vault.upsert_host(host).expect("a temporary vault");
    }
    vault
}
