//! The `annulus-agent` command (ADR-0020, ADR-0027, F-AUTO-002/005): drives a running Annulus through its
//! control door, while its vault is unlocked and Settings → Agents allows it. `annulus-agent mcp`
//! offers the same as tools to AI agents.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]
// A command-line tool: printing is its output.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod mcp;
mod scp;

use std::fmt::Write as _;
use std::process::ExitCode;

use annulus_control::client::{Client, ClientError};
use annulus_control::protocol::{HostInfo, RunOutput, TabInfo};

const USAGE: &str = "\
Drives a running Annulus while its vault is unlocked and Settings > Agents
allows it.

Usage: annulus-agent [--json] <command>

Commands:
  status                        Annulus's version and state
  hosts                         the saved hosts (never their passwords or keys)
  scp [-r] [--overwrite] <source> <destination>
                                copy a file, or with -r a directory, over SFTP
                                one path is <host-id>:<path>; the other is local
                                use a trailing / for a destination folder
                                existing files stay unless --overwrite is set
  notes <host> [--set <text...>] print saved host <host>'s notes, or replace them (2,000
                                characters at most; --set alone clears them)
  tabs                          the open tabs (AI marks the ones an agent opened)
  read <tab> [lines]            the text of terminal tab <tab> (an id from `tabs`)
  open [host]                   open a tab of your own: an SSH session to saved host <host>
                                (an id from `hosts`), or a local shell
  run <tab> [--wait S] <cmd...> type <cmd> and Enter into your tab <tab> and print what it
                                printed once its prompt is back (or its output stops, or
                                after S seconds, 30 by default)
  wait <tab> [S]                wait up to S seconds (30 by default) for the last command in
                                your tab <tab> to finish, and print as run does
  interrupt <tab>               press Ctrl+C in your tab <tab> (a command that hangs stops)
  close <tab>                   close your tab <tab>, once the owner lets agents close tabs
                                (Settings > Agents; not by default)
  mcp                           serve these as MCP tools over stdin and stdout

You can type only into tabs you opened; the owner's tabs are read-only. run and wait exit
with 3, and say why on stderr, when the command has not finished.

Options:
  --json                        print what Annulus answered as JSON
";

/// The exit code of a `run` or `wait` whose command has not finished.
const UNFINISHED: u8 = 3;

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Help,
    Status,
    Hosts,
    Scp(annulus_control::protocol::TransferRequest),
    Notes {
        host: u64,
        /// The notes to save; `None` prints them.
        set: Option<String>,
    },
    Tabs,
    Read {
        tab: u64,
        lines: Option<usize>,
    },
    Open {
        host: Option<u64>,
    },
    Run {
        tab: u64,
        command: String,
        wait: Option<u64>,
    },
    Wait {
        tab: u64,
        wait: Option<u64>,
    },
    Interrupt {
        tab: u64,
    },
    Close {
        tab: u64,
    },
    Mcp,
}

fn tab_id(word: &str) -> Result<u64, String> {
    word.parse()
        .map_err(|_| format!("`{word}` is not a tab id: see `annulus-agent tabs`"))
}

fn host_id(word: &str) -> Result<u64, String> {
    word.parse()
        .map_err(|_| format!("`{word}` is not a host id: see `annulus-agent hosts`"))
}

/// Reads the arguments after the program's name: the command, and whether to print JSON.
fn parse(args: &[String]) -> Result<(Command, bool), String> {
    let start = usize::from(args.first().is_some_and(|arg| arg == "--json"));
    if args.get(start).is_some_and(|arg| arg == "scp") {
        let mut json = start == 1;
        let mut options = true;
        let mut words = Vec::new();
        for arg in &args[start + 1..] {
            if options && arg == "--json" {
                json = true;
            } else {
                if arg == "--" {
                    options = false;
                }
                words.push(arg.as_str());
            }
        }
        return Ok((Command::Scp(scp::parse(&words)?), json));
    }
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
        ["notes", host] => Command::Notes {
            host: host_id(host)?,
            set: None,
        },
        ["notes", host, "--set", text @ ..] => Command::Notes {
            host: host_id(host)?,
            set: Some(text.join(" ")),
        },
        ["tabs"] => Command::Tabs,
        ["mcp"] => Command::Mcp,
        ["read", tab, rest @ ..] if rest.len() <= 1 => Command::Read {
            tab: tab_id(tab)?,
            lines: match rest {
                [lines] => Some(
                    lines
                        .parse()
                        .map_err(|_| format!("`{lines}` is not a number of lines"))?,
                ),
                _ => None,
            },
        },
        ["open"] => Command::Open { host: None },
        ["open", host] => Command::Open {
            host: Some(host_id(host)?),
        },
        ["run", tab, rest @ ..] => {
            let (wait, rest) = match rest {
                ["--wait", seconds, rest @ ..] => (
                    Some(
                        seconds
                            .parse()
                            .map_err(|_| format!("`{seconds}` is not a number of seconds"))?,
                    ),
                    rest,
                ),
                _ => (None, rest),
            };
            if rest.is_empty() {
                return Err("run needs a command to type".into());
            }
            Command::Run {
                tab: tab_id(tab)?,
                command: rest.join(" "),
                wait,
            }
        }
        ["wait", tab, rest @ ..] if rest.len() <= 1 => Command::Wait {
            tab: tab_id(tab)?,
            wait: match rest {
                [seconds] => Some(
                    seconds
                        .parse()
                        .map_err(|_| format!("`{seconds}` is not a number of seconds"))?,
                ),
                _ => None,
            },
        },
        ["interrupt", tab] => Command::Interrupt { tab: tab_id(tab)? },
        ["close", tab] => Command::Close { tab: tab_id(tab)? },
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

/// The open tabs as a table: the one in front marked `*`, an agent's marked AI.
fn tabs_table(tabs: &[TabInfo]) -> String {
    let mut out = String::from("ID\tKIND\tTITLE\tSTATUS\n");
    for tab in tabs {
        let _ = writeln!(
            out,
            "{}{}\t{}\t{}{}\t{}",
            tab.id,
            if tab.active { "*" } else { "" },
            tab.kind,
            if tab.agent { "[AI] " } else { "" },
            tab.title,
            tab.status
        );
    }
    out
}

/// What a command prints, and for a `run` or `wait` whose command has not finished, why (on
/// stderr, with exit code 3).
struct Printed {
    out: String,
    unfinished: Option<&'static str>,
}

impl From<String> for Printed {
    fn from(out: String) -> Self {
        Self {
            out,
            unfinished: None,
        }
    }
}

/// A command's answer as printed: its output (or all of it as JSON), and why it is
/// unfinished when it is.
fn ran(answer: &RunOutput, json: bool) -> Printed {
    Printed {
        out: if json {
            serde_json::to_string_pretty(answer).unwrap_or_default() + "\n"
        } else {
            answer.output.clone()
        },
        unfinished: answer.state.advice(),
    }
}

fn run(command: Command, json: bool) -> Result<Printed, ClientError> {
    let mut client = Client::connect(&annulus_control::data_dir())?;
    let printed =
        |value: serde_json::Value| serde_json::to_string_pretty(&value).unwrap_or_default() + "\n";
    let out = match command {
        Command::Scp(transfer) => {
            let answer = client.scp(transfer).map_err(scp::failed)?;
            scp::printed(&answer, json)
        }
        Command::Run { tab, command, wait } => {
            return Ok(ran(&client.run(tab, &command, wait)?, json));
        }
        Command::Wait { tab, wait } => return Ok(ran(&client.wait(tab, wait)?, json)),
        Command::Help | Command::Mcp => USAGE.to_owned(),
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
        Command::Notes { host, set } => {
            let found = match set {
                Some(notes) => client.set_notes(host, &notes)?,
                None => client
                    .hosts()?
                    .into_iter()
                    .find(|known| known.id == host)
                    .ok_or_else(|| {
                        ClientError::Refused(format!(
                            "no saved host {host}: see `annulus-agent hosts`"
                        ))
                    })?,
            };
            if json {
                printed(serde_json::json!(found))
            } else if found.notes.is_empty() {
                String::new()
            } else {
                format!("{}\n", found.notes)
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
        Command::Open { host } => {
            let tab = client.open(host)?;
            if json {
                printed(serde_json::json!(tab))
            } else {
                format!("{}\n", tab.id)
            }
        }
        Command::Interrupt { tab } => {
            client.interrupt(tab)?;
            String::new()
        }
        Command::Close { tab } => {
            client.close(tab)?;
            String::new()
        }
        Command::Read { tab, lines } => text(client.read(tab, lines)?, json),
    };
    Ok(out.into())
}

/// A terminal's text as printed: as it is, or as a JSON string.
fn text(text: String, json: bool) -> String {
    if json {
        serde_json::Value::String(text).to_string() + "\n"
    } else {
        text
    }
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
    match command {
        Command::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Mcp => {
            // Replies go out from several threads, each whole under the output's lock.
            return match mcp::serve(std::io::stdin().lock(), std::io::stdout()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("annulus-agent mcp: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }
    match run(command, json) {
        Ok(printed) => {
            print!("{}", printable(&printed.out));
            match printed.unfinished {
                None => ExitCode::SUCCESS,
                Some(why) => {
                    eprintln!("annulus-agent: {}", printable(why));
                    ExitCode::from(UNFINISHED)
                }
            }
        }
        Err(error) => {
            eprintln!("annulus-agent: {}", printable(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}

/// `text` without the control characters a terminal acts on (escape sequences, C1 codes,
/// bells), line breaks and tabs kept. What is printed comes from servers and the programs
/// in the tabs, and must not drive the terminal it is read in: set its title, write to the
/// clipboard, or hide lines.
fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn what_is_printed_cannot_drive_the_terminal() {
        // A server's text that would set the clipboard (OSC 52) and recolor through C1 CSI.
        assert_eq!(
            printable("ok\u{1b}]52;c;ZXZpbA==\u{7}\u{9b}31m\nnext\tcolumn\r"),
            "ok]52;c;ZXZpbA==31m\nnext\tcolumn"
        );
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
        assert_eq!(
            parse(&args("open 12")),
            Ok((Command::Open { host: Some(12) }, false))
        );
        assert_eq!(
            parse(&args("open")),
            Ok((Command::Open { host: None }, false))
        );
        assert_eq!(
            parse(&args("run 9 --wait 5 ls -la /var/log")),
            Ok((
                Command::Run {
                    tab: 9,
                    command: "ls -la /var/log".into(),
                    wait: Some(5)
                },
                false
            ))
        );
        assert_eq!(
            parse(&args("run 9 uptime")),
            Ok((
                Command::Run {
                    tab: 9,
                    command: "uptime".into(),
                    wait: None
                },
                false
            ))
        );
        assert_eq!(
            parse(&args("close 9")),
            Ok((Command::Close { tab: 9 }, false))
        );
        assert_eq!(
            parse(&args("interrupt 4")),
            Ok((Command::Interrupt { tab: 4 }, false))
        );
        assert_eq!(parse(&args("mcp")), Ok((Command::Mcp, false)));
        assert_eq!(
            parse(&args("wait 9 120")),
            Ok((
                Command::Wait {
                    tab: 9,
                    wait: Some(120)
                },
                false
            ))
        );
        assert_eq!(
            parse(&args("wait 9")),
            Ok((Command::Wait { tab: 9, wait: None }, false))
        );
        assert!(parse(&args("wait 9 soon")).is_err());
        assert!(parse(&args("run 9")).is_err());
        assert!(parse(&args("open web-1")).is_err());
        assert!(parse(&args("read web-1")).is_err());
        assert!(parse(&args("read 7 40 9")).is_err());
        assert!(parse(&args("delete everything")).is_err());
    }

    #[test]
    fn a_hosts_notes_are_printed_or_set_and_set_alone_clears_them() {
        assert_eq!(
            parse(&args("notes 3")),
            Ok((Command::Notes { host: 3, set: None }, false))
        );
        assert_eq!(
            parse(&args("notes 3 --set nginx front end")),
            Ok((
                Command::Notes {
                    host: 3,
                    set: Some("nginx front end".into())
                },
                false
            ))
        );
        assert_eq!(
            parse(&args("notes 3 --set")),
            Ok((
                Command::Notes {
                    host: 3,
                    set: Some(String::new())
                },
                false
            ))
        );
        assert!(parse(&args("notes web-1")).is_err());
    }

    #[test]
    fn a_finished_command_prints_its_output_and_an_unfinished_one_says_why() {
        use annulus_control::protocol::RunState;
        let answer = |state| RunOutput {
            output: "$ make\nbuilding\n".into(),
            state,
            millis: 2_000,
        };
        let done = ran(&answer(RunState::Done), false);
        assert_eq!(done.out, "$ make\nbuilding\n");
        assert_eq!(done.unfinished, None);
        let going = ran(&answer(RunState::Running), false);
        assert_eq!(going.out, "$ make\nbuilding\n", "what it printed so far");
        assert!(
            going.unfinished.is_some_and(|why| why.contains("wait")),
            "and what to do next"
        );
        let json = ran(&answer(RunState::Waiting), true);
        assert!(json.out.contains(r#""state": "waiting""#), "{}", json.out);
    }

    #[test]
    fn tables_show_a_port_only_when_it_is_not_22_and_mark_the_tab_in_front_and_the_agents() {
        let host = |port| HostInfo {
            id: 3,
            label: "web".into(),
            address: "10.0.0.5".into(),
            port,
            user: "deploy".into(),
            group: "Prod".into(),
            os: String::new(),
            notes: String::new(),
        };
        let table = hosts_table(&[host(22), host(2222)]);
        assert!(
            table.contains("3\tweb\t10.0.0.5\tdeploy\tProd\n"),
            "{table}"
        );
        assert!(table.contains("10.0.0.5:2222"), "{table}");
        let tab = |id, active, agent| TabInfo {
            id,
            title: "web".into(),
            kind: "terminal".into(),
            status: String::new(),
            active,
            agent,
        };
        let tabs = tabs_table(&[tab(4, true, false), tab(5, false, true)]);
        assert!(tabs.contains("4*\tterminal\tweb\t\n"), "{tabs}");
        assert!(tabs.contains("5\tterminal\t[AI] web\t\n"), "{tabs}");
    }
}
