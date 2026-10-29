//! `annulus-server`: the self-hosted sync server (ADR-0008).
//!
//! ```text
//! annulus-server [--listen 127.0.0.1:8443] [--data annulus-server.db]
//!                [--signups open|closed] [--behind-proxy]
//! ```
//!
//! Built-in TLS arrives with a later step; until then the server listens on loopback only, or
//! anywhere with `--behind-proxy` when a TLS reverse proxy (Caddy, nginx) sits in front.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use annulus_server::{Server, Signups, router};

/// Parsed command line.
struct Options {
    listen: SocketAddr,
    data: PathBuf,
    signups: Signups,
    behind_proxy: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        listen: SocketAddr::from(([127, 0, 0, 1], 8443)),
        data: PathBuf::from("annulus-server.db"),
        signups: Signups::Closed,
        behind_proxy: false,
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--listen" => {
                options.listen = value()?
                    .parse()
                    .map_err(|_| "--listen takes an address like 127.0.0.1:8443".to_owned())?;
            }
            "--data" => options.data = PathBuf::from(value()?),
            "--signups" => {
                options.signups = match value()?.as_str() {
                    "open" => Signups::Open,
                    "closed" => Signups::Closed,
                    _ => return Err("--signups takes open or closed".into()),
                };
            }
            "--behind-proxy" => options.behind_proxy = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if !options.listen.ip().is_loopback() && !options.behind_proxy {
        return Err(
            "refusing plain HTTP on a non-loopback address: put a TLS proxy in front and pass \
             --behind-proxy"
                .into(),
        );
    }
    Ok(options)
}

fn fail(message: &str) -> std::process::ExitCode {
    let _ = writeln!(std::io::stderr(), "annulus-server: {message}");
    std::process::ExitCode::FAILURE
}

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => return fail(&message),
    };
    let server = match Server::open(&options.data, options.signups) {
        Ok(server) => Arc::new(server),
        Err(error) => return fail(&error.to_string()),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return fail(&error.to_string()),
    };
    let result = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(options.listen).await?;
        let _ = writeln!(
            std::io::stderr(),
            "annulus-server listening on {}",
            options.listen
        );
        axum::serve(listener, router(server)).await
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => fail(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|arg| (*arg).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn plain_http_beyond_loopback_needs_a_proxy() {
        assert!(parse(args(&["--listen", "0.0.0.0:8443"])).is_err());
        let options = parse(args(&["--listen", "0.0.0.0:8443", "--behind-proxy"])).unwrap();
        assert!(options.behind_proxy);
        let defaults = parse(args(&[])).unwrap();
        assert_eq!(defaults.signups, Signups::Closed);
        assert!(defaults.listen.ip().is_loopback());
        assert!(parse(args(&["--signups", "maybe"])).is_err());
    }
}
