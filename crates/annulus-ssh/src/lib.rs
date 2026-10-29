//! SSH connections on `russh` (ADR-0004): connect, verify the host key against our
//! known-hosts file, authenticate, and open an interactive shell with a PTY.
//!
//! Questions for the user (an unknown host key, a password) go through [`Prompt`], so the UI
//! answers them and tests script them. A changed host key is a hard stop: nothing is sent.

#![forbid(unsafe_code)]

pub mod forward;

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

/// Answers to questions only the user can answer.
pub trait Prompt: Send + Sync {
    /// The server's key is not known yet: trust it (and remember it)?
    fn trust_new_host_key(&self, question: HostKeyQuestion)
    -> tokio::sync::oneshot::Receiver<bool>;

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
                let trusted = self
                    .prompt
                    .trust_new_host_key(question)
                    .await
                    .unwrap_or(false);
                if trusted {
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
                Ok(trusted)
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
}

/// Connects to `target`, checks its key against `known_hosts`, and logs in: first trying
/// the default key files in `key_dir` (`id_ed25519`, `id_ecdsa`, `id_rsa`, unencrypted
/// ones only), then asking for the password up to three times.
///
/// # Errors
/// See [`SshError`].
pub async fn connect(
    target: &Target,
    known_hosts: &Path,
    key_dir: Option<&Path>,
    prompt: Arc<dyn Prompt>,
) -> Result<Connection, SshError> {
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
        prompt: prompt.clone(),
        changed: changed.clone(),
        remote: remote.clone(),
    };
    let address = (target.host.as_str(), target.port);
    let mut handle = match client::connect(config, address, checker).await {
        Ok(handle) => handle,
        Err(error) => {
            if let Some(fingerprint) = changed.lock().ok().and_then(|mut slot| slot.take()) {
                return Err(SshError::HostKeyChanged {
                    host: known_hosts_name(&target.host, target.port),
                    fingerprint,
                });
            }
            return Err(match error {
                russh::Error::UnknownKey => SshError::HostKeyRejected,
                russh::Error::IO(io) => SshError::Unreachable(
                    known_hosts_name(&target.host, target.port),
                    io.to_string(),
                ),
                other => SshError::Protocol(other.to_string()),
            });
        }
    };

    let mut tried = Vec::new();
    let mut offered = String::from("unknown");
    if let Some(key_dir) = key_dir {
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let Ok(key) = russh::keys::load_secret_key(key_dir.join(name), None) else {
                continue;
            };
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            tried.push(format!("key {name}"));
            match handle.authenticate_publickey(&target.user, key).await? {
                AuthResult::Success => return Ok(Connection { handle, remote }),
                AuthResult::Failure {
                    remaining_methods, ..
                } => offered = format!("{remaining_methods:?}"),
            }
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
            AuthResult::Success => return Ok(Connection { handle, remote }),
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
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", columns, rows, 0, 0, &[])
            .await?;
        channel.request_shell(true).await?;
        Ok(Shell { channel })
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

/// An interactive shell channel.
pub struct Shell {
    channel: Channel<Msg>,
}

/// What came from the shell.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    /// Bytes for the terminal (stdout and stderr interleaved, as a PTY does).
    Data(Vec<u8>),
    /// The remote program exited or the channel closed.
    Closed,
}

impl Shell {
    /// Waits for the next output.
    pub async fn next(&mut self) -> Output {
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    return Output::Data(data.to_vec());
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => return Output::Closed,
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
