//! Target parsing, and whole connections against an in-process russh server on localhost:
//! host-key trust and change detection, password retries, and an echoing shell.

use std::sync::Mutex;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg as ServerMsg, Session};
use russh::{Channel as ServerChannel, ChannelId};
use tokio::sync::oneshot;

use super::*;

#[test]
fn targets_parse_with_defaults() {
    let parse = |text: &str| Target::parse(text, "me");
    assert_eq!(
        parse("prod-db"),
        Some(Target {
            host: "prod-db".into(),
            port: 22,
            user: "me".into()
        })
    );
    assert_eq!(
        parse("admin@10.0.4.12:2222"),
        Some(Target {
            host: "10.0.4.12".into(),
            port: 2222,
            user: "admin".into()
        })
    );
    assert_eq!(
        parse("root@[fe80::1]:2200"),
        Some(Target {
            host: "fe80::1".into(),
            port: 2200,
            user: "root".into()
        })
    );
    assert_eq!(parse("fe80::1").map(|t| t.host), Some("fe80::1".into()));
    assert_eq!(parse("@host"), None);
    assert_eq!(parse("host:0"), None);
    assert_eq!(parse("host:http"), None);
    assert_eq!(parse("a b"), None);
}

/// Accepts alice/secret, opens shells that greet and echo.
#[derive(Clone)]
struct TestServer;

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "alice" && password == "secret" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: ServerChannel<ServerMsg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // The trait's signature.
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"welcome\r\n".to_vec())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, data.to_vec())
    }
}

/// Starts a server with `key` on a free localhost port; serves one connection.
async fn serve(key: PrivateKey) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..server::Config::default()
    });
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(running) = server::run_stream(config, stream, TestServer).await
        {
            let _ = running.await;
        }
    });
    port
}

/// Scripted answers: whether to trust a new key, and passwords to hand out in order.
struct Script {
    trust: bool,
    passwords: Mutex<Vec<&'static str>>,
    asked_key: Mutex<Option<HostKeyQuestion>>,
}

impl Script {
    fn new(trust: bool, passwords: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            trust,
            passwords: Mutex::new(passwords.iter().rev().copied().collect()),
            asked_key: Mutex::new(None),
        })
    }
}

impl Prompt for Script {
    fn trust_new_host_key(&self, question: HostKeyQuestion) -> oneshot::Receiver<bool> {
        *self.asked_key.lock().unwrap() = Some(question);
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(self.trust);
        rx
    }

    fn password(
        &self,
        _user: String,
        _host: String,
    ) -> oneshot::Receiver<Option<Zeroizing<String>>> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(
            self.passwords
                .lock()
                .unwrap()
                .pop()
                .map(|p| Zeroizing::new(p.to_owned())),
        );
        rx
    }
}

fn known_hosts(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("annulus-ssh-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("known_hosts")
}

fn target(port: u16, user: &str) -> Target {
    Target {
        host: "127.0.0.1".into(),
        port,
        user: user.into(),
    }
}

fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

#[tokio::test]
async fn first_connection_asks_to_trust_the_key_then_the_shell_echoes() {
    let port = serve(new_key()).await;
    let path = known_hosts("trust");
    let script = Script::new(true, &["wrong", "secret"]);
    let connection = connect(&target(port, "alice"), &path, None, script.clone())
        .await
        .unwrap();
    let question = script.asked_key.lock().unwrap().clone().unwrap();
    assert!(question.fingerprint.starts_with("SHA256:"));
    assert_eq!(question.algorithm, "ssh-ed25519");

    let mut shell = connection.shell(80, 24).await.unwrap();
    assert_eq!(shell.next().await, Output::Data(b"welcome\r\n".to_vec()));
    shell.write(b"ls\r").await.unwrap();
    assert_eq!(shell.next().await, Output::Data(b"ls\r".to_vec()));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("ssh-ed25519")
    );
}

#[tokio::test]
async fn a_rejected_new_key_stops_before_any_password_is_sent() {
    let port = serve(new_key()).await;
    let script = Script::new(false, &["secret"]);
    let result = connect(
        &target(port, "alice"),
        &known_hosts("reject"),
        None,
        script.clone(),
    )
    .await;
    assert!(
        matches!(result, Err(SshError::HostKeyRejected)),
        "{:?}",
        result.err()
    );
    assert_eq!(
        script.passwords.lock().unwrap().len(),
        1,
        "no password was asked for"
    );
}

#[tokio::test]
async fn a_changed_host_key_is_a_hard_stop() {
    let path = known_hosts("changed");
    let port = serve(new_key()).await;
    connect(
        &target(port, "alice"),
        &path,
        None,
        Script::new(true, &["secret"]),
    )
    .await
    .unwrap();
    // Same host and port, different key: the server was replaced, or someone is in between.
    let recorded = std::fs::read_to_string(&path).unwrap();
    let port2 = serve(new_key()).await;
    std::fs::write(
        &path,
        recorded.replace(
            &format!("[127.0.0.1]:{port}"),
            &format!("[127.0.0.1]:{port2}"),
        ),
    )
    .unwrap();
    let script = Script::new(true, &["secret"]);
    let result = connect(&target(port2, "alice"), &path, None, script.clone()).await;
    assert!(
        matches!(result, Err(SshError::HostKeyChanged { .. })),
        "{:?}",
        result.err()
    );
    assert!(
        script.asked_key.lock().unwrap().is_none(),
        "the user is not offered to trust a changed key"
    );
}

#[tokio::test]
async fn three_wrong_passwords_fail_with_what_was_tried() {
    let port = serve(new_key()).await;
    let result = connect(
        &target(port, "alice"),
        &known_hosts("wrong"),
        None,
        Script::new(true, &["a", "b", "c"]),
    )
    .await;
    match result {
        Err(SshError::AuthFailed { tried, .. }) => assert_eq!(tried, "password"),
        other => panic!("expected AuthFailed, got {:?}", other.err()),
    }
}
