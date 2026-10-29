//! A self-hosted server's own certificate, trusted by its fingerprint (ADR-0018).
//!
//! A server on a home network has no certificate from a public authority. The first time,
//! the app reads the certificate the server shows ([`certificate_of`]) and asks the user to
//! compare its fingerprint with the one `annulus-server fingerprint` prints on the server.
//! After a yes, that certificate is the only one trusted for the account
//! ([`crate::http::HttpServer::pinned`]), checked like any root by rustls on every request,
//! so a different one later stops sync instead of being asked about again.

use std::io::Read as _;
use std::net::{TcpStream, ToSocketAddrs as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::engine::SyncError;

pub use annulus_proto::fingerprint;

/// The certificate to pin for the server at `base` (an `https://` URL): the last one it
/// sends, its own self-signed certificate or the top of its chain. Nothing is trusted by
/// reading it; it is only shown to the user.
///
/// # Errors
/// The address does not parse, or the server cannot be reached or does not finish a TLS
/// handshake.
pub fn certificate_of(base: &str) -> Result<Vec<u8>, SyncError> {
    let failed =
        |what: &str| SyncError::Transport(format!("could not read the certificate: {what}"));
    let uri: ureq::http::Uri = base.trim().parse().map_err(|_| failed("not an address"))?;
    let host = uri.host().ok_or_else(|| failed("no host"))?;
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(443);
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| failed(&error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Shown {
            provider,
            seen: seen.clone(),
        }))
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).map_err(|_| failed("not a host name"))?;
    let mut tls = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|error| failed(&error.to_string()))?;
    let address = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| failed(&error.to_string()))?
        .next()
        .ok_or_else(|| failed("the name has no address"))?;
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(10))
        .map_err(|error| failed(&error.to_string()))?;
    let _ = socket.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = socket.set_write_timeout(Some(Duration::from_secs(10)));
    while tls.is_handshaking() {
        tls.complete_io(&mut socket)
            .map_err(|error| failed(&error.to_string()))?;
    }
    // A clean goodbye; the connection is not used.
    tls.send_close_notify();
    let _ = tls.write_tls(&mut socket);
    let _ = socket.read(&mut [0; 1]);
    let chain = std::mem::take(&mut *seen.lock().map_err(|_| failed("internal"))?);
    chain
        .last()
        .map(|der| der.as_ref().to_vec())
        .ok_or_else(|| failed("the server sent none"))
}

/// Accepts whatever certificate the server shows and keeps it (the user decides on its
/// fingerprint), while still checking that the server holds its key.
#[derive(Debug)]
struct Shown {
    provider: Arc<CryptoProvider>,
    seen: Arc<Mutex<Vec<CertificateDer<'static>>>>,
}

impl ServerCertVerifier for Shown {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut seen) = self.seen.lock() {
            *seen = std::iter::once(end_entity)
                .chain(intermediates)
                .map(|der| der.clone().into_owned())
                .collect();
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
