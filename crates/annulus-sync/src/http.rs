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
    Change, Changes, Devices, Failure, LoginFinish, LoginStart, LoginStartResponse,
    PROTOCOL_VERSION, Push, PushResult, RegisterFinish, RegisterStart, RegisterStartResponse,
    Revoke, Session, VERSION_HEADER, decode, encode, path,
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
    /// matching the server's own rule).
    pub fn new(base: &str, token: Option<String>) -> Result<Self, SyncError> {
        let base = base.trim().trim_end_matches('/').to_owned();
        if !(base.starts_with("https://") || is_loopback_http(&base)) {
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
            .timeout_global(Some(Duration::from_secs(30)))
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
        let url = format!("{}{route}", self.base);
        let version = PROTOCOL_VERSION.to_string();
        let authorization = self.token.as_ref().map(|token| format!("Bearer {token}"));
        let response = if let Some(body) = body {
            let mut request = self.agent.post(&url).header(VERSION_HEADER, &version);
            if let Some(value) = &authorization {
                request = request.header("Authorization", value);
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

    /// Signs `device` out.
    ///
    /// # Errors
    /// Unreachable or signed out.
    pub fn revoke(&self, device: u64) -> Result<(), SyncError> {
        self.post(path::REVOKE, &Revoke { device })
    }
}

/// `http://localhost…` or `http://127.x…`/`http://[::1]…`, for a server on this computer.
fn is_loopback_http(base: &str) -> bool {
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map_or(host, |(name, _)| name)
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
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
    ) -> Result<RegisterStartResponse, SyncError> {
        self.post(path::REGISTER_START, request)
    }

    fn register_finish(&mut self, request: &RegisterFinish) -> Result<Session, SyncError> {
        self.post(path::REGISTER_FINISH, request)
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
    }
}
