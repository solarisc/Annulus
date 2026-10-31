//! What goes through the door: one JSON object per line each way. A client's first line is
//! its [`Hello`]; after that each [`Call`] gets one [`Reply`] with the same id.
//!
//! Nothing here can hold a secret: hosts carry their names and addresses only, never a
//! password, key or proxy login.

use serde::{Deserialize, Serialize};

/// A client's first line: the token from the door file. `Debug` never shows the token.
#[derive(Serialize, Deserialize)]
pub struct Hello {
    /// The door's token.
    pub token: String,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hello { token: <hidden> }")
    }
}

/// One request, with the id its reply carries back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// Picked by the client; its reply carries it back.
    pub id: u64,
    /// What is asked.
    #[serde(flatten)]
    pub request: Request,
}

/// What a client asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The app's version and state.
    Status,
    /// The saved hosts.
    Hosts,
    /// The open tabs.
    Tabs,
    /// The text of terminal tab `tab` (by uid): its last `lines` lines, or all it holds.
    Read {
        /// The tab's id.
        tab: u64,
        /// How many of the last lines; all of them when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<usize>,
    },
    /// Opens a terminal tab of the agent's own, in front: an SSH session to saved host `host`,
    /// or the default local shell when `None`.
    Open {
        /// The saved host's id, from `hosts`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<u64>,
    },
    /// Types `command` and Enter into agent tab `tab`, then answers with the output once it
    /// has been quiet a moment, or after `wait` seconds at most.
    Run {
        /// The agent tab's id.
        tab: u64,
        /// What to type; each line is entered in turn.
        command: String,
        /// The most seconds to wait for output; 30 when `None`, 300 at most.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<u64>,
    },
    /// Presses Ctrl+C in agent tab `tab`: a command that hangs or runs on stops.
    Interrupt {
        /// The agent tab's id.
        tab: u64,
    },
    /// Closes agent tab `tab`.
    Close {
        /// The agent tab's id.
        tab: u64,
    },
}

/// The answer to one [`Call`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The id of the call it answers.
    pub id: u64,
    /// Its result.
    #[serde(flatten)]
    pub result: Outcome,
}

/// A request's result, as JSON the client reads as the type it asked for, or why it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It worked: what was asked for.
    Ok(serde_json::Value),
    /// It failed, and why.
    Error(String),
}

impl Outcome {
    /// The outcome of a request the app answered with `answer`.
    #[must_use]
    pub fn of(answer: Result<Response, String>) -> Self {
        match answer {
            Ok(Response::Status(status)) => Self::ok(&status),
            Ok(Response::Hosts(hosts)) => Self::ok(&hosts),
            Ok(Response::Tabs(tabs)) => Self::ok(&tabs),
            Ok(Response::Text(text)) => Self::Ok(serde_json::Value::String(text)),
            Ok(Response::Tab(tab)) => Self::ok(&tab),
            Ok(Response::Done) => Self::Ok(serde_json::Value::Null),
            Err(error) => Self::Error(error),
        }
    }

    fn ok<T: Serialize>(value: &T) -> Self {
        Self::Ok(serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
    }
}

/// What the app answers a request with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// For [`Request::Status`].
    Status(Status),
    /// For [`Request::Hosts`].
    Hosts(Vec<HostInfo>),
    /// For [`Request::Tabs`].
    Tabs(Vec<TabInfo>),
    /// A terminal's text, for [`Request::Read`] and [`Request::Run`].
    Text(String),
    /// The tab [`Request::Open`] opened.
    Tab(TabInfo),
    /// Done, with nothing to say ([`Request::Close`]).
    Done,
}

/// The app's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Annulus's version.
    pub version: String,
    /// The vault is unlocked (the door is never open otherwise).
    pub unlocked: bool,
    /// How many tabs are open, the fixed ones included.
    pub tabs: usize,
}

/// A saved host, as a person sees it in the list. Never a credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// Its id in the vault.
    pub id: u64,
    /// The name shown in the list.
    pub label: String,
    /// Hostname or IP address.
    pub address: String,
    /// The port it connects to, its group's or 22 when it names none.
    pub port: u16,
    /// The login name, its group's when it names none; empty if neither does.
    pub user: String,
    /// Its group's path, top level first ("Prod / Web"); empty at the top level.
    pub group: String,
    /// Its OS key (`ubuntu`, `debian`, …), empty while unknown.
    pub os: String,
}

/// An open tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Stable for the tab's life.
    pub id: u64,
    /// Its label.
    pub title: String,
    /// `vault`, `files`, `terminal`, `sftp` or `new_tab`.
    pub kind: String,
    /// Its status dot: `unread`, `input` or `error`; empty for none.
    pub status: String,
    /// It is the tab in front.
    pub active: bool,
    /// An agent opened it: `run` and `close` work in it.
    #[serde(default)]
    pub agent: bool,
}

/// One line of JSON for `value`, without its newline.
#[must_use]
pub fn encode<T: Serialize>(value: &T) -> String {
    // The protocol's types have only string keys, so they always serialize.
    serde_json::to_string(value).unwrap_or_default()
}

/// The value one line of JSON holds.
///
/// # Errors
/// When the line is not JSON of that shape.
pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, String> {
    serde_json::from_str(line).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_and_replies_read_back_as_written_with_the_op_and_ok_names_on_the_wire() {
        let call = Call {
            id: 7,
            request: Request::Read {
                tab: 3,
                lines: Some(20),
            },
        };
        let line = encode(&call);
        assert_eq!(line, r#"{"id":7,"op":"read","tab":3,"lines":20}"#);
        assert_eq!(decode::<Call>(&line), Ok(call));
        assert_eq!(
            decode::<Call>(r#"{"id":1,"op":"hosts"}"#),
            Ok(Call {
                id: 1,
                request: Request::Hosts
            })
        );
        let reply = Reply {
            id: 7,
            result: Outcome::of(Ok(Response::Text("$ ls\n".into()))),
        };
        let line = encode(&reply);
        assert_eq!(line, r#"{"id":7,"ok":"$ ls\n"}"#);
        assert_eq!(decode::<Reply>(&line), Ok(reply));
        let failed = Reply {
            id: 2,
            result: Outcome::Error("no such tab".into()),
        };
        assert_eq!(decode::<Reply>(&encode(&failed)), Ok(failed));
        assert!(decode::<Call>(r#"{"id":1,"op":"format_disk"}"#).is_err());
    }
}
