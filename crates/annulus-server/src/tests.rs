//! The server logic with a real OPAQUE client: registration, login, the sync rules, and
//! revocation. The HTTP layer is covered from `annulus-sync`, which talks to it.

use std::path::Path;

use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, Identifiers, RegistrationResponse,
};
use opaque_rand::rngs::OsRng;
use annulus_proto::{Change, LoginFinish, LoginStart, RegisterFinish, RegisterStart, Suite};

use super::*;
use crate::store::Caller;

/// Argon2id with tiny costs, so tests run fast.
fn cheap() -> Argon2<'static> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(8, 1, 1, None).unwrap(),
    )
}

fn server(signups: Signups) -> Server {
    Server::open(Path::new(":memory:"), signups).unwrap()
}

fn register(
    server: &Server,
    account: &str,
    password: &str,
) -> Result<annulus_proto::Session, ServerError> {
    let ksf = cheap();
    let start = ClientRegistration::<Suite>::start(&mut OsRng, password.as_bytes()).unwrap();
    let response = server.register_start(&RegisterStart {
        account: account.into(),
        request: start.message.serialize().to_vec(),
    })?;
    let finish = start
        .state
        .finish(
            &mut OsRng,
            password.as_bytes(),
            RegistrationResponse::deserialize(&response.response).unwrap(),
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )
        .unwrap();
    server.register_finish(&RegisterFinish {
        account: account.into(),
        upload: finish.message.serialize().to_vec(),
        wrapped_keys: b"wrapped".to_vec(),
        device: "laptop".into(),
    })
}

fn login(
    server: &Server,
    account: &str,
    password: &str,
) -> Result<annulus_proto::Session, ServerError> {
    let ksf = cheap();
    let start = ClientLogin::<Suite>::start(&mut OsRng, password.as_bytes()).unwrap();
    let response = server.login_start(&LoginStart {
        account: account.into(),
        request: start.message.serialize().to_vec(),
    })?;
    // A wrong password already fails on the client: it cannot open the envelope. Send a
    // garbage finalization then, as an attacker would.
    let finalization = start
        .state
        .finish(
            &mut OsRng,
            password.as_bytes(),
            CredentialResponse::deserialize(&response.response).unwrap(),
            ClientLoginFinishParameters::new(None, Identifiers::default(), Some(&ksf)),
        )
        .map_or_else(|_| vec![0u8; 64], |done| done.message.serialize().to_vec());
    server.login_finish(&LoginFinish {
        login: response.login,
        finalization,
        device: "phone".into(),
    })
}

fn caller(server: &Server, token: &str) -> Caller {
    server.authenticate(token).unwrap()
}

fn change(id: &str, base: u64, body: &[u8]) -> Change {
    Change {
        id: id.into(),
        base,
        ciphertext: body.to_vec(),
        deleted: false,
    }
}

#[test]
fn register_then_login_returns_the_wrapped_keys_to_a_new_device() {
    let server = server(Signups::Open);
    let first = register(&server, "ana", "correct horse").unwrap();
    let second = login(&server, "ana", "correct horse").unwrap();
    assert_eq!(second.wrapped_keys, b"wrapped");
    assert_ne!(first.token, second.token);
    let devices = server
        .devices(caller(&server, &second.token))
        .unwrap()
        .devices;
    assert_eq!(devices.len(), 2);
    assert!(
        devices
            .iter()
            .any(|device| device.current && device.name == "phone")
    );
}

#[test]
fn wrong_passwords_and_unknown_accounts_fail_the_same_way_then_throttle() {
    let server = server(Signups::Open);
    register(&server, "ana", "correct horse").unwrap();
    let wrong = login(&server, "ana", "battery staple")
        .unwrap_err()
        .to_string();
    let unknown = login(&server, "bob", "anything").unwrap_err().to_string();
    assert_eq!(wrong, unknown);
    for _ in 0..4 {
        let _ = login(&server, "ana", "battery staple");
    }
    assert!(matches!(
        login(&server, "ana", "correct horse"),
        Err(ServerError::Throttled)
    ));
}

#[test]
fn closed_signups_and_taken_names_are_refused() {
    assert!(matches!(
        register(&server(Signups::Closed), "ana", "pw"),
        Err(ServerError::Refused(_))
    ));
    let open = server(Signups::Open);
    register(&open, "ana", "pw one").unwrap();
    assert!(matches!(
        register(&open, "ana", "pw two"),
        Err(ServerError::Refused(_))
    ));
}

#[test]
fn a_change_from_an_outdated_version_comes_back_as_a_conflict() {
    let server = server(Signups::Open);
    let session = register(&server, "ana", "pw").unwrap();
    let me = caller(&server, &session.token);

    let first = server.push(me, &[change("r1", 0, b"v1")]).unwrap();
    assert_eq!(first.accepted, [("r1".to_owned(), 1)]);
    server.push(me, &[change("r1", 1, b"v2")]).unwrap();
    // Another device still on version 1.
    let stale = server.push(me, &[change("r1", 1, b"other")]).unwrap();
    assert_eq!(stale.accepted, []);
    assert_eq!(stale.conflicts[0].ciphertext, b"v2");
    assert_eq!(stale.conflicts[0].seq, 2);

    let pulled = server.pull(me, 0).unwrap();
    assert_eq!(pulled.latest, 2);
    assert_eq!(pulled.records.len(), 1);
    assert_eq!(server.pull(me, 2).unwrap().records, []);
    assert_eq!(server.history(me, "r1").unwrap()[0].ciphertext, b"v1");
}

#[test]
fn a_deletion_is_a_tombstone_with_history() {
    let server = server(Signups::Open);
    let session = register(&server, "ana", "pw").unwrap();
    let me = caller(&server, &session.token);
    server.push(me, &[change("r1", 0, b"v1")]).unwrap();
    let tombstone = Change {
        deleted: true,
        ..change("r1", 1, b"ignored")
    };
    server.push(me, &[tombstone]).unwrap();
    let pulled = server.pull(me, 1).unwrap();
    assert!(pulled.records[0].deleted);
    assert_eq!(pulled.records[0].ciphertext, b"");
    assert_eq!(server.history(me, "r1").unwrap()[0].ciphertext, b"v1");
}

#[test]
fn accounts_never_see_each_others_records() {
    let server = server(Signups::Open);
    let ana = caller(&server, &register(&server, "ana", "pw").unwrap().token);
    let bob = caller(&server, &register(&server, "bob", "pw").unwrap().token);
    server.push(ana, &[change("r1", 0, b"ana's")]).unwrap();
    assert_eq!(server.pull(bob, 0).unwrap().records, []);
    // Same id in another account is a separate record.
    assert_eq!(
        server
            .push(bob, &[change("r1", 0, b"bob's")])
            .unwrap()
            .accepted,
        [("r1".to_owned(), 1)]
    );
}

#[test]
fn a_revoked_device_is_signed_out_at_once() {
    let server = server(Signups::Open);
    let laptop = register(&server, "ana", "pw").unwrap();
    let phone = login(&server, "ana", "pw").unwrap();
    server
        .revoke(caller(&server, &laptop.token), phone.device)
        .unwrap();
    assert!(matches!(
        server.authenticate(&phone.token),
        Err(ServerError::Unauthorized)
    ));
    assert!(server.authenticate(&laptop.token).is_ok());
}

#[test]
fn the_admin_sees_sizes_deletes_accounts_and_takes_backups() {
    let server = server(Signups::Open);
    let ana = caller(&server, &register(&server, "ana", "pw").unwrap().token);
    register(&server, "bob", "pw").unwrap();
    server.push(ana, &[change("r1", 0, b"12345")]).unwrap();
    let accounts = server.accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        (
            accounts[0].name.as_str(),
            accounts[0].records,
            accounts[0].bytes
        ),
        ("ana", 1, 5)
    );

    let backup =
        std::env::temp_dir().join(format!("annulus-server-backup-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&backup);
    server.backup(&backup).unwrap();
    let restored = Server::open(&backup, Signups::Closed).unwrap();
    assert_eq!(restored.accounts().unwrap().len(), 2);
    // The copy keeps the OPAQUE keys, so existing accounts can still sign in.
    assert!(login(&restored, "ana", "pw").is_ok());

    assert!(server.delete_account("bob").unwrap());
    assert!(!server.delete_account("bob").unwrap());
    assert!(login(&server, "bob", "pw").is_err());
}

#[test]
fn history_keeps_the_last_twenty_versions() {
    let server = server(Signups::Open);
    let me = caller(&server, &register(&server, "ana", "pw").unwrap().token);
    for version in 0..25u64 {
        server
            .push(me, &[change("r1", version, &version.to_le_bytes())])
            .unwrap();
    }
    assert_eq!(server.history(me, "r1").unwrap().len(), 20);
}
