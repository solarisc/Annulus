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
    Change, Changes, Device, Devices, LoginFinish, LoginStart, LoginStartResponse, PasswordBegin,
    PasswordBeginResponse, PasswordChange, PushResult, Record, RegisterFinish, RegisterStart,
    RegisterStartResponse, Session, Suite,
};

/// Versions kept per record besides the current one (ADR-0008 history).
const HISTORY: i64 = 20;

/// Failed logins per account before it is paused. Higher than the per-address limit, so
/// one guesser cannot lock a user out.
const MAX_ACCOUNT_FAILURES: u32 = 10;

/// Failed logins per client address before it is paused.
const MAX_ADDRESS_FAILURES: u32 = 5;

/// Most names and addresses tracked for throttling, and most logins in progress; beyond
/// that, the oldest entries make room for new ones instead of memory growing.
const MAX_TRACKED: usize = 10_000;

/// How long failed logins count, and how long a paused account waits.
const FAILURE_WINDOW: Duration = Duration::from_mins(15);

/// How long a started login may take to finish.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest account name, in bytes (an e-mail address fits).
const MAX_ACCOUNT_NAME: usize = 254;

/// A device token unused for this long (90 days, in seconds) stops working; the device
/// signs in again.
const TOKEN_IDLE: i64 = 90 * 24 * 60 * 60;

/// The largest key file a password change may store: the real one is a few hundred bytes.
const MAX_KEY_FILE: usize = 4096;

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

/// What a login proves once finished.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// Signs a new device in.
    SignIn,
    /// The current password, for a password change of this account id.
    PasswordChange(i64),
}

/// A login between its two steps.
struct PendingLogin {
    account: String,
    address: Option<IpAddr>,
    state: ServerLogin<Suite>,
    started: Instant,
    purpose: Purpose,
}

/// Makes room for one more entry in `map`, which holds at most `limit`: drops the entries
/// `expired` says are done, then the lowest-`rank`ed ones, so new names and addresses are
/// always tracked.
fn make_room<K, V, R>(
    map: &mut HashMap<K, V>,
    limit: usize,
    expired: impl Fn(&V) -> bool,
    rank: impl Fn(&V) -> R,
) where
    K: Eq + std::hash::Hash + Clone,
    R: Ord,
{
    if map.len() < limit {
        return;
    }
    map.retain(|_, value| !expired(value));
    while map.len() >= limit.max(1) {
        let Some(lowest) = map
            .iter()
            .min_by_key(|(_, value)| rank(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&lowest);
    }
}

/// The trimmed account name, if it is one: not empty and at most [`MAX_ACCOUNT_NAME`].
fn valid_name(name: &str) -> Result<&str, ServerError> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_ACCOUNT_NAME {
        return Err(ServerError::BadRequest("account name".into()));
    }
    Ok(name)
}

/// Bytes in a key file: base64 text, or (files from earlier builds) an array of numbers.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum KeyBytes {
    Text(String),
    Numbers(Vec<u8>),
}

impl KeyBytes {
    fn is_empty(&self) -> bool {
        match self {
            KeyBytes::Text(text) => text.is_empty(),
            KeyBytes::Numbers(numbers) => numbers.is_empty(),
        }
    }
}

/// A wrapped key in a key file.
#[derive(serde::Deserialize)]
struct SealedShape {
    nonce: KeyBytes,
    ciphertext: KeyBytes,
}

/// The Argon2id costs in a key file.
#[derive(serde::Deserialize)]
struct KdfShape {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

/// The shape of the vault's key file (ADR-0007), which the server stores but cannot open.
#[derive(serde::Deserialize)]
struct KeyFileShape {
    salt: KeyBytes,
    kdf: KdfShape,
    by_password: SealedShape,
    by_recovery: SealedShape,
}

/// Refuses a `wrapped_keys` that is not a key file, so a bad upload cannot replace the one
/// every device needs to unlock the account.
fn check_key_file(bytes: &[u8]) -> Result<(), ServerError> {
    let refused = || ServerError::BadRequest("the wrapped keys are not a key file".into());
    if bytes.len() > MAX_KEY_FILE {
        return Err(refused());
    }
    let file: KeyFileShape = serde_json::from_slice(bytes).map_err(|_| refused())?;
    let sealed_ok =
        |sealed: &SealedShape| !sealed.nonce.is_empty() && !sealed.ciphertext.is_empty();
    let kdf_ok = file.kdf.memory_kib > 0 && file.kdf.iterations > 0 && file.kdf.parallelism > 0;
    if file.salt.is_empty()
        || !kdf_ok
        || !sealed_ok(&file.by_password)
        || !sealed_ok(&file.by_recovery)
    {
        return Err(refused());
    }
    Ok(())
}

/// The server state: the database, the OPAQUE keys and logins in progress.
pub struct Server {
    db: Mutex<Connection>,
    setup: ServerSetup<Suite>,
    signups: Signups,
    /// Most entries in `logins` and in `failures` ([`MAX_TRACKED`]; smaller in tests).
    max_tracked: usize,
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

/// `N` random bytes as lowercase hex.
fn random_hex<const N: usize>() -> Result<String, ServerError> {
    use std::fmt::Write as _;
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|_| ServerError::Storage("no randomness".into()))?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(2 * N), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

fn new_token() -> Result<String, ServerError> {
    random_hex::<32>()
}

/// The hash an invite code is stored under: the code without the dashes and spaces it is
/// shown or typed with, in lowercase.
fn invite_hash(code: &str) -> Vec<u8> {
    let plain: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    token_hash(&plain)
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
                 replaced_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS invites (
                 code_hash BLOB PRIMARY KEY,
                 expires INTEGER NOT NULL);",
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
            max_tracked: MAX_TRACKED,
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

    /// A single-use invite code that lets one account register while sign-ups are closed,
    /// valid for `valid_for`. Only its hash is stored.
    ///
    /// # Errors
    /// The database failed, or no randomness.
    pub fn create_invite(&self, valid_for: Duration) -> Result<String, ServerError> {
        let code = random_hex::<16>()?;
        let expires = now().saturating_add(i64::try_from(valid_for.as_secs()).unwrap_or(i64::MAX));
        let db = self.db()?;
        db.execute("DELETE FROM invites WHERE expires <= ?1", [now()])?;
        db.execute(
            "INSERT INTO invites (code_hash, expires) VALUES (?1, ?2)",
            params![invite_hash(&code), expires],
        )?;
        // Shown in groups of four, like 1a2b-3c4d-…; dashes are ignored when it is used.
        let groups: Vec<&str> = (0..code.len())
            .step_by(4)
            .map(|at| &code[at..at + 4])
            .collect();
        Ok(groups.join("-"))
    }

    /// Refuses a registration while sign-ups are closed, unless `invite` is a code that is
    /// still unused and unexpired. Says nothing about which account names exist.
    fn check_signup(&self, db: &Connection, invite: Option<&str>) -> Result<(), ServerError> {
        if self.signups == Signups::Open {
            return Ok(());
        }
        let valid = match invite {
            Some(code) => db
                .query_row(
                    "SELECT 1 FROM invites WHERE code_hash = ?1 AND expires > ?2",
                    params![invite_hash(code), now()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
            None => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ServerError::Refused(
                "this server does not accept new accounts without an invite code",
            ))
        }
    }

    /// Registration step 1. While sign-ups are closed it needs an `invite` code (from
    /// [`Server::create_invite`]); the code is used up only by [`Server::register_finish`].
    ///
    /// # Errors
    /// Signups are closed and the invite is missing or not valid, the name is taken, or
    /// the request is malformed.
    pub fn register_start(
        &self,
        request: &RegisterStart,
        invite: Option<&str>,
    ) -> Result<RegisterStartResponse, ServerError> {
        let db = self.db()?;
        self.check_signup(&db, invite)?;
        let account = valid_name(&request.account)?;
        if Self::account_id(&db, account)?.is_some() {
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

    /// Registration step 2: stores the account and signs this device in. While sign-ups
    /// are closed, the `invite` is used up in the same transaction, so a refused name
    /// leaves it valid.
    ///
    /// # Errors
    /// As [`Server::register_start`].
    pub fn register_finish(
        &self,
        request: &RegisterFinish,
        invite: Option<&str>,
    ) -> Result<Session, ServerError> {
        let account = valid_name(&request.account)?;
        let upload = RegistrationUpload::<Suite>::deserialize(&request.upload)
            .map_err(bad("registration upload"))?;
        let record = ServerRegistration::<Suite>::finish(upload);
        let mut db = self.db()?;
        let tx = db.transaction()?;
        if self.signups == Signups::Closed {
            let used = tx.execute(
                "DELETE FROM invites WHERE code_hash = ?1 AND expires > ?2",
                params![invite_hash(invite.unwrap_or_default()), now()],
            )?;
            if used == 0 {
                return Err(ServerError::Refused(
                    "this server does not accept new accounts without an invite code",
                ));
            }
        }
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

    /// Counts a login attempt against the account and the address. It is counted when the
    /// login starts, not when it fails: an OPAQUE client learns on its own whether a guess
    /// was right, so a guesser never has to send a failing proof. A login that succeeds is
    /// taken back ([`Server::forgive`]).
    ///
    /// A full table drops entries that expired, then the oldest of those that are not near
    /// a pause, so a flood of new names or addresses neither goes untracked nor clears an
    /// account's pause first.
    fn record_attempt(&self, account: &str, address: Option<IpAddr>) {
        let Ok(mut failures) = self.failures.lock() else {
            return;
        };
        for (key, _) in Self::keys(account, address) {
            let Some(key) = key else { continue };
            if !failures.contains_key(&key) {
                make_room(
                    &mut failures,
                    self.max_tracked,
                    |(_, since)| since.elapsed() >= FAILURE_WINDOW,
                    |(count, since)| (*count >= MAX_ADDRESS_FAILURES, *since),
                );
            }
            let entry = failures.entry(key).or_insert((0, Instant::now()));
            if entry.1.elapsed() >= FAILURE_WINDOW {
                *entry = (0, Instant::now());
            }
            entry.0 += 1;
        }
    }

    /// Takes back the attempt a successful login counted.
    fn forgive(&self, account: &str, address: Option<IpAddr>) {
        let Ok(mut failures) = self.failures.lock() else {
            return;
        };
        for (key, _) in Self::keys(account, address) {
            if let Some(entry) = key.and_then(|key| failures.get_mut(&key)) {
                entry.0 = entry.0.saturating_sub(1);
            }
        }
    }

    /// Keeps a started login for its second step, making room by dropping expired and then
    /// the oldest logins when the table is full.
    fn remember_login(&self, pending: PendingLogin) -> Result<u64, ServerError> {
        let login = random_u64()?;
        let mut logins = self
            .logins
            .lock()
            .map_err(|_| ServerError::Storage("lock poisoned".into()))?;
        make_room(
            &mut logins,
            self.max_tracked,
            |pending| pending.started.elapsed() >= LOGIN_TIMEOUT,
            |pending| pending.started,
        );
        logins.insert(login, pending);
        Ok(login)
    }

    /// Removes and returns started login `login` if it is still in time.
    fn take_login(&self, login: u64) -> Option<PendingLogin> {
        self.logins
            .lock()
            .ok()
            .and_then(|mut logins| logins.remove(&login))
            .filter(|pending| pending.started.elapsed() < LOGIN_TIMEOUT)
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
        let account = valid_name(&request.account)?;
        let (login, response) =
            self.start_login(account, &request.request, address, Purpose::SignIn)?;
        Ok(LoginStartResponse { login, response })
    }

    /// The OPAQUE login start for `account` (a fake one if it does not exist), counted as
    /// an attempt and kept for its second step: the login id and the credential response.
    fn start_login(
        &self,
        account: &str,
        request: &[u8],
        address: Option<IpAddr>,
        purpose: Purpose,
    ) -> Result<(u64, Vec<u8>), ServerError> {
        if self.throttled(account, address) {
            return Err(ServerError::Throttled);
        }
        let stored: Option<Vec<u8>> = self
            .db()?
            .query_row(
                "SELECT opaque FROM accounts WHERE name = ?1",
                [account],
                |row| row.get(0),
            )
            .optional()?;
        let password_file = stored
            .map(|bytes| ServerRegistration::<Suite>::deserialize(&bytes))
            .transpose()
            .map_err(|_| ServerError::Storage("a stored login record is damaged".into()))?;
        let message =
            CredentialRequest::<Suite>::deserialize(request).map_err(bad("login request"))?;
        let result = ServerLogin::start(
            &mut OsRng,
            &self.setup,
            password_file,
            message,
            account.as_bytes(),
            ServerLoginParameters::default(),
        )
        .map_err(bad("login request"))?;
        self.record_attempt(account, address);
        let login = self.remember_login(PendingLogin {
            account: account.to_owned(),
            address,
            state: result.state,
            started: Instant::now(),
            purpose,
        })?;
        Ok((login, result.message.serialize().to_vec()))
    }

    /// Checks the proof that finishes started login `login` for `purpose`; a good proof
    /// takes back the attempt it counted. Returns the login's account name.
    fn finish_login(
        &self,
        login: u64,
        finalization: &[u8],
        purpose: Purpose,
        failed: &'static str,
    ) -> Result<String, ServerError> {
        let pending = self
            .take_login(login)
            .filter(|pending| pending.purpose == purpose)
            .ok_or(ServerError::Refused(failed))?;
        let finalization = CredentialFinalization::<Suite>::deserialize(finalization)
            .map_err(bad("login finalization"))?;
        pending
            .state
            .finish(finalization, ServerLoginParameters::default())
            .map_err(|_| ServerError::Refused(failed))?;
        self.forgive(&pending.account, pending.address);
        Ok(pending.account)
    }

    /// Login step 2: checks the client's proof and signs the device in.
    ///
    /// # Errors
    /// The password was wrong, the account does not exist, or the login expired; all say
    /// the same thing.
    pub fn login_finish(&self, request: &LoginFinish) -> Result<Session, ServerError> {
        const FAILED: &str = "wrong account name or password";
        let account = self.finish_login(
            request.login,
            &request.finalization,
            Purpose::SignIn,
            FAILED,
        )?;
        let db = self.db()?;
        let found: Option<(i64, Vec<u8>)> = db
            .query_row(
                "SELECT id, wrapped_keys FROM accounts WHERE name = ?1",
                [&account],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (account, wrapped_keys) = found.ok_or(ServerError::Refused(FAILED))?;
        Self::add_device(&db, account, &request.device, wrapped_keys)
    }

    /// The caller behind a bearer token; also marks the device as seen. A token unused for
    /// [`TOKEN_IDLE`] is signed out instead.
    ///
    /// # Errors
    /// The token is unknown, was revoked, or sat unused too long.
    pub fn authenticate(&self, token: &str) -> Result<Caller, ServerError> {
        let db = self.db()?;
        let (caller, last_seen) = db
            .query_row(
                "SELECT account, id, last_seen FROM devices WHERE token_hash = ?1",
                [token_hash(token)],
                |row| {
                    Ok((
                        Caller {
                            account: row.get(0)?,
                            device: row.get(1)?,
                        },
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(ServerError::Unauthorized)?;
        if now().saturating_sub(last_seen) > TOKEN_IDLE {
            db.execute("DELETE FROM devices WHERE id = ?1", [caller.device])?;
            return Err(ServerError::Unauthorized);
        }
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

    /// Password change step 1, for the signed-in caller's account from `address`: starts a
    /// login with the current password (a device token alone cannot change the password)
    /// and the registration of the new one. Counted and throttled like a login.
    ///
    /// # Errors
    /// Throttled, or the request is malformed.
    pub fn password_begin(
        &self,
        caller: Caller,
        request: &PasswordBegin,
        address: Option<IpAddr>,
    ) -> Result<PasswordBeginResponse, ServerError> {
        let account = self.account_name(caller)?;
        let message = RegistrationRequest::<Suite>::deserialize(&request.registration)
            .map_err(bad("registration request"))?;
        let registration =
            ServerRegistration::<Suite>::start(&self.setup, message, account.as_bytes())
                .map_err(bad("registration request"))?;
        let (login, credential) = self.start_login(
            &account,
            &request.login,
            address,
            Purpose::PasswordChange(caller.account),
        )?;
        Ok(PasswordBeginResponse {
            login,
            credential,
            registration: registration.message.serialize().to_vec(),
        })
    }

    /// Password change step 2: checks the proof of the current password, then stores the
    /// new login record and key file and signs every other device out (they sign in again
    /// with the new password).
    ///
    /// # Errors
    /// The current password was not proven (or the login expired), `wrapped_keys` is not
    /// a key file, the request is malformed, or the database failed (nothing changed).
    pub fn password_change(
        &self,
        caller: Caller,
        request: &PasswordChange,
    ) -> Result<(), ServerError> {
        self.finish_login(
            request.login,
            &request.finalization,
            Purpose::PasswordChange(caller.account),
            "the current password is wrong",
        )?;
        check_key_file(&request.wrapped_keys)?;
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

    /// Tests: tracks at most `limit` names, addresses and started logins.
    #[cfg(test)]
    pub(crate) fn with_tracking_limit(mut self, limit: usize) -> Self {
        self.max_tracked = limit;
        self
    }

    /// Tests: every device was last seen `seconds` earlier than recorded.
    #[cfg(test)]
    pub(crate) fn backdate_devices(&self, seconds: i64) {
        self.db()
            .unwrap()
            .execute("UPDATE devices SET last_seen = last_seen - ?1", [seconds])
            .unwrap();
    }
}
