//! The `annulus` command (ADR-0020, F-AUTO-002): drives a running Annulus through its control
//! door, while its vault is unlocked and Settings → Automation allows it.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]
// A command-line tool: printing is its output.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fmt::Write as _;
use std::process::ExitCode;

use annulus_control::client::{Client, ClientError};
use annulus_control::protocol::{HostInfo, TabInfo};

const USAGE: &str = "\
Drives a running Annulus while its vault is unlocked and Settings > Automation allows it.

Usage: annulus [--json] <command>

Commands:
  status               Annulus's version and state
  hosts                the saved hosts (never their passwords or keys)
  tabs                 the open tabs
  read <tab> [lines]   the text of terminal tab <tab> (an id from `tabs`), or its last lines

Options:
  --json               print what Annulus answered as JSON
";

/// What the command line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Help,
    Status,
    Hosts,
    Tabs,
    Read { tab: u64, lines: Option<usize> },
}

/// Reads the arguments after the program's name: the command, and whether to print JSON.
fn parse(args: &[String]) -> Result<(Command, bool), String> {
    let json = args.iter().any(|arg| arg == "--json");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--json")
        .collect();
    let command = match words.as_slice() {
        [] | ["help" | "--help" | "-h"] => Command::Help,
        ["status"] => Command::Status,
        ["hosts"] => Command::Hosts,
        ["tabs"] => Command::Tabs,
        ["read", tab, rest @ ..] if rest.len() <= 1 => Command::Read {
            tab: tab
                .parse()
                .map_err(|_| format!("`{tab}` is not a tab id: see `annulus tabs`"))?,
            lines: match rest {
                [lines] => Some(
                    lines
                        .parse()
                        .map_err(|_| format!("`{lines}` is not a number of lines"))?,
                ),
                _ => None,
            },
        },
        _ => return Err(format!("not a command: {}\n\n{USAGE}", words.join(" "))),
    };
    Ok((command, json))
}

/// The saved hosts as a table.
fn hosts_table(hosts: &[HostInfo]) -> String {
    let mut out = String::from("ID\tHOST\tADDRESS\tUSER\tGROUP\n");
    for host in hosts {
        let address = if host.port == 22 {
            host.address.clone()
        } else {
            format!("{}:{}", host.address, host.port)
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{address}\t{}\t{}",
            host.id, host.label, host.user, host.group
        );
    }
    out
}

/// The open tabs as a table, the one in front marked.
fn tabs_table(tabs: &[TabInfo]) -> String {
    let mut out = String::from("ID\tKIND\tTITLE\tSTATUS\n");
    for tab in tabs {
        let _ = writeln!(
            out,
            "{}{}\t{}\t{}\t{}",
            tab.id,
            if tab.active { "*" } else { "" },
            tab.kind,
            tab.title,
            tab.status
        );
    }
    out
}

fn run(command: Command, json: bool) -> Result<String, ClientError> {
    let mut client = Client::connect(&annulus_control::data_dir())?;
    let printed =
        |value: serde_json::Value| serde_json::to_string_pretty(&value).unwrap_or_default() + "\n";
    Ok(match command {
        Command::Help => USAGE.to_owned(),
        Command::Status => {
            let status = client.status()?;
            if json {
                printed(serde_json::json!(status))
            } else {
                format!(
                    "Annulus {}, vault {}, {} tabs open\n",
                    status.version,
                    if status.unlocked {
                        "unlocked"
                    } else {
                        "locked"
                    },
                    status.tabs
                )
            }
        }
        Command::Hosts => {
            let hosts = client.hosts()?;
            if json {
                printed(serde_json::json!(hosts))
            } else {
                hosts_table(&hosts)
            }
        }
        Command::Tabs => {
            let tabs = client.tabs()?;
            if json {
                printed(serde_json::json!(tabs))
            } else {
                tabs_table(&tabs)
            }
        }
        Command::Read { tab, lines } => {
            let text = client.read(tab, lines)?;
            if json {
                printed(serde_json::Value::String(text))
            } else {
                text
            }
        }
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, json) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::from(2);
        }
    };
    if command == Command::Help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(command, json) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("annulus: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_command_line_reads_each_command_and_refuses_what_it_does_not_know() {
        assert_eq!(parse(&args("")), Ok((Command::Help, false)));
        assert_eq!(parse(&args("hosts --json")), Ok((Command::Hosts, true)));
        assert_eq!(
            parse(&args("read 7 40")),
            Ok((
                Command::Read {
                    tab: 7,
                    lines: Some(40)
                },
                false
            ))
        );
        assert_eq!(
            parse(&args("--json read 7")),
            Ok((
                Command::Read {
                    tab: 7,
                    lines: None
                },
                true
            ))
        );
        assert!(parse(&args("read web-1")).is_err());
        assert!(parse(&args("read 7 40 9")).is_err());
        assert!(parse(&args("delete everything")).is_err());
    }

    #[test]
    fn tables_show_a_port_only_when_it_is_not_22_and_mark_the_tab_in_front() {
        let host = |port| HostInfo {
            id: 3,
            label: "web".into(),
            address: "10.0.0.5".into(),
            port,
            user: "deploy".into(),
            group: "Prod".into(),
            os: String::new(),
        };
        let table = hosts_table(&[host(22), host(2222)]);
        assert!(
            table.contains("3\tweb\t10.0.0.5\tdeploy\tProd\n"),
            "{table}"
        );
        assert!(table.contains("10.0.0.5:2222"), "{table}");
        let tabs = tabs_table(&[TabInfo {
            id: 4,
            title: "web".into(),
            kind: "terminal".into(),
            status: String::new(),
            active: true,
        }]);
        assert!(tabs.contains("4*\tterminal\tweb\t\n"), "{tabs}");
    }
}
