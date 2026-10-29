//! OpenSSH client config (`~/.ssh/config`) import and export (F-IMP-001, F-IMP-003).
//!
//! Import follows ssh's own rule: for each host, every `Host` block whose patterns match it
//! is read in order, and the first value of each keyword wins. `Include` is followed
//! (relative to `~/.ssh`, with `*` in file names), `Match` blocks are skipped since they
//! depend on runtime conditions, and wildcard-only `Host` lines give defaults, not hosts.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use annulus_vault::forwards::{ForwardKind, PortForward};
use annulus_vault::hosts::{DEFAULT_PORT, Host};

/// One `Host` block.
#[derive(Debug, Default)]
struct Block {
    patterns: Vec<String>,
    /// Keyword (lowercase) and its arguments, in file order.
    options: Vec<(String, String)>,
}

/// A host found in the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The alias after `Host`.
    pub alias: String,
    /// `HostName`, or the alias.
    pub address: String,
    /// `Port`.
    pub port: u16,
    /// `User`.
    pub user: Option<String>,
    /// `IdentityFile` entries, `~` expanded.
    pub identity_files: Vec<PathBuf>,
    /// `ProxyJump`, as written.
    pub proxy_jump: Option<String>,
    /// `LocalForward`, `RemoteForward` and `DynamicForward` lines, as rules.
    pub forwards: Vec<FoundForward>,
}

/// A forward line, before it has a host and an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundForward {
    /// Local, remote or dynamic.
    pub kind: ForwardKind,
    /// Listen address.
    pub bind_address: String,
    /// Listen port.
    pub bind_port: u16,
    /// Destination host (not for dynamic).
    pub destination_host: String,
    /// Destination port (not for dynamic).
    pub destination_port: u16,
}

/// Reads files for `Include`: the text of a file, or `None` if it cannot be read.
pub type ReadFile<'a> = &'a dyn Fn(&Path) -> Option<String>;

/// Lists a directory for `Include` patterns.
pub type ListDir<'a> = &'a dyn Fn(&Path) -> Vec<PathBuf>;

/// Parses `text` (the content of `~/.ssh/config`), with `ssh_dir` for relative includes
/// and `home` for `~`.
#[must_use]
pub fn parse(
    text: &str,
    ssh_dir: &Path,
    home: &Path,
    read: ReadFile<'_>,
    list: ListDir<'_>,
) -> Vec<Found> {
    let mut blocks = vec![Block {
        patterns: vec!["*".into()],
        options: Vec::new(),
    }];
    read_blocks(text, ssh_dir, home, read, list, &mut blocks, 0);
    let mut aliases: Vec<String> = Vec::new();
    for block in &blocks {
        for pattern in &block.patterns {
            let concrete = !pattern.contains(['*', '?', '!']);
            if concrete && !aliases.contains(pattern) {
                aliases.push(pattern.clone());
            }
        }
    }
    aliases
        .into_iter()
        .map(|alias| resolve(&alias, &blocks, home))
        .collect()
}

fn read_blocks(
    text: &str,
    ssh_dir: &Path,
    home: &Path,
    read: ReadFile<'_>,
    list: ListDir<'_>,
    blocks: &mut Vec<Block>,
    depth: usize,
) {
    let mut skipping = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, value) = split_option(line);
        match keyword.as_str() {
            "host" => {
                skipping = false;
                blocks.push(Block {
                    patterns: value.split_whitespace().map(str::to_owned).collect(),
                    options: Vec::new(),
                });
            }
            "match" => skipping = true,
            "include" if depth < 8 => {
                for pattern in value.split_whitespace() {
                    for path in expand_include(pattern, ssh_dir, home, list) {
                        if let Some(included) = read(&path) {
                            read_blocks(&included, ssh_dir, home, read, list, blocks, depth + 1);
                        }
                    }
                }
            }
            _ if skipping => {}
            _ => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((keyword, value));
                }
            }
        }
    }
}

/// `Keyword value`, `Keyword=value` or `Keyword = "value"`: the keyword lowercased and the
/// value without surrounding quotes.
fn split_option(line: &str) -> (String, String) {
    let split = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..split].to_ascii_lowercase();
    let value = line[split..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim()
        .trim_matches('"')
        .to_owned();
    (keyword, value)
}

fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    }
}

fn expand_include(pattern: &str, ssh_dir: &Path, home: &Path, list: ListDir<'_>) -> Vec<PathBuf> {
    let path = expand_tilde(pattern, home);
    let path = if path.is_absolute() {
        path
    } else {
        ssh_dir.join(path)
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut matches: Vec<PathBuf> = list(dir)
        .into_iter()
        .filter(|candidate| {
            candidate
                .file_name()
                .is_some_and(|file| glob(&name, &file.to_string_lossy()))
        })
        .collect();
    matches.sort();
    matches
}

/// ssh's pattern match: `*` any run, `?` one character.
fn glob(pattern: &str, text: &str) -> bool {
    fn go(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            Some(('?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((c, rest)) => text
                .split_first()
                .is_some_and(|(t, tail)| t.eq_ignore_ascii_case(c) && go(rest, tail)),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    go(&pattern, &text)
}

/// Whether a `Host` line's patterns match `alias`: any positive match and no negated one.
fn matches(patterns: &[String], alias: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob(negated, alias) {
                return false;
            }
        } else if glob(pattern, alias) {
            matched = true;
        }
    }
    matched
}

fn resolve(alias: &str, blocks: &[Block], home: &Path) -> Found {
    let mut found = Found {
        alias: alias.to_owned(),
        address: alias.to_owned(),
        port: DEFAULT_PORT,
        user: None,
        identity_files: Vec::new(),
        proxy_jump: None,
        forwards: Vec::new(),
    };
    let (mut address, mut port, mut user) = (None, None, None);
    for block in blocks
        .iter()
        .filter(|block| matches(&block.patterns, alias))
    {
        for (keyword, value) in &block.options {
            match keyword.as_str() {
                "hostname" => {
                    address.get_or_insert_with(|| value.replace("%h", alias));
                }
                "port" => {
                    if port.is_none() {
                        port = value.parse::<u16>().ok();
                    }
                }
                "user" => {
                    user.get_or_insert_with(|| value.clone());
                }
                // These accumulate, as in ssh.
                "identityfile" => found.identity_files.push(expand_tilde(value, home)),
                "proxyjump" => {
                    if found.proxy_jump.is_none() && !value.eq_ignore_ascii_case("none") {
                        found.proxy_jump = Some(value.clone());
                    }
                }
                "localforward" | "remoteforward" | "dynamicforward" => {
                    if let Some(forward) = parse_forward(keyword, value) {
                        found.forwards.push(forward);
                    }
                }
                _ => {}
            }
        }
    }
    found.address = address.unwrap_or(found.address);
    found.port = port.unwrap_or(DEFAULT_PORT);
    found.user = user;
    found
}

/// `[bind:]port`, with `[v6]` brackets allowed.
fn split_endpoint(text: &str) -> Option<(String, u16)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_owned(), port.parse().ok()?));
    }
    match text.rsplit_once(':') {
        Some((host, port)) => Some((host.to_owned(), port.parse().ok()?)),
        None => Some((String::new(), text.parse().ok()?)),
    }
}

fn parse_forward(keyword: &str, value: &str) -> Option<FoundForward> {
    let mut parts = value.split_whitespace();
    let (bind_address, bind_port) = split_endpoint(parts.next()?)?;
    let (kind, destination_host, destination_port) = if keyword == "dynamicforward" {
        (ForwardKind::Dynamic, String::new(), 0)
    } else {
        let (host, port) = split_endpoint(parts.next()?)?;
        let kind = if keyword == "localforward" {
            ForwardKind::Local
        } else {
            ForwardKind::Remote
        };
        (kind, host, port)
    };
    let bind_address = match (bind_address.as_str(), kind) {
        ("", ForwardKind::Remote) => "localhost".to_owned(),
        ("" | "localhost", _) => "127.0.0.1".to_owned(),
        ("*", _) => "0.0.0.0".to_owned(),
        _ => bind_address,
    };
    Some(FoundForward {
        kind,
        bind_address,
        bind_port,
        destination_host,
        destination_port,
    })
}

impl Found {
    /// The host as a saved Annulus host with id `id`.
    #[must_use]
    pub fn to_host(&self, id: u64) -> Host {
        Host {
            id,
            label: self.alias.clone(),
            address: self.address.clone(),
            port: self.port,
            username: self.user.clone().unwrap_or_default(),
            tags: vec!["ssh_config".into()],
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
        }
    }

    /// Its forward lines as rules through host `host_id`, numbered from `first_id`.
    #[must_use]
    pub fn to_rules(&self, host_id: u64, first_id: u64) -> Vec<PortForward> {
        self.forwards
            .iter()
            .zip(first_id..)
            .map(|(forward, id)| PortForward {
                label: format!("{} {}", self.alias, forward.bind_port),
                kind: forward.kind,
                bind_address: forward.bind_address.clone(),
                bind_port: forward.bind_port,
                destination_host: forward.destination_host.clone(),
                destination_port: forward.destination_port,
                ..PortForward::new(id, host_id)
            })
            .collect()
    }
}

/// `hosts` as an OpenSSH config: alias, address, port and user only. Saved passwords are
/// never written.
#[must_use]
pub fn export(hosts: &[Host]) -> String {
    let mut out = String::from("# Exported from Annulus. Passwords are not included.\n");
    for host in hosts {
        let alias: String = host
            .label
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .collect();
        let _ = write!(
            out,
            "\nHost {alias}\n    HostName {}\n",
            host.address.trim()
        );
        if host.port != DEFAULT_PORT {
            let _ = writeln!(out, "    Port {}", host.port);
        }
        if !host.username.trim().is_empty() {
            let _ = writeln!(out, "    User {}", host.username.trim());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn parse_with(text: &str, files: &[(&str, &str)]) -> Vec<Found> {
        let files: HashMap<PathBuf, String> = files
            .iter()
            .map(|(path, text)| (PathBuf::from(path), (*text).to_owned()))
            .collect();
        let names: Vec<PathBuf> = files.keys().cloned().collect();
        parse(
            text,
            Path::new("/home/me/.ssh"),
            Path::new("/home/me"),
            &|path| files.get(path).cloned(),
            &|dir| {
                names
                    .iter()
                    .filter(|name| name.parent() == Some(dir))
                    .cloned()
                    .collect()
            },
        )
    }

    #[test]
    fn the_first_value_wins_and_wildcards_give_defaults() {
        let found = parse_with(
            "Host prod-db db2\n  HostName 10.0.4.12\n  Port 2222\n\nHost prod-*\n  User admin\n  Port 22\n\nHost *\n  User nobody\n  IdentityFile ~/.ssh/id_work\n",
            &[],
        );
        assert_eq!(found.len(), 2);
        let prod = &found[0];
        assert_eq!(prod.alias, "prod-db");
        assert_eq!((prod.address.as_str(), prod.port), ("10.0.4.12", 2222));
        assert_eq!(prod.user.as_deref(), Some("admin"));
        assert_eq!(
            prod.identity_files,
            [PathBuf::from("/home/me").join(".ssh/id_work")]
        );
        assert_eq!(
            found[1].user.as_deref(),
            Some("nobody"),
            "db2 is not prod-*"
        );
    }

    #[test]
    fn includes_negations_and_match_blocks() {
        let found = parse_with(
            "Include config.d/*\nHost * !bastion\n  ProxyJump bastion\nMatch exec \"true\"\n  User ignored\n",
            &[
                (
                    "/home/me/.ssh/config.d/a.conf",
                    "Host bastion\n  HostName bastion.example.com\n",
                ),
                (
                    "/home/me/.ssh/config.d/b.conf",
                    "Host app\n  HostName=app.internal\n",
                ),
            ],
        );
        let names: Vec<&str> = found.iter().map(|f| f.alias.as_str()).collect();
        assert_eq!(names, ["bastion", "app"]);
        assert_eq!(found[0].proxy_jump, None, "negated");
        assert_eq!(found[1].proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(found[1].address, "app.internal");
        assert_eq!(found[1].user, None, "Match blocks are skipped");
    }

    #[test]
    fn forward_lines_become_rules() {
        let found = parse_with(
            "Host box\n  LocalForward 8080 localhost:80\n  RemoteForward 9000 127.0.0.1:3000\n  DynamicForward *:1080\n",
            &[],
        );
        let rules = found[0].to_rules(7, 1);
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].summary(), "127.0.0.1:8080 \u{2192} localhost:80");
        assert_eq!(rules[1].kind, ForwardKind::Remote);
        assert_eq!(rules[1].bind_address, "localhost");
        assert_eq!(rules[2].summary(), "SOCKS on 0.0.0.0:1080");
        assert!(rules.iter().all(|rule| rule.host_id == 7));
    }

    #[test]
    fn export_writes_no_secrets_and_round_trips() {
        let mut host = found_host();
        host.password = Some(annulus_vault::hosts::Secret::new("hunter2"));
        let text = export(&[host]);
        assert!(!text.contains("hunter2"));
        let back = parse_with(&text, &[]);
        assert_eq!(
            (
                back[0].alias.as_str(),
                back[0].address.as_str(),
                back[0].port
            ),
            ("prod-db", "10.0.4.12", 2222)
        );
        assert_eq!(back[0].user.as_deref(), Some("admin"));
    }

    fn found_host() -> Host {
        Host {
            id: 1,
            label: "prod-db".into(),
            address: "10.0.4.12".into(),
            port: 2222,
            username: "admin".into(),
            tags: Vec::new(),
            password: None,
            key_id: None,
            jump_id: None,
            proxy: None,
        }
    }
}
