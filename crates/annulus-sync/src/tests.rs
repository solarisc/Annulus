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
            .login_start(request)
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
#[derive(Default)]
struct Device {
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
}

fn account(server: &Server) -> (Caller, Caller) {
    let mut direct = Direct {
        server,
        caller: None,
    };
    let first =
        account::register(&mut direct, &cheap(), "ana", "pw", b"keys".to_vec(), "a").unwrap();
    let second = account::login(&mut direct, &cheap(), "ana", "pw", "b").unwrap();
    assert_eq!(second.wrapped_keys, b"keys");
    (
        server.authenticate(&first.token).unwrap(),
        server.authenticate(&second.token).unwrap(),
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
