//! Reaching an SSH server through a proxy (F-SSH-015): SOCKS5 (RFC 1928, with optional
//! username/password, RFC 1929) and HTTP `CONNECT` (with optional Basic auth). A proxy is
//! written as a URL: `socks5://[user:pass@]host:port` or `http://[user:pass@]host:port`.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

/// Which protocol the proxy speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// SOCKS version 5.
    Socks5,
    /// HTTP `CONNECT`.
    Http,
}

/// A proxy to connect through.
#[derive(Clone, PartialEq, Eq)]
pub struct Proxy {
    /// The protocol.
    pub kind: ProxyKind,
    /// The proxy's host.
    pub host: String,
    /// The proxy's port.
    pub port: u16,
    /// Username and password, if the proxy wants them.
    pub login: Option<(String, Zeroizing<String>)>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("login", &self.login.as_ref().map(|(user, _)| user))
            .finish()
    }
}

impl Proxy {
    /// Reads `socks5://[user:pass@]host[:port]` or `http://[user:pass@]host[:port]`
    /// (default ports 1080 and 8080).
    #[must_use]
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        let (kind, rest, default_port) = match url.strip_prefix("socks5://") {
            Some(rest) => (ProxyKind::Socks5, rest, 1080),
            None => (ProxyKind::Http, url.strip_prefix("http://")?, 8080),
        };
        let rest = rest.trim_end_matches('/');
        let (login, address) = match rest.rsplit_once('@') {
            Some((login, address)) => {
                let (user, password) = login.split_once(':').unwrap_or((login, ""));
                (
                    Some((user.to_owned(), Zeroizing::new(password.to_owned()))),
                    address,
                )
            }
            None => (None, rest),
        };
        let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
            let (host, tail) = bracketed.split_once(']')?;
            let port = match tail.strip_prefix(':') {
                Some(port) => port.parse().ok()?,
                None => default_port,
            };
            (host.to_owned(), port)
        } else {
            match address.rsplit_once(':') {
                Some((host, port)) => (host.to_owned(), port.parse().ok()?),
                None => (address.to_owned(), default_port),
            }
        };
        if host.is_empty() || port == 0 {
            return None;
        }
        Some(Self {
            kind,
            host,
            port,
            login,
        })
    }

    /// Opens a TCP stream to `host:port` through this proxy.
    ///
    /// # Errors
    /// The proxy is unreachable, refuses, or answers with something unexpected.
    pub async fn connect(&self, host: &str, port: u16) -> std::io::Result<TcpStream> {
        let mut stream = TcpStream::connect((self.host.as_str(), self.port)).await?;
        match self.kind {
            ProxyKind::Socks5 => self.socks5(&mut stream, host, port).await?,
            ProxyKind::Http => self.http(&mut stream, host, port).await?,
        }
        Ok(stream)
    }

    async fn socks5(&self, stream: &mut TcpStream, host: &str, port: u16) -> std::io::Result<()> {
        let method = if self.login.is_some() { 2 } else { 0 };
        stream.write_all(&[5, 1, method]).await?;
        let mut choice = [0u8; 2];
        stream.read_exact(&mut choice).await?;
        if choice != [5, method] {
            return Err(refused("the SOCKS proxy wants another login method"));
        }
        if let Some((user, password)) = &self.login {
            let (user, password) = (user.as_bytes(), password.as_bytes());
            let too_long =
                |len: usize| u8::try_from(len).map_err(|_| refused("the proxy login is too long"));
            let mut request = vec![1, too_long(user.len())?];
            request.extend_from_slice(user);
            request.push(too_long(password.len())?);
            request.extend_from_slice(password);
            stream.write_all(&request).await?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await?;
            if status[1] != 0 {
                return Err(refused("the SOCKS proxy refused the login"));
            }
        }
        let name = host.as_bytes();
        let length = u8::try_from(name.len()).map_err(|_| refused("the host name is too long"))?;
        let mut request = vec![5, 1, 0, 3, length];
        request.extend_from_slice(name);
        request.extend_from_slice(&port.to_be_bytes());
        stream.write_all(&request).await?;
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await?;
        if reply[1] != 0 {
            return Err(refused(&format!(
                "the SOCKS proxy could not connect (code {})",
                reply[1]
            )));
        }
        // Skip the bound address in the reply.
        let address_len = match reply[3] {
            1 => 4,
            4 => 16,
            3 => usize::from(stream.read_u8().await?),
            _ => return Err(refused("the SOCKS proxy sent an unknown address type")),
        };
        let mut skip = vec![0u8; address_len + 2];
        stream.read_exact(&mut skip).await?;
        Ok(())
    }

    async fn http(&self, stream: &mut TcpStream, host: &str, port: u16) -> std::io::Result<()> {
        let target = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let mut request =
            Zeroizing::new(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n"));
        if let Some((user, password)) = &self.login {
            let credentials = Zeroizing::new(format!("{user}:{}", password.as_str()));
            request.push_str("Proxy-Authorization: Basic ");
            request.push_str(&base64(credentials.as_bytes()));
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await?;
        // Read the response head byte by byte, so nothing of the SSH stream after it is lost.
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 8192 {
                return Err(refused("the HTTP proxy's answer is too long"));
            }
            head.push(stream.read_u8().await?);
        }
        let status_line = String::from_utf8_lossy(&head);
        let status = status_line.split_whitespace().nth(1).unwrap_or_default();
        if status == "200" {
            Ok(())
        } else {
            Err(refused(&format!("the HTTP proxy answered {status}")))
        }
    }
}

fn refused(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::ConnectionRefused, message.to_owned())
}

/// Standard base64, for the Basic auth header.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls_parse_with_defaults_and_logins() {
        let socks = Proxy::parse("socks5://bastion.example.com").unwrap();
        assert_eq!(
            (socks.kind, socks.host.as_str(), socks.port),
            (ProxyKind::Socks5, "bastion.example.com", 1080)
        );
        let http = Proxy::parse("http://ana:s3cret@10.0.0.1:3128/").unwrap();
        assert_eq!((http.kind, http.port), (ProxyKind::Http, 3128));
        assert_eq!(
            http.login
                .as_ref()
                .map(|(user, pw)| (user.as_str(), pw.as_str())),
            Some(("ana", "s3cret"))
        );
        assert!(!format!("{http:?}").contains("s3cret"));
        assert_eq!(Proxy::parse("socks5://[::1]:9050").unwrap().host, "::1");
        assert!(Proxy::parse("ftp://x").is_none());
        assert!(Proxy::parse("http://:80").is_none());
    }

    #[test]
    fn basic_auth_is_standard_base64() {
        assert_eq!(
            base64(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b""), "");
    }

    #[tokio::test]
    async fn an_http_proxy_tunnels_and_a_refusal_is_reported() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            for answer in [
                &b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0-test\r\n"[..],
                b"HTTP/1.1 403 Forbidden\r\n\r\n",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0u8; 512];
                let read = socket.read(&mut request).await.unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..read])
                        .starts_with("CONNECT db.internal:22 HTTP/1.1\r\n")
                );
                socket.write_all(answer).await.unwrap();
            }
        });
        let proxy = Proxy::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let mut stream = proxy.connect("db.internal", 22).await.unwrap();
        let mut banner = [0u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(
            &banner, b"SSH-2.0-test\r\n",
            "nothing after the head is lost"
        );
        assert!(proxy.connect("db.internal", 22).await.is_err());
    }
}
