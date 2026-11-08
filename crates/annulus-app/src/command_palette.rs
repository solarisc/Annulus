//! The command palette (F-APP-005, Ctrl+Shift+P): one box to reach saved hosts first, then
//! open tabs, snippets and commands. Typing narrows the list with a fuzzy match; the
//! arrows choose and Enter runs, so it never needs the mouse.

use std::borrow::Cow;

use crate::native::{Section, settings};

/// What choosing an item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Connect to what was typed (`user@host:port`), saved or not (F-APP-006).
    QuickConnect(String),
    /// Connect to the saved host with this id.
    Connect(u64),
    /// Open SFTP for the saved host with this id.
    HostSftp(u64),
    /// Open the saved host in its editor.
    EditHost(u64),
    /// Open the saved key in its editor.
    EditKey(u64),
    /// Open the saved group in its editor.
    EditGroup(String),
    /// Open the saved snippet in its editor.
    EditSnippet(u64),
    /// Open the saved forwarding rule in its editor.
    EditForward(u64),
    /// Bring the tab at this index to the front.
    Tab(usize),
    /// Type the snippet with this id into the terminal in front.
    Snippet(u64),
    /// Open a local terminal.
    NewLocal,
    /// Lock the vault.
    Lock,
    /// Show a page of the Vault tab.
    Navigate(Section),
    /// Show one page of Settings.
    Settings(settings::Page),
}

/// One line of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it is called.
    pub label: String,
    /// Its kind, shown dimmed: "Host", "Tab", "Snippet" or "Command".
    pub kind: &'static str,
    /// What choosing it does.
    pub action: Action,
    /// What `Edit …` does for this item, when it can be edited.
    pub edit: Option<Action>,
    /// What `SFTP …` does for this item, when it has files.
    pub sftp: Option<Action>,
    /// Extra words that can find this item without changing the label shown.
    pub keywords: String,
}

impl Item {
    /// One searchable item with no named actions or hidden words yet.
    #[must_use]
    pub fn new(label: impl Into<String>, kind: &'static str, action: Action) -> Self {
        Self {
            label: label.into(),
            kind,
            action,
            edit: None,
            sftp: None,
            keywords: String::new(),
        }
    }

    /// Adds words that search can match without showing them in the result.
    #[must_use]
    pub fn with_keywords(mut self, words: impl Into<String>) -> Self {
        self.keywords = words.into();
        self
    }

    /// Adds an `Edit …` action.
    #[must_use]
    pub fn editable(mut self, action: Action) -> Self {
        self.edit = Some(action);
        self
    }

    /// Adds an `SFTP …` action.
    #[must_use]
    pub fn with_sftp(mut self, action: Action) -> Self {
        self.sftp = Some(action);
        self
    }
}

/// One result, with a named action applied when the query asked for one.
pub struct Match<'a> {
    /// The source item.
    pub item: &'a Item,
    /// What Enter does.
    pub action: Action,
    prefix: Option<&'static str>,
}

impl Match<'_> {
    /// The label shown in the result.
    #[must_use]
    pub fn label(&self) -> Cow<'_, str> {
        match self.prefix {
            Some(prefix) if !self.item.label.eq_ignore_ascii_case(prefix) => {
                Cow::Owned(format!("{prefix} {}", self.item.label))
            }
            _ => Cow::Borrowed(&self.item.label),
        }
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Edit,
    Sftp,
    Connect,
    Open,
}

/// A leading action word and the name after it. Parentheses around a name are optional.
fn intent(query: &str) -> (Option<Verb>, &str) {
    let query = query.trim();
    let (first, rest) = query.split_once(char::is_whitespace).unwrap_or((query, ""));
    let verb = match first.to_ascii_lowercase().as_str() {
        "edit" => Some(Verb::Edit),
        "sftp" => Some(Verb::Sftp),
        "connect" => Some(Verb::Connect),
        "open" | "go" => Some(Verb::Open),
        _ => None,
    };
    let rest = if verb.is_some() {
        rest.trim()
            .strip_prefix('(')
            .unwrap_or(rest.trim())
            .strip_suffix(')')
            .unwrap_or_else(|| rest.trim().strip_prefix('(').unwrap_or(rest.trim()))
            .trim()
    } else {
        query
    };
    (verb, rest)
}

/// Searches `items`, applying a leading action word such as `Edit` or `SFTP`.
#[must_use]
pub fn search<'a>(query: &str, items: &'a [Item]) -> Vec<Match<'a>> {
    let (verb, words) = intent(query);
    let mut scored: Vec<(i64, usize, Match<'a>)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let (action, prefix) = match verb {
                Some(Verb::Edit) => (item.edit.clone()?, Some("Edit")),
                Some(Verb::Sftp) => (item.sftp.clone()?, Some("SFTP")),
                Some(Verb::Connect) => match item.action {
                    Action::Connect(_) | Action::QuickConnect(_) => {
                        (item.action.clone(), Some("Connect"))
                    }
                    _ => return None,
                },
                Some(Verb::Open) | None => (item.action.clone(), None),
            };
            // Prefer the visible label, then hidden words. Do not build a joined string for
            // each item on each key press: a vault can hold thousands of hosts.
            let score = score(words, &item.label)
                .map(|score| score + 100)
                .into_iter()
                .chain(score(words, &item.keywords))
                .max()?;
            Some((
                score,
                index,
                Match {
                    item,
                    action,
                    prefix,
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, found)| found).collect()
}

/// How well `query` matches `text`: its letters in order (any case), higher for a match at
/// the start, at word starts and in runs; `None` when it does not match at all.
#[must_use]
pub fn score(query: &str, text: &str) -> Option<i64> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let (mut at, mut total, mut previous) = (0, 0i64, None::<usize>);
    for wanted in query {
        let found = (at..text.len()).find(|&index| text[index] == wanted)?;
        total += 1;
        if found == 0 {
            total += 8;
        } else if !text[found - 1].is_alphanumeric() {
            total += 5;
        }
        if previous.is_some_and(|last| last + 1 == found) {
            total += 3;
        }
        previous = Some(found);
        at = found + 1;
    }
    // Shorter texts win ties: "db" beats "db-backup-old" for "db".
    Some(total * 100 - i64::try_from(text.len()).unwrap_or(i64::MAX).min(99))
}

/// What was typed, when it reads as an SSH target (`user@host`, `host:port`, an address
/// or a dotted name) rather than words to search for.
#[must_use]
pub fn typed_target(query: &str) -> Option<&str> {
    let query = query.trim();
    let looks = !query.is_empty()
        && !query.contains(char::is_whitespace)
        && query.contains(['@', ':', '.']);
    looks.then_some(query)
}

/// `items` that match `query`, best first; with no query, in their own order (hosts first).
#[must_use]
#[cfg(test)]
pub fn rank<'a>(query: &str, items: &'a [Item]) -> Vec<&'a Item> {
    let mut scored: Vec<(i64, usize, &Item)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((score(query, &item.label)?, index, item)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> Item {
        Item::new(label, "Host", Action::NewLocal)
    }

    #[test]
    fn a_typed_target_is_offered_as_a_quick_connect() {
        assert_eq!(
            typed_target(" root@10.0.0.9:2222 "),
            Some("root@10.0.0.9:2222")
        );
        assert_eq!(typed_target("db.example.com"), Some("db.example.com"));
        assert_eq!(typed_target("prod db"), None, "words are a search");
        assert_eq!(typed_target("settings"), None);
    }

    #[test]
    fn letters_in_order_match_and_better_matches_come_first() {
        assert!(score("pdb", "prod-db").is_some());
        assert_eq!(score("dbp", "prod-db"), None, "order matters");
        assert!(score("PROD", "prod-db").is_some(), "any case");
        let items = [
            item("db-backup-old"),
            item("web-prod"),
            item("db"),
            item("staging"),
        ];
        let labels: Vec<&str> = rank("db", &items)
            .iter()
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(labels, ["db", "db-backup-old"]);
        let all: Vec<&str> = rank("", &items).iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            all,
            ["db-backup-old", "web-prod", "db", "staging"],
            "no query: as given"
        );
        let pair = [item("web-prod"), item("ape")];
        let word: Vec<&str> = rank("p", &pair).iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            word,
            ["web-prod", "ape"],
            "a word start beats the middle of a word"
        );
    }

    #[test]
    fn a_named_action_scopes_the_search_and_accepts_parentheses() {
        let mut host = item("web-prod");
        host.edit = Some(Action::EditHost(7));
        host.sftp = Some(Action::HostSftp(7));
        let key = Item {
            label: "deploy key".into(),
            kind: "Key",
            action: Action::Navigate(crate::native::Section::Keychain),
            edit: Some(Action::EditKey(9)),
            sftp: None,
            keywords: "credential ssh".into(),
        };
        let items = [host, key];

        let edit = search("Edit (WEB prod)", &items);
        assert_eq!(edit.len(), 1);
        assert_eq!(edit[0].action, Action::EditHost(7));
        assert_eq!(edit[0].label(), "Edit web-prod");

        let sftp = search("sftp web", &items);
        assert_eq!(sftp.len(), 1);
        assert_eq!(sftp[0].action, Action::HostSftp(7));
        assert_eq!(sftp[0].label(), "SFTP web-prod");

        let key = search("edit credential", &items);
        assert_eq!(key.len(), 1);
        assert_eq!(key[0].action, Action::EditKey(9));
    }

    #[test]
    fn hidden_words_find_a_page_without_changing_its_name() {
        let hosts = Item {
            label: "Hosts".into(),
            kind: "Page",
            action: Action::Navigate(crate::native::Section::Hosts),
            edit: None,
            sftp: None,
            keywords: "vault servers connections".into(),
        };
        let items = [hosts];
        let found = search("vault", &items);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label(), "Hosts");
    }

    #[test]
    fn search_finds_one_host_in_a_vault_of_five_thousand() {
        let items: Vec<Item> = (0_u64..5_000)
            .map(|id| {
                Item::new(format!("host-{id:04}"), "Host", Action::Connect(id))
                    .with_keywords(format!("10.20.{}.{}", id / 256, id % 256))
                    .editable(Action::EditHost(id))
            })
            .collect();
        let found = search("edit host-4999", &items);
        assert_eq!(found[0].action, Action::EditHost(4_999));
        assert_eq!(found[0].label(), "Edit host-4999");
    }
}
