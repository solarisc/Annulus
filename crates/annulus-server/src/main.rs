//! `annulus-server`: the self-hosted sync server (ADR-0008).
//!
//! ```text
//! annulus-server [serve] [--listen 127.0.0.1:8443] [--data annulus-server.db]
//!                [--signups open|closed] [--tls-cert cert.pem --tls-key key.pem]
//!                [--behind-proxy]
//! annulus-server accounts [--data …]               list accounts and storage use
//! annulus-server delete-account <name> [--data …]  delete an account and its data
//! annulus-server backup <file> [--data …]          consistent copy while running
//! annulus-server invite [--expires 24h] [--data …] single-use invite code for one account
//! annulus-server tls-init --name 10.1.1.20[,sync.lan] [--out .]
//!                                                  a self-signed certificate for a LAN server
//! annulus-server fingerprint [--tls-cert cert.pem] the fingerprint the app asks to compare
//! ```
//!
//! Plain HTTP is allowed on loopback only; elsewhere the server needs a certificate, or
//! `--behind-proxy` when a TLS reverse proxy (Caddy, nginx) sits in front.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use annulus_server::{Server, Signups, service};

/// Options of `serve`.
struct Serve {
    listen: SocketAddr,
    data: PathBuf,
    signups: Signups,
    tls: Option<(PathBuf, PathBuf)>,
    behind_proxy: bool,
}

/// What to do.
enum Command {
    Serve(Serve),
    Accounts { data: PathBuf },
    DeleteAccount { data: PathBuf, name: String },
    Backup { data: PathBuf, file: PathBuf },
    Invite { data: PathBuf, expires: Duration },
    TlsInit { out: PathBuf, names: Vec<String> },
    Fingerprint { cert: PathBuf },
}

const DEFAULT_DATA: &str = "annulus-server.db";

/// How long an invite is valid unless `--expires` says otherwise.
const DEFAULT_INVITE: Duration = Duration::from_hours(24);

/// The longest an invite may be valid.
const MAX_INVITE: Duration = Duration::from_hours(30 * 24);

/// `90m`, `24h` or `7d`: at least a minute, at most 30 days.
fn invite_duration(text: &str) -> Result<Duration, String> {
    let wrong = || "--expires takes a time like 90m, 24h or 7d (at most 30d)".to_owned();
    let unit = text.chars().last().ok_or_else(wrong)?;
    let number: u64 = text[..text.len() - unit.len_utf8()]
        .parse()
        .map_err(|_| wrong())?;
    let minutes = match unit {
        'm' => number,
        'h' => number.checked_mul(60).ok_or_else(wrong)?,
        'd' => number.checked_mul(24 * 60).ok_or_else(wrong)?,
        _ => return Err(wrong()),
    };
    if minutes == 0 || minutes > MAX_INVITE.as_secs() / 60 {
        return Err(wrong());
    }
    Ok(Duration::from_mins(minutes))
}

fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    let command = match args.peek().map(String::as_str) {
        Some(word) if !word.starts_with("--") => args.next(),
        _ => None,
    };
    let mut positional = Vec::new();
    let mut serve = Serve {
        listen: SocketAddr::from(([127, 0, 0, 1], 8443)),
        data: PathBuf::from(DEFAULT_DATA),
        signups: Signups::Closed,
        tls: None,
        behind_proxy: false,
    };
    let (mut cert, mut key) = (None, None);
    let mut expires = None;
    let (mut names, mut out) = (Vec::new(), PathBuf::from("."));
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--expires" => expires = Some(invite_duration(&value()?)?),
            "--listen" => {
                serve.listen = value()?
                    .parse()
                    .map_err(|_| "--listen takes an address like 127.0.0.1:8443".to_owned())?;
            }
            "--data" => serve.data = PathBuf::from(value()?),
            "--signups" => {
                serve.signups = match value()?.as_str() {
                    "open" => Signups::Open,
                    "closed" => Signups::Closed,
                    _ => return Err("--signups takes open or closed".into()),
                };
            }
            "--tls-cert" => cert = Some(PathBuf::from(value()?)),
            "--tls-key" => key = Some(PathBuf::from(value()?)),
            "--behind-proxy" => serve.behind_proxy = true,
            "--name" => names.extend(
                value()?
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            ),
            "--out" => out = PathBuf::from(value()?),
            other if !other.starts_with("--") => positional.push(other.to_owned()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let fingerprint_of = cert.clone();
    serve.tls = match (cert, key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        _ if command.as_deref() == Some("fingerprint") => None,
        _ => return Err("--tls-cert and --tls-key go together".into()),
    };
    let one = |what: &str| match positional.as_slice() {
        [value] => Ok(value.clone()),
        _ => Err(format!("expected {what}")),
    };
    match command.as_deref() {
        None | Some("serve") => {
            if !serve.listen.ip().is_loopback() && serve.tls.is_none() && !serve.behind_proxy {
                return Err(
                    "refusing plain HTTP on a non-loopback address: pass --tls-cert and \
                     --tls-key, or put a TLS proxy in front and pass --behind-proxy"
                        .into(),
                );
            }
            Ok(Command::Serve(serve))
        }
        Some("accounts") => Ok(Command::Accounts { data: serve.data }),
        Some("delete-account") => Ok(Command::DeleteAccount {
            name: one("an account name")?,
            data: serve.data,
        }),
        Some("backup") => Ok(Command::Backup {
            file: PathBuf::from(one("a backup file name")?),
            data: serve.data,
        }),
        Some("invite") => Ok(Command::Invite {
            data: serve.data,
            expires: expires.unwrap_or(DEFAULT_INVITE),
        }),
        Some("tls-init") if names.is_empty() => {
            Err("tls-init needs --name with the address the app uses, e.g. 10.1.1.20".into())
        }
        Some("tls-init") => Ok(Command::TlsInit { out, names }),
        Some("fingerprint") => Ok(Command::Fingerprint {
            cert: fingerprint_of.unwrap_or_else(|| PathBuf::from("cert.pem")),
        }),
        Some(other) => Err(format!("unknown command {other}")),
    }
}

fn say(message: &str) {
    let _ = writeln!(std::io::stdout(), "{message}");
}

fn fail(message: &str) -> std::process::ExitCode {
    let _ = writeln!(std::io::stderr(), "annulus-server: {message}");
    std::process::ExitCode::FAILURE
}

fn serve(options: Serve) -> Result<(), String> {
    let server = Arc::new(Server::open(&options.data, options.signups).map_err(|e| e.to_string())?);
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let app = service(server, options.behind_proxy);
        let _ = writeln!(
            std::io::stderr(),
            "annulus-server listening on {} ({})",
            options.listen,
            if options.tls.is_some() {
                "TLS"
            } else {
                "plain HTTP"
            }
        );
        if let Some((cert, key)) = options.tls {
            // The same line `annulus-server fingerprint` prints, for the app's question.
            if let Ok(fingerprint) = std::fs::read_to_string(&cert)
                .map_err(|e| e.to_string())
                .and_then(|pem| annulus_server::tls::fingerprint_of_pem(&pem))
            {
                let _ = writeln!(std::io::stderr(), "certificate fingerprint {fingerprint}");
            }
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .map_err(|e| format!("cannot load the certificate: {e}"))?;
            axum_server::bind_rustls(options.listen, config)
                .serve(app)
                .await
                .map_err(|e| e.to_string())
        } else {
            let listener = tokio::net::TcpListener::bind(options.listen)
                .await
                .map_err(|e| e.to_string())?;
            axum::serve(listener, app).await.map_err(|e| e.to_string())
        }
    })
}

fn run(command: Command) -> Result<(), String> {
    let open = |data: &PathBuf| Server::open(data, Signups::Closed).map_err(|e| e.to_string());
    match command {
        Command::Serve(options) => serve(options),
        Command::Accounts { data } => {
            let accounts = open(&data)?.accounts().map_err(|e| e.to_string())?;
            say("account\tdevices\trecords\tbytes");
            for account in accounts {
                say(&format!(
                    "{}\t{}\t{}\t{}",
                    account.name, account.devices, account.records, account.bytes
                ));
            }
            Ok(())
        }
        Command::DeleteAccount { data, name } => {
            if open(&data)?
                .delete_account(&name)
                .map_err(|e| e.to_string())?
            {
                say(&format!("deleted {name}"));
                Ok(())
            } else {
                Err(format!("no account named {name}"))
            }
        }
        Command::Backup { data, file } => {
            open(&data)?.backup(&file).map_err(|e| e.to_string())?;
            say(&format!("backup written to {}", file.display()));
            Ok(())
        }
        Command::TlsInit { out, names } => {
            let fingerprint = annulus_server::tls::init(&out, &names)?;
            say(&fingerprint);
            let _ = writeln!(
                std::io::stderr(),
                "wrote cert.pem and key.pem in {}: serve with --tls-cert and --tls-key; the app \
                 shows this fingerprint at sign-in for you to compare",
                out.display()
            );
            Ok(())
        }
        Command::Fingerprint { cert } => {
            let pem =
                std::fs::read_to_string(&cert).map_err(|e| format!("{}: {e}", cert.display()))?;
            say(&annulus_server::tls::fingerprint_of_pem(&pem)?);
            Ok(())
        }
        Command::Invite { data, expires } => {
            let code = open(&data)?
                .create_invite(expires)
                .map_err(|e| e.to_string())?;
            say(&code);
            let _ = writeln!(
                std::io::stderr(),
                "single use, valid for {} hours: enter it under Invite code when creating \
                 the account in the app",
                expires.as_secs().div_ceil(3600)
            );
            Ok(())
        }
    }
}

fn main() -> std::process::ExitCode {
    match parse(std::env::args().skip(1)).and_then(run) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(list: &[&str]) -> Result<Command, String> {
        parse(list.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn plain_http_beyond_loopback_needs_tls_or_a_proxy() {
        assert!(parsed(&["--listen", "0.0.0.0:8443"]).is_err());
        assert!(parsed(&["--listen", "0.0.0.0:8443", "--behind-proxy"]).is_ok());
        assert!(
            parsed(&[
                "serve",
                "--listen",
                "0.0.0.0:443",
                "--tls-cert",
                "c.pem",
                "--tls-key",
                "k.pem"
            ])
            .is_ok()
        );
        assert!(
            parsed(&["--tls-cert", "c.pem"]).is_err(),
            "a certificate needs its key"
        );
        let Ok(Command::Serve(defaults)) = parsed(&[]) else {
            panic!("no arguments serve");
        };
        assert_eq!(defaults.signups, Signups::Closed);
        assert!(defaults.listen.ip().is_loopback());
        assert!(parsed(&["--signups", "maybe"]).is_err());
    }

    #[test]
    fn admin_commands_take_their_argument() {
        assert!(matches!(
            parsed(&["delete-account", "ana", "--data", "x.db"]),
            Ok(Command::DeleteAccount { name, .. }) if name == "ana"
        ));
        assert!(parsed(&["delete-account"]).is_err());
        assert!(matches!(
            parsed(&["backup", "copy.db"]),
            Ok(Command::Backup { .. })
        ));
        assert!(matches!(
            parsed(&["accounts"]),
            Ok(Command::Accounts { .. })
        ));
        assert!(parsed(&["frobnicate"]).is_err());
    }

    #[test]
    fn an_invite_lasts_a_day_unless_told_otherwise_and_never_beyond_thirty() {
        let expires = |list: &[&str]| match parsed(list) {
            Ok(Command::Invite { expires, .. }) => Ok(expires),
            Ok(_) => Err("not an invite".to_owned()),
            Err(error) => Err(error),
        };
        assert_eq!(expires(&["invite"]), Ok(Duration::from_hours(24)));
        assert_eq!(
            expires(&["invite", "--expires", "90m"]),
            Ok(Duration::from_mins(90))
        );
        assert_eq!(
            expires(&["invite", "--expires", "7d", "--data", "x.db"]),
            Ok(Duration::from_hours(7 * 24))
        );
        assert_eq!(
            expires(&["invite", "--expires", "30d"]),
            Ok(Duration::from_hours(30 * 24))
        );
        for wrong in [
            "31d",
            "0h",
            "h",
            "",
            "12",
            "5é",
            "-1h",
            "300000000000000000d",
        ] {
            assert!(expires(&["invite", "--expires", wrong]).is_err(), "{wrong}");
        }
    }
}
