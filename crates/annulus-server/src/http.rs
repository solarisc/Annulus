//! HTTP in front of [`Server`]: postcard bodies, the protocol version header, and bearer
//! device tokens.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use annulus_proto::{
    Failure, History, INVITE_HEADER, Latest, LoginFinish, LoginStart, PROTOCOL_VERSION,
    PasswordBegin, PasswordChange, Push, RegisterFinish, RegisterStart, Revoke, VERSION_HEADER,
    decode, encode, path,
};
use axum::Router;
use axum::body::Bytes;
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::store::{Caller, Server, ServerError};

/// One SQLite connection backs the service. Queue for it here instead of filling Tokio's
/// blocking pool with threads that wait on the same connection lock.
static STORE_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// The HTTP API over `server`, ready to serve: handlers see the client's address. With
/// `behind_proxy` (a TLS proxy in front), the address comes from `X-Forwarded-For`.
pub fn service(
    server: Arc<Server>,
    behind_proxy: bool,
    trusted_proxies: Vec<IpAddr>,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    router(server)
        .layer(axum::Extension(ProxyTrust {
            enabled: behind_proxy,
            addresses: trusted_proxies.into(),
        }))
        .into_make_service_with_connect_info::<SocketAddr>()
}

/// Whether a proxy in front sets `X-Forwarded-For`, and which non-loopback peers may be it.
#[derive(Clone)]
struct ProxyTrust {
    enabled: bool,
    addresses: Arc<[IpAddr]>,
}

/// Whether `ip` is this machine or one of the proxy addresses the admin named.
fn trusted_hop(ip: IpAddr, trusted: &[IpAddr]) -> bool {
    let ip = ip.to_canonical();
    ip.is_loopback() || trusted.iter().any(|known| known.to_canonical() == ip)
}

/// The client's address. Without a proxy in front it is the connection's. Behind one, the
/// `X-Forwarded-For` hops are walked from the right, the proxies' end, past every trusted
/// proxy: the first other one is the client. Entries to its
/// left were written by the client itself, so a client cannot pick its own address, and
/// a client on the internet that connects directly is never believed at all.
fn client_address(peer: SocketAddr, headers: &HeaderMap, trust: &ProxyTrust) -> IpAddr {
    let mut client = peer.ip().to_canonical();
    if !trust.enabled || !trusted_hop(client, &trust.addresses) {
        return client;
    }
    let hops: Vec<&str> = headers
        .get_all("X-Forwarded-For")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .collect();
    for hop in hops.iter().rev() {
        // Something unreadable: the last hop that could be read is as far as is known.
        let Ok(ip) = hop.trim().parse::<IpAddr>() else {
            break;
        };
        client = ip.to_canonical();
        if !trusted_hop(client, &trust.addresses) {
            break;
        }
    }
    client
}

/// The invite code a registration step carries, if any.
fn invite(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(INVITE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|code| !code.is_empty())
}

/// The HTTP API over `server`.
fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route(path::HEALTH, get(|| async { "ok" }))
        .route(path::REGISTER_START, post(register_start))
        .route(path::REGISTER_FINISH, post(register_finish))
        .route(path::LOGIN_START, post(login_start))
        .route(path::LOGIN_FINISH, post(login_finish))
        // Only a signed-in device's push may be large, and the token is checked before any
        // of the body is read; everything else stays small.
        .route(
            path::RECORDS,
            get(pull)
                .post(push)
                .layer(axum::extract::DefaultBodyLimit::max(annulus_proto::MAX_BODY))
                .layer(axum::middleware::from_fn_with_state(
                    server.clone(),
                    signed_in_first,
                )),
        )
        .route(path::HISTORY, get(history))
        .route(path::WAIT, get(wait))
        .route(path::PASSWORD_BEGIN, post(password_begin))
        .route(path::PASSWORD_CHANGE, post(password_change))
        .route(path::DEVICES, get(devices))
        .route(path::REVOKE, post(revoke))
        .layer(axum::extract::DefaultBodyLimit::max(
            annulus_proto::MAX_SMALL_BODY,
        ))
        .layer(axum::middleware::from_fn(within_deadline))
        .with_state(server)
}

/// How long one request may take, its body included. A long poll ([`WAIT_FOR`]) answers well
/// within it, and a device's push batch is 1 MiB at most; a client that trickles a body loses
/// its connection instead of holding it, and a file handle, for good.
const REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

async fn within_deadline(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match tokio::time::timeout(REQUEST_DEADLINE, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

/// Refuses a request without a valid device token before its body is read, so a stranger
/// cannot make the server take in a large push.
async fn signed_in_first(
    State(server): State<Arc<Server>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match caller(server, request.headers()).await {
        Ok(_) => next.run(request).await,
        Err(refused) => *refused,
    }
}

fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    let body = encode(&Failure {
        message: message.into(),
    });
    (status, body).into_response()
}

fn reply<T: Serialize>(result: Result<T, ServerError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, encode(&value)).into_response(),
        Err(error) => {
            let status = match &error {
                ServerError::BadRequest(_) => StatusCode::BAD_REQUEST,
                ServerError::Refused(_) => StatusCode::FORBIDDEN,
                ServerError::Throttled => StatusCode::TOO_MANY_REQUESTS,
                ServerError::Unauthorized => StatusCode::UNAUTHORIZED,
                ServerError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            // Storage details stay in the server; clients get a generic message.
            let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
                "the server could not complete the request".to_owned()
            } else {
                error.to_string()
            };
            failure(status, message)
        }
    }
}

/// Checks the version header and decodes the body.
fn read<T: DeserializeOwned>(headers: &HeaderMap, body: &[u8]) -> Result<T, Box<Response>> {
    check_version(headers)?;
    decode(body).map_err(|error| Box::new(failure(StatusCode::BAD_REQUEST, error.to_string())))
}

fn check_version(headers: &HeaderMap) -> Result<(), Box<Response>> {
    let version = headers
        .get(VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u32>().ok());
    if version == Some(PROTOCOL_VERSION) {
        Ok(())
    } else {
        Err(Box::new(failure(
            StatusCode::BAD_REQUEST,
            format!("this server speaks protocol version {PROTOCOL_VERSION}; update the app"),
        )))
    }
}

/// Runs one blocking store call away from Tokio's request threads.
async fn store_call<T: Send + 'static>(
    server: Arc<Server>,
    work: impl FnOnce(&Server) -> Result<T, ServerError> + Send + 'static,
) -> Result<T, ServerError> {
    let permit = STORE_SLOT
        .acquire()
        .await
        .map_err(|_| ServerError::Storage("the request queue stopped".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(&server)
    })
    .await
    .map_err(|_| ServerError::Storage("a request worker stopped".into()))?
}

async fn caller(server: Arc<Server>, headers: &HeaderMap) -> Result<Caller, Box<Response>> {
    check_version(headers)?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned)
        .ok_or_else(|| Box::new(reply::<()>(Err(ServerError::Unauthorized))))?;
    store_call(server, move |server| server.authenticate(&token))
        .await
        .map_err(|error| Box::new(reply::<()>(Err(error))))
}

async fn register_start(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<RegisterStart>(&headers, &body) {
        Ok(request) => {
            let invite = invite(&headers).map(str::to_owned);
            reply(
                store_call(server, move |server| {
                    server.register_start(&request, invite.as_deref())
                })
                .await,
            )
        }
        Err(response) => *response,
    }
}

async fn register_finish(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<RegisterFinish>(&headers, &body) {
        Ok(request) => {
            let invite = invite(&headers).map(str::to_owned);
            reply(
                store_call(server, move |server| {
                    server.register_finish(&request, invite.as_deref())
                })
                .await,
            )
        }
        Err(response) => *response,
    }
}

async fn login_start(
    State(server): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    axum::Extension(trust): axum::Extension<ProxyTrust>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<LoginStart>(&headers, &body) {
        Ok(request) => {
            let address = client_address(peer, &headers, &trust);
            reply(
                store_call(server, move |server| {
                    server.login_start(&request, Some(address))
                })
                .await,
            )
        }
        Err(response) => *response,
    }
}

async fn login_finish(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<LoginFinish>(&headers, &body) {
        Ok(request) => reply(store_call(server, move |server| server.login_finish(&request)).await),
        Err(response) => *response,
    }
}

async fn push(State(server): State<Arc<Server>>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<Push>(&body) {
        Ok(request) => {
            reply(store_call(server, move |server| server.push(caller, &request.changes)).await)
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn pull(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    let since = query
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("since="))
        .map_or(Some(0), |value| value.parse::<u64>().ok());
    match since {
        Some(since) => reply(store_call(server, move |server| server.pull(caller, since)).await),
        None => failure(StatusCode::BAD_REQUEST, "since must be a number"),
    }
}

/// How long a wait request is held open before answering with no news.
const WAIT_FOR: std::time::Duration = std::time::Duration::from_secs(25);

async fn wait(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    let since = query
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("since="))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut receiver = match store_call(server, move |server| server.watch(caller)).await {
        Ok(receiver) => receiver,
        Err(error) => return reply::<()>(Err(error)),
    };
    let newer = |latest: &u64| *latest > since;
    let _ = tokio::time::timeout(WAIT_FOR, receiver.wait_for(newer)).await;
    let latest = *receiver.borrow();
    reply(Ok(Latest { latest }))
}

async fn history(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    let id = query
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("id="))
        .unwrap_or_default();
    if id.is_empty() || id.len() > 128 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return failure(StatusCode::BAD_REQUEST, "id must be a record id");
    }
    let id = id.to_owned();
    reply(
        store_call(server, move |server| server.history(caller, &id))
            .await
            .map(|records| History { records }),
    )
}

async fn password_begin(
    State(server): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    axum::Extension(trust): axum::Extension<ProxyTrust>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<PasswordBegin>(&body) {
        Ok(request) => {
            let address = client_address(peer, &headers, &trust);
            reply(
                store_call(server, move |server| {
                    server.password_begin(caller, &request, Some(address))
                })
                .await,
            )
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn password_change(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<PasswordChange>(&body) {
        Ok(request) => reply(
            store_call(server, move |server| {
                server.password_change(caller, &request)
            })
            .await,
        ),
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn devices(State(server): State<Arc<Server>>, headers: HeaderMap) -> Response {
    match caller(server.clone(), &headers).await {
        Ok(caller) => reply(store_call(server, move |server| server.devices(caller)).await),
        Err(response) => *response,
    }
}

async fn revoke(State(server): State<Arc<Server>>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match caller(server.clone(), &headers).await {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<Revoke>(&body) {
        Ok(request) => {
            reply(store_call(server, move |server| server.revoke(caller, request.device)).await)
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(lines: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append("X-Forwarded-For", line.parse().unwrap());
        }
        headers
    }

    #[test]
    fn forwarded_addresses_are_trusted_only_from_a_local_proxy() {
        let proxy = SocketAddr::from(([172, 18, 0, 2], 40000));
        let internet = SocketAddr::from(([198, 51, 100, 4], 40000));
        let client = IpAddr::from([203, 0, 113, 9]);
        let headers = forwarded(&["203.0.113.9, 10.0.0.1"]);
        let trusted = ProxyTrust {
            enabled: true,
            addresses: vec![proxy.ip(), IpAddr::from([10, 0, 0, 1])].into(),
        };
        assert_eq!(client_address(proxy, &headers, &trusted), client);
        assert_eq!(client_address(internet, &headers, &trusted), internet.ip());
        assert_eq!(
            client_address(proxy, &HeaderMap::new(), &trusted),
            proxy.ip()
        );
        // Without --behind-proxy the header is never read, even from a private address.
        let disabled = ProxyTrust {
            enabled: false,
            addresses: trusted.addresses.clone(),
        };
        assert_eq!(client_address(proxy, &headers, &disabled), proxy.ip());
        // A private peer not named by the admin is a client, not a proxy.
        let unnamed = SocketAddr::from(([172, 18, 0, 3], 40000));
        assert_eq!(client_address(unnamed, &headers, &trusted), unnamed.ip());
    }

    #[test]
    fn a_client_cannot_pick_its_address_by_sending_its_own_forwarded_header() {
        let proxy = SocketAddr::from(([127, 0, 0, 1], 40000));
        let client = IpAddr::from([203, 0, 113, 9]);
        let trusted = ProxyTrust {
            enabled: true,
            addresses: Arc::default(),
        };
        // The client sent "1.2.3.4"; the proxy appended the address it saw.
        let headers = forwarded(&["1.2.3.4, 203.0.113.9"]);
        assert_eq!(client_address(proxy, &headers, &trusted), client);
        // The same with the client's header and the proxy's on separate lines.
        let headers = forwarded(&["1.2.3.4", "203.0.113.9"]);
        assert_eq!(client_address(proxy, &headers, &trusted), client);
        // Garbage to the left of the real hop is ignored; garbage at the proxy's end
        // leaves only the proxy as known.
        let headers = forwarded(&["nonsense, 203.0.113.9"]);
        assert_eq!(client_address(proxy, &headers, &trusted), client);
        let headers = forwarded(&["203.0.113.9, nonsense"]);
        assert_eq!(client_address(proxy, &headers, &trusted), proxy.ip());
        // A client on the private network is kept as the client; it is not skipped merely
        // because its address is private.
        let headers = forwarded(&["10.1.1.20"]);
        assert_eq!(
            client_address(proxy, &headers, &trusted),
            IpAddr::from([10, 1, 1, 20])
        );
    }

    #[tokio::test]
    async fn store_work_runs_off_the_async_request_thread() {
        let request_thread = std::thread::current().id();
        let server = Arc::new(
            Server::open(std::path::Path::new(":memory:"), crate::Signups::Closed).unwrap(),
        );
        let store_thread = store_call(server.clone(), |_| Ok(std::thread::current().id()))
            .await
            .unwrap();
        assert_ne!(store_thread, request_thread);

        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let work = |active: Arc<std::sync::atomic::AtomicUsize>,
                    peak: Arc<std::sync::atomic::AtomicUsize>| {
            move |_: &Server| {
                let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(20));
                active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        };
        let first = store_call(server.clone(), work(active.clone(), peak.clone()));
        let second = store_call(server, work(active, peak.clone()));
        let (first, second) = tokio::join!(first, second);
        first.unwrap();
        second.unwrap();
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
