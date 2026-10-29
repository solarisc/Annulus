//! Accounts, devices and records in one SQLite file, and the OPAQUE server side.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
};
use opaque_rand::rngs::OsRng;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use annulus_proto::{
    Change, Changes, Device, Devices, LoginFinish, LoginStart, LoginStartResponse, PushResult,
    Record, RegisterFinish, RegisterStart, RegisterStartResponse, Session, Suite,
};

/// Versions kept per record besides the current one (ADR-0008 history).
const HISTORY: i64 = 20;

/// Failed logins per account before it is paused. Higher than the per-address limit, so
/// one guesser cannot lock a user out.
const MAX_ACCOUNT_FAILURES: u32 = 10;

/// Failed logins per client address before it is paused.
const MAX_ADDRESS_FAILURES: u32 = 5;

/// Most names and addresses tracked for throttling, and most logins in progress; beyond
/// that, new ones are refused instead of growing memory.
const MAX_TRACKED: usize = 10_000;

/// How long failed logins count, and how long a paused account waits.
const FAILURE_WINDOW: Duration = Duration::from_mins(15);

/// How long a started login may take to finish.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Who may create accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signups {
    /// Anyone who can reach the server (for first setup, or a trusted network).
    Open,
    /// Nobody; the default once the accounts exist.
    Closed,
}

/// Why a request failed. Login and registration errors say nothing about whether an
/// account exists.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The request was malformed.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Registration is closed, the name is taken, or the login failed.
    #[error("{0}")]
    Refused(&'static str),
    /// Too many failed logins; try again later.
    #[error("too many attempts; try again in a few minutes")]
    Throttled,
    /// No valid device token.
    #[error("not signed in")]
    Unauthorized,
    /// The database failed.
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<rusqlite::Error> for ServerError {
    fn from(error: rusqlite::Error) -> Self {
        ServerError::Storage(error.to_string())
    }
}

fn bad(what: &str) -> impl Fn(opaque_ke::errors::ProtocolError) -> ServerError + '_ {
    move |_| ServerError::BadRequest(what.to_owned())
}

/// A login between its two steps.
struct PendingLogin {
    account: String,
    address: Option<IpAddr>,
    state: ServerLogin<Suite>,
    started: Instant,
}

/// The server state: the database, the OPAQUE keys and logins in progress.
pub struct Server {
    db: Mutex<Connection>,
    setup: ServerSetup<Suite>,
    signups: Signups,
    logins: Mutex<HashMap<u64, PendingLogin>>,
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    /// Per account, the newest sequence number, for clients waiting for changes.
    latest: Mutex<HashMap<i64, tokio::sync::watch::Sender<u64>>>,
}

/// One account as the admin sees it: no record contents, which the server cannot read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    /// The account name.
    pub name: String,
    /// Signed-in devices.
    pub devices: u64,
    /// Records, not counting tombstones.
    pub records: u64,
    /// Ciphertext stored, history included.
    pub bytes: u64,
}

/// The account and device a token belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caller {
    account: i64,
    device: i64,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn random_u64() -> Result<u64, ServerError> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| ServerError::Storage("no randomness".into()))?;
    Ok(u64::from_le_bytes(bytes))
}

fn new_token() -> Result<String, ServerError> {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ServerError::Storage("no randomness".into()))?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

fn seq(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

impl Server {
    /// Opens (or creates) the database at `path`; `:memory:` keeps it in memory.
    ///
    /// # Errors
    /// The file cannot be opened or its stored OPAQUE keys are damaged.
    pub fn open(path: &Path, signups: Signups) -> Result<Self, ServerError> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS server (key TEXT PRIMARY KEY, value BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS accounts (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE,
                 opaque BLOB NOT NULL,
                 wrapped_keys BLOB NOT NULL,
                 last_seq INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS devices (
                 id INTEGER PRIMARY KEY,
                 account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 token_hash BLOB NOT NULL UNIQUE,
                 last_seen INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS records (
                 account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 id TEXT NOT NULL,
                 seq INTEGER NOT NULL,
                 ciphertext BLOB NOT NULL,
                 deleted INTEGER NOT NULL,
                 PRIMARY KEY (account, id));
             CREATE INDEX IF NOT EXISTS records_by_seq ON records (account, seq);
             CREATE TABLE IF NOT EXISTS history (
                 account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 id TEXT NOT NULL,
                 seq INTEGER NOT NULL,
                 ciphertext BLOB NOT NULL,
                 deleted INTEGER NOT NULL,
                 replaced_at INTEGER NOT NULL);",
        )?;
        let stored: Option<Vec<u8>> = db
            .query_row(
                "SELECT value FROM server WHERE key = 'opaque_setup'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let setup = if let Some(bytes) = stored {
            ServerSetup::<Suite>::deserialize(&bytes)
                .map_err(|_| ServerError::Storage("the stored OPAQUE keys are damaged".into()))?
        } else {
            let setup = ServerSetup::<Suite>::new(&mut OsRng);
            db.execute(
                "INSERT INTO server (key, value) VALUES ('opaque_setup', ?1)",
                params![setup.serialize().to_vec()],
            )?;
            setup
        };
        Ok(Self {
            db: Mutex::new(db),
            setup,
            signups,
            logins: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            latest: Mutex::new(HashMap::new()),
        })
    }

    fn db(&self) -> Result<std::sync::MutexGuard<'_, Connection>, ServerError> {
        self.db
            .lock()
            .map_err(|_| ServerError::Storage("database lock poisoned".into()))
    }

    fn account_id(db: &Connection, name: &str) -> Result<Option<i64>, ServerError> {
        Ok(db
            .query_row("SELECT id FROM accounts WHERE name = ?1", [name], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Registration step 1.
    ///
    /// # Errors
    /// Signups are closed, the name is taken, or the request is malformed.
    pub fn register_start(
        &self,
        request: &RegisterStart,
    ) -> Result<RegisterStartResponse, ServerError> {
        let account = request.account.trim();
        if self.signups == Signups::Closed {
            return Err(ServerError::Refused(
                "this server does not accept new accounts",
            ));
        }
        if account.is_empty() || account.len() > 254 {
            return Err(ServerError::BadRequest("account name".into()));
        }
        if Self::account_id(&*self.db()?, account)?.is_some() {
            return Err(ServerError::Refused("that account name is not available"));
        }
        let message = RegistrationRequest::<Suite>::deserialize(&request.request)
            .map_err(bad("registration request"))?;
        let result = ServerRegistration::<Suite>::start(&self.setup, message, account.as_bytes())
            .map_err(bad("registration request"))?;
        Ok(RegisterStartResponse {
            response: result.message.serialize().to_vec(),
        })
    }

    /// Registration step 2: stores the account and signs this device in.
    ///
    /// # Errors
    /// As [`Server::register_start`].
    pub fn register_finish(&self, request: &RegisterFinish) -> Result<Session, ServerError> {
        let account = request.account.trim();
        if self.signups == Signups::Closed {
            return Err(ServerError::Refused(
                "this server does not accept new accounts",
            ));
        }
        let upload = RegistrationUpload::<Suite>::deserialize(&request.upload)
            .map_err(bad("registration upload"))?;
        let record = ServerRegistration::<Suite>::finish(upload);
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO accounts (name, opaque, wrapped_keys) VALUES (?1, ?2, ?3)",
            params![account, record.serialize().to_vec(), request.wrapped_keys],
        )?;
        if inserted == 0 {
            return Err(ServerError::Refused("that account name is not available"));
        }
        let id = tx.last_insert_rowid();
        let session = Self::add_device(&tx, id, &request.device, request.wrapped_keys.clone())?;
        tx.commit()?;
        Ok(session)
    }

    fn add_device(
        db: &Connection,
        account: i64,
        name: &str,
        wrapped_keys: Vec<u8>,
    ) -> Result<Session, ServerError> {
        let token = new_token()?;
        db.execute(
            "INSERT INTO devices (account, name, token_hash, last_seen) VALUES (?1, ?2, ?3, ?4)",
            params![account, name.trim(), token_hash(&token), now()],
        )?;
        Ok(Session {
            token,
            device: seq(db.last_insert_rowid()),
            wrapped_keys,
        })
    }

    fn keys(account: &str, address: Option<IpAddr>) -> [(Option<String>, u32); 2] {
        [
            (Some(format!("account {account}")), MAX_ACCOUNT_FAILURES),
            (
                address.map(|ip| format!("address {ip}")),
                MAX_ADDRESS_FAILURES,
            ),
        ]
    }

    fn throttled(&self, account: &str, address: Option<IpAddr>) -> bool {
        self.failures.lock().is_ok_and(|failures| {
            Self::keys(account, address).iter().any(|(key, limit)| {
                key.as_ref()
                    .and_then(|key| failures.get(key))
                    .is_some_and(|(count, since)| {
                        *count >= *limit && since.elapsed() < FAILURE_WINDOW
                    })
            })
        })
    }

    fn record_failure(&self, account: &str, address: Option<IpAddr>) {
        let Ok(mut failures) = self.failures.lock() else {
            return;
        };
        if failures.len() >= MAX_TRACKED {
            failures.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
        }
        for (key, _) in Self::keys(account, address) {
            let Some(key) = key else { continue };
            if failures.len() >= MAX_TRACKED && !failures.contains_key(&key) {
                continue;
            }
            let entry = failures.entry(key).or_insert((0, Instant::now()));
            if entry.1.elapsed() >= FAILURE_WINDOW {
                *entry = (0, Instant::now());
            }
            entry.0 += 1;
        }
    }

    /// Login step 1 from `address` (the client, if known). Unknown accounts get a response
    /// that looks the same as a real one.
    ///
    /// # Errors
    /// The account or address is throttled, or the request is malformed.
    pub fn login_start(
        &self,
        request: &LoginStart,
        address: Option<IpAddr>,
    ) -> Result<LoginStartResponse, ServerError> {
        let account = request.account.trim().to_owned();
        if self.throttled(&account, address) {
            return Err(ServerError::Throttled);
        }
        let stored: Option<Vec<u8>> = self
            .db()?
            .query_row(
                "SELECT opaque FROM accounts WHERE name = ?1",
                [&account],
                |row| row.get(0),
            )
            .optional()?;
        let password_file = stored
            .map(|bytes| ServerRegistration::<Suite>::deserialize(&bytes))
            .transpose()
            .map_err(|_| ServerError::Storage("a stored login record is damaged".into()))?;
        let message = CredentialRequest::<Suite>::deserialize(&request.request)
            .map_err(bad("login request"))?;
        let result = ServerLogin::start(
            &mut OsRng,
            &self.setup,
            password_file,
            message,
            account.as_bytes(),
            ServerLoginParameters::default(),
        )
        .map_err(bad("login request"))?;
        let login = random_u64()?;
        if let Ok(mut logins) = self.logins.lock() {
            logins.retain(|_, pending| pending.started.elapsed() < LOGIN_TIMEOUT);
            if logins.len() >= MAX_TRACKED {
                return Err(ServerError::Throttled);
            }
            logins.insert(
                login,
                PendingLogin {
                    account,
                    address,
                    state: result.state,
                    started: Instant::now(),
                },
            );
        }
        Ok(LoginStartResponse {
            login,
            response: result.message.serialize().to_vec(),
        })
    }

    /// Login step 2: checks the client's proof and signs the device in.
    ///
    /// # Errors
    /// The password was wrong, the account does not exist, or the login expired; all say
    /// the same thing.
    pub fn login_finish(&self, request: &LoginFinish) -> Result<Session, ServerError> {
        const FAILED: ServerError = ServerError::Refused("wrong account name or password");
        let pending = self
            .logins
            .lock()
            .ok()
            .and_then(|mut logins| logins.remove(&request.login))
            .filter(|pending| pending.started.elapsed() < LOGIN_TIMEOUT)
            .ok_or(FAILED)?;
        let finalization = CredentialFinalization::<Suite>::deserialize(&request.finalization)
            .map_err(bad("login finalization"))?;
        if pending
            .state
            .finish(finalization, ServerLoginParameters::default())
            .is_err()
        {
            self.record_failure(&pending.account, pending.address);
            return Err(FAILED);
        }
        let db = self.db()?;
        let found: Option<(i64, Vec<u8>)> = db
            .query_row(
                "SELECT id, wrapped_keys FROM accounts WHERE name = ?1",
                [&pending.account],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (account, wrapped_keys) = found.ok_or(FAILED)?;
        Self::add_device(&db, account, &request.device, wrapped_keys)
    }

    /// The caller behind a bearer token; also marks the device as seen.
    ///
    /// # Errors
    /// The token is unknown or was revoked.
    pub fn authenticate(&self, token: &str) -> Result<Caller, ServerError> {
        let db = self.db()?;
        let caller = db
            .query_row(
                "SELECT account, id FROM devices WHERE token_hash = ?1",
                [token_hash(token)],
                |row| {
                    Ok(Caller {
                        account: row.get(0)?,
                        device: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(ServerError::Unauthorized)?;
        db.execute(
            "UPDATE devices SET last_seen = ?1 WHERE id = ?2",
            params![now(), caller.device],
        )?;
        Ok(caller)
    }

    /// Applies `changes` in order. A change made from an outdated version is not applied;
    /// the current version comes back as a conflict.
    ///
    /// # Errors
    /// The database failed; nothing was applied.
    pub fn push(&self, caller: Caller, changes: &[Change]) -> Result<PushResult, ServerError> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let mut last: i64 = tx.query_row(
            "SELECT last_seq FROM accounts WHERE id = ?1",
            [caller.account],
            |row| row.get(0),
        )?;
        let mut result = PushResult::default();
        for change in changes {
            if change.id.is_empty() || change.id.len() > 128 {
                return Err(ServerError::BadRequest("record id".into()));
            }
            let current: Option<Record> = tx
                .query_row(
                    "SELECT seq, ciphertext, deleted FROM records WHERE account = ?1 AND id = ?2",
                    params![caller.account, change.id],
                    |row| {
                        Ok(Record {
                            id: change.id.clone(),
                            seq: seq(row.get(0)?),
                            ciphertext: row.get(1)?,
                            deleted: row.get(2)?,
                        })
                    },
                )
                .optional()?;
            // A change based on a version the server no longer has (it was restored from a
            // backup) overwrites nothing, so it is accepted like a new record.
            if let Some(record) = current.as_ref().filter(|record| record.seq != change.base) {
                result.conflicts.push(record.clone());
                continue;
            }
            last += 1;
            if current.is_some() {
                tx.execute(
                    "INSERT INTO history (account, id, seq, ciphertext, deleted, replaced_at)
                     SELECT account, id, seq, ciphertext, deleted, ?3 FROM records
                     WHERE account = ?1 AND id = ?2",
                    params![caller.account, change.id, now()],
                )?;
                tx.execute(
                    "DELETE FROM history WHERE account = ?1 AND id = ?2 AND seq NOT IN (
                         SELECT seq FROM history WHERE account = ?1 AND id = ?2
                         ORDER BY seq DESC LIMIT ?3)",
                    params![caller.account, change.id, HISTORY],
                )?;
            }
            tx.execute(
                "INSERT INTO records (account, id, seq, ciphertext, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (account, id) DO UPDATE SET
                     seq = excluded.seq, ciphertext = excluded.ciphertext,
                     deleted = excluded.deleted",
                // A deletion keeps its ciphertext too: a sealed tombstone, which lets devices
                // tell a real deletion from one the server made up. Older clients send none.
                params![
                    caller.account,
                    change.id,
                    last,
                    change.ciphertext,
                    change.deleted
                ],
            )?;
            result.accepted.push((change.id.clone(), seq(last)));
        }
        tx.execute(
            "UPDATE accounts SET last_seq = ?1 WHERE id = ?2",
            params![last, caller.account],
        )?;
        tx.commit()?;
        if let Ok(waiting) = self.latest.lock()
            && let Some(sender) = waiting.get(&caller.account)
        {
            sender.send_replace(seq(last));
        }
        Ok(result)
    }

    /// Records changed after `since`, in sequence order, and the newest sequence number.
    ///
    /// # Errors
    /// The database failed.
    pub fn pull(&self, caller: Caller, since: u64) -> Result<Changes, ServerError> {
        let db = self.db()?;
        let since = i64::try_from(since).unwrap_or(i64::MAX);
        let mut statement = db.prepare(
            "SELECT id, seq, ciphertext, deleted FROM records
             WHERE account = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let records = statement
            .query_map(params![caller.account, since], |row| {
                Ok(Record {
                    id: row.get(0)?,
                    seq: seq(row.get(1)?),
                    ciphertext: row.get(2)?,
                    deleted: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let latest: i64 = db.query_row(
            "SELECT last_seq FROM accounts WHERE id = ?1",
            [caller.account],
            |row| row.get(0),
        )?;
        Ok(Changes {
            records,
            latest: seq(latest),
        })
    }

    /// A receiver that sees the account's newest sequence number change.
    ///
    /// # Errors
    /// The database failed.
    pub fn watch(&self, caller: Caller) -> Result<tokio::sync::watch::Receiver<u64>, ServerError> {
        let latest: i64 = self.db()?.query_row(
            "SELECT last_seq FROM accounts WHERE id = ?1",
            [caller.account],
            |row| row.get(0),
        )?;
        let mut waiting = self
            .latest
            .lock()
            .map_err(|_| ServerError::Storage("lock poisoned".into()))?;
        let sender = waiting
            .entry(caller.account)
            .or_insert_with(|| tokio::sync::watch::channel(seq(latest)).0);
        sender.send_if_modified(|value| {
            let changed = *value != seq(latest);
            *value = (*value).max(seq(latest));
            changed
        });
        Ok(sender.subscribe())
    }

    /// Earlier versions of a record, newest first (for restoring from history or trash).
    ///
    /// # Errors
    /// The database failed.
    pub fn history(&self, caller: Caller, id: &str) -> Result<Vec<Record>, ServerError> {
        let db = self.db()?;
        let mut statement = db.prepare(
            "SELECT seq, ciphertext, deleted FROM history
             WHERE account = ?1 AND id = ?2 ORDER BY seq DESC",
        )?;
        let records = statement
            .query_map(params![caller.account, id], |row| {
                Ok(Record {
                    id: id.to_owned(),
                    seq: seq(row.get(0)?),
                    ciphertext: row.get(1)?,
                    deleted: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// The account's signed-in devices.
    ///
    /// # Errors
    /// The database failed.
    pub fn devices(&self, caller: Caller) -> Result<Devices, ServerError> {
        let db = self.db()?;
        let mut statement =
            db.prepare("SELECT id, name, last_seen FROM devices WHERE account = ?1 ORDER BY id")?;
        let devices = statement
            .query_map([caller.account], |row| {
                let id: i64 = row.get(0)?;
                Ok(Device {
                    id: seq(id),
                    name: row.get(1)?,
                    last_seen: seq(row.get(2)?),
                    current: id == caller.device,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Devices { devices })
    }

    fn account_name(&self, caller: Caller) -> Result<String, ServerError> {
        Ok(self.db()?.query_row(
            "SELECT name FROM accounts WHERE id = ?1",
            [caller.account],
            |row| row.get(0),
        )?)
    }

    /// Password change step 1, for the signed-in caller's account.
    ///
    /// # Errors
    /// The request is malformed.
    pub fn password_start(
        &self,
        caller: Caller,
        request: &RegisterStart,
    ) -> Result<RegisterStartResponse, ServerError> {
        let account = self.account_name(caller)?;
        let message = RegistrationRequest::<Suite>::deserialize(&request.request)
            .map_err(bad("registration request"))?;
        let result = ServerRegistration::<Suite>::start(&self.setup, message, account.as_bytes())
            .map_err(bad("registration request"))?;
        Ok(RegisterStartResponse {
            response: result.message.serialize().to_vec(),
        })
    }

    /// Password change step 2: stores the new login record and wrapped key, and signs every
    /// other device out (they sign in again with the new password).
    ///
    /// # Errors
    /// The request is malformed, or the database failed (nothing changed).
    pub fn password_finish(
        &self,
        caller: Caller,
        request: &RegisterFinish,
    ) -> Result<(), ServerError> {
        let upload = RegistrationUpload::<Suite>::deserialize(&request.upload)
            .map_err(bad("registration upload"))?;
        let record = ServerRegistration::<Suite>::finish(upload);
        let mut db = self.db()?;
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE accounts SET opaque = ?1, wrapped_keys = ?2 WHERE id = ?3",
            params![
                record.serialize().to_vec(),
                request.wrapped_keys,
                caller.account
            ],
        )?;
        tx.execute(
            "DELETE FROM devices WHERE account = ?1 AND id != ?2",
            params![caller.account, caller.device],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every account with its device count, record count and stored bytes, for the admin.
    ///
    /// # Errors
    /// The database failed.
    pub fn accounts(&self) -> Result<Vec<AccountInfo>, ServerError> {
        let db = self.db()?;
        let mut statement = db.prepare(
            "SELECT a.name,
                    (SELECT COUNT(*) FROM devices d WHERE d.account = a.id),
                    (SELECT COUNT(*) FROM records r WHERE r.account = a.id AND r.deleted = 0),
                    (SELECT COALESCE(SUM(LENGTH(r.ciphertext)), 0) FROM records r WHERE r.account = a.id)
                  + (SELECT COALESCE(SUM(LENGTH(h.ciphertext)), 0) FROM history h WHERE h.account = a.id)
             FROM accounts a ORDER BY a.name",
        )?;
        let accounts = statement
            .query_map([], |row| {
                let count = |value: i64| u64::try_from(value).unwrap_or(0);
                Ok(AccountInfo {
                    name: row.get(0)?,
                    devices: count(row.get(1)?),
                    records: count(row.get(2)?),
                    bytes: count(row.get(3)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(accounts)
    }

    /// Deletes an account with its devices, records and history. Returns false if there
    /// was no such account.
    ///
    /// # Errors
    /// The database failed.
    pub fn delete_account(&self, name: &str) -> Result<bool, ServerError> {
        Ok(self
            .db()?
            .execute("DELETE FROM accounts WHERE name = ?1", [name])?
            > 0)
    }

    /// Writes a consistent copy of the database to `path` while the server runs.
    ///
    /// # Errors
    /// `path` exists or cannot be written.
    pub fn backup(&self, path: &Path) -> Result<(), ServerError> {
        let target = path
            .to_str()
            .ok_or_else(|| ServerError::BadRequest("backup path".into()))?;
        self.db()?.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }

    /// Signs a device of the caller's account out; its token stops working at once.
    ///
    /// # Errors
    /// The database failed.
    pub fn revoke(&self, caller: Caller, device: u64) -> Result<(), ServerError> {
        self.db()?.execute(
            "DELETE FROM devices WHERE account = ?1 AND id = ?2",
            params![caller.account, i64::try_from(device).unwrap_or(-1)],
        )?;
        Ok(())
    }
}
