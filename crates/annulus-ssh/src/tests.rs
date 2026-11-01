//! Target parsing, and whole connections against an in-process russh server on localhost:
//! host-key trust and change detection, password retries, an echoing shell, the SFTP
//! subsystem, and port forwarding through echoing `direct-tcpip` channels.

// The test server implements russh's async trait; some handlers have nothing to await.
#![allow(clippy::unused_async_trait_impl)]

use std::collections::{HashMap, HashSet};
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

/// Accepts alice/secret, opens shells that greet and echo, and serves an SFTP subsystem
/// that only answers `realpath`.
#[derive(Clone, Default)]
struct TestServer {
    /// Open channels not yet claimed by a shell or the SFTP subsystem.
    channels: Arc<Mutex<HashMap<ChannelId, ServerChannel<ServerMsg>>>>,
    /// Channels speaking SFTP or carrying a remote forward; their data is not echoed.
    quiet: Arc<Mutex<HashSet<ChannelId>>>,
    /// Every channel and request the client made, in order (F-SSH-011).
    requests: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    fn record(&self, what: impl Into<String>) {
        self.requests.lock().unwrap().push(what.into());
    }
}

/// Answers `realpath` with `/home/alice`; everything else is unsupported.
struct HomeOnly;

impl russh_sftp::server::Handler for HomeOnly {
    type Error = russh_sftp::protocol::StatusCode;

    fn unimplemented(&self) -> Self::Error {
        russh_sftp::protocol::StatusCode::OpUnsupported
    }

    async fn realpath(
        &mut self,
        id: u32,
        _path: String,
    ) -> Result<russh_sftp::protocol::Name, Self::Error> {
        Ok(russh_sftp::protocol::Name {
            id,
            files: vec![russh_sftp::protocol::File::dummy("/home/alice")],
        })
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    /// bob logs in with any key he can sign with (russh checks the signature).
    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(if user == "bob" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "alice" && password == "secret" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: ServerChannel<ServerMsg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record("session");
        self.channels.lock().unwrap().insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // The trait's signature.
    async fn channel_open_direct_tcpip(
        &mut self,
        _channel: ServerChannel<ServerMsg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record("direct-tcpip");
        reply.accept().await;
        Ok(())
    }

    /// Listens on a free localhost port and opens a `forwarded-tcpip` channel per
    /// connection, as sshd does for `ssh -R`.
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        *port = u32::from(listener.local_addr()?.port());
        let (handle, address, bound, quiet) = (
            session.handle(),
            address.to_owned(),
            *port,
            self.quiet.clone(),
        );
        tokio::spawn(async move {
            while let Ok((mut socket, peer)) = listener.accept().await {
                let Ok(channel) = handle
                    .channel_open_forwarded_tcpip(
                        address.clone(),
                        bound,
                        peer.ip().to_string(),
                        u32::from(peer.port()),
                    )
                    .await
                else {
                    break;
                };
                quiet.lock().unwrap().insert(channel.id());
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                });
            }
        });
        Ok(true)
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
        self.record("pty");
        session.channel_success(channel)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record("shell");
        self.channels.lock().unwrap().remove(&channel);
        session.channel_success(channel)?;
        session.data(channel, b"welcome\r\n".to_vec())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(format!("exec {}", String::from_utf8_lossy(data)));
        self.channels.lock().unwrap().remove(&channel);
        session.channel_success(channel)?;
        if data == b"flood" {
            // Three mebibytes, more than the client keeps.
            let chunk = vec![b'y'; 64 * 1024];
            for _ in 0..48 {
                session.data(channel, chunk.clone())?;
            }
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        if data == b"open-channels" {
            // A hostile server opens channels the client never asked for, then answers.
            let (handle, requests) = (session.handle(), self.requests.clone());
            tokio::spawn(async move {
                let record = |kind: &str, opened: bool| {
                    let outcome = if opened { "accepted" } else { "refused" };
                    requests.lock().unwrap().push(format!("{kind} {outcome}"));
                };
                record(
                    "x11",
                    handle.channel_open_x11("127.0.0.1", 6000).await.is_ok(),
                );
                record("agent", handle.channel_open_agent().await.is_ok());
                record("session", handle.channel_open_session().await.is_ok());
                let opened = handle.channel_open_direct_streamlocal("/tmp/socket").await;
                record("direct-streamlocal", opened.is_ok());
                let opened = handle
                    .channel_open_forwarded_streamlocal("/tmp/socket")
                    .await;
                record("forwarded-streamlocal", opened.is_ok());
                let opened = handle
                    .channel_open_direct_tcpip("127.0.0.1", 22, "127.0.0.1", 5000)
                    .await;
                record("direct-tcpip", opened.is_ok());
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }
        if data == remote::PROBE.as_bytes() {
            // Answers as cmd does: a Windows server.
            session.data(channel, b"Windows_NT$env:OS\r\n".to_vec())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        session.data(channel, [b"ran: ", data].concat())?;
        session.exit_status_request(channel, 3)?;
        session.eof(channel)?;
        session.close(channel)
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(format!("subsystem {name}"));
        let claimed = self.channels.lock().unwrap().remove(&channel);
        match claimed {
            Some(stream) if name == "sftp" => {
                self.quiet.lock().unwrap().insert(channel);
                session.channel_success(channel)?;
                russh_sftp::server::run(stream.into_stream(), HomeOnly).await;
                Ok(())
            }
            _ => session.channel_failure(channel),
        }
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.quiet.lock().unwrap().contains(&channel) {
            return Ok(());
        }
        match data {
            b"exit\r" => {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)
            }
            // The connection drops without a word, as when the network goes away.
            b"drop\r" => Err(russh::Error::Disconnect),
            _ => session.data(channel, data.to_vec()),
        }
    }
}

/// Starts a server with `key` on a free localhost port; serves one connection.
async fn serve(key: PrivateKey) -> u16 {
    serve_recording(key).await.0
}

/// [`serve`] with several host keys of different kinds.
async fn serve_keys(keys: Vec<PrivateKey>) -> u16 {
    serve_with(keys).await.0
}

/// [`serve`], also returning the list of what the client asked for.
async fn serve_recording(key: PrivateKey) -> (u16, Arc<Mutex<Vec<String>>>) {
    serve_with(vec![key]).await
}

/// A server with `keys` on a free localhost port; serves one connection. Returns its port and
/// what the client asked for.
async fn serve_with(keys: Vec<PrivateKey>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys,
        ..server::Config::default()
    });
    let handler = TestServer::default();
    let requests = handler.requests.clone();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(running) = server::run_stream(config, stream, handler).await
        {
            let _ = running.await;
        }
    });
    (port, requests)
}

/// Scripted answers: whether to trust a new key, and passwords to hand out in order.
struct Script {
    trust: bool,
    /// Overrides `trust` with a specific answer, such as connecting once.
    answer: Option<Trust>,
    passwords: Mutex<Vec<&'static str>>,
    asked_key: Mutex<Option<HostKeyQuestion>>,
}

impl Script {
    fn new(trust: bool, passwords: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            trust,
            answer: None,
            passwords: Mutex::new(passwords.iter().rev().copied().collect()),
            asked_key: Mutex::new(None),
        })
    }
}

impl Prompt for Script {
    fn trust_new_host_key(&self, question: HostKeyQuestion) -> oneshot::Receiver<Trust> {
        *self.asked_key.lock().unwrap() = Some(question);
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(self.answer.unwrap_or(self.trust.into()));
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
    let connection = connect(&target(port, "alice"), &path, None, &[], script.clone())
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

/// Reads past data until the shell ends.
async fn end_of(shell: &mut crate::Shell) -> Output {
    loop {
        match shell.next().await {
            Output::Data(_) => {}
            end => return end,
        }
    }
}

#[tokio::test]
async fn a_session_asks_the_server_for_nothing_the_user_did_not() {
    let (port, requests) = serve_recording(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("hidden"),
        None,
        &[],
        script,
    )
    .await
    .unwrap();
    let mut shell = connection.shell(80, 24).await.unwrap();
    assert_eq!(shell.next().await, Output::Data(b"welcome\r\n".to_vec()));
    let _sftp = connection.sftp_stream().await.unwrap();
    // Idle for a moment: nothing runs in the background (no probes, no exec).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        *requests.lock().unwrap(),
        ["session", "pty", "shell", "session", "subsystem sftp"],
        "no exec, env or forwarding the user did not ask for (F-SSH-011)"
    );
}

#[tokio::test]
async fn a_host_key_that_cannot_be_saved_is_reported_not_forgotten() {
    let port = serve(new_key()).await;
    // known_hosts would go inside a file, which cannot be.
    let blocker = known_hosts("unsaved").with_file_name("not-a-folder");
    std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
    std::fs::write(&blocker, b"").unwrap();
    let result = connect(
        &target(port, "alice"),
        &blocker.join("known_hosts"),
        None,
        &[],
        Script::new(true, &["secret"]),
    )
    .await;
    assert!(
        matches!(&result, Err(SshError::Protocol(why)) if why.contains("could not be saved")),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn a_server_that_never_answers_fails_in_time() {
    // It takes the connection and never speaks, as a stalled or hostile host can.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    let started = std::time::Instant::now();
    let result = connect(
        &target(port, "alice"),
        &known_hosts("silent"),
        None,
        &[],
        Script::new(true, &[]),
    )
    .await;
    assert!(
        matches!(&result, Err(SshError::Unreachable(_, why)) if why.starts_with("no answer")),
        "{:?}",
        result.err()
    );
    assert!(started.elapsed() < ANSWER * 2);
}

#[tokio::test]
async fn the_time_spent_deciding_on_a_host_key_does_not_count() {
    struct Slow;
    impl Prompt for Slow {
        fn trust_new_host_key(&self, _: HostKeyQuestion) -> oneshot::Receiver<Trust> {
            let (tx, rx) = oneshot::channel();
            tokio::spawn(async move {
                // Longer than any one step may take.
                tokio::time::sleep(ANSWER + std::time::Duration::from_secs(1)).await;
                let _ = tx.send(Trust::Once);
            });
            rx
        }
        fn password(&self, _: String, _: String) -> oneshot::Receiver<Option<Zeroizing<String>>> {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Some(Zeroizing::new("secret".to_owned())));
            rx
        }
    }
    let port = serve(new_key()).await;
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("slow-user"),
        None,
        &[],
        Arc::new(Slow),
    )
    .await;
    assert!(connection.is_ok(), "{:?}", connection.err());
}

#[tokio::test]
async fn a_command_that_prints_without_end_is_cut_at_a_mebibyte() {
    let port = serve(new_key()).await;
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("flood"),
        None,
        &[],
        Script::new(true, &["secret"]),
    )
    .await
    .unwrap();
    let exec = connection.exec("flood").await.unwrap();
    assert_eq!(exec.output.len(), MOST_OUTPUT + CUT.len());
    assert!(exec.output.ends_with(CUT));
}

#[tokio::test]
async fn channels_a_server_opens_unasked_are_refused() {
    let (port, requests) = serve_recording(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("unasked-channels"),
        None,
        &[],
        script,
    )
    .await
    .unwrap();
    connection.exec("open-channels").await.unwrap();
    let requests = requests.lock().unwrap().clone();
    for kind in [
        "x11",
        "agent",
        "session",
        "direct-streamlocal",
        "forwarded-streamlocal",
        "direct-tcpip",
    ] {
        assert!(
            requests.contains(&format!("{kind} refused")),
            "{kind}: {requests:?}"
        );
    }
}

#[tokio::test]
async fn exec_returns_the_output_and_the_exit_status() {
    let port = serve(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("exec"),
        None,
        &[],
        script,
    )
    .await
    .unwrap();
    let exec = connection.exec("uptime").await.unwrap();
    assert_eq!(exec.output, b"ran: uptime");
    assert_eq!(exec.status, Some(3));
}

#[tokio::test]
async fn a_command_for_the_servers_shell_runs_after_asking_which_shell_it_is() {
    let (port, requests) = serve_recording(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("exec-in-shell"),
        None,
        &[],
        script,
    )
    .await
    .unwrap();
    let commands = remote::Commands {
        unix: "uname".into(),
        windows: "ver".into(),
    };
    let exec = connection.exec_in_shell(&commands).await.unwrap();
    assert_eq!(exec.output, b"ran: ver");
    let execs: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.starts_with("exec "))
        .cloned()
        .collect();
    assert_eq!(
        execs,
        [format!("exec {}", remote::PROBE), "exec ver".into()]
    );
}

#[tokio::test]
async fn an_exit_is_told_apart_from_a_dropped_connection() {
    for (typed, expected) in [("exit\r", Output::Exited), ("drop\r", Output::Lost)] {
        let port = serve(new_key()).await;
        let path = known_hosts(&format!("end-{}", expected == Output::Lost));
        let script = Script::new(true, &["secret"]);
        let connection = connect(&target(port, "alice"), &path, None, &[], script)
            .await
            .unwrap();
        let mut shell = connection.shell(80, 24).await.unwrap();
        shell.write(typed.as_bytes()).await.unwrap();
        assert_eq!(end_of(&mut shell).await, expected, "after {typed:?}");
    }
}

#[tokio::test]
async fn the_sftp_subsystem_opens_on_a_connection() {
    let port = serve(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = connect(
        &target(port, "alice"),
        &known_hosts("sftp"),
        None,
        &[],
        script,
    )
    .await
    .unwrap();
    let stream = connection.sftp_stream().await.unwrap();
    let sftp = russh_sftp::client::SftpSession::new(stream).await.unwrap();
    assert_eq!(sftp.canonicalize(".").await.unwrap(), "/home/alice");
}

#[tokio::test]
async fn local_and_socks_forwards_carry_bytes_and_a_busy_port_is_reported() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = serve(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = Arc::new(
        connect(
            &target(port, "alice"),
            &known_hosts("forward"),
            None,
            &[],
            script,
        )
        .await
        .unwrap(),
    );
    let any_port: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = forward::start(
        connection.clone(),
        forward::Rule::Local {
            bind: any_port,
            host: "echo.internal".into(),
            port: 7,
        },
    )
    .await
    .unwrap();
    let mut client = tokio::net::TcpStream::connect(local.local().unwrap())
        .await
        .unwrap();
    client.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"ping");
    assert_eq!(local.connections(), 1);

    let busy = forward::start(
        connection.clone(),
        forward::Rule::Dynamic {
            bind: local.local().unwrap(),
        },
    )
    .await;
    assert!(matches!(busy, Err(forward::ForwardError::PortInUse(_))));

    let socks = forward::start(connection, forward::Rule::Dynamic { bind: any_port })
        .await
        .unwrap();
    let mut client = tokio::net::TcpStream::connect(socks.local().unwrap())
        .await
        .unwrap();
    client
        .write_all(&[5, 1, 0, 5, 1, 0, 3, 4, b'h', b'o', b's', b't', 0, 80])
        .await
        .unwrap();
    let mut replies = [0u8; 12];
    client.read_exact(&mut replies).await.unwrap();
    assert_eq!(replies[..2], [5, 0]);
    assert_eq!(replies[2..4], [5, 0]);
    client.write_all(b"pong").await.unwrap();
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"pong");
}

#[tokio::test]
async fn a_remote_forward_reaches_a_destination_on_this_side() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // The destination: an echo service only this side can reach.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = socket.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    let port = serve(new_key()).await;
    let script = Script::new(true, &["secret"]);
    let connection = Arc::new(
        connect(
            &target(port, "alice"),
            &known_hosts("remote"),
            None,
            &[],
            script,
        )
        .await
        .unwrap(),
    );
    let remote = forward::start(
        connection,
        forward::Rule::Remote {
            bind_host: "127.0.0.1".into(),
            bind_port: 0,
            host: "127.0.0.1".into(),
            port: echo_port,
        },
    )
    .await
    .unwrap();
    let server_port = u16::try_from(remote.remote_port().unwrap()).unwrap();
    assert_eq!(remote.local(), None);

    // A client of the server's port reaches the echo service through the tunnel.
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", server_port))
        .await
        .unwrap();
    client.write_all(b"remote").await.unwrap();
    let mut echoed = [0u8; 6];
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"remote");
    assert_eq!(remote.connections(), 1);
}

#[tokio::test]
async fn a_keychain_key_logs_in_without_asking_for_a_password() {
    let port = serve(new_key()).await;
    let key = keys::generate(keys::KeyKind::Ed25519, "bob@test").unwrap();
    // No passwords to hand out: asking for one would fail the login.
    let script = Script::new(true, &[]);
    let connection = connect(
        &target(port, "bob"),
        &known_hosts("keychain"),
        None,
        &[key.private],
        script,
    )
    .await;
    assert!(connection.is_ok());
}

/// A server that forwards `direct-tcpip` channels to real TCP ports on this machine, like
/// a bastion, and otherwise behaves like [`TestServer`].
#[derive(Clone, Default)]
struct Bastion;

impl server::Handler for Bastion {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "jumper" && password == "hop" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    #[allow(clippy::too_many_arguments)] // The trait's signature.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: ServerChannel<ServerMsg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let port = u16::try_from(port_to_connect).unwrap_or(0);
        let Ok(mut socket) = tokio::net::TcpStream::connect((host_to_connect, port)).await else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
        Ok(())
    }
}

#[tokio::test]
async fn a_host_behind_a_jump_host_is_reached_through_it() {
    let inner = serve(new_key()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bastion_port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![new_key()],
        ..server::Config::default()
    });
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(running) = server::run_stream(config, stream, Bastion).await
        {
            let _ = running.await;
        }
    });
    let path = known_hosts("jump");
    let jump = connect(
        &target(bastion_port, "jumper"),
        &path,
        None,
        &[],
        Script::new(true, &["hop"]),
    )
    .await
    .unwrap();
    let script = Script::new(true, &["secret"]);
    let connection = connect_via(
        Arc::new(jump),
        &target(inner, "alice"),
        &path,
        None,
        &[],
        script.clone(),
    )
    .await
    .unwrap();
    // The inner host's key was checked too, and the shell works through the hop.
    assert!(script.asked_key.lock().unwrap().is_some());
    let mut shell = connection.shell(80, 24).await.unwrap();
    assert_eq!(shell.next().await, Output::Data(b"welcome\r\n".to_vec()));
}

#[tokio::test]
async fn a_server_is_reached_through_a_socks_proxy() {
    let inner = serve(new_key()).await;
    // A plain local SOCKS proxy that dials directly, standing in for a corporate proxy.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut client, _) = listener.accept().await.unwrap();
        let mut greeting = [0u8; 3];
        client.read_exact(&mut greeting).await.unwrap();
        client.write_all(&[5, 0]).await.unwrap();
        let mut head = [0u8; 5];
        client.read_exact(&mut head).await.unwrap();
        let mut name = vec![0u8; usize::from(head[4])];
        client.read_exact(&mut name).await.unwrap();
        let port = client.read_u16().await.unwrap();
        let mut upstream =
            tokio::net::TcpStream::connect((String::from_utf8(name).unwrap().as_str(), port))
                .await
                .unwrap();
        client
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    });
    let proxy = proxy::Proxy::parse(&format!("socks5://127.0.0.1:{proxy_port}")).unwrap();
    let connection = connect_proxied(
        &proxy,
        &target(inner, "alice"),
        &known_hosts("socks"),
        None,
        &[],
        Script::new(true, &["secret"]),
    )
    .await
    .unwrap();
    let mut shell = connection.shell(80, 24).await.unwrap();
    assert_eq!(shell.next().await, Output::Data(b"welcome\r\n".to_vec()));
}

#[tokio::test]
async fn a_key_trusted_once_is_not_remembered() {
    let port = serve(new_key()).await;
    let path = known_hosts("once");
    let script = Arc::new(Script {
        answer: Some(Trust::Once),
        ..Arc::try_unwrap(Script::new(false, &["secret"]))
            .ok()
            .unwrap()
    });
    connect(&target(port, "alice"), &path, None, &[], script)
        .await
        .expect("connects this once");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("ssh-ed25519"),
        "nothing was saved"
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
        &[],
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
        &[],
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
    let result = connect(&target(port2, "alice"), &path, None, &[], script.clone()).await;
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

/// A `known_hosts` file that holds `key` for 127.0.0.1 on `port`.
fn known_as(name: &str, port: u16, key: &PrivateKey) -> PathBuf {
    let path = known_hosts(name);
    russh::keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, key.public_key(), &path)
        .unwrap();
    path
}

#[tokio::test]
async fn a_known_host_presenting_another_kind_of_key_is_a_changed_key() {
    // Security review: the host is known by an Ed25519 key; someone in between offers only
    // an ECDSA key. That is a changed key and a hard stop, not a first meeting to trust.
    let known = new_key();
    let other = PrivateKey::random(
        &mut rand::rng(),
        Algorithm::Ecdsa {
            curve: russh::keys::EcdsaCurve::NistP256,
        },
    )
    .unwrap();
    let port = serve(other).await;
    let path = known_as("other-kind", port, &known);
    let script = Script::new(true, &["secret"]);
    let result = connect(&target(port, "alice"), &path, None, &[], script.clone()).await;
    assert!(
        matches!(result, Err(SshError::HostKeyChanged { .. })),
        "{:?}",
        result.err()
    );
    assert!(
        script.asked_key.lock().unwrap().is_none(),
        "the user is not offered to trust it as a new host"
    );
}

#[tokio::test]
async fn a_host_known_by_an_ecdsa_key_is_asked_for_that_key() {
    // A server with both kinds of key, known by its ECDSA one: Annulus asks for the kind it
    // knows, so it connects without a question (and without mistaking it for a change).
    let ecdsa = PrivateKey::random(
        &mut rand::rng(),
        Algorithm::Ecdsa {
            curve: russh::keys::EcdsaCurve::NistP256,
        },
    )
    .unwrap();
    let port = serve_keys(vec![new_key(), ecdsa.clone()]).await;
    let path = known_as("known-ecdsa", port, &ecdsa);
    let script = Script::new(true, &["secret"]);
    connect(&target(port, "alice"), &path, None, &[], script.clone())
        .await
        .unwrap();
    assert!(script.asked_key.lock().unwrap().is_none(), "no question");
}

#[test]
fn sha1_rsa_signatures_are_never_offered() {
    // Security review: rsa-sha2-256/512 only; an RSA key still works.
    let offered = host_key_algorithms("127.0.0.1", 22, &known_hosts("no-sha1"));
    assert!(
        !offered.contains(&Algorithm::Rsa { hash: None }),
        "{offered:?}"
    );
    assert!(offered.contains(&Algorithm::Rsa {
        hash: Some(russh::keys::HashAlg::Sha512)
    }));
}

#[tokio::test]
async fn three_wrong_passwords_fail_with_what_was_tried() {
    let port = serve(new_key()).await;
    let result = connect(
        &target(port, "alice"),
        &known_hosts("wrong"),
        None,
        &[],
        Script::new(true, &["a", "b", "c"]),
    )
    .await;
    match result {
        Err(SshError::AuthFailed { tried, .. }) => assert_eq!(tried, "password"),
        other => panic!("expected AuthFailed, got {:?}", other.err()),
    }
}
