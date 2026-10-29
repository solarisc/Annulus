//! Two devices syncing through a real server (in-process, without HTTP): offline edits,
//! conflicts both ways, edit against delete, and the OPAQUE account flow.

use std::collections::HashMap;
use std::path::Path;

use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use annulus_proto::{
    Change, Changes, LoginFinish, LoginStart, LoginStartResponse, PushResult, RegisterFinish,
    RegisterStart, RegisterStartResponse, Session,
};
use annulus_server::{Caller, Server, ServerError, Signups};

use crate::account::{self, Auth};
use crate::engine::{Item, Outgoing, Replica, SyncError, Transport, sync};

fn cheap() -> Argon2<'static> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(8, 1, 1, None).unwrap(),
    )
}

fn transport(error: &ServerError) -> SyncError {
    SyncError::Transport(error.to_string())
}

/// The server called directly.
struct Direct<'a> {
    server: &'a Server,
    caller: Option<Caller>,
}

impl Direct<'_> {
    fn caller(&self) -> Result<Caller, SyncError> {
        self.caller.ok_or(SyncError::Unauthorized)
    }
}

impl Transport for Direct<'_> {
    fn push(&mut self, changes: &[Change]) -> Result<PushResult, SyncError> {
        self.server
            .push(self.caller()?, changes)
            .map_err(|error| transport(&error))
    }

    fn pull(&mut self, since: u64) -> Result<Changes, SyncError> {
        self.server
            .pull(self.caller()?, since)
            .map_err(|error| transport(&error))
    }
}

impl Auth for Direct<'_> {
    fn register_start(
        &mut self,
        request: &RegisterStart,
    ) -> Result<RegisterStartResponse, SyncError> {
        self.server
            .register_start(request)
            .map_err(|error| transport(&error))
    }

    fn register_finish(&mut self, request: &RegisterFinish) -> Result<Session, SyncError> {
        self.server
            .register_finish(request)
            .map_err(|error| transport(&error))
    }

    fn login_start(&mut self, request: &LoginStart) -> Result<LoginStartResponse, SyncError> {
        self.server
            .login_start(request, None)
            .map_err(|error| transport(&error))
    }

    fn login_finish(&mut self, request: &LoginFinish) -> Result<Session, SyncError> {
        self.server
            .login_finish(request)
            .map_err(|error| transport(&error))
    }
}

/// A device's records in memory. "Encryption" is a plain layout; the vault does the real
/// sealing.
/// Clock times in these tests end in the device's number: 1001 is device 1's.
#[derive(Default)]
struct Device {
    node: u64,
    cursor: u64,
    records: HashMap<String, (u64, Option<Item>)>,
    pending: HashMap<String, Outgoing>,
}

impl Device {
    fn edit(&mut self, id: &str, body: &str, hlc: u64) {
        let base = self.records.get(id).map_or(0, |(seq, _)| *seq);
        let item = Item {
            kind: 1,
            hlc,
            body: body.as_bytes().to_vec(),
        };
        self.pending.insert(
            id.to_owned(),
            Outgoing {
                id: id.to_owned(),
                base,
                item: Some(item),
                hlc,
            },
        );
    }

    fn delete(&mut self, id: &str, hlc: u64) {
        let base = self.records.get(id).map_or(0, |(seq, _)| *seq);
        self.pending.insert(
            id.to_owned(),
            Outgoing {
                id: id.to_owned(),
                base,
                item: None,
                hlc,
            },
        );
    }

    fn body(&self, id: &str) -> Option<String> {
        let (_, item) = self.records.get(id)?;
        item.as_ref()
            .map(|item| String::from_utf8_lossy(&item.body).into_owned())
    }
}

fn seal(item: &Item) -> Vec<u8> {
    let mut bytes = vec![item.kind];
    bytes.extend_from_slice(&item.hlc.to_le_bytes());
    bytes.extend_from_slice(&item.body);
    bytes
}

impl Replica for Device {
    fn cursor(&self) -> u64 {
        self.cursor
    }

    fn set_cursor(&mut self, cursor: u64) {
        self.cursor = cursor;
    }

    fn pending(&self) -> Vec<Outgoing> {
        self.pending.values().cloned().collect()
    }

    fn version(&self, id: &str) -> u64 {
        self.records.get(id).map_or(0, |(seq, _)| *seq)
    }

    fn seal(&self, _id: &str, item: &Item) -> Result<Vec<u8>, SyncError> {
        Ok(seal(item))
    }

    fn open(&self, id: &str, ciphertext: &[u8]) -> Result<Item, SyncError> {
        let (kind, rest) = ciphertext
            .split_first()
            .ok_or_else(|| SyncError::Undecryptable(id.to_owned()))?;
        let (hlc, body) = rest.split_at(8);
        Ok(Item {
            kind: *kind,
            hlc: u64::from_le_bytes(hlc.try_into().unwrap()),
            body: body.to_vec(),
        })
    }

    fn pushed(&mut self, id: &str, seq: u64) {
        let item = self.pending.remove(id).map_or_else(
            || self.records.get(id).and_then(|(_, item)| item.clone()),
            |change| change.item,
        );
        self.records.insert(id.to_owned(), (seq, item));
    }

    fn apply(&mut self, id: &str, seq: u64, item: Option<Item>) {
        self.pending.remove(id);
        self.records.insert(id.to_owned(), (seq, item));
    }

    fn rebase(&mut self, id: &str, seq: u64) {
        if let Some(change) = self.pending.get_mut(id) {
            change.base = seq;
        }
    }

    fn own_time(&self, hlc: u64) -> bool {
        hlc % 1000 == self.node
    }

    fn restart(&mut self) {
        self.cursor = 0;
        for (id, (seq, item)) in &mut self.records {
            *seq = 0;
            let pending = self.pending.entry(id.clone()).or_insert_with(|| Outgoing {
                id: id.clone(),
                base: 0,
                item: item.clone(),
                hlc: item.as_ref().map_or(0, |item| item.hlc),
            });
            pending.base = 0;
        }
    }
}

/// Registers "ana" on one device and signs in a second; returns both device tokens.
fn tokens(server: &Server) -> (String, String) {
    let mut direct = Direct {
        server,
        caller: None,
    };
    let first =
        account::register(&mut direct, &cheap(), "ana", "pw", b"keys".to_vec(), "a").unwrap();
    let second = account::login(&mut direct, &cheap(), "ana", "pw", "b").unwrap();
    assert_eq!(second.wrapped_keys, b"keys");
    (first.token, second.token)
}

fn account(server: &Server) -> (Caller, Caller) {
    let (first, second) = tokens(server);
    (
        server.authenticate(&first).unwrap(),
        server.authenticate(&second).unwrap(),
    )
}

fn run(server: &Server, caller: Caller, device: &mut Device) -> crate::engine::Report {
    let mut direct = Direct {
        server,
        caller: Some(caller),
    };
    sync(&mut direct, device).unwrap()
}

fn server() -> Server {
    Server::open(Path::new(":memory:"), Signups::Open).unwrap()
}

#[test]
fn a_record_made_on_one_device_reaches_the_other() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "prod-db", 10);
    assert_eq!(run(&server, a_id, &mut a).pushed, 1);
    assert_eq!(run(&server, b_id, &mut b).pulled, 1);
    assert_eq!(b.body("r1").as_deref(), Some("prod-db"));
    // Nothing new: a second round moves nothing.
    let quiet = run(&server, b_id, &mut b);
    assert_eq!((quiet.pulled, quiet.pushed), (0, 0));
}

#[test]
fn the_newer_of_two_offline_edits_wins_on_both_devices() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);

    a.edit("r1", "a's edit", 100);
    b.edit("r1", "b's newer edit", 200);
    run(&server, a_id, &mut a);
    let report = run(&server, b_id, &mut b);
    assert_eq!(report.conflicts, ["r1"]);
    run(&server, a_id, &mut a);
    assert_eq!(a.body("r1").as_deref(), Some("b's newer edit"));
    assert_eq!(b.body("r1").as_deref(), Some("b's newer edit"));
}

#[test]
fn a_losing_local_edit_is_kept_in_the_server_history() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);

    b.edit("r1", "b's older edit", 100);
    a.edit("r1", "a's newer edit", 200);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1").as_deref(), Some("a's newer edit"));
    let history = server.history(b_id, "r1").unwrap();
    assert!(
        history
            .iter()
            .any(|old| old.ciphertext.ends_with(b"b's older edit")),
        "the losing edit must be recoverable"
    );
    run(&server, a_id, &mut a);
    assert_eq!(a.body("r1").as_deref(), Some("a's newer edit"));
}

#[test]
fn an_edit_beats_a_deletion_from_another_device() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);

    a.delete("r1", 300);
    b.edit("r1", "still needed", 100);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    run(&server, a_id, &mut a);
    assert_eq!(a.body("r1").as_deref(), Some("still needed"));
    assert_eq!(b.body("r1").as_deref(), Some("still needed"));
}

#[test]
fn a_deletion_reaches_the_other_device() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    a.delete("r1", 20);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1"), None);
    assert!(b.records.contains_key("r1"), "kept as a tombstone");
}

#[test]
fn a_wrong_password_fails_before_the_server_can_check_it() {
    let server = server();
    account(&server);
    let mut direct = Direct {
        server: &server,
        caller: None,
    };
    assert!(account::login(&mut direct, &cheap(), "ana", "wrong", "c").is_err());
}

/// Starts the real HTTP server on a free loopback port, on its own thread.
fn http_server() -> String {
    let server = std::sync::Arc::new(server());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, annulus_server::service(server))
                .await
                .unwrap();
        });
    });
    format!("http://{address}")
}

#[test]
fn a_full_round_over_http_then_a_revoked_device_is_refused() {
    let base = http_server();
    let mut laptop = crate::http::HttpServer::new(&base, None).unwrap();
    let session = account::register(
        &mut laptop,
        &cheap(),
        "ana",
        "pw",
        b"keys".to_vec(),
        "laptop",
    )
    .unwrap();
    laptop.set_token(Some(session.token.clone()));
    let mut device = Device::default();
    device.edit("r1", "prod-db", 10);
    assert_eq!(sync(&mut laptop, &mut device).unwrap().pushed, 1);

    let mut phone = crate::http::HttpServer::new(&base, None).unwrap();
    let phone_session = account::login(&mut phone, &cheap(), "ana", "pw", "phone").unwrap();
    phone.set_token(Some(phone_session.token));
    let mut phone_copy = Device::default();
    sync(&mut phone, &mut phone_copy).unwrap();
    assert_eq!(phone_copy.body("r1").as_deref(), Some("prod-db"));
    assert_eq!(laptop.devices().unwrap().devices.len(), 2);

    laptop.revoke(phone_session.device).unwrap();
    assert!(matches!(
        sync(&mut phone, &mut phone_copy),
        Err(SyncError::Unauthorized)
    ));
}

mod vaults {
    //! Two real vaults, on two "devices", syncing hosts and forwarding rules.

    use annulus_vault::crypto::KdfParams;
    use annulus_vault::hosts::Secret;
    use annulus_vault::sync::SyncAccount;
    use annulus_vault::vault::Vault;

    use super::*;
    use crate::replica::VaultReplica;

    const CHEAP_KDF: KdfParams = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("annulus-sync-vault-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("vault.anv")
    }

    fn round(server: &Server, caller: Caller, vault: &mut Vault) {
        let mut direct = Direct {
            server,
            caller: Some(caller),
        };
        let mut replica = VaultReplica(vault.sync_copy());
        sync(&mut direct, &mut replica).unwrap();
        assert!(vault.commit_sync(replica.0).unwrap());
    }

    fn account(device: u64) -> SyncAccount {
        SyncAccount {
            server: "http://127.0.0.1".into(),
            name: "ana".into(),
            device,
            token: Secret::new("t"),
        }
    }

    #[test]
    fn hosts_and_rules_reach_a_device_that_joins_and_edits_come_back() {
        let server = server();
        let (mut laptop, _) = Vault::create(temp("laptop"), "pw", CHEAP_KDF).unwrap();
        let mut host = laptop.draft_host();
        host.label = "bastion".into();
        host.address = "10.0.0.1".into();
        let key = annulus_vault::keychain::SshKey {
            id: laptop.next_key_id(),
            label: "work key".into(),
            private: Secret::new("private"),
            public: "ssh-ed25519 AAAA".into(),
            fingerprint: "SHA256:k".into(),
        };
        laptop.upsert_key(key.clone()).unwrap();
        host.key_id = Some(key.id);
        laptop.upsert_host(host).unwrap();
        let host_id = laptop.hosts()[0].id;
        let mut rule = laptop.draft_forward(host_id);
        rule.label = "db".into();
        rule.bind_port = 15432;
        rule.destination_host = "db".into();
        rule.destination_port = 5432;
        laptop.upsert_forward(rule).unwrap();
        let mut snippet = laptop.draft_snippet();
        snippet.label = "disk".into();
        snippet.script = "df -h".into();
        laptop.upsert_snippet(snippet).unwrap();

        let mut direct = Direct {
            server: &server,
            caller: None,
        };
        let first = account::register(
            &mut direct,
            &cheap(),
            "ana",
            "pw",
            laptop.key_file_bytes(),
            "laptop",
        )
        .unwrap();
        laptop
            .set_sync_account(Some(account(first.device)))
            .unwrap();
        let laptop_caller = server.authenticate(&first.token).unwrap();
        round(&server, laptop_caller, &mut laptop);
        assert_eq!(laptop.sync_state().pending_count(), 0);

        // The second device signs in and builds its vault from the account's key file.
        let desktop_path = temp("desktop");
        let second = account::login(&mut direct, &cheap(), "ana", "pw", "desktop").unwrap();
        let mut desktop = Vault::join(
            desktop_path.clone(),
            &second.wrapped_keys,
            "pw",
            account(second.device),
        )
        .unwrap();
        let desktop_caller = server.authenticate(&second.token).unwrap();
        round(&server, desktop_caller, &mut desktop);
        assert_eq!(desktop.hosts()[0].label, "bastion");
        let synced_rule = &desktop.forwards()[0];
        assert_eq!(
            synced_rule.host_id,
            desktop.hosts()[0].id,
            "the rule finds its host"
        );
        assert_eq!(desktop.ssh_keys()[0].label, "work key");
        assert_eq!(desktop.snippets()[0].script, "df -h");
        assert_eq!(
            desktop.hosts()[0].key_id,
            Some(desktop.ssh_keys()[0].id),
            "the host finds its key"
        );

        // An edit on the desktop reaches the laptop; a deletion on the laptop reaches back.
        let mut edited = desktop.hosts()[0].clone();
        edited.address = "10.0.0.2".into();
        desktop.upsert_host(edited).unwrap();
        round(&server, desktop_caller, &mut desktop);
        round(&server, laptop_caller, &mut laptop);
        assert_eq!(laptop.hosts()[0].address, "10.0.0.2");
        let rule_id = laptop.forwards()[0].id;
        laptop.remove_forward(rule_id).unwrap();
        round(&server, laptop_caller, &mut laptop);
        round(&server, desktop_caller, &mut desktop);
        assert_eq!(desktop.forwards(), []);
        // Both survive a lock and unlock with their sync state.
        let reopened = Vault::unlock(&desktop_path, "pw").unwrap();
        assert_eq!(reopened.sync_state().cursor, desktop.sync_state().cursor);
    }

    #[test]
    fn a_snapshot_older_than_a_local_edit_is_not_applied() {
        let (mut vault, _) = Vault::create(temp("stale"), "pw", CHEAP_KDF).unwrap();
        let copy = vault.sync_copy();
        let mut host = vault.draft_host();
        host.label = "new".into();
        host.address = "h".into();
        vault.upsert_host(host).unwrap();
        assert!(!vault.commit_sync(copy).unwrap());
        assert_eq!(vault.hosts().len(), 1, "the local edit stays");
    }
}

#[test]
fn a_round_whose_result_was_dropped_is_not_a_conflict_next_time() {
    let server = server();
    let (a_id, _) = account(&server);
    let mut a = Device {
        node: 1,
        ..Device::default()
    };
    a.edit("r1", "v1", 1001);
    // The round pushes, but its result is thrown away (the vault was edited meanwhile).
    let mut dropped = Device {
        node: 1,
        ..Device::default()
    };
    dropped.edit("r1", "v1", 1001);
    run(&server, a_id, &mut dropped);

    let report = run(&server, a_id, &mut a);
    assert_eq!(report.conflicts, Vec::<String>::new());
    assert_eq!(server.history(a_id, "r1").unwrap(), []);
    assert_eq!(a.pending.len(), 0);
}

#[test]
fn devices_recover_from_a_server_restored_from_an_older_backup() {
    let first = server();
    let (a_token, b_token) = tokens(&first);
    let (a_id, b_id) = (
        first.authenticate(&a_token).unwrap(),
        first.authenticate(&b_token).unwrap(),
    );
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "kept", 10);
    run(&first, a_id, &mut a);
    let backup =
        std::env::temp_dir().join(format!("annulus-sync-restore-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&backup);
    first.backup(&backup).unwrap();
    a.edit("r2", "lost by the restore", 20);
    run(&first, a_id, &mut a);
    run(&first, b_id, &mut b);

    // The server comes back from the backup: it never saw r2.
    let restored = Server::open(&backup, Signups::Closed).unwrap();
    let a_restored = restored.authenticate(&a_token).unwrap();
    let b_restored = restored.authenticate(&b_token).unwrap();
    run(&restored, a_restored, &mut a);
    run(&restored, b_restored, &mut b);
    b.edit("r3", "new after the restore", 30);
    run(&restored, b_restored, &mut b);
    run(&restored, a_restored, &mut a);
    for device in [&a, &b] {
        assert_eq!(device.body("r2").as_deref(), Some("lost by the restore"));
    }
    assert_eq!(a.body("r3").as_deref(), Some("new after the restore"));
    assert_eq!(restored.pull(b_restored, 0).unwrap().records.len(), 3);
}
