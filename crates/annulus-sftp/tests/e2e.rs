//! End-to-end tests against real Linux servers (OpenSSH on Ubuntu): logins, host keys,
//! shells, SFTP, all three kinds of port forwarding, a jump host, and a keychain key.
//!
//! They need two test VMs (see docs/TESTING.md §End to end) and are ignored by default:
//!
//! ```text
//! ANNULUS_E2E_VM1=192.0.2.11 ANNULUS_E2E_VM2=192.0.2.12 ANNULUS_E2E_USER=demo \
//! ANNULUS_E2E_PASSWORD=… ANNULUS_E2E_KEY=~/.ssh/id_ed25519 \
//!     cargo test -p annulus-sftp --test e2e -- --ignored --test-threads 1
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use annulus_sftp::{Conflict, Outcome, Sftp};
use annulus_ssh::forward::{self, ForwardError, Rule};
use annulus_ssh::{
    Connection, HostKeyQuestion, Output, Prompt, SshError, Target, connect, connect_via,
};
use zeroize::Zeroizing;

struct Env {
    vm1: String,
    vm2: String,
    user: String,
    password: String,
    key: String,
}

fn env() -> Env {
    let var = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is not set; see the top of this file"))
    };
    let key_path = var("ANNULUS_E2E_KEY");
    Env {
        vm1: var("ANNULUS_E2E_VM1"),
        vm2: var("ANNULUS_E2E_VM2"),
        user: var("ANNULUS_E2E_USER"),
        password: var("ANNULUS_E2E_PASSWORD"),
        key: std::fs::read_to_string(&key_path)
            .unwrap_or_else(|_| panic!("cannot read {key_path}")),
    }
}

/// Answers prompts from a script and records what was asked.
#[derive(Default)]
struct Answers {
    trust: bool,
    password: Option<String>,
    asked_key: Mutex<Option<HostKeyQuestion>>,
    asked_password: Mutex<u32>,
}

impl Prompt for Answers {
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
        *self.asked_password.lock().unwrap() += 1;
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(self.password.clone().map(Zeroizing::new));
        rx
    }
}

fn trusting(password: Option<&str>) -> Arc<Answers> {
    Arc::new(Answers {
        trust: true,
        password: password.map(str::to_owned),
        ..Answers::default()
    })
}

fn known_hosts(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("annulus-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("known_hosts")
}

fn target(host: &str, user: &str) -> Target {
    Target {
        host: host.to_owned(),
        port: 22,
        user: user.to_owned(),
    }
}

async fn with_key(env: &Env, host: &str, name: &str) -> Connection {
    connect(
        &target(host, &env.user),
        &known_hosts(name),
        None,
        &[Zeroizing::new(env.key.clone())],
        trusting(None),
    )
    .await
    .expect("key login")
}

/// Runs `command` in a shell and returns what it printed before `__done__`.
async fn run(connection: &Connection, command: &str) -> String {
    let mut shell = connection.shell(200, 50).await.unwrap();
    shell
        // Quoted apart, so the echoed command line never contains the marker the output does.
        .write(format!("{command}; echo __do\"\"ne__\n").as_bytes())
        .await
        .unwrap();
    let mut out = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let next = tokio::time::timeout_at(deadline, shell.next())
            .await
            .expect("shell answered");
        match next {
            Output::Data(bytes) => out.push_str(&String::from_utf8_lossy(&bytes)),
            Output::Exited | Output::Lost => break,
        }
        if out.contains("__done__") {
            break;
        }
    }
    out
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn a_new_host_key_is_asked_once_and_remembered() {
    let env = env();
    let path = known_hosts("trust");
    let first = trusting(None);
    connect(
        &target(&env.vm1, &env.user),
        &path,
        None,
        &[Zeroizing::new(env.key.clone())],
        first.clone(),
    )
    .await
    .unwrap();
    let question = first
        .asked_key
        .lock()
        .unwrap()
        .clone()
        .expect("asked to trust");
    assert!(question.fingerprint.starts_with("SHA256:"));

    let second = trusting(None);
    connect(
        &target(&env.vm1, &env.user),
        &path,
        None,
        &[Zeroizing::new(env.key.clone())],
        second.clone(),
    )
    .await
    .unwrap();
    assert!(second.asked_key.lock().unwrap().is_none(), "remembered");

    // Another server's key under this name is a hard stop.
    let text = std::fs::read_to_string(&path).unwrap();
    let vm2_line = {
        let other = known_hosts("other");
        connect(
            &target(&env.vm2, &env.user),
            &other,
            None,
            &[Zeroizing::new(env.key.clone())],
            trusting(None),
        )
        .await
        .unwrap();
        std::fs::read_to_string(&other).unwrap()
    };
    let swapped = vm2_line.replacen(&env.vm2, &env.vm1, 1);
    std::fs::write(&path, swapped).unwrap();
    let result = connect(
        &target(&env.vm1, &env.user),
        &path,
        None,
        &[Zeroizing::new(env.key.clone())],
        trusting(None),
    )
    .await;
    assert!(
        matches!(result, Err(SshError::HostKeyChanged { .. })),
        "{text}"
    );
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn password_login_and_a_shell() {
    let env = env();
    let answers = trusting(Some(&env.password));
    let connection = connect(
        &target(&env.vm1, &env.user),
        &known_hosts("password"),
        None,
        &[],
        answers.clone(),
    )
    .await
    .expect("password login");
    assert_eq!(*answers.asked_password.lock().unwrap(), 1);
    assert!(run(&connection, "echo $((6*7))").await.contains("42"));

    let wrong = trusting(Some("not the password"));
    let result = connect(
        &target(&env.vm1, &env.user),
        &known_hosts("wrong"),
        None,
        &[],
        wrong,
    )
    .await;
    assert!(matches!(result, Err(SshError::AuthFailed { .. })));
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn sftp_round_trip_with_conflicts() {
    let env = env();
    let connection = with_key(&env, &env.vm1, "sftp").await;
    let sftp = Sftp::open(&connection).await.unwrap();
    let home = sftp.home().await.unwrap();
    let dir = format!("{home}/annulus-e2e");
    let _ = sftp.remove(&dir).await;
    sftp.create_dir(&dir).await.unwrap();

    let local = known_hosts("sftp-local").parent().unwrap().to_path_buf();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(local.join("blob.bin"), &data).unwrap();
    let mut progress = |_: u64, _: u64| true;
    let up = sftp
        .upload(
            &local.join("blob.bin"),
            &dir,
            Conflict::KeepBoth,
            &mut progress,
        )
        .await
        .unwrap();
    assert!(matches!(
        up,
        Outcome::Done {
            bytes: 3_000_000,
            ..
        }
    ));
    let again = sftp
        .upload(
            &local.join("blob.bin"),
            &dir,
            Conflict::KeepBoth,
            &mut progress,
        )
        .await
        .unwrap();
    assert!(matches!(again, Outcome::Done { ref to, .. } if to.ends_with("blob (1).bin")));

    let names: Vec<String> = sftp
        .list(&dir)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["blob (1).bin", "blob.bin"]);

    let downloads = local.join("down");
    std::fs::create_dir_all(&downloads).unwrap();
    sftp.download(
        &format!("{dir}/blob.bin"),
        &downloads,
        Conflict::KeepBoth,
        &mut progress,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(downloads.join("blob.bin")).unwrap(), data);

    // A cancelled overwrite keeps the server's file whole.
    let mut cancel = |_: u64, _: u64| false;
    std::fs::write(local.join("blob.bin"), b"short").unwrap();
    let cancelled = sftp
        .upload(
            &local.join("blob.bin"),
            &dir,
            Conflict::Overwrite,
            &mut cancel,
        )
        .await;
    assert!(cancelled.is_err());
    sftp.download(
        &format!("{dir}/blob.bin"),
        &downloads,
        Conflict::Overwrite,
        &mut progress,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(downloads.join("blob.bin")).unwrap(), data);

    sftp.rename(
        &format!("{dir}/blob (1).bin"),
        &format!("{dir}/renamed.bin"),
    )
    .await
    .unwrap();
    sftp.remove(&dir).await.unwrap();
    assert!(sftp.list(&dir).await.is_err());
}

/// Reads an SSH banner through `stream`.
async fn banner(stream: &mut tokio::net::TcpStream) -> String {
    let mut buffer = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buffer[..read]).into_owned()
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn local_socks_and_remote_forwards() {
    let env = env();
    let connection = Arc::new(with_key(&env, &env.vm1, "forward").await);
    let any: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    // Local: a port here reaches VM 2's SSH server, as VM 1 sees it.
    let local = forward::start(
        connection.clone(),
        Rule::Local {
            bind: any,
            host: env.vm2.clone(),
            port: 22,
        },
    )
    .await
    .unwrap();
    let mut stream = tokio::net::TcpStream::connect(local.local().unwrap())
        .await
        .unwrap();
    assert!(banner(&mut stream).await.starts_with("SSH-2.0-"));
    let busy = forward::start(
        connection.clone(),
        Rule::Dynamic {
            bind: local.local().unwrap(),
        },
    )
    .await;
    assert!(matches!(busy, Err(ForwardError::PortInUse(_))));

    // SOCKS: a client names VM 2 by address.
    let socks = forward::start(connection.clone(), Rule::Dynamic { bind: any })
        .await
        .unwrap();
    let mut client = tokio::net::TcpStream::connect(socks.local().unwrap())
        .await
        .unwrap();
    let ip: std::net::Ipv4Addr = env.vm2.parse().unwrap();
    let mut request = vec![5, 1, 0, 5, 1, 0, 1];
    request.extend_from_slice(&ip.octets());
    request.extend_from_slice(&22u16.to_be_bytes());
    client.write_all(&request).await.unwrap();
    let mut reply = [0u8; 12];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[..4], [5, 0, 5, 0]);
    assert!(banner(&mut client).await.starts_with("SSH-2.0-"));

    // Remote: VM 1 listens; what connects there reaches an echo service on this computer.
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
    let remote = forward::start(
        connection.clone(),
        Rule::Remote {
            bind_host: "127.0.0.1".into(),
            bind_port: 0,
            host: "127.0.0.1".into(),
            port: echo_port,
        },
    )
    .await
    .unwrap();
    let port = remote.remote_port().unwrap();
    let out = run(
        &connection,
        &format!("exec 3<>/dev/tcp/127.0.0.1/{port}; echo annulus-ping >&3; head -n1 <&3"),
    )
    .await;
    assert!(out.matches("annulus-ping").count() >= 2, "{out}");
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn a_jump_host_carries_the_connection() {
    let env = env();
    let jump = Arc::new(with_key(&env, &env.vm1, "jump").await);
    let inner = connect_via(
        jump,
        &target(&env.vm2, &env.user),
        &known_hosts("jump-inner"),
        None,
        &[],
        trusting(Some(&env.password)),
    )
    .await
    .expect("through the jump host");
    let out = run(&inner, "echo from=${SSH_CONNECTION%% *}").await;
    assert!(out.contains(&format!("from={}", env.vm1)), "{out}");
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn a_generated_keychain_key_logs_in_once_installed() {
    let env = env();
    let key = annulus_ssh::keys::generate("annulus-e2e").unwrap();
    let admin = with_key(&env, &env.vm2, "keychain-admin").await;
    run(
        &admin,
        &format!("echo '{}' >> ~/.ssh/authorized_keys", key.public),
    )
    .await;

    let answers = trusting(None);
    let connection = connect(
        &target(&env.vm2, &env.user),
        &known_hosts("keychain"),
        None,
        &[key.private],
        answers.clone(),
    )
    .await
    .expect("the new key logs in");
    assert_eq!(*answers.asked_password.lock().unwrap(), 0);
    run(
        &connection,
        &format!("sed -i '/{}/d' ~/.ssh/authorized_keys", "annulus-e2e"),
    )
    .await;
}

/// Reads a shell until it ends.
async fn end_of(shell: &mut annulus_ssh::Shell) -> Output {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(20), shell.next())
            .await
            .expect("shell ended");
        if !matches!(next, Output::Data(_)) {
            return next;
        }
    }
}

#[tokio::test]
#[ignore = "needs the test servers"]
async fn an_exit_ends_the_session_but_a_killed_server_is_a_lost_connection() {
    let env = env();
    let connection = with_key(&env, &env.vm1, "end").await;
    let mut shell = connection.shell(80, 24).await.unwrap();
    shell.write(b"exit 3\n").await.unwrap();
    assert_eq!(end_of(&mut shell).await, Output::Exited);

    // The shell's parent is this login's sshd: killing it drops the TCP connection with no
    // exit status, as a network failure would (F-SSH-009).
    let connection = with_key(&env, &env.vm1, "end").await;
    let mut shell = connection.shell(80, 24).await.unwrap();
    shell.write(b"kill -9 $PPID\n").await.unwrap();
    assert_eq!(end_of(&mut shell).await, Output::Lost);
}
