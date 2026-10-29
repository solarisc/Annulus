//! Snippets (F-SNIP-001): saved commands, grouped in packages, stored in the vault and
//! synced with the account.

use serde::{Deserialize, Serialize};

/// One saved command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    /// Stable identifier within the vault.
    pub id: u64,
    /// The name shown in lists.
    pub label: String,
    /// The text sent to the terminal; several lines run one after another.
    pub script: String,
    /// The package it belongs to; empty for none.
    #[serde(default)]
    pub package: String,
}

impl Snippet {
    /// Whether every word of `query` appears in the label, script or package.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.label, self.script, self.package).to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }

    /// The bytes to send: the script with `\r` line ends, ending in `\r` when it should
    /// run, or without a final one to leave it at the prompt.
    #[must_use]
    pub fn input(&self, run: bool) -> Vec<u8> {
        let mut text = self
            .script
            .trim_end_matches(['\n', '\r'])
            .replace("\r\n", "\n")
            .replace('\n', "\r");
        if run {
            text.push('\r');
        }
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(script: &str) -> Snippet {
        Snippet {
            id: 1,
            label: "Disk usage".into(),
            script: script.into(),
            package: "ops".into(),
        }
    }

    #[test]
    fn running_ends_with_enter_and_pasting_does_not() {
        assert_eq!(snippet("df -h\n").input(true), b"df -h\r");
        assert_eq!(snippet("df -h\n").input(false), b"df -h");
        assert_eq!(snippet("cd /var\r\nls\n").input(true), b"cd /var\rls\r");
    }

    #[test]
    fn search_needs_every_word_in_label_script_or_package() {
        let s = snippet("df -h");
        assert!(s.matches("disk ops"));
        assert!(s.matches("DF"));
        assert!(!s.matches("disk memory"));
    }
}
