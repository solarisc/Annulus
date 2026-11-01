//! The server logic with a real OPAQUE client: registration, login, the sync rules, and
//! revocation. The HTTP layer is covered from `annulus-sync`, which talks to it.

use std::path::Path;

use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, Identifiers, RegistrationResponse,
};
use opaque_rand::rngs::OsRng;
use annulus_proto::{
    Change, LoginFinish, LoginStart, PasswordBegin, PasswordChange, RegisterFinish, RegisterStart,
    Suite,
};

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
    register_invited(server, account, password, None)
}

fn register_invited(
    server: &Server,
    account: &str,
    password: &str,
    invite: Option<&str>,
) -> Result<annulus_proto::Session, ServerError> {
    let finish = registration(server, account, password, invite)?;
    server.register_finish(&finish, invite)
}

/// Registration step 1, and the client's step 2 request, not yet sent.
fn registration(
    server: &Server,
    account: &str,
    password: &str,
    invite: Option<&str>,
) -> Result<RegisterFinish, ServerError> {
    let ksf = cheap();
    let start = ClientRegistration::<Suite>::start(&mut OsRng, password.as_bytes()).unwrap();
    let response = server.register_start(
        &RegisterStart {
            account: account.into(),
            request: start.message.serialize().to_vec(),
        },
        invite,
    )?;
    let finish = start
        .state
        .finish(
            &mut OsRng,
            password.as_bytes(),
            RegistrationResponse::deserialize(&response.response).unwrap(),
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )
        .unwrap();
    Ok(RegisterFinish {
        account: account.into(),
        upload: finish.message.serialize().to_vec(),
        wrapped_keys: b"wrapped".to_vec(),
        device: "laptop".into(),
    })
}

/// A key file in the vault's format (ADR-0007), as a password change uploads it.
const KEY_FILE: &[u8] = br#"{"salt":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16],
    "kdf":{"memory_kib":65536,"iterations":3,"parallelism":1},
    "by_password":{"nonce":"AAECAwQFBgcICQoLDA0ODxAREhMUFRYX","ciphertext":"c2VhbGVk"},
    "by_recovery":{"nonce":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23],
                   "ciphertext":[1,2,3]}}"#;

/// Changes the password of `token`'s account, proving `current` (a wrong one sends a
/// garbage proof, as an attacker would).
fn change_password(
    server: &Server,
    token: &str,
    current: &str,
    new: &str,
    wrapped_keys: &[u8],
) -> Result<(), ServerError> {
    let ksf = cheap();
    let me = caller(server, token);
    let login = ClientLogin::<Suite>::start(&mut OsRng, current.as_bytes()).unwrap();
    let registration = ClientRegistration::<Suite>::start(&mut OsRng, new.as_bytes()).unwrap();
    let response = server.password_begin(
        me,
        &PasswordBegin {
            login: login.message.serialize().to_vec(),
            registration: registration.message.serialize().to_vec(),
        },
        None,
    )?;
    let finalization = login
        .state
        .finish(
            &mut OsRng,
            current.as_bytes(),
            CredentialResponse::deserialize(&response.credential).unwrap(),
            ClientLoginFinishParameters::new(None, Identifiers::default(), Some(&ksf)),
        )
        .map_or_else(|_| vec![0u8; 64], |done| done.message.serialize().to_vec());
    let upload = registration
        .state
        .finish(
            &mut OsRng,
            new.as_bytes(),
            RegistrationResponse::deserialize(&response.registration).unwrap(),
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )
        .unwrap();
    server.password_change(
        me,
        &PasswordChange {
            login: response.login,
            finalization,
            upload: upload.message.serialize().to_vec(),
            wrapped_keys: wrapped_keys.to_vec(),
        },
    )
}

fn login(
    server: &Server,
    account: &str,
    password: &str,
) -> Result<annulus_proto::Session, ServerError> {
    login_from(server, account, password, None)
}

fn login_from(
    server: &Server,
    account: &str,
    password: &str,
    address: Option<std::net::IpAddr>,
) -> Result<annulus_proto::Session, ServerError> {
    let ksf = cheap();
    let start = ClientLogin::<Suite>::start(&mut OsRng, password.as_bytes()).unwrap();
    let response = server.login_start(
        &LoginStart {
            account: account.into(),
            request: start.message.serialize().to_vec(),
        },
        address,
    )?;
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
    // One address guessing is stopped after 5 tries, without locking the account for
    // everyone else.
    let guesser = Some(std::net::IpAddr::from([203, 0, 113, 7]));
    for _ in 0..5 {
        let _ = login_from(&server, "ana", "battery staple", guesser);
    }
    assert!(matches!(
        login_from(&server, "ana", "correct horse", guesser),
        Err(ServerError::Throttled)
    ));
    let owner = Some(std::net::IpAddr::from([198, 51, 100, 1]));
    assert!(login_from(&server, "ana", "correct horse", owner).is_ok());
    // Many addresses together still pause the account.
    for last in 0..10u8 {
        let _ = login_from(
            &server,
            "ana",
            "battery staple",
            Some(std::net::IpAddr::from([192, 0, 2, last])),
        );
    }
    assert!(matches!(
        login_from(&server, "ana", "correct horse", owner),
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
    // A sealed tombstone: the deletion's ciphertext is kept and handed out like any other.
    let tombstone = Change {
        deleted: true,
        ..change("r1", 1, b"sealed tombstone")
    };
    server.push(me, &[tombstone]).unwrap();
    let pulled = server.pull(me, 1).unwrap();
    assert!(pulled.records[0].deleted);
    assert_eq!(pulled.records[0].ciphertext, b"sealed tombstone");
    assert_eq!(server.history(me, "r1").unwrap()[0].ciphertext, b"v1");
    // A conflict on a deleted record returns the tombstone with its ciphertext.
    let stale = server.push(me, &[change("r1", 1, b"edit")]).unwrap();
    assert_eq!(stale.conflicts[0].ciphertext, b"sealed tombstone");
    assert!(stale.conflicts[0].deleted);

    // An older client's tombstone has no ciphertext, and is still accepted.
    server.push(me, &[change("r2", 0, b"v1")]).unwrap();
    let empty = Change {
        deleted: true,
        ..change("r2", 3, b"")
    };
    assert_eq!(server.push(me, &[empty]).unwrap().accepted.len(), 1);
    let pulled = server.pull(me, 3).unwrap();
    assert!(pulled.records[0].deleted);
    assert_eq!(pulled.records[0].ciphertext, b"");
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
    // Only the server's user may read it, whatever the umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&backup).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A copy is never written over another.
    assert!(server.backup(&backup).is_err());
    let restored = Server::open(&backup, Signups::Closed).unwrap();
    assert_eq!(restored.accounts().unwrap().len(), 2);
    // The copy keeps the OPAQUE keys, so existing accounts can still sign in.
    assert!(login(&restored, "ana", "pw").is_ok());

    assert!(server.delete_account("bob").unwrap());
    assert!(!server.delete_account("bob").unwrap());
    assert!(login(&server, "bob", "pw").is_err());
}

#[test]
fn a_push_of_too_many_changes_is_refused_whole() {
    let server = server(Signups::Open);
    let me = caller(&server, &register(&server, "ana", "pw").unwrap().token);
    let many: Vec<Change> = (0..=annulus_proto::MAX_PUSH_CHANGES)
        .map(|index| change(&index.to_string(), 0, b"x"))
        .collect();
    assert!(matches!(
        server.push(me, &many),
        Err(ServerError::BadRequest(_))
    ));
    // Nothing of it was applied; a push within the cap goes through.
    assert_eq!(server.pull(me, 0).unwrap().records.len(), 0);
    let within = &many[..annulus_proto::MAX_PUSH_CHANGES];
    assert_eq!(
        server.push(me, within).unwrap().accepted.len(),
        annulus_proto::MAX_PUSH_CHANGES
    );
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

#[test]
fn a_password_change_needs_the_current_password_not_just_a_device_token() {
    let server = server(Signups::Open);
    let laptop = register(&server, "ana", "old pw").unwrap();
    let phone = login(&server, "ana", "old pw").unwrap();
    // Someone holding only the phone's token, guessing the password: refused, nothing
    // changes, and the guess counts like a failed login.
    assert!(change_password(&server, &phone.token, "a guess", "theirs", KEY_FILE).is_err());
    assert!(login(&server, "ana", "theirs").is_err());
    assert!(server.authenticate(&laptop.token).is_ok());

    // A key file that is not one is refused, even with the right password.
    let mut too_big = KEY_FILE.to_vec();
    too_big.resize(5000, b' ');
    let missing_salt =
        String::from_utf8_lossy(KEY_FILE).replace("[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]", "[]");
    for garbage in [
        &b"new keys"[..],
        &b"{}"[..],
        &too_big[..],
        missing_salt.as_bytes(),
    ] {
        assert!(matches!(
            change_password(&server, &laptop.token, "old pw", "new pw", garbage),
            Err(ServerError::BadRequest(_))
        ));
    }
    assert!(login(&server, "ana", "new pw").is_err());

    // The right password and a key file: the change goes through, other devices are
    // signed out, and the new password signs in and returns the new key file.
    change_password(&server, &laptop.token, "old pw", "new pw", KEY_FILE).unwrap();
    assert!(matches!(
        server.authenticate(&phone.token),
        Err(ServerError::Unauthorized)
    ));
    assert!(server.authenticate(&laptop.token).is_ok());
    assert!(login(&server, "ana", "old pw").is_err());
    assert_eq!(
        login(&server, "ana", "new pw").unwrap().wrapped_keys,
        KEY_FILE
    );
}

#[test]
fn a_login_proves_only_what_and_whom_it_was_started_for() {
    let server = server(Signups::Open);
    let ana = register(&server, "ana", "ana's").unwrap();
    let bob = register(&server, "bob", "bob's").unwrap();
    let ksf = cheap();
    // Bob starts a password change and proves his own password...
    let start = ClientLogin::<Suite>::start(&mut OsRng, b"bob's").unwrap();
    let registration = ClientRegistration::<Suite>::start(&mut OsRng, b"new").unwrap();
    let response = server
        .password_begin(
            caller(&server, &bob.token),
            &PasswordBegin {
                login: start.message.serialize().to_vec(),
                registration: registration.message.serialize().to_vec(),
            },
            None,
        )
        .unwrap();
    let proof = start
        .state
        .finish(
            &mut OsRng,
            b"bob's",
            CredentialResponse::deserialize(&response.credential).unwrap(),
            ClientLoginFinishParameters::new(None, Identifiers::default(), Some(&ksf)),
        )
        .unwrap()
        .message
        .serialize()
        .to_vec();
    let upload = registration
        .state
        .finish(
            &mut OsRng,
            b"new",
            RegistrationResponse::deserialize(&response.registration).unwrap(),
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )
        .unwrap()
        .message
        .serialize()
        .to_vec();
    // ...which neither changes Ana's password with her token nor signs a device in.
    let change = PasswordChange {
        login: response.login,
        finalization: proof.clone(),
        upload,
        wrapped_keys: KEY_FILE.to_vec(),
    };
    assert!(
        server
            .password_change(caller(&server, &ana.token), &change)
            .is_err()
    );
    assert!(
        server
            .login_finish(&LoginFinish {
                login: response.login,
                finalization: proof,
                device: "intruder".into(),
            })
            .is_err()
    );
    assert!(login(&server, "ana", "ana's").is_ok());
    assert!(login(&server, "bob", "bob's").is_ok());
}

#[test]
fn a_device_token_unused_for_ninety_days_stops_working() {
    const DAY: i64 = 24 * 60 * 60;
    let server = server(Signups::Open);
    let laptop = register(&server, "ana", "pw").unwrap();
    // Each use counts as seen, so a device in use never expires.
    server.backdate_devices(89 * DAY);
    assert!(server.authenticate(&laptop.token).is_ok());
    server.backdate_devices(89 * DAY);
    assert!(server.authenticate(&laptop.token).is_ok());
    server.backdate_devices(91 * DAY);
    assert!(matches!(
        server.authenticate(&laptop.token),
        Err(ServerError::Unauthorized)
    ));
    // It is signed out for good, and the device is gone from the list.
    server.backdate_devices(-91 * DAY);
    assert!(server.authenticate(&laptop.token).is_err());
    let phone = login(&server, "ana", "pw").unwrap();
    let devices = server.devices(caller(&server, &phone.token)).unwrap();
    assert_eq!(devices.devices.len(), 1);
}

#[test]
fn account_names_are_checked_before_anything_is_kept() {
    let server = server(Signups::Open);
    let long = "a".repeat(255);
    let start = ClientLogin::<Suite>::start(&mut OsRng, b"pw").unwrap();
    assert!(matches!(
        server.login_start(
            &LoginStart {
                account: long.clone(),
                request: start.message.serialize().to_vec(),
            },
            None,
        ),
        Err(ServerError::BadRequest(_))
    ));
    // A registration started with a good name cannot be finished under a long one.
    let ksf = cheap();
    let start = ClientRegistration::<Suite>::start(&mut OsRng, b"pw").unwrap();
    let response = server
        .register_start(
            &RegisterStart {
                account: "ana".into(),
                request: start.message.serialize().to_vec(),
            },
            None,
        )
        .unwrap();
    let finish = start
        .state
        .finish(
            &mut OsRng,
            b"pw",
            RegistrationResponse::deserialize(&response.response).unwrap(),
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )
        .unwrap();
    for name in [long.as_str(), "  "] {
        assert!(matches!(
            server.register_finish(
                &RegisterFinish {
                    account: name.into(),
                    upload: finish.message.serialize().to_vec(),
                    wrapped_keys: b"wrapped".to_vec(),
                    device: "laptop".into(),
                },
                None,
            ),
            Err(ServerError::BadRequest(_))
        ));
    }
    assert_eq!(server.accounts().unwrap(), []);
}

#[test]
fn guessing_without_sending_a_proof_still_counts() {
    let server = server(Signups::Open);
    register(&server, "ana", "pw").unwrap();
    let guesser = Some(std::net::IpAddr::from([203, 0, 113, 7]));
    // An OPAQUE client sees for itself that a guess was wrong; it need not finish.
    for _ in 0..5 {
        let start = ClientLogin::<Suite>::start(&mut OsRng, b"guess").unwrap();
        server
            .login_start(
                &LoginStart {
                    account: "ana".into(),
                    request: start.message.serialize().to_vec(),
                },
                guesser,
            )
            .unwrap();
    }
    assert!(matches!(
        login_from(&server, "ana", "pw", guesser),
        Err(ServerError::Throttled)
    ));
    // Logins that succeed are taken back: the owner signs in as often as needed.
    let owner = Some(std::net::IpAddr::from([198, 51, 100, 1]));
    for _ in 0..12 {
        login_from(&server, "ana", "pw", owner).unwrap();
    }
}

#[test]
fn a_full_failure_table_makes_room_for_a_new_guesser() {
    let server = server(Signups::Open).with_tracking_limit(4);
    register(&server, "ana", "pw").unwrap();
    // Two strangers fill the table with their names and addresses.
    for last in 1..=2u8 {
        let _ = login_from(
            &server,
            &format!("x{last}"),
            "pw",
            Some(std::net::IpAddr::from([192, 0, 2, last])),
        );
    }
    // A new guesser is still tracked, and paused after 5 tries.
    let guesser = Some(std::net::IpAddr::from([203, 0, 113, 7]));
    for _ in 0..5 {
        let _ = login_from(&server, "ana", "wrong", guesser);
    }
    assert!(matches!(
        login_from(&server, "ana", "pw", guesser),
        Err(ServerError::Throttled)
    ));
}

#[test]
fn a_full_login_table_drops_the_oldest_login() {
    let server = server(Signups::Open).with_tracking_limit(2);
    register(&server, "ana", "pw").unwrap();
    let ksf = cheap();
    let mut started = Vec::new();
    for _ in 0..3 {
        let start = ClientLogin::<Suite>::start(&mut OsRng, b"pw").unwrap();
        let response = server
            .login_start(
                &LoginStart {
                    account: "ana".into(),
                    request: start.message.serialize().to_vec(),
                },
                None,
            )
            .unwrap();
        started.push((start, response));
    }
    let results: Vec<bool> = started
        .into_iter()
        .map(|(start, response)| {
            let finish = start
                .state
                .finish(
                    &mut OsRng,
                    b"pw",
                    CredentialResponse::deserialize(&response.response).unwrap(),
                    ClientLoginFinishParameters::new(None, Identifiers::default(), Some(&ksf)),
                )
                .unwrap();
            server
                .login_finish(&LoginFinish {
                    login: response.login,
                    finalization: finish.message.serialize().to_vec(),
                    device: "phone".into(),
                })
                .is_ok()
        })
        .collect();
    assert_eq!(results, [false, true, true]);
}

#[test]
fn closed_signups_take_a_single_use_unexpired_invite() {
    let server = server(Signups::Closed);
    let invite = server
        .create_invite(std::time::Duration::from_hours(24))
        .unwrap();
    assert!(matches!(
        register(&server, "ana", "pw"),
        Err(ServerError::Refused(_))
    ));
    assert!(matches!(
        register_invited(&server, "ana", "pw", Some("0000-1111")),
        Err(ServerError::Refused(_))
    ));
    // Typed in capitals and without its dashes, it still works; once.
    let typed = invite.replace('-', "").to_uppercase();
    register_invited(&server, "ana", "pw", Some(&typed)).unwrap();
    assert!(matches!(
        register_invited(&server, "bob", "pw", Some(&invite)),
        Err(ServerError::Refused(_))
    ));
    // Two people start registering the same name; the one who loses it keeps the invite.
    let first = server
        .create_invite(std::time::Duration::from_hours(1))
        .unwrap();
    let second = server
        .create_invite(std::time::Duration::from_hours(1))
        .unwrap();
    let winner = registration(&server, "bob", "pw", Some(&first)).unwrap();
    let loser = registration(&server, "bob", "other", Some(&second)).unwrap();
    server.register_finish(&winner, Some(&first)).unwrap();
    assert!(matches!(
        server.register_finish(&loser, Some(&second)),
        Err(ServerError::Refused("that account name is not available"))
    ));
    register_invited(&server, "cleo", "pw", Some(&second)).unwrap();
    // An expired invite is refused.
    let expired = server.create_invite(std::time::Duration::ZERO).unwrap();
    assert!(matches!(
        register_invited(&server, "dora", "pw", Some(&expired)),
        Err(ServerError::Refused(_))
    ));
    assert_eq!(server.accounts().unwrap().len(), 3);
}
