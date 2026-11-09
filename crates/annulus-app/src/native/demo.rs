//! A throwaway vault with made-up hosts: for `annulus-app --preview` and the scene
//! catalogue, so the new UI can be looked at without touching anyone's real vault.

#![allow(clippy::expect_used)] // A preview that cannot write its temporary folder cannot run.

use annulus_term::Emulator;
use annulus_vault::crypto::KdfParams;
use annulus_vault::vault::Vault;

/// A made-up host: label, address, user, port, tags and the index of its group.
type Fixture = (
    &'static str,
    &'static str,
    &'static str,
    Option<u16>,
    &'static [&'static str],
    Option<usize>,
);

/// A vault in a fresh temporary folder, with made-up hosts and groups when `populated`. Its
/// key derivation is cheap: it holds nothing real.
///
/// # Panics
/// When the temporary folder cannot be written.
#[must_use]
#[allow(clippy::too_many_lines)] // Made-up data, one entity after another.
pub fn fixture_vault(name: &str, populated: bool) -> Vault {
    let dir = std::env::temp_dir().join(format!("annulus-scenes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cheap = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    let (mut vault, _) =
        Vault::create(dir.join("vault.anv"), "pw", cheap).expect("a temporary vault");
    if !populated {
        return vault;
    }
    let mut group_uids = Vec::new();
    for name in ["Production", "Staging", "Homelab"] {
        let mut group = vault.draft_group(None).expect("a temporary vault");
        group.name = name.into();
        group_uids.push(group.uid.clone());
        vault.upsert_group(group).expect("a temporary vault");
    }
    let hosts: [Fixture; 14] = [
        (
            "prod-db",
            "10.0.4.12",
            "postgres",
            None,
            &["postgres", "prod"],
            None,
        ),
        (
            "web-1",
            "web-1.example.com",
            "deploy",
            None,
            &["nginx"],
            None,
        ),
        (
            "bastion",
            "jump.example.com",
            "admin",
            Some(2222),
            &["jump"],
            None,
        ),
        (
            "cache-eu-1",
            "10.0.7.31",
            "ubuntu",
            None,
            &["redis", "eu"],
            None,
        ),
        (
            "grafana",
            "grafana.internal",
            "ops",
            None,
            &["monitoring"],
            None,
        ),
        ("build-runner", "10.1.1.40", "ci", None, &[], None),
        (
            "nas",
            "nas.local",
            "admin",
            Some(2200),
            &["storage", "home"],
            None,
        ),
        ("pi-hole", "10.1.1.2", "pi", None, &["dns"], None),
        ("api-1", "10.0.5.21", "deploy", None, &["api"], Some(0)),
        ("api-2", "10.0.5.22", "deploy", None, &["api"], Some(0)),
        ("worker-1", "10.0.6.11", "deploy", None, &[], Some(0)),
        (
            "staging-web",
            "staging.example.com",
            "deploy",
            None,
            &["nginx"],
            Some(1),
        ),
        ("staging-db", "10.1.4.12", "postgres", None, &[], Some(1)),
        ("router", "10.1.1.1", "root", None, &["network"], Some(2)),
    ];
    for (label, address, user, port, tags, group) in hosts {
        let mut host = vault.draft_host();
        host.label = label.into();
        host.address = address.into();
        host.username = user.into();
        host.port = port;
        host.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
        host.group = group.map(|at| group_uids[at].clone());
        vault.upsert_host(host).expect("a temporary vault");
    }
    // What their servers said they run (F-HOST-009); the others show their initials.
    for (label, os) in [
        ("prod-db", "debian"),
        ("web-1", "ubuntu"),
        ("cache-eu-1", "ubuntu"),
        ("nas", "freebsd"),
        ("pi-hole", "raspbian"),
        ("api-1", "alpine"),
        ("api-2", "alpine"),
        ("staging-web", "ubuntu"),
        ("router", "router"),
    ] {
        if let Some(mut host) = vault
            .hosts()
            .iter()
            .find(|host| host.label == label)
            .cloned()
        {
            os.clone_into(&mut host.os);
            vault.upsert_host(host).expect("a temporary vault");
        }
    }
    let first = vault.hosts()[0].id;
    for (label, script) in [
        ("Restart nginx", "sudo systemctl restart nginx"),
        ("Disk usage", "df -h --total | tail -1"),
        ("Tail the app log", "journalctl -u app -f -n 100"),
    ] {
        let mut snippet = vault.draft_snippet();
        snippet.label = label.into();
        snippet.script = script.into();
        vault.upsert_snippet(snippet).expect("a temporary vault");
    }
    let mut rule = vault.draft_forward(first);
    rule.label = "Postgres on prod".into();
    rule.bind_port = 15432;
    rule.destination_host = "localhost".into();
    rule.destination_port = 5432;
    vault.upsert_forward(rule).expect("a temporary vault");
    let mut socks = vault.draft_forward(first);
    socks.kind = annulus_vault::forwards::ForwardKind::Dynamic;
    socks.bind_port = 1080;
    vault.upsert_forward(socks).expect("a temporary vault");
    vault
        .add_log(
            "prod-db",
            "deploy@prod-db:~$ uptime\n 09:12:44 up 2 days,  load average: 0.08\n",
            1_793_260_800,
        )
        .expect("a temporary vault");
    vault
        .add_log(
            "web-1",
            "deploy@web-1:~$ sudo nginx -t\nsyntax is ok\n",
            1_793_264_400,
        )
        .expect("a temporary vault");
    let pair = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "demo@annulus")
        .expect("Ed25519 keys always generate");
    let key = crate::keychain_screen::stored(&vault, "Work laptop", &pair);
    vault.upsert_key(key).expect("a temporary vault");
    vault
}

/// A temporary path for the `what` of demo `name`, never handed out twice in this process:
/// the scenes render side by side and must not write into each other's files.
pub(crate) fn scratch(what: &str, name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("annulus-{what}-{}-{name}-{n}", std::process::id()))
}

/// A `known_hosts` file in a temporary folder with three made-up servers.
///
/// # Panics
/// When the temporary folder cannot be written.
#[must_use]
pub fn known_hosts(name: &str) -> std::path::PathBuf {
    let path = scratch("known", name);
    let lines: Vec<String> = ["bastion.example.com", "10.0.4.12", "[nas.local]:2200"]
        .iter()
        .map(|host| {
            let pair = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "")
                .expect("Ed25519 keys always generate");
            let mut parts = pair.public.split_whitespace();
            let (kind, blob) = (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            );
            format!("{host} {kind} {blob}")
        })
        .collect();
    std::fs::write(&path, lines.join("\n") + "\n").expect("a temporary file");
    path
}

/// An SFTP tab that never connects, showing a made-up folder on a server beside a temporary
/// local folder with two files (the scenes and the preview).
///
/// # Panics
/// When the temporary folder cannot be written.
#[must_use]
pub fn sftp_tab(name: &str) -> crate::sftp_view::SftpTab {
    use annulus_sftp::{Entry, Kind};
    let mut tab = crate::sftp_view::SftpTab::unavailable("web-1 files", "");
    tab.failed = None;
    tab.cwd = Some("/srv/app".into());
    let entry = |name: &str, kind: Kind, size: u64, modified: u32, permissions: u32| Entry {
        name: name.into(),
        kind,
        size,
        modified: Some(modified),
        permissions: Some(permissions),
        // The deploy user's, but the log, which the web server's group may read.
        uid: Some(1000),
        gid: Some(if name == "app.log" { 33 } else { 1000 }),
        accessed: Some(modified + 3_600),
    };
    tab.owners = annulus_sftp::Owners {
        users: [(0, "root".to_owned()), (1000, "deploy".to_owned())].into(),
        groups: [(33, "www-data".to_owned()), (1000, "deploy".to_owned())].into(),
    };
    tab.entries = vec![
        entry("config", Kind::Dir, 0, 1_761_662_520, 0o755),
        entry("releases", Kind::Dir, 0, 1_761_489_000, 0o755),
        entry(
            "current",
            Kind::Link { to_dir: true },
            0,
            1_761_662_000,
            0o777,
        ),
        entry("deploy.sh", Kind::File, 8312, 1_761_670_920, 0o755),
        entry("app.log", Kind::File, 2_411_520, 1_761_741_600, 0o640),
        entry("README.md", Kind::File, 612, 1_761_478_000, 0o644),
        entry(".env", Kind::File, 214, 1_761_400_000, 0o600),
    ];
    let local = scratch("sftp", name);
    let _ = std::fs::remove_dir_all(&local);
    std::fs::create_dir_all(local.join("notes")).expect("a temporary folder");
    std::fs::write(local.join("backup.tar.gz"), vec![0; 4096]).expect("a temporary file");
    std::fs::write(local.join("todo.txt"), "ship it\n").expect("a temporary file");
    tab.local = crate::local_pane::LocalPane::new_loaded(local);
    tab
}

/// A terminal replaying a made-up session on a database server: a colored prompt,
/// `systemctl`, `ls` and `git log` output.
#[must_use]
pub fn terminal() -> Emulator {
    const PROMPT: &str = "\x1b[1;32mdeploy@prod-db\x1b[0m:\x1b[1;34m~/app\x1b[0m$ ";
    let mut emulator = Emulator::new(100, 30);
    let lines = [
        format!("{PROMPT}systemctl status postgresql"),
        "\x1b[1;32m\u{25cf}\x1b[0m postgresql.service - PostgreSQL RDBMS".to_owned(),
        "     Loaded: loaded (/lib/systemd/system/postgresql.service; \x1b[1;32menabled\x1b[0m)".to_owned(),
        "     Active: \x1b[1;32mactive (running)\x1b[0m since Tue 2026-10-27 09:12:44 UTC; 2 days ago".to_owned(),
        "   Main PID: 812 (postgres)".to_owned(),
        "      Tasks: 9 (limit: 4613)".to_owned(),
        "     Memory: 61.4M".to_owned(),
        String::new(),
        format!("{PROMPT}ls -l"),
        "total 24".to_owned(),
        "drwxr-xr-x 2 deploy deploy 4096 Oct 28 17:02 \x1b[1;34mconfig\x1b[0m".to_owned(),
        "-rwxr-xr-x 1 deploy deploy 8312 Oct 28 17:02 \x1b[1;32mdeploy.sh\x1b[0m".to_owned(),
        "drwxr-xr-x 4 deploy deploy 4096 Oct 27 09:10 \x1b[1;34mmigrations\x1b[0m".to_owned(),
        "-rw-r--r-- 1 deploy deploy  612 Oct 26 11:45 README.md".to_owned(),
        String::new(),
        format!("{PROMPT}git log --oneline -4"),
        "\x1b[33m4f1c9e2\x1b[0m (\x1b[1;36mHEAD -> \x1b[1;32mmain\x1b[0m) Tune autovacuum for the events table".to_owned(),
        "\x1b[33m9b07d3a\x1b[0m Add an index on sessions.expires_at".to_owned(),
        "\x1b[33m1e5a8c4\x1b[0m Move backups to the new bucket".to_owned(),
        "\x1b[33m72d4f10\x1b[0m Raise max_connections to 200".to_owned(),
        String::new(),
        PROMPT.to_owned(),
    ];
    let text = lines.join("\r\n");
    emulator.advance(text.as_bytes());
    emulator
}
