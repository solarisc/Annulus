//! Sync against a deployed `annulus-server` (the Linux release build on a test VM),
//! reached at `ANNULUS_E2E_SYNC_URL` (an `http://127.0.0.1:…` tunnel to it). Ignored by
//! default; docs/TESTING.md "Test VMs" says how to run it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use annulus_sync::account;
use annulus_sync::engine::{restore, sync};
use annulus_sync::http::HttpServer;
use annulus_sync::replica::VaultReplica;
use annulus_vault::crypto::KdfParams;
use annulus_vault::hosts::Secret;
use annulus_vault::sync::SyncAccount;
use annulus_vault::vault::Vault;

const CHEAP: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("annulus-e2e-sync-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("vault.anv")
}

fn round(url: &str, account: &SyncAccount, vault: &mut Vault) {
    let mut http = HttpServer::new(url, Some(account.token.expose().to_owned())).unwrap();
    let mut replica = VaultReplica(vault.sync_copy());
    sync(&mut http, &mut replica).expect("sync round");
    assert!(vault.commit_sync(replica.0).unwrap());
}

/// Host "db" in group "databases", which gives it user postgres and port 5422, and a
/// 1 MiB session log.
fn add_grouped_host(vault: &mut Vault) {
    let mut group = vault.draft_group(None).unwrap();
    group.name = "databases".into();
    group.defaults.username = "postgres".into();
    group.defaults.port = Some(5422);
    vault.upsert_group(group.clone()).unwrap();
    let mut host = vault.draft_host();
    host.label = "db".into();
    host.address = "10.0.0.5".into();
    host.group = Some(group.uid);
    vault.upsert_host(host).unwrap();
    // A full-size session log (1 MiB, kept): bigger than one push batch once sealed.
    vault
        .add_log("db", &"line of output\n".repeat(80_000), 1)
        .unwrap();
}

#[test]
#[ignore = "needs a deployed annulus-server"]
fn two_devices_sync_restore_and_change_password_through_a_real_server() {
    let url = std::env::var("ANNULUS_E2E_SYNC_URL").expect("ANNULUS_E2E_SYNC_URL");
    let name = format!("e2e-{}", std::process::id());
    let ksf = account::default_ksf();

    // Device A creates the account from its vault.
    let (mut laptop, _) = Vault::create(temp("a"), "first password!", CHEAP).unwrap();
    add_grouped_host(&mut laptop);
    let mut http = HttpServer::new(&url, None).unwrap();
    let session = account::register(
        &mut http,
        &ksf,
        &name,
        "first password!",
        laptop.key_file_bytes(),
        "laptop",
    )
    .expect("register");
    let laptop_account = SyncAccount {
        server: url.clone(),
        name: name.clone(),
        device: session.device,
        token: Secret::new(session.token),
    };
    laptop
        .set_sync_account(Some(laptop_account.clone()))
        .unwrap();
    round(&url, &laptop_account, &mut laptop);

    // Device B signs in and gets the host.
    let session =
        account::login(&mut http, &ksf, &name, "first password!", "desktop").expect("login");
    let desktop_account = SyncAccount {
        server: url.clone(),
        name: name.clone(),
        device: session.device,
        token: Secret::new(session.token),
    };
    let mut desktop = Vault::join(
        temp("b"),
        &session.wrapped_keys,
        "first password!",
        desktop_account.clone(),
    )
    .unwrap();
    round(&url, &desktop_account, &mut desktop);
    assert_eq!(desktop.hosts()[0].address, "10.0.0.5");
    let (resolved, sources) = desktop.resolve(&desktop.hosts()[0]);
    assert_eq!(
        (resolved.username.as_str(), resolved.port),
        ("postgres", Some(5422))
    );
    assert_eq!(sources.port.as_deref(), Some("databases"));
    assert!(
        desktop.logs()[0].text.len() > 1_000_000,
        "the full-size log arrived"
    );

    // Deleted on B, restored from the trash on A.
    let id = desktop.hosts()[0].id;
    desktop.remove_host(id).unwrap();
    round(&url, &desktop_account, &mut desktop);
    round(&url, &laptop_account, &mut laptop);
    assert_eq!(laptop.hosts(), []);
    let authed = HttpServer::new(&url, Some(laptop_account.token.expose().to_owned())).unwrap();
    let mut replica = VaultReplica(laptop.sync_copy());
    let (sync_id, _) = replica.0.deleted()[0].clone();
    let version = authed
        .history(&sync_id)
        .unwrap()
        .into_iter()
        .find(|v| !v.deleted)
        .unwrap();
    let mut transport =
        HttpServer::new(&url, Some(laptop_account.token.expose().to_owned())).unwrap();
    restore(&mut transport, &mut replica, &sync_id, &version).unwrap();
    assert!(laptop.commit_sync(replica.0).unwrap());
    round(&url, &desktop_account, &mut desktop);
    assert_eq!(desktop.hosts()[0].label, "db");

    // A password change signs B out; B signs in again with the new one and relinks.
    let file = laptop
        .password_kit()
        .rewrap("first password!", "second password!")
        .unwrap();
    let mut authed = HttpServer::new(&url, Some(laptop_account.token.expose().to_owned())).unwrap();
    account::change_password(&mut authed, &ksf, "second password!", file.clone()).unwrap();
    laptop.set_key_file(&file).unwrap();
    let mut b_http =
        HttpServer::new(&url, Some(desktop_account.token.expose().to_owned())).unwrap();
    let mut replica = VaultReplica(desktop.sync_copy());
    assert!(sync(&mut b_http, &mut replica).is_err(), "signed out");
    let session = account::login(&mut http, &ksf, &name, "second password!", "desktop").unwrap();
    assert!(
        desktop
            .sync_copy()
            .same_account(&session.wrapped_keys, "second password!")
    );
    let relinked = SyncAccount {
        device: session.device,
        token: Secret::new(session.token),
        ..desktop_account
    };
    desktop
        .relink(&session.wrapped_keys, relinked.clone())
        .unwrap();
    round(&url, &relinked, &mut desktop);
    assert_eq!(desktop.hosts().len(), 1, "nothing duplicated");
}
