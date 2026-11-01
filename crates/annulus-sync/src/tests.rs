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
        invite: Option<&str>,
    ) -> Result<RegisterStartResponse, SyncError> {
        self.server
            .register_start(request, invite)
            .map_err(|error| transport(&error))
    }

    fn register_finish(
        &mut self,
        request: &RegisterFinish,
        invite: Option<&str>,
    ) -> Result<Session, SyncError> {
        self.server
            .register_finish(request, invite)
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
    /// When each deletion held was made.
    deleted_at: HashMap<String, u64>,
    /// The clock time of the last version taken from or pushed to the server, per record.
    seen: HashMap<String, u64>,
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
        let item = match self.pending.remove(id) {
            Some(change) => {
                if change.item.is_none() {
                    self.deleted_at.insert(id.to_owned(), change.hlc);
                }
                self.seen.insert(id.to_owned(), change.hlc);
                change.item
            }
            None => self.records.get(id).and_then(|(_, item)| item.clone()),
        };
        self.records.insert(id.to_owned(), (seq, item));
    }

    fn apply(&mut self, id: &str, seq: u64, hlc: u64, item: Option<Item>) {
        self.pending.remove(id);
        self.seen.insert(id.to_owned(), hlc);
        if item.is_none() {
            self.deleted_at.insert(id.to_owned(), hlc);
        }
        self.records.insert(id.to_owned(), (seq, item));
    }

    fn rebase(&mut self, id: &str, seq: u64) {
        if let Some(change) = self.pending.get_mut(id) {
            change.base = seq;
        }
    }

    fn next_time(&mut self, after: u64) -> u64 {
        (after / 1000 + 1) * 1000 + self.node
    }

    fn renew(&mut self, id: &str, after: u64) {
        let hlc = self.next_time(after);
        if let Some(change) = self.pending.get_mut(id) {
            change.hlc = hlc;
            if let Some(item) = &mut change.item {
                item.hlc = hlc;
            }
        }
    }

    fn seen_time(&self, id: &str) -> Option<u64> {
        self.seen.get(id).copied()
    }

    fn held_time(&self, id: &str) -> Option<u64> {
        let (_, item) = self.records.get(id)?;
        item.as_ref()
            .map(|item| item.hlc)
            .or_else(|| self.deleted_at.get(id).copied())
    }

    fn reassert(&mut self, id: &str, seq: u64) {
        let Some((_, item)) = self.records.get(id) else {
            return;
        };
        let hlc = self.held_time(id).unwrap_or(0);
        let item = item.clone();
        self.pending.insert(
            id.to_owned(),
            Outgoing {
                id: id.to_owned(),
                base: seq,
                item,
                hlc,
            },
        );
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
fn a_deletion_the_server_made_up_is_passed_over() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "prod-db", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    // Marked deleted with nothing sealed, as a hostile server (or a stolen token) can.
    let seq = a.version("r1");
    let forged = Change {
        id: "r1".into(),
        base: seq,
        ciphertext: Vec::new(),
        deleted: true,
    };
    assert_eq!(server.push(a_id, &[forged]).unwrap().accepted.len(), 1);
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1").as_deref(), Some("prod-db"), "still there");
}

#[test]
fn an_old_version_served_again_is_not_applied_and_the_newer_goes_back() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "old address", 10);
    run(&server, a_id, &mut a);
    let old = seal(&Item {
        kind: 1,
        hlc: 10,
        body: b"old address".to_vec(),
    });
    a.edit("r1", "new address", 20);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1").as_deref(), Some("new address"));
    // The server serves the old ciphertext again as the newest version.
    let replay = Change {
        id: "r1".into(),
        base: a.version("r1"),
        ciphertext: old,
        deleted: false,
    };
    server.push(a_id, &[replay]).unwrap();
    run(&server, b_id, &mut b);
    assert_eq!(
        b.body("r1").as_deref(),
        Some("new address"),
        "not rolled back"
    );
    run(&server, a_id, &mut a);
    assert_eq!(
        a.body("r1").as_deref(),
        Some("new address"),
        "the newer copy went back over the old one"
    );
}

#[test]
fn a_pending_deletion_is_not_undone_by_an_old_version_served_again() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    let last = seal(&Item {
        kind: 1,
        hlc: 10,
        body: b"v1".to_vec(),
    });
    run(&server, b_id, &mut b);
    // b deletes offline; meanwhile the server serves r1's same old version again.
    b.delete("r1", 20);
    let replay = Change {
        id: "r1".into(),
        base: a.version("r1"),
        ciphertext: last,
        deleted: false,
    };
    server.push(a_id, &[replay]).unwrap();
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1"), None, "the deletion stands");
    run(&server, a_id, &mut a);
    assert_eq!(a.body("r1"), None, "and reaches the other device");
}

#[test]
fn a_record_behind_a_made_up_deletion_is_not_stuck() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    let forged = Change {
        id: "r1".into(),
        base: a.version("r1"),
        ciphertext: Vec::new(),
        deleted: true,
    };
    server.push(a_id, &[forged]).unwrap();
    run(&server, b_id, &mut b);
    // b's copy went back over the made-up deletion, so an edit of it still pushes.
    b.edit("r1", "v2", 30);
    let report = run(&server, b_id, &mut b);
    assert_eq!(b.pending.len(), 0, "{report:?}");
    run(&server, a_id, &mut a);
    assert_eq!(a.body("r1").as_deref(), Some("v2"));
}

#[test]
fn a_deleted_record_is_not_brought_back_by_its_last_version() {
    let server = server();
    let (a_id, b_id) = account(&server);
    let (mut a, mut b) = (Device::default(), Device::default());
    a.edit("r1", "v1", 10);
    run(&server, a_id, &mut a);
    let last = seal(&Item {
        kind: 1,
        hlc: 10,
        body: b"v1".to_vec(),
    });
    run(&server, b_id, &mut b);
    a.delete("r1", 20);
    run(&server, a_id, &mut a);
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1"), None);
    let replay = Change {
        id: "r1".into(),
        base: a.version("r1"),
        ciphertext: last,
        deleted: false,
    };
    server.push(a_id, &[replay]).unwrap();
    run(&server, b_id, &mut b);
    assert_eq!(b.body("r1"), None, "still deleted");
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

/// A key file in the vault's shape, which the server checks a password change uploads.
const KEY_FILE: &[u8] = br#"{"salt":[1,2,3,4,5,6,7,8],"kdf":{"memory_kib":8,"iterations":1,"parallelism":1},"by_password":{"nonce":"AAAA","ciphertext":"AAAA"},"by_recovery":{"nonce":"AAAA","ciphertext":"AAAA"}}"#;

#[test]
fn closed_sign_ups_take_an_invite_code_over_http() {
    let server = std::sync::Arc::new(Server::open(Path::new(":memory:"), Signups::Closed).unwrap());
    let invite = server
        .create_invite(std::time::Duration::from_hours(1))
        .unwrap();
    let base = http_server_with(server);
    let mut http = crate::http::HttpServer::new(&base, None).unwrap();
    let create = |http: &mut crate::http::HttpServer, name: &str, invite: Option<&str>| {
        account::register_with_invite(
            http,
            &cheap(),
            name,
            "pw",
            KEY_FILE.to_vec(),
            "laptop",
            invite,
        )
    };
    assert!(create(&mut http, "ana", None).is_err());
    assert!(account::register(&mut http, &cheap(), "ana", "pw", KEY_FILE.to_vec(), "x").is_err());
    let session = create(&mut http, "ana", Some(&format!(" {invite} "))).unwrap();
    assert_eq!(session.wrapped_keys, KEY_FILE);
    assert!(
        create(&mut http, "bob", Some(&invite)).is_err(),
        "single use"
    );
    assert!(account::login(&mut http, &cheap(), "ana", "pw", "phone").is_ok());
}

/// Starts the real HTTP server on a free loopback port, on its own thread.
fn http_server() -> String {
    http_server_with(std::sync::Arc::new(server()))
}

/// Serves `server` over HTTP on a free loopback port, on its own thread.
fn http_server_with(server: std::sync::Arc<Server>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, annulus_server::service(server, false))
                .await
                .unwrap();
        });
    });
    format!("http://{address}")
}

/// Serves a fresh server over TLS with a new self-signed certificate for 127.0.0.1, on a
/// free loopback port; returns its address and the certificate (DER).
fn https_server() -> (String, Vec<u8>) {
    let made = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let der = made.cert.der().to_vec();
    let (cert, key) = (made.cert.pem(), made.signing_key.serialize_pem());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            listener.set_nonblocking(true).unwrap();
            let config = axum_server::tls_rustls::RustlsConfig::from_pem(
                cert.into_bytes(),
                key.into_bytes(),
            )
            .await
            .unwrap();
            let service = annulus_server::service(std::sync::Arc::new(server()), false);
            axum_server::from_tcp_rustls(listener, config)
                .unwrap()
                .serve(service)
                .await
                .unwrap();
        });
    });
    (format!("https://{address}"), der)
}

#[test]
fn a_servers_own_certificate_is_trusted_only_once_pinned_and_only_that_one() {
    let (base, certificate) = https_server();
    let register = |http: &mut crate::http::HttpServer, name: &str| {
        account::register(http, &cheap(), name, "pw", KEY_FILE.to_vec(), "laptop")
    };
    // Not from a public authority: refused, and said so, so the app can ask.
    let mut public = crate::http::HttpServer::new(&base, None).unwrap();
    assert!(
        matches!(register(&mut public, "ana"), Err(SyncError::Untrusted(_))),
        "a self-signed certificate is not trusted by default"
    );
    // What the app shows for the user to compare is that certificate's.
    let shown = crate::pin::certificate_of(&base).unwrap();
    assert_eq!(shown, certificate);
    assert_eq!(
        crate::pin::fingerprint(&shown),
        annulus_proto::fingerprint(&certificate)
    );
    // Once pinned, it works.
    let mut pinned = crate::http::HttpServer::new(&base, None)
        .unwrap()
        .pinned(Some(&shown));
    let made = register(&mut pinned, "ana");
    assert!(made.is_ok(), "{:?}", made.err());
    // Any other certificate, even another valid one for the same address, is refused.
    let other = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let mut wrong = crate::http::HttpServer::new(&base, None)
        .unwrap()
        .pinned(Some(other.cert.der()));
    assert!(matches!(
        register(&mut wrong, "bob"),
        Err(SyncError::Untrusted(_))
    ));
}

#[test]
fn the_health_check_answers_anyone_without_a_version_or_a_token() {
    let base = http_server();
    // As a container's health check or an uptime monitor asks: a bare GET.
    let mut response = ureq::get(&format!("{base}{}", annulus_proto::path::HEALTH))
        .call()
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.body_mut().read_to_string().unwrap(), "ok");
}

#[test]
fn only_a_signed_in_push_may_be_large_and_strangers_are_refused_first() {
    let base = http_server();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let status = |route: &str, bytes: usize| {
        agent
            .post(&format!("{base}{route}"))
            .header(
                annulus_proto::VERSION_HEADER,
                &annulus_proto::PROTOCOL_VERSION.to_string(),
            )
            .send(&vec![0u8; bytes][..])
            .map_or(0, |response| response.status().as_u16())
    };
    let big = annulus_proto::MAX_SMALL_BODY + 1;
    for route in [
        annulus_proto::path::REGISTER_START,
        annulus_proto::path::LOGIN_START,
        annulus_proto::path::LOGIN_FINISH,
    ] {
        assert_eq!(status(route, big), 413, "{route}");
        assert_ne!(status(route, 100), 413, "{route}: a normal size is read");
    }
    // Without a token a push is refused before its body is read, whatever its size: 401,
    // or (0) the connection closed on the rest of the upload, which is never read.
    assert_eq!(status(annulus_proto::path::RECORDS, 10), 401);
    for bytes in [big, annulus_proto::MAX_BODY + 1] {
        let refused = status(annulus_proto::path::RECORDS, bytes);
        assert!(matches!(refused, 401 | 0), "{bytes} bytes: {refused}");
    }
    // A signed-in device may push large bodies, up to the limit.
    let mut http = crate::http::HttpServer::new(&base, None).unwrap();
    let session = account::register(
        &mut http,
        &cheap(),
        "limits",
        "pw",
        b"keys".to_vec(),
        "laptop",
    )
    .unwrap();
    let signed_in = |bytes: usize| {
        agent
            .post(&format!("{base}{}", annulus_proto::path::RECORDS))
            .header(
                annulus_proto::VERSION_HEADER,
                &annulus_proto::PROTOCOL_VERSION.to_string(),
            )
            .header("Authorization", &format!("Bearer {}", session.token))
            .send(&vec![0u8; bytes][..])
            .map_or(0, |response| response.status().as_u16())
    };
    assert_eq!(signed_in(annulus_proto::MAX_BODY + 1), 413);
    assert_eq!(signed_in(big), 200, "read (zeros are an empty push)");
}

#[test]
fn a_first_sync_with_several_large_records_goes_through_over_http() {
    let base = http_server();
    let mut laptop = crate::http::HttpServer::new(&base, None).unwrap();
    let session = account::register(
        &mut laptop,
        &cheap(),
        "big",
        "pw",
        b"keys".to_vec(),
        "laptop",
    )
    .unwrap();
    laptop.set_token(Some(session.token));
    // Session logs may be up to 1 MiB each; a new device pushes them all at once.
    let mut device = Device::default();
    let log = "x".repeat(1024 * 1024);
    for (index, id) in ["l1", "l2", "l3", "l4", "l5"].into_iter().enumerate() {
        device.edit(id, &log, 10 + index as u64);
    }
    // A sealed 1 MiB log is several MiB on the wire; one record that size goes alone.
    device.edit("huge", &"y".repeat(3 * 1024 * 1024), 20);
    assert_eq!(sync(&mut laptop, &mut device).unwrap().pushed, 6);
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
    let laptop_token = session.token.clone();
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
    // Live sync: a wait returns as soon as another device pushes.
    let cursor = phone_copy.cursor;
    let waiting = std::thread::spawn({
        let base = base.clone();
        let token = laptop_token.clone();
        move || {
            let http = crate::http::HttpServer::new(&base, Some(token)).unwrap();
            let started = std::time::Instant::now();
            (http.wait(cursor).unwrap(), started.elapsed())
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    phone_copy.edit("r9", "from the phone", 50);
    sync(&mut phone, &mut phone_copy).unwrap();
    let (newest, elapsed) = waiting.join().unwrap();
    assert!(newest > cursor);
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "woke on the push, not the timeout"
    );
    device.edit("r1", "edited", 20);
    sync(&mut laptop, &mut device).unwrap();
    assert_eq!(
        laptop.history("r1").unwrap().len(),
        1,
        "the first version is in history"
    );

    // A password change proves the current password: the phone's token alone cannot.
    assert!(
        account::change_password(&mut phone, &cheap(), "a guess", "theirs", KEY_FILE.to_vec())
            .is_err()
    );
    // It re-registers the account and signs the other devices out; the new password then
    // signs in and the old one does not.
    account::change_password(&mut laptop, &cheap(), "pw", "new pw", KEY_FILE.to_vec()).unwrap();
    assert!(matches!(
        sync(&mut phone, &mut phone_copy),
        Err(SyncError::Unauthorized)
    ));
    let mut tablet = crate::http::HttpServer::new(&base, None).unwrap();
    assert!(account::login(&mut tablet, &cheap(), "ana", "pw", "tablet").is_err());
    let tablet_session = account::login(&mut tablet, &cheap(), "ana", "new pw", "tablet").unwrap();
    assert_eq!(tablet_session.wrapped_keys, KEY_FILE);
    sync(&mut laptop, &mut device).unwrap();
    laptop.revoke(tablet_session.device).unwrap();
    tablet.set_token(Some(tablet_session.token));
    assert!(matches!(tablet.devices(), Err(SyncError::Unauthorized)));
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
            pin: None,
        }
    }

    /// Adds host "web" (sorted after "bastion"), reached through host `jump`.
    fn add_web_behind(vault: &mut Vault, jump: u64) {
        let mut web = vault.draft_host();
        web.label = "web".into();
        web.address = "web.internal".into();
        web.jump_id = Some(jump);
        vault.upsert_host(web).unwrap();
    }

    #[test]
    fn a_real_vault_key_file_passes_the_servers_check_on_a_password_change() {
        let (laptop, _) = Vault::create(temp("rewrap"), "pw", CHEAP_KDF).unwrap();
        let base = http_server();
        let mut http = crate::http::HttpServer::new(&base, None).unwrap();
        let session = account::register(
            &mut http,
            &cheap(),
            "ana",
            "pw",
            laptop.key_file_bytes(),
            "laptop",
        )
        .unwrap();
        http.set_token(Some(session.token));
        let rewrapped = laptop.password_kit().rewrap("pw", "new pw").unwrap();
        account::change_password(&mut http, &cheap(), "pw", "new pw", rewrapped.clone()).unwrap();
        let mut phone = crate::http::HttpServer::new(&base, None).unwrap();
        let session = account::login(&mut phone, &cheap(), "ana", "new pw", "phone").unwrap();
        assert_eq!(session.wrapped_keys, rewrapped);
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
        add_web_behind(&mut laptop, host_id);
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
            desktop.hosts()[1].jump_id,
            Some(desktop.hosts()[0].id),
            "jump link"
        );
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
    fn groups_and_what_hosts_inherit_reach_another_device() {
        let server = server();
        let (a_token, b_token) = tokens(&server);
        let (a_id, b_id) = (
            server.authenticate(&a_token).unwrap(),
            server.authenticate(&b_token).unwrap(),
        );
        let (mut laptop, _) = Vault::create(temp("groups-a"), "pw", CHEAP_KDF).unwrap();
        let key = annulus_vault::keychain::SshKey {
            id: laptop.next_key_id(),
            label: "prod key".into(),
            private: Secret::new("private"),
            public: "ssh-ed25519 AAAA".into(),
            fingerprint: "SHA256:k".into(),
        };
        laptop.upsert_key(key.clone()).unwrap();
        let mut bastion = laptop.draft_host();
        (bastion.label, bastion.address) = ("bastion".into(), "10.0.0.1".into());
        laptop.upsert_host(bastion.clone()).unwrap();
        let mut prod = laptop.draft_group(None).unwrap();
        prod.name = "prod".into();
        prod.defaults.jump_id = Some(bastion.id);
        prod.defaults.username = "admin".into();
        prod.defaults.port = Some(2222);
        prod.defaults.key_id = Some(key.id);
        laptop.upsert_group(prod.clone()).unwrap();
        let mut db = laptop.draft_group(Some(prod.uid.clone())).unwrap();
        db.name = "db".into();
        db.defaults.username = "postgres".into();
        laptop.upsert_group(db.clone()).unwrap();
        let mut host = laptop.draft_host();
        host.label = "pg1".into();
        host.address = "10.0.0.5".into();
        host.group = Some(db.uid.clone());
        laptop.upsert_host(host).unwrap();
        round(&server, a_id, &mut laptop);

        let mut desktop =
            Vault::join(temp("groups-b"), &laptop.key_file_bytes(), "pw", account(2)).unwrap();
        round(&server, b_id, &mut desktop);
        let pg1 = desktop.hosts().iter().find(|h| h.label == "pg1").unwrap();
        let bastion_here = desktop
            .hosts()
            .iter()
            .find(|h| h.label == "bastion")
            .unwrap();
        let (resolved, sources) = desktop.resolve(pg1);
        assert_eq!(
            resolved.jump_id,
            Some(bastion_here.id),
            "jump host by this device's id"
        );
        assert_eq!(resolved.username, "postgres");
        assert_eq!(resolved.port, Some(2222));
        assert_eq!(
            resolved.key_id,
            Some(desktop.ssh_keys()[0].id),
            "the group's key, by this device's id"
        );
        assert_eq!(sources.port.as_deref(), Some("prod"));

        // Deleting "prod" on the desktop moves "db" to the top level everywhere; the host
        // stays in "db" and no longer gets the port.
        let prod_id = desktop
            .groups()
            .iter()
            .find(|g| g.name == "prod")
            .unwrap()
            .id;
        desktop.remove_group(prod_id).unwrap();
        round(&server, b_id, &mut desktop);
        round(&server, a_id, &mut laptop);
        assert_eq!(laptop.groups().len(), 1);
        assert_eq!(laptop.groups()[0].parent, None);
        let pg1 = laptop.hosts().iter().find(|h| h.label == "pg1").unwrap();
        let (resolved, _) = laptop.resolve(pg1);
        assert_eq!(
            (resolved.username.as_str(), resolved.port),
            ("postgres", None)
        );
        assert_eq!(
            annulus_vault::sync::SyncCopy::describe(
                annulus_vault::sync::kind::GROUP,
                br#"{"id":1,"uid":"x","name":"prod"}"#
            ),
            "Group prod"
        );
    }

    #[test]
    fn a_link_to_a_host_deleted_elsewhere_never_lands_on_a_new_host() {
        let server = server();
        let (a_token, b_token) = tokens(&server);
        let (a_id, b_id) = (
            server.authenticate(&a_token).unwrap(),
            server.authenticate(&b_token).unwrap(),
        );
        let (mut laptop, _) = Vault::create(temp("stale-a"), "pw", CHEAP_KDF).unwrap();
        let mut bastion = laptop.draft_host();
        (bastion.label, bastion.address) = ("bastion".into(), "10.0.0.1".into());
        laptop.upsert_host(bastion).unwrap();
        round(&server, a_id, &mut laptop);
        let mut desktop =
            Vault::join(temp("stale-b"), &laptop.key_file_bytes(), "pw", account(2)).unwrap();
        round(&server, b_id, &mut desktop);

        // The desktop adds a rule on bastion but has not synced it when the laptop deletes
        // bastion.
        let old_id = desktop.hosts()[0].id;
        let mut rule = desktop.draft_forward(old_id);
        rule.label = "db".into();
        desktop.upsert_forward(rule).unwrap();
        let id = laptop.hosts()[0].id;
        laptop.remove_host(id).unwrap();
        round(&server, a_id, &mut laptop);
        round(&server, b_id, &mut desktop);
        assert_eq!(desktop.hosts(), []);

        let mut fresh = desktop.draft_host();
        (fresh.label, fresh.address) = ("new".into(), "10.0.0.2".into());
        let fresh_id = fresh.id;
        desktop.upsert_host(fresh).unwrap();
        assert_ne!(
            fresh_id, old_id,
            "a deleted host's id is never handed out again"
        );
        assert_ne!(desktop.forwards()[0].host_id, fresh_id);
    }

    #[test]
    fn a_draft_left_open_during_a_sync_never_overwrites_the_arrival() {
        let server = server();
        let (a_token, b_token) = tokens(&server);
        let (a_id, b_id) = (
            server.authenticate(&a_token).unwrap(),
            server.authenticate(&b_token).unwrap(),
        );
        let (mut laptop, _) = Vault::create(temp("draft-a"), "pw", CHEAP_KDF).unwrap();
        round(&server, a_id, &mut laptop);
        let mut desktop =
            Vault::join(temp("draft-b"), &laptop.key_file_bytes(), "pw", account(2)).unwrap();
        round(&server, b_id, &mut desktop);

        // The laptop's "New host" editor is open (its draft not saved) while the desktop
        // adds a host and the laptop syncs.
        let mut draft = laptop.draft_host();
        (draft.label, draft.address) = ("mine".into(), "10.0.0.1".into());
        let mut theirs = desktop.draft_host();
        (theirs.label, theirs.address) = ("theirs".into(), "10.0.0.2".into());
        desktop.upsert_host(theirs).unwrap();
        round(&server, b_id, &mut desktop);
        round(&server, a_id, &mut laptop);
        laptop.upsert_host(draft).unwrap();

        let labels: Vec<&str> = laptop.hosts().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["mine", "theirs"], "both kept");
        assert_ne!(laptop.hosts()[0].id, laptop.hosts()[1].id);
        round(&server, a_id, &mut laptop);
        round(&server, b_id, &mut desktop);
        let there: Vec<&str> = desktop.hosts().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(
            there,
            ["mine", "theirs"],
            "nothing overwritten on the other device"
        );
    }

    #[test]
    fn a_deleted_host_comes_back_from_the_trash_on_every_device() {
        let server = server();
        let (a_token, b_token) = tokens(&server);
        let (a_id, b_id) = (
            server.authenticate(&a_token).unwrap(),
            server.authenticate(&b_token).unwrap(),
        );
        let (mut laptop, _) = Vault::create(temp("trash-a"), "pw", CHEAP_KDF).unwrap();
        let mut host = laptop.draft_host();
        host.label = "old-db".into();
        host.address = "10.0.0.9".into();
        laptop.upsert_host(host).unwrap();
        round(&server, a_id, &mut laptop);
        let mut desktop =
            Vault::join(temp("trash-b"), &laptop.key_file_bytes(), "pw", account(2)).unwrap();
        round(&server, b_id, &mut desktop);
        let id = laptop.hosts()[0].id;
        laptop.remove_host(id).unwrap();
        round(&server, a_id, &mut laptop);
        round(&server, b_id, &mut desktop);
        assert_eq!(desktop.hosts(), []);

        // The trash lists it by name, from its last version in the server's history.
        let mut replica = VaultReplica(laptop.sync_copy());
        let (sync_id, kind) = replica.0.deleted()[0].clone();
        let versions = server.history(a_id, &sync_id).unwrap();
        let (_, _, body) = replica.0.open(&sync_id, &versions[0].ciphertext).unwrap();
        assert_eq!(
            annulus_vault::sync::SyncCopy::describe(kind, &body),
            "Host old-db"
        );
        let mut direct = Direct {
            server: &server,
            caller: Some(a_id),
        };
        crate::engine::restore(&mut direct, &mut replica, &sync_id, &versions[0]).unwrap();
        assert!(laptop.commit_sync(replica.0).unwrap());
        assert_eq!(laptop.hosts()[0].label, "old-db");
        round(&server, b_id, &mut desktop);
        assert_eq!(desktop.hosts()[0].address, "10.0.0.9");
    }

    #[test]
    fn signing_in_again_on_the_same_vault_relinks_without_duplicates() {
        let server = server();
        let (a_token, _) = tokens(&server);
        let a_id = server.authenticate(&a_token).unwrap();
        let (mut laptop, _) = Vault::create(temp("relink"), "pw", CHEAP_KDF).unwrap();
        let mut host = laptop.draft_host();
        host.label = "db".into();
        host.address = "db.internal".into();
        laptop.upsert_host(host).unwrap();
        laptop.set_sync_account(Some(account(1))).unwrap();
        round(&server, a_id, &mut laptop);

        laptop.set_sync_account(None).unwrap();
        // The account's key file is this vault's own (it registered with it).
        let key_file = laptop.key_file_bytes();
        assert!(laptop.sync_copy().same_account(&key_file, "pw"));
        laptop.relink(&key_file, account(1)).unwrap();
        round(&server, a_id, &mut laptop);
        assert_eq!(
            server.pull(a_id, 0).unwrap().records.len(),
            1,
            "no duplicate on the server"
        );
        assert_eq!(laptop.hosts().len(), 1);

        // Another vault is not the same account, whatever its password.
        let (other, _) = Vault::create(temp("relink-other"), "pw", CHEAP_KDF).unwrap();
        assert!(!other.sync_copy().same_account(&key_file, "pw"));
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
