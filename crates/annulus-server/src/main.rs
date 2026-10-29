//! `annulus-server`: the self-hosted sync server (ADR-0008).
//!
//! ```text
//! annulus-server [serve] [--listen 127.0.0.1:8443] [--data annulus-server.db]
//!                [--signups open|closed] [--tls-cert cert.pem --tls-key key.pem]
//!                [--behind-proxy]
//! annulus-server accounts [--data …]               list accounts and storage use
//! annulus-server delete-account <name> [--data …]  delete an account and its data
//! annulus-server backup <file> [--data …]          consistent copy while running
//! ```
//!
//! Plain HTTP is allowed on loopback only; elsewhere the server needs a certificate, or
//! `--behind-proxy` when a TLS reverse proxy (Caddy, nginx) sits in front.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

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
}

const DEFAULT_DATA: &str = "annulus-server.db";

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
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
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
            other if !other.starts_with("--") => positional.push(other.to_owned()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    serve.tls = match (cert, key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
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
        let app = service(server);
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
}
