//! What a terminal tab knows besides its screen (shared rules, tested here): the status dot,
//! when a dropped SSH session tries again (F-SSH-009), when a paste needs a yes and how it is
//! sent (F-TERM-006), and what to try after a failure (F-APP-007). `native::live` draws them.

use annulus_term::alacritty_terminal::term::TermMode;

/// What to try next after a failed connection, by what went wrong (F-APP-007).
pub(crate) fn next_step(message: &str) -> Option<&'static str> {
    let message = message.to_lowercase();
    if message.contains("has changed") {
        Some(
            "If the server was reinstalled, forget its old key on the Known Hosts screen and \
             connect again. If not, do not connect: someone may be in between.",
        )
    } else if message.contains("was not trusted") {
        Some("Connect again and trust the key if its fingerprint matches the server's.")
    } else if message.contains("authentication failed") {
        Some(
            "Check the username and the key or password in the host's settings; the server's \
             accepted methods are listed above.",
        )
    } else if message.contains("proxy") {
        Some("Check the proxy address and login in the host's settings.")
    } else if message.contains("could not reach") {
        Some(
            "Check the address and port, and that the server is on and reachable from here \
             (VPN, firewall).",
        )
    } else {
        None
    }
}

/// The text "Copy details" puts on the clipboard: the app version, the tab and the error.
pub(crate) fn diagnostic(title: &str, message: &str) -> String {
    format!(
        "Annulus {} on {}\nConnecting to: {title}\nError: {message}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// What a tab's status dot says (F-APP-002), least to most urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TabStatus {
    /// Output arrived while the tab was not shown.
    Unread,
    /// A question (host key, password) waits for an answer.
    Input,
    /// The session failed or its connection dropped.
    Error,
}

impl TabStatus {
    /// The tooltip and accessible name of the dot.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            TabStatus::Unread => "New output",
            TabStatus::Input => "Waiting for your answer",
            TabStatus::Error => "Disconnected or failed",
        }
    }
}

/// Tries before an automatic reconnect gives up and shows the error.
pub(crate) const RECONNECT_TRIES: u32 = 8;

/// Seconds to wait before automatic reconnect `attempt` (1-based): 1, 2, 4 … up to 30.
pub(crate) fn backoff(attempt: u32) -> f64 {
    f64::from(1_u32 << attempt.saturating_sub(1).min(5)).min(30.0)
}

/// Automatic reconnects of a dropped SSH session (F-SSH-009): when to try next, and when to
/// give up and show the error.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reconnect {
    /// When the next try is due (window clock, seconds), while one is.
    pub(crate) at: Option<f64>,
    /// Tries in this run of drops.
    pub(crate) tries: u32,
    /// When the current session started, to tell a fresh drop from a retry.
    pub(crate) started: f64,
}

impl Reconnect {
    /// The connection dropped at `now`, or a reconnect failed with `failure`: schedules the
    /// next try, or gives up after [`RECONNECT_TRIES`] with the error to show. A drop a minute
    /// after the last connect starts a new run.
    pub(crate) fn dropped(&mut self, now: f64, failure: Option<String>) -> Option<String> {
        if failure.is_none() && now - self.started >= 60.0 {
            self.tries = 0;
        }
        self.tries += 1;
        if self.tries > RECONNECT_TRIES {
            (self.tries, self.at) = (0, None);
            return Some(failure.unwrap_or_else(|| "The connection keeps dropping.".into()));
        }
        self.at = Some(now + backoff(self.tries));
        None
    }

    /// No more automatic tries (the session ended, or the user said stop).
    pub(crate) fn stop(&mut self) {
        (self.tries, self.at) = (0, None);
    }
}

/// A multi-line paste joined into one line (the paste card's "Paste as one line").
pub(crate) fn one_line(text: &str) -> String {
    text.split(['\r', '\n'])
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The earlier output with a line that marks where the new connection begins.
pub(crate) fn carried(transcript: &str) -> String {
    format!(
        "{}\n\u{2500}\u{2500} reconnected \u{2500}\u{2500}\n",
        transcript.trim_end()
    )
}

/// Whether a paste should be confirmed first: it holds a line break and the program did
/// not ask for bracketed paste, so each line would run as a command.
pub(crate) fn needs_confirm(text: &str, mode: TermMode) -> bool {
    !mode.contains(TermMode::BRACKETED_PASTE) && text.contains(['\n', '\r'])
}

/// Pasted text, wrapped in bracketed-paste markers when the program asked for them, so a
/// pasted line is never run by accident.
pub(crate) fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A paste that contains the end marker could break out of the bracket; drop it, again
        // and again, since dropping one can join the pieces around it into another
        // (`ESC[2` `ESC[201~` `01~`). Each pass shortens the text, so this ends.
        let mut safe = normalized;
        while safe.contains("\x1b[201~") {
            safe = safe.replace("\x1b[201~", "");
        }
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_come_with_a_next_step_and_copyable_details() {
        let changed = "web: the host key of web has changed (SHA256:x); the connection was stopped";
        assert!(next_step(changed).unwrap().contains("Known Hosts"));
        assert!(
            next_step("authentication failed (tried: password; the server accepts: publickey)")
                .unwrap()
                .contains("username")
        );
        assert!(
            next_step("could not reach db:22: refused")
                .unwrap()
                .contains("port")
        );
        assert!(
            next_step("could not reach the proxy: refused")
                .unwrap()
                .contains("proxy")
        );
        assert_eq!(next_step("something else"), None);
        let details = diagnostic("db", "could not reach db:22: refused");
        assert!(details.starts_with("Annulus "));
        assert!(details.contains("Connecting to: db\nError: could not reach db:22: refused"));
    }

    #[test]
    fn reconnects_back_off_up_to_half_a_minute() {
        let waits: Vec<f64> = (1..=8).map(backoff).collect();
        assert_eq!(waits, [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0]);
    }

    #[test]
    fn a_dropped_connection_retries_then_gives_up_with_the_error() {
        let mut reconnect = Reconnect {
            started: 100.0,
            ..Reconnect::default()
        };
        assert_eq!(reconnect.dropped(500.0, None), None);
        assert_eq!((reconnect.at, reconnect.tries), (Some(501.0), 1));
        for _ in 2..=RECONNECT_TRIES {
            assert_eq!(reconnect.dropped(600.0, Some("refused".into())), None);
        }
        assert_eq!(reconnect.at, Some(630.0));
        assert_eq!(
            reconnect.dropped(700.0, Some("refused".into())).as_deref(),
            Some("refused")
        );
        assert_eq!((reconnect.at, reconnect.tries), (None, 0));
    }

    #[test]
    fn a_drop_long_after_connecting_starts_the_backoff_over() {
        let mut reconnect = Reconnect {
            tries: 5,
            started: 100.0,
            ..Reconnect::default()
        };
        assert_eq!(reconnect.dropped(200.0, None), None);
        assert_eq!((reconnect.at, reconnect.tries), (Some(201.0), 1));
    }

    #[test]
    fn a_paste_joined_into_one_line_drops_the_breaks() {
        assert_eq!(one_line("cd /srv\r\n\ngit pull\n"), "cd /srv git pull");
    }

    #[test]
    fn kept_output_ends_with_a_reconnected_line() {
        assert_eq!(
            carried("$ ls\nfile\n\n"),
            "$ ls\nfile\n\u{2500}\u{2500} reconnected \u{2500}\u{2500}\n"
        );
    }

    #[test]
    fn several_lines_are_confirmed_unless_the_program_brackets_pastes() {
        assert!(needs_confirm("ls\nrm -rf x", TermMode::empty()));
        assert!(
            needs_confirm("reboot\r", TermMode::empty()),
            "a trailing Enter runs it too"
        );
        assert!(!needs_confirm("one line", TermMode::empty()));
        assert!(!needs_confirm("ls\nrm", TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn bracketed_paste_wraps_and_cannot_be_escaped() {
        let pasted = paste_bytes("ls\nrm -rf x\x1b[201~; reboot", TermMode::BRACKETED_PASTE);
        assert_eq!(pasted, b"\x1b[200~ls\rrm -rf x; reboot\x1b[201~");
        // Taking out one marker must not leave a new one made of the pieces around it.
        let nested = paste_bytes("x\x1b[2\x1b[201~01~; reboot", TermMode::BRACKETED_PASTE);
        let body = &nested[6..nested.len() - 6];
        assert!(
            !body.windows(6).any(|window| window == b"\x1b[201~"),
            "{:?}",
            String::from_utf8_lossy(&nested)
        );
        assert_eq!(paste_bytes("a\r\nb", TermMode::empty()), b"a\rb");
    }
}
