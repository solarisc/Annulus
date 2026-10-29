//! The sync server over HTTPS: postcard bodies, the protocol version header and the device
//! token. TLS is rustls on aws-lc-rs (the crypto library SSH already uses) with the Mozilla
//! root certificates.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::Agent;
use ureq::tls::{TlsConfig, TlsProvider};
use annulus_proto::{
    Change, Changes, Devices, Failure, History, INVITE_HEADER, LoginFinish, LoginStart,
    LoginStartResponse, PROTOCOL_VERSION, PasswordBegin, PasswordBeginResponse, PasswordChange,
    Push, PushResult, Record, RegisterFinish, RegisterStart, RegisterStartResponse, Revoke,
    Session, VERSION_HEADER, decode, encode, path,
};

use crate::account::Auth;
use crate::engine::{SyncError, Transport};

/// A sync server at a base URL such as `https://sync.example.com`.
pub struct HttpServer {
    base: String,
    agent: Agent,
    token: Option<String>,
}

impl HttpServer {
    /// A connection to `base`, signed in with `token` if there is one.
    ///
    /// # Errors
    /// `base` is not an `https://` URL (plain `http://` is allowed for loopback only,
    /// matching the server's own rule), or it names a user (`user@host`).
    pub fn new(base: &str, token: Option<String>) -> Result<Self, SyncError> {
        let base = base.trim().trim_end_matches('/').to_owned();
        if !allowed(&base) {
            return Err(SyncError::Transport(
                "the server address must start with https://".into(),
            ));
        }
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .build();
        let agent: Agent = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(40)))
            .build()
            .into();
        Ok(Self { base, agent, token })
    }

    /// Signs later requests with `token`.
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    fn request<T: DeserializeOwned>(
        &self,
        route: &str,
        body: Option<Vec<u8>>,
    ) -> Result<T, SyncError> {
        self.request_invited(route, body, None)
    }

    /// A request that may carry an invite code (registration only).
    fn request_invited<T: DeserializeOwned>(
        &self,
        route: &str,
        body: Option<Vec<u8>>,
        invite: Option<&str>,
    ) -> Result<T, SyncError> {
        let url = format!("{}{route}", self.base);
        let version = PROTOCOL_VERSION.to_string();
        let authorization = self.token.as_ref().map(|token| format!("Bearer {token}"));
        let response = if let Some(body) = body {
            let mut request = self.agent.post(&url).header(VERSION_HEADER, &version);
            if let Some(value) = &authorization {
                request = request.header("Authorization", value);
            }
            if let Some(code) = invite.map(str::trim).filter(|code| !code.is_empty()) {
                request = request.header(INVITE_HEADER, code);
            }
            request.send(&body[..])
        } else {
            let mut request = self.agent.get(&url).header(VERSION_HEADER, &version);
            if let Some(value) = &authorization {
                request = request.header("Authorization", value);
            }
            request.call()
        };
        let mut response = response.map_err(|error| SyncError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .map_err(|error| SyncError::Transport(error.to_string()))?;
        match status {
            200 => decode(&bytes).map_err(|error| SyncError::Transport(error.to_string())),
            401 => Err(SyncError::Unauthorized),
            _ => Err(SyncError::Transport(decode::<Failure>(&bytes).map_or_else(
                |_| format!("the server answered {status}"),
                |failure| failure.message,
            ))),
        }
    }

    fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        route: &str,
        body: &B,
    ) -> Result<T, SyncError> {
        self.request(route, Some(encode(body)))
    }

    /// The account's signed-in devices.
    ///
    /// # Errors
    /// Unreachable or signed out.
    pub fn devices(&self) -> Result<Devices, SyncError> {
        self.request(path::DEVICES, None)
    }

    /// Password change step 1 (see [`crate::account::change_password`]).
    ///
    /// # Errors
    /// Unreachable, signed out, or throttled.
    pub fn password_begin(
        &self,
        request: &PasswordBegin,
    ) -> Result<PasswordBeginResponse, SyncError> {
        self.post(path::PASSWORD_BEGIN, request)
    }

    /// Password change step 2.
    ///
    /// # Errors
    /// Unreachable, signed out, or the current password was not proven.
    pub fn password_change(&self, request: &PasswordChange) -> Result<(), SyncError> {
        self.post(path::PASSWORD_CHANGE, request)
    }

    /// Waits (up to about 25 s) until the account has changes newer than `since`; returns
    /// the newest sequence number, which equals `since` when nothing changed.
    ///
    /// # Errors
    /// Unreachable or signed out.
    pub fn wait(&self, since: u64) -> Result<u64, SyncError> {
        let latest: annulus_proto::Latest =
            self.request(&format!("{}?since={since}", path::WAIT), None)?;
        Ok(latest.latest)
    }

    /// Earlier versions of record `id`, newest first.
    ///
    /// # Errors
    /// Unreachable or signed out.
    pub fn history(&self, id: &str) -> Result<Vec<Record>, SyncError> {
        let history: History = self.request(&format!("{}?id={id}", path::HISTORY), None)?;
        Ok(history.records)
    }

    /// Signs `device` out.
    ///
    /// # Errors
    /// Unreachable or signed out.
    pub fn revoke(&self, device: u64) -> Result<(), SyncError> {
        self.post(path::REVOKE, &Revoke { device })
    }
}

/// Whether the client may connect to `base`: any `https://` server, and plain `http://`
/// only to this computer (`localhost`, `127.x`, `[::1]`). The host is the parsed one, so
/// `http://localhost:80@evil.com` (user `localhost`, host `evil.com`) is not loopback; a
/// URL that names a user is refused outright.
fn allowed(base: &str) -> bool {
    let Ok(uri) = base.parse::<ureq::http::Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    if authority.as_str().contains('@') {
        return false;
    }
    match uri.scheme_str() {
        Some("https") => true,
        Some("http") => {
            let host = authority.host();
            let host = host
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(host);
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

impl Transport for HttpServer {
    fn push(&mut self, changes: &[Change]) -> Result<PushResult, SyncError> {
        self.post(
            path::RECORDS,
            &Push {
                changes: changes.to_vec(),
            },
        )
    }

    fn pull(&mut self, since: u64) -> Result<Changes, SyncError> {
        self.request(&format!("{}?since={since}", path::RECORDS), None)
    }
}

impl Auth for HttpServer {
    fn register_start(
        &mut self,
        request: &RegisterStart,
        invite: Option<&str>,
    ) -> Result<RegisterStartResponse, SyncError> {
        self.request_invited(path::REGISTER_START, Some(encode(request)), invite)
    }

    fn register_finish(
        &mut self,
        request: &RegisterFinish,
        invite: Option<&str>,
    ) -> Result<Session, SyncError> {
        self.request_invited(path::REGISTER_FINISH, Some(encode(request)), invite)
    }

    fn login_start(&mut self, request: &LoginStart) -> Result<LoginStartResponse, SyncError> {
        self.post(path::LOGIN_START, request)
    }

    fn login_finish(&mut self, request: &LoginFinish) -> Result<Session, SyncError> {
        self.post(path::LOGIN_FINISH, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_only_for_this_computer() {
        assert!(HttpServer::new("https://sync.example.com/", None).is_ok());
        assert!(HttpServer::new("http://127.0.0.1:8443", None).is_ok());
        assert!(HttpServer::new("http://localhost:8443", None).is_ok());
        assert!(HttpServer::new("http://[::1]:8443", None).is_ok());
        assert!(HttpServer::new("http://sync.example.com", None).is_err());
        assert!(HttpServer::new("http://127.0.0.1.evil.com", None).is_err());
        assert!(HttpServer::new("ftp://x", None).is_err());
        // The host is what follows a user name: these are evil.com, over plain HTTP.
        assert!(HttpServer::new("http://localhost:80@evil.com", None).is_err());
        assert!(HttpServer::new("http://127.0.0.1@evil.com:8443", None).is_err());
        assert!(HttpServer::new("http://[::1]@evil.com", None).is_err());
        assert!(HttpServer::new("https://user@sync.example.com", None).is_err());
        assert!(HttpServer::new("http://localhost.evil.com", None).is_err());
        assert!(HttpServer::new("http://", None).is_err());
    }
}
