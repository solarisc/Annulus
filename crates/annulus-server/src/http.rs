//! HTTP in front of [`Server`]: postcard bodies, the protocol version header, and bearer
//! device tokens.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;
use serde::de::DeserializeOwned;
use annulus_proto::{
    Failure, History, Latest, LoginFinish, LoginStart, PROTOCOL_VERSION, Push, RegisterFinish,
    RegisterStart, Revoke, VERSION_HEADER, decode, encode, path,
};

use crate::store::{Caller, Server, ServerError};

/// The HTTP API over `server`, ready to serve: handlers see the client's address.
pub fn service(server: Arc<Server>) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    router(server).into_make_service_with_connect_info::<SocketAddr>()
}

/// The client's address: the connection's, or the first `X-Forwarded-For` entry when the
/// connection comes from a proxy on this machine or a private network (Caddy, nginx, a
/// container network). A client on the internet cannot pick its own address this way.
fn client_address(peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let from_proxy = match peer.ip() {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xFE00) == 0xFC00,
    };
    from_proxy
        .then(|| headers.get("X-Forwarded-For"))
        .flatten()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|first| first.trim().parse().ok())
        .unwrap_or(peer.ip())
}

/// The HTTP API over `server`.
fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route(path::HEALTH, get(|| async { "ok" }))
        .route(path::REGISTER_START, post(register_start))
        .route(path::REGISTER_FINISH, post(register_finish))
        .route(path::LOGIN_START, post(login_start))
        .route(path::LOGIN_FINISH, post(login_finish))
        .route(path::RECORDS, get(pull).post(push))
        .route(path::HISTORY, get(history))
        .route(path::WAIT, get(wait))
        .route(path::PASSWORD_START, post(password_start))
        .route(path::PASSWORD_FINISH, post(password_finish))
        .route(path::DEVICES, get(devices))
        .route(path::REVOKE, post(revoke))
        .layer(axum::extract::DefaultBodyLimit::max(
            annulus_proto::MAX_BODY,
        ))
        .with_state(server)
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

fn caller(server: &Server, headers: &HeaderMap) -> Result<Caller, Box<Response>> {
    check_version(headers)?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| Box::new(reply::<()>(Err(ServerError::Unauthorized))))?;
    server
        .authenticate(token)
        .map_err(|error| Box::new(reply::<()>(Err(error))))
}

async fn register_start(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<RegisterStart>(&headers, &body) {
        Ok(request) => reply(server.register_start(&request)),
        Err(response) => *response,
    }
}

async fn register_finish(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<RegisterFinish>(&headers, &body) {
        Ok(request) => reply(server.register_finish(&request)),
        Err(response) => *response,
    }
}

async fn login_start(
    State(server): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<LoginStart>(&headers, &body) {
        Ok(request) => reply(server.login_start(&request, Some(client_address(peer, &headers)))),
        Err(response) => *response,
    }
}

async fn login_finish(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match read::<LoginFinish>(&headers, &body) {
        Ok(request) => reply(server.login_finish(&request)),
        Err(response) => *response,
    }
}

async fn push(State(server): State<Arc<Server>>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match caller(&server, &headers) {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<Push>(&body) {
        Ok(request) => reply(server.push(caller, &request.changes)),
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn pull(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let caller = match caller(&server, &headers) {
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
        Some(since) => reply(server.pull(caller, since)),
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
    let caller = match caller(&server, &headers) {
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
    let mut receiver = match server.watch(caller) {
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
    let caller = match caller(&server, &headers) {
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
    reply(
        server
            .history(caller, id)
            .map(|records| History { records }),
    )
}

async fn password_start(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match caller(&server, &headers) {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<RegisterStart>(&body) {
        Ok(request) => reply(server.password_start(caller, &request)),
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn password_finish(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match caller(&server, &headers) {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<RegisterFinish>(&body) {
        Ok(request) => reply(server.password_finish(caller, &request)),
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn devices(State(server): State<Arc<Server>>, headers: HeaderMap) -> Response {
    match caller(&server, &headers) {
        Ok(caller) => reply(server.devices(caller)),
        Err(response) => *response,
    }
}

async fn revoke(State(server): State<Arc<Server>>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match caller(&server, &headers) {
        Ok(caller) => caller,
        Err(response) => return *response,
    };
    match decode::<Revoke>(&body) {
        Ok(request) => reply(server.revoke(caller, request.device)),
        Err(error) => failure(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_addresses_are_trusted_only_from_a_local_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", "203.0.113.9, 10.0.0.1".parse().unwrap());
        let proxy = SocketAddr::from(([172, 18, 0, 2], 40000));
        let internet = SocketAddr::from(([198, 51, 100, 4], 40000));
        assert_eq!(
            client_address(proxy, &headers),
            IpAddr::from([203, 0, 113, 9])
        );
        assert_eq!(client_address(internet, &headers), internet.ip());
        assert_eq!(client_address(proxy, &HeaderMap::new()), proxy.ip());
    }
}
