//! Session logs (F-LOG-001…005): the text of a finished terminal session, encrypted in the
//! vault and synced, with a size cap and keep-the-last-N retention per host.

use serde::{Deserialize, Serialize};

/// Largest stored log; longer ones keep their end.
pub const MAX_LOG_BYTES: usize = 1024 * 1024;

/// Logs kept per title (host or profile); older ones go first.
pub const KEEP_PER_TITLE: usize = 5;

/// One recorded session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLog {
    /// Stable identifier within the vault.
    pub id: u64,
    /// What the session was: the host's label or the shell's name.
    pub title: String,
    /// When it was saved, seconds since the Unix epoch.
    pub saved_at: u64,
    /// The session's text.
    pub text: String,
}

/// `text` cut to [`MAX_LOG_BYTES`], keeping the end (the most recent output) and saying
/// that the start was dropped.
#[must_use]
pub fn capped(text: &str) -> String {
    if text.len() <= MAX_LOG_BYTES {
        return text.to_owned();
    }
    let mut start = text.len() - MAX_LOG_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "[earlier output dropped: the log keeps the last 1 MiB]\n{}",
        &text[start..]
    )
}

/// The ids to delete so that every title keeps its [`KEEP_PER_TITLE`] newest logs.
#[must_use]
pub fn over_retention(logs: &[SessionLog]) -> Vec<u64> {
    let mut sorted: Vec<&SessionLog> = logs.iter().collect();
    sorted.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(b.id.cmp(&a.id)));
    let mut seen: Vec<(&str, usize)> = Vec::new();
    let mut drop = Vec::new();
    for log in sorted {
        match seen.iter_mut().find(|(title, _)| *title == log.title) {
            Some((_, count)) if *count >= KEEP_PER_TITLE => drop.push(log.id),
            Some((_, count)) => *count += 1,
            None => seen.push((&log.title, 1)),
        }
    }
    drop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_logs_keep_their_end() {
        let text = "x".repeat(MAX_LOG_BYTES + 10) + "the end";
        let kept = capped(&text);
        assert!(kept.starts_with("[earlier output dropped"));
        assert!(kept.ends_with("the end"));
        assert!(kept.len() < MAX_LOG_BYTES + 100);
        assert_eq!(capped("short"), "short");
    }

    #[test]
    fn each_title_keeps_its_five_newest() {
        let log = |id: u64, title: &str| SessionLog {
            id,
            title: title.into(),
            saved_at: id,
            text: String::new(),
        };
        let mut logs: Vec<SessionLog> = (1..=7).map(|id| log(id, "db")).collect();
        logs.push(log(8, "web"));
        let mut dropped = over_retention(&logs);
        dropped.sort_unstable();
        assert_eq!(dropped, [1, 2]);
    }
}
