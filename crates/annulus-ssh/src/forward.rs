//! Port forwarding (F-FWD-001…003). Local and dynamic (SOCKS5) rules listen on a local
//! address and carry every accepted connection through its own `direct-tcpip` channel.
//! Remote rules ask the server to listen and carry each `forwarded-tcpip` channel it opens
//! to a destination reached from this computer.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use russh::Channel;
use russh::client::Msg;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::Connection;

/// What a rule forwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Connections to `bind` go to `host:port` as seen from the server.
    Local {
        /// The local address to listen on.
        bind: SocketAddr,
        /// The destination host, resolved by the server.
        host: String,
        /// The destination port.
        port: u16,
    },
    /// A SOCKS5 proxy on `bind`; each client names its own destination.
    Dynamic {
        /// The local address to listen on.
        bind: SocketAddr,
    },
    /// The server listens on `bind_host:bind_port`; connections there reach `host:port` as
    /// seen from this computer.
    Remote {
        /// The address the server listens on (`localhost` keeps it on the server).
        bind_host: String,
        /// The server port; 0 lets the server choose.
        bind_port: u16,
        /// The destination host, resolved here.
        host: String,
        /// The destination port.
        port: u16,
    },
}

/// Where a remote rule's connections go, with its connection counter.
#[derive(Clone)]
pub(crate) struct RemoteTarget {
    host: String,
    port: u16,
    active: Arc<AtomicUsize>,
}

/// Remote rules of one connection, by the server port they listen on.
pub(crate) type RemoteMap = Arc<std::sync::Mutex<HashMap<u32, RemoteTarget>>>;

/// Carries one `forwarded-tcpip` channel to its rule's destination.
pub(crate) async fn carry_remote(channel: Channel<Msg>, target: RemoteTarget) {
    target.active.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut socket) = TcpStream::connect((target.host.as_str(), target.port)).await {
        let mut stream = channel.into_stream();
        let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
    }
    target.active.fetch_sub(1, Ordering::Relaxed);
}

/// Why a rule could not start.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    /// Something else listens on the address (F-FWD-003).
    #[error("{0} is already in use by another program")]
    PortInUse(SocketAddr),
    /// The address cannot be listened on (no permission, not a local address, …).
    #[error("cannot listen on {0}: {1}")]
    Bind(SocketAddr, std::io::Error),
    /// The server refused to listen (port taken, or forwarding disabled in its config).
    #[error("the server refused to listen on {0}: {1}")]
    RemoteRefused(String, String),
}

/// A running rule. Dropping it stops listening; connections already open keep going
/// until either side closes them.
pub struct Forward {
    listening: Listening,
    active: Arc<AtomicUsize>,
    task: Option<JoinHandle<()>>,
}

/// Where a running rule listens.
enum Listening {
    Here(SocketAddr),
    /// On the server: the cancel request needs the connection, address and port.
    Server(Arc<Connection>, String, u32),
}

impl Forward {
    /// The local address actually listened on (the real port when the rule asked for port
    /// 0); `None` for remote rules.
    #[must_use]
    pub fn local(&self) -> Option<SocketAddr> {
        match self.listening {
            Listening::Here(address) => Some(address),
            Listening::Server(..) => None,
        }
    }

    /// The port the server listens on, for remote rules.
    #[must_use]
    pub fn remote_port(&self) -> Option<u32> {
        match self.listening {
            Listening::Here(_) => None,
            Listening::Server(_, _, port) => Some(port),
        }
    }

    /// Connections currently open through the rule.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        if let Listening::Server(connection, address, port) = &self.listening {
            if let Ok(mut map) = connection.remote.lock() {
                map.remove(port);
            }
            // Ask the server to stop listening; if the runtime is gone, so is the connection.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let (connection, address, port) = (connection.clone(), address.clone(), *port);
                runtime.spawn(async move {
                    let _ = connection.handle.cancel_tcpip_forward(address, port).await;
                });
            }
        }
    }
}

/// Starts `rule` over `connection`. Must be called inside a tokio runtime, which then
/// runs the listener and its connections.
///
/// # Errors
/// The local address is in use or cannot be listened on.
pub async fn start(connection: Arc<Connection>, rule: Rule) -> Result<Forward, ForwardError> {
    let bind = match &rule {
        Rule::Local { bind, .. } | Rule::Dynamic { bind } => *bind,
        Rule::Remote {
            bind_host,
            bind_port,
            host,
            port,
        } => return start_remote(connection, bind_host, *bind_port, host, *port).await,
    };
    let listener = TcpListener::bind(bind).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            ForwardError::PortInUse(bind)
        } else {
            ForwardError::Bind(bind, error)
        }
    })?;
    let local = listener.local_addr().unwrap_or(bind);
    let active = Arc::new(AtomicUsize::new(0));
    let counter = active.clone();
    let task = tokio::spawn(async move {
        loop {
            // One failed accept (too many open files, a reset before it was taken) is not the
            // end of the rule: wait a moment and keep listening.
            let Ok((socket, peer)) = listener.accept().await else {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            };
            let connection = connection.clone();
            let rule = rule.clone();
            let counter = counter.clone();
            counter.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                // A failed connection only ends itself; the rule keeps listening.
                let _ = carry(&connection, &rule, socket, peer).await;
                counter.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
    Ok(Forward {
        listening: Listening::Here(local),
        active,
        task: Some(task),
    })
}

async fn start_remote(
    connection: Arc<Connection>,
    bind_host: &str,
    bind_port: u16,
    host: &str,
    port: u16,
) -> Result<Forward, ForwardError> {
    let place = format!("{bind_host}:{bind_port}");
    let granted = connection
        .handle
        .tcpip_forward(bind_host, u32::from(bind_port))
        .await
        .map_err(|error| ForwardError::RemoteRefused(place, error.to_string()))?;
    // Servers answer 0 when the requested port was used as asked.
    let server_port = if granted == 0 {
        u32::from(bind_port)
    } else {
        granted
    };
    let active = Arc::new(AtomicUsize::new(0));
    if let Ok(mut map) = connection.remote.lock() {
        map.insert(
            server_port,
            RemoteTarget {
                host: host.to_owned(),
                port,
                active: active.clone(),
            },
        );
    }
    Ok(Forward {
        listening: Listening::Server(connection, bind_host.to_owned(), server_port),
        active,
        task: None,
    })
}

async fn carry(
    connection: &Connection,
    rule: &Rule,
    mut socket: TcpStream,
    peer: SocketAddr,
) -> std::io::Result<()> {
    let (host, port) = match rule {
        Rule::Local { host, port, .. } => (host.clone(), *port),
        Rule::Dynamic { .. } => socks5_request(&mut socket).await?,
        Rule::Remote { .. } => return Ok(()),
    };
    let channel = connection
        .handle
        .channel_open_direct_tcpip(
            host,
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await;
    let channel = match (channel, rule) {
        (Ok(channel), Rule::Dynamic { .. }) => {
            socket.write_all(&socks5_reply(SOCKS_OK)).await?;
            channel
        }
        (Ok(channel), Rule::Local { .. } | Rule::Remote { .. }) => channel,
        (Err(error), rule) => {
            if matches!(rule, Rule::Dynamic { .. }) {
                socket.write_all(&socks5_reply(SOCKS_REFUSED)).await?;
            }
            return Err(std::io::Error::other(error.to_string()));
        }
    };
    let mut stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut socket, &mut stream).await?;
    Ok(())
}

const SOCKS_OK: u8 = 0;
const SOCKS_REFUSED: u8 = 5;
const SOCKS_UNSUPPORTED_COMMAND: u8 = 7;
const SOCKS_UNSUPPORTED_ADDRESS: u8 = 8;

/// A SOCKS5 reply with `status` and an all-zero bound address.
fn socks5_reply(status: u8) -> [u8; 10] {
    [5, status, 0, 1, 0, 0, 0, 0, 0, 0]
}

fn invalid(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}

/// Runs the server side of a SOCKS5 greeting and CONNECT request (RFC 1928, no
/// authentication) and returns the destination. Anything else gets an error reply.
async fn socks5_request<S>(socket: &mut S) -> std::io::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(invalid("not SOCKS5"));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        // No acceptable method.
        socket.write_all(&[5, 0xFF]).await?;
        return Err(invalid("the client requires SOCKS authentication"));
    }
    socket.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await?;
    if request[0] != 5 {
        return Err(invalid("not SOCKS5"));
    }
    if request[1] != 1 {
        socket
            .write_all(&socks5_reply(SOCKS_UNSUPPORTED_COMMAND))
            .await?;
        return Err(invalid("only CONNECT is supported"));
    }
    let host = match request[3] {
        1 => {
            let mut octets = [0u8; 4];
            socket.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let length = socket.read_u8().await?;
            let mut name = vec![0u8; usize::from(length)];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| invalid("the host name is not UTF-8"))?
        }
        4 => {
            let mut octets = [0u8; 16];
            socket.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => {
            socket
                .write_all(&socks5_reply(SOCKS_UNSUPPORTED_ADDRESS))
                .await?;
            return Err(invalid("unknown address type"));
        }
    };
    let port = socket.read_u16().await?;
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn handshake(client_bytes: &[u8]) -> (std::io::Result<(String, u16)>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(client_bytes).await.unwrap();
        let result = socks5_request(&mut server).await;
        drop(server);
        let mut replies = Vec::new();
        client.read_to_end(&mut replies).await.unwrap();
        (result, replies)
    }

    #[tokio::test]
    async fn socks5_connect_by_name_ipv4_and_ipv6() {
        let by_name = [
            5, 1, 0, 5, 1, 0, 3, 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0, 80,
        ];
        let (result, replies) = handshake(&by_name).await;
        assert_eq!(result.unwrap(), ("example".to_owned(), 80));
        assert_eq!(replies, [5, 0]);

        let ipv4 = [5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0x1F, 0x90];
        assert_eq!(
            handshake(&ipv4).await.0.unwrap(),
            ("10.0.0.7".to_owned(), 8080)
        );

        let mut ipv6 = vec![5, 1, 0, 5, 1, 0, 4];
        ipv6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        ipv6.extend_from_slice(&[0, 22]);
        assert_eq!(handshake(&ipv6).await.0.unwrap(), ("::1".to_owned(), 22));
    }

    #[tokio::test]
    async fn socks5_refuses_authentication_only_clients_and_other_commands() {
        let (result, replies) = handshake(&[5, 1, 2]).await;
        assert!(result.is_err());
        assert_eq!(replies, [5, 0xFF]);

        // BIND (2) instead of CONNECT.
        let (result, replies) = handshake(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).await;
        assert!(result.is_err());
        assert_eq!(replies[2..], socks5_reply(SOCKS_UNSUPPORTED_COMMAND));
    }
}
