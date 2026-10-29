//! SSH connections on `russh` (ADR-0004): connect, verify the host key against our
//! known-hosts file, authenticate, and open an interactive shell with a PTY.
//!
//! Questions for the user (an unknown host key, a password) go through [`Prompt`], so the UI
//! answers them and tests script them. A changed host key is a hard stop: nothing is sent.

#![forbid(unsafe_code)]

pub mod forward;
pub mod keys;
pub mod known;
pub mod proxy;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{self, AuthResult, Handle, Msg};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts_path};
use russh::{Channel, ChannelMsg};
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroizing;

pub use russh;

/// Where to connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Hostname or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Login name.
    pub user: String,
}

impl Target {
    /// Parses `user@host`, `host`, `user@host:port` or `user@[v6]:port`; the user defaults to
    /// `default_user` and the port to 22.
    #[must_use]
    pub fn parse(text: &str, default_user: &str) -> Option<Self> {
        let text = text.trim();
        let (user, rest) = match text.rsplit_once('@') {
            Some((user, rest)) if !user.is_empty() => (user.to_owned(), rest),
            Some(_) => return None,
            None => (default_user.to_owned(), text),
        };
        let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
            let (host, after) = inner.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(port) => port.parse().ok()?,
                None if after.is_empty() => 22,
                None => return None,
            };
            (host.to_owned(), port)
        } else {
            match rest.rsplit_once(':') {
                Some((host, port)) if !host.contains(':') => (host.to_owned(), port.parse().ok()?),
                _ => (rest.to_owned(), 22),
            }
        };
        (!host.is_empty() && port != 0 && !host.contains(char::is_whitespace)).then_some(Self {
            host,
            port,
            user,
        })
    }
}

/// What the host key check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyQuestion {
    /// `host` or `[host]:port`, as `known_hosts` names it.
    pub host: String,
    /// The key type, e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// `SHA256:…` fingerprint.
    pub fingerprint: String,
}

/// What to do with a server key seen for the first time (F-KNOWN-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Trust it and remember it in the known-hosts file.
    Save,
    /// Trust it for this connection only; the next one asks again.
    Once,
    /// Do not connect.
    Reject,
}

impl From<bool> for Trust {
    /// `true` trusts and remembers; `false` rejects.
    fn from(trust: bool) -> Self {
        if trust { Trust::Save } else { Trust::Reject }
    }
}

/// Answers to questions only the user can answer.
pub trait Prompt: Send + Sync {
    /// The server's key is not known yet: trust it, and remember it?
    fn trust_new_host_key(
        &self,
        question: HostKeyQuestion,
    ) -> tokio::sync::oneshot::Receiver<Trust>;

    /// The password for `user@host`, or `None` to give up.
    fn password(
        &self,
        user: String,
        host: String,
    ) -> tokio::sync::oneshot::Receiver<Option<Zeroizing<String>>>;
}

/// Why a connection failed, in terms the app turns into a message with a next step.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// The address could not be reached.
    #[error("could not reach {0}: {1}")]
    Unreachable(String, String),
    /// The host's key differs from the one recorded: possibly an attack. Nothing was sent.
    #[error("the host key of {host} has changed ({fingerprint}); the connection was stopped")]
    HostKeyChanged {
        /// The host.
        host: String,
        /// The new key's fingerprint.
        fingerprint: String,
    },
    /// The user did not trust the new host key.
    #[error("the host key was not trusted")]
    HostKeyRejected,
    /// No offered method was accepted.
    #[error("authentication failed (tried: {tried}; the server accepts: {offered})")]
    AuthFailed {
        /// What was tried.
        tried: String,
        /// What the server said it accepts.
        offered: String,
    },
    /// The user cancelled a prompt.
    #[error("cancelled")]
    Cancelled,
    /// The protocol or the connection failed after connecting.
    #[error("ssh: {0}")]
    Protocol(String),
}

impl From<russh::Error> for SshError {
    fn from(error: russh::Error) -> Self {
        SshError::Protocol(error.to_string())
    }
}

/// What the client handler learned during key exchange.
struct Checker {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    prompt: Arc<dyn Prompt>,
    changed: Arc<std::sync::Mutex<Option<String>>>,
    /// Where connections to each remote-forwarded port go.
    remote: forward::RemoteMap,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    /// A connection arrived at a port the server listens on for a remote rule: carry it to
    /// the rule's destination. Ports no rule asked for are refused (by dropping `reply`).
    #[allow(clippy::too_many_arguments)] // The trait's signature.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let target = self
            .remote
            .lock()
            .ok()
            .and_then(|map| map.get(&connected_port).cloned());
        if let Some(target) = target {
            reply.accept().await;
            tokio::spawn(forward::carry_remote(channel, target));
        }
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            // TODO(F-KEY-006): verify host certificates against configured CAs.
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        match check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let question = HostKeyQuestion {
                    host: known_hosts_name(&self.host, self.port),
                    algorithm: key.algorithm().as_str().to_owned(),
                    fingerprint,
                };
                let answer = self
                    .prompt
                    .trust_new_host_key(question)
                    .await
                    .unwrap_or(Trust::Reject);
                if answer == Trust::Save {
                    if let Some(parent) = self.known_hosts.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = russh::keys::known_hosts::learn_known_hosts_path(
                        &self.host,
                        self.port,
                        key,
                        &self.known_hosts,
                    );
                }
                Ok(answer != Trust::Reject)
            }
            Err(_) => {
                if let Ok(mut changed) = self.changed.lock() {
                    *changed = Some(fingerprint);
                }
                Ok(false)
            }
        }
    }
}

fn known_hosts_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

/// An authenticated connection.
pub struct Connection {
    handle: Handle<Checker>,
    remote: forward::RemoteMap,
    /// The jump host this connection runs through, kept open while this one lives.
    _via: Option<Arc<Connection>>,
}

/// The keys to offer, in order: keychain keys, then the default unencrypted key files.
fn candidate_keys(
    identities: &[Zeroizing<String>],
    key_dir: Option<&Path>,
) -> Vec<(String, russh::keys::PrivateKey)> {
    let mut candidates: Vec<(String, russh::keys::PrivateKey)> = identities
        .iter()
        .filter_map(|identity| keys::decode(identity))
        .map(|key| ("saved key".to_owned(), key))
        .collect();
    if let Some(key_dir) = key_dir {
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            if let Ok(key) = russh::keys::load_secret_key(key_dir.join(name), None) {
                candidates.push((format!("key {name}"), key));
            }
        }
    }
    candidates
}

/// Connects to `target`, checks its key against `known_hosts`, and logs in: first with
/// `identities` (private keys from the keychain, OpenSSH text), then the default key files in `key_dir` (`id_ed25519`, `id_ecdsa`, `id_rsa`, unencrypted
/// ones only), then asking for the password up to three times.
///
/// # Errors
/// See [`SshError`].
pub async fn connect(
    target: &Target,
    known_hosts: &Path,
    key_dir: Option<&Path>,
    identities: &[Zeroizing<String>],
    prompt: Arc<dyn Prompt>,
) -> Result<Connection, SshError> {
    let (handle, remote) = handshake(target, known_hosts, prompt.clone(), Route::Direct).await?;
    log_in(handle, remote, None, target, key_dir, identities, prompt).await
}

/// As [`connect`], but the TCP connection goes through `proxy` (SOCKS5 or HTTP CONNECT).
///
/// # Errors
/// As [`connect`]; [`SshError::Unreachable`] names the proxy when it refuses.
pub async fn connect_proxied(
    proxy: &proxy::Proxy,
    target: &Target,
    known_hosts: &Path,
    key_dir: Option<&Path>,
    identities: &[Zeroizing<String>],
    prompt: Arc<dyn Prompt>,
) -> Result<Connection, SshError> {
    let (handle, remote) =
        handshake(target, known_hosts, prompt.clone(), Route::Proxy(proxy)).await?;
    log_in(handle, remote, None, target, key_dir, identities, prompt).await
}

/// How the SSH transport reaches its server.
enum Route<'a> {
    Direct,
    Jump(&'a Arc<Connection>),
    Proxy(&'a proxy::Proxy),
}

/// As [`connect`], but through `jump`: the connection to `target` runs inside a
/// `direct-tcpip` channel of the jump host's connection (OpenSSH's `ProxyJump`), with its
/// own host-key check and login. The jump connection stays open while the result lives.
///
/// # Errors
/// As [`connect`]; [`SshError::Unreachable`] when the jump host cannot reach `target`.
pub async fn connect_via(
    jump: Arc<Connection>,
    target: &Target,
    known_hosts: &Path,
    key_dir: Option<&Path>,
    identities: &[Zeroizing<String>],
    prompt: Arc<dyn Prompt>,
) -> Result<Connection, SshError> {
    let (handle, remote) =
        handshake(target, known_hosts, prompt.clone(), Route::Jump(&jump)).await?;
    log_in(
        handle,
        remote,
        Some(jump),
        target,
        key_dir,
        identities,
        prompt,
    )
    .await
}

/// Opens the SSH transport to `target` (directly, or through `via`) and checks its host key.
async fn handshake(
    target: &Target,
    known_hosts: &Path,
    prompt: Arc<dyn Prompt>,
    route: Route<'_>,
) -> Result<(Handle<Checker>, forward::RemoteMap), SshError> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        ..client::Config::default()
    });
    let changed = Arc::new(std::sync::Mutex::new(None));
    let remote = forward::RemoteMap::default();
    let checker = Checker {
        host: target.host.clone(),
        port: target.port,
        known_hosts: known_hosts.to_path_buf(),
        prompt,
        changed: changed.clone(),
        remote: remote.clone(),
    };
    let name = known_hosts_name(&target.host, target.port);
    let connected = match route {
        Route::Direct => {
            client::connect(config, (target.host.as_str(), target.port), checker).await
        }
        Route::Jump(jump) => {
            let channel = jump
                .handle
                .channel_open_direct_tcpip(
                    target.host.as_str(),
                    u32::from(target.port),
                    "127.0.0.1",
                    0,
                )
                .await
                .map_err(|error| {
                    SshError::Unreachable(
                        name.clone(),
                        format!("the jump host could not connect: {error}"),
                    )
                })?;
            client::connect_stream(config, channel.into_stream(), checker).await
        }
        Route::Proxy(proxy) => {
            let stream = proxy
                .connect(&target.host, target.port)
                .await
                .map_err(|error| {
                    SshError::Unreachable(
                        name.clone(),
                        format!("through the proxy {}:{}: {error}", proxy.host, proxy.port),
                    )
                })?;
            client::connect_stream(config, stream, checker).await
        }
    };
    match connected {
        Ok(handle) => Ok((handle, remote)),
        Err(error) => {
            if let Some(fingerprint) = changed.lock().ok().and_then(|mut slot| slot.take()) {
                return Err(SshError::HostKeyChanged {
                    host: name,
                    fingerprint,
                });
            }
            Err(match error {
                russh::Error::UnknownKey => SshError::HostKeyRejected,
                russh::Error::IO(io) => SshError::Unreachable(name, io.to_string()),
                other => SshError::Protocol(other.to_string()),
            })
        }
    }
}

/// Logs in on an open transport: keys first, then up to three passwords.
async fn log_in(
    mut handle: Handle<Checker>,
    remote: forward::RemoteMap,
    via: Option<Arc<Connection>>,
    target: &Target,
    key_dir: Option<&Path>,
    identities: &[Zeroizing<String>],
    prompt: Arc<dyn Prompt>,
) -> Result<Connection, SshError> {
    let mut tried = Vec::new();
    let mut offered = String::from("unknown");
    for (label, key) in candidate_keys(identities, key_dir) {
        let hash = handle
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        tried.push(label);
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        match handle.authenticate_publickey(&target.user, key).await? {
            AuthResult::Success => {
                return Ok(Connection {
                    handle,
                    remote,
                    _via: via,
                });
            }
            AuthResult::Failure {
                remaining_methods, ..
            } => offered = format!("{remaining_methods:?}"),
        }
    }
    for _ in 0..3 {
        let Some(password) = prompt
            .password(target.user.clone(), target.host.clone())
            .await
            .ok()
            .flatten()
        else {
            return Err(SshError::Cancelled);
        };
        tried.push("password".into());
        match handle
            .authenticate_password(&target.user, password.as_str())
            .await?
        {
            AuthResult::Success => {
                return Ok(Connection {
                    handle,
                    remote,
                    _via: via,
                });
            }
            AuthResult::Failure {
                remaining_methods, ..
            } => offered = format!("{remaining_methods:?}"),
        }
    }
    tried.dedup();
    Err(SshError::AuthFailed {
        tried: tried.join(", "),
        offered,
    })
}

impl Connection {
    /// Whether the connection has ended (the server closed it, or the network dropped it
    /// and keepalives failed).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Opens a shell with a PTY of `columns` by `rows`, advertising `TERM=xterm-256color`.
    ///
    /// # Errors
    /// The server refused the channel, the PTY or the shell.
    pub async fn shell(&self, columns: u32, rows: u32) -> Result<Shell, SshError> {
        self.shell_with_env(columns, rows, &[]).await
    }

    /// [`Connection::shell`], first sending `env` (F-SSH-012). A server that does not accept
    /// a variable (sshd's `AcceptEnv`) ignores it; that is not an error.
    ///
    /// # Errors
    /// The server refused the channel, the PTY or the shell.
    pub async fn shell_with_env(
        &self,
        columns: u32,
        rows: u32,
        env: &[(String, String)],
    ) -> Result<Shell, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", columns, rows, 0, 0, &[])
            .await?;
        for (name, value) in env {
            channel
                .set_env(false, name.as_str(), value.as_str())
                .await?;
        }
        channel.request_shell(true).await?;
        Ok(Shell {
            channel,
            exited: false,
        })
    }

    /// Runs `command` without a PTY and collects what it printed (stdout and stderr) and
    /// its exit status. Only for an action the user asked for, such as installing a key
    /// (F-SSH-011: no hidden exec channels).
    ///
    /// # Errors
    /// The server refused the channel or the command.
    pub async fn exec(&self, command: &str) -> Result<Exec, SshError> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        let mut exec = Exec::default();
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    exec.output.extend_from_slice(&data);
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => exec.status = Some(exit_status),
                Some(ChannelMsg::Close) | None => return Ok(exec),
                Some(_) => {}
            }
        }
    }

    /// Opens the `sftp` subsystem and returns its byte stream, for `annulus-sftp`.
    ///
    /// # Errors
    /// The server refused the channel or the subsystem.
    pub async fn sftp_stream(
        &self,
    ) -> Result<impl AsyncRead + AsyncWrite + Unpin + Send + 'static, SshError> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(channel.into_stream())
    }
}

/// How a command run with [`Connection::exec`] ended.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Exec {
    /// The exit status, if the server sent one.
    pub status: Option<u32>,
    /// Everything it printed.
    pub output: Vec<u8>,
}

/// An interactive shell channel.
pub struct Shell {
    channel: Channel<Msg>,
    /// The server reported the program's exit status (or the signal that ended it).
    exited: bool,
}

/// What came from the shell.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    /// Bytes for the terminal (stdout and stderr interleaved, as a PTY does).
    Data(Vec<u8>),
    /// The remote program exited (the server sent its exit status): a normal end.
    Exited,
    /// The channel ended without an exit status: the connection dropped (F-SSH-009).
    Lost,
}

impl Shell {
    /// Waits for the next output.
    pub async fn next(&mut self) -> Output {
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    return Output::Data(data.to_vec());
                }
                Some(ChannelMsg::ExitStatus { .. } | ChannelMsg::ExitSignal { .. }) => {
                    self.exited = true;
                }
                // OpenSSH sends EOF before the exit status; the close comes last.
                Some(ChannelMsg::Close) | None => {
                    return if self.exited {
                        Output::Exited
                    } else {
                        Output::Lost
                    };
                }
                Some(_) => {}
            }
        }
    }

    /// Sends input to the shell.
    ///
    /// # Errors
    /// The channel is closed.
    pub async fn write(&self, bytes: &[u8]) -> Result<(), SshError> {
        self.channel.data(bytes).await?;
        Ok(())
    }

    /// Tells the server the terminal's new size.
    ///
    /// # Errors
    /// The channel is closed.
    pub async fn resize(&self, columns: u32, rows: u32) -> Result<(), SshError> {
        self.channel.window_change(columns, rows, 0, 0).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
