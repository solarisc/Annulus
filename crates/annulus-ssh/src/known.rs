//! The known-hosts file as a list (F-KNOWN-004): each trusted server key with its host
//! names, key type and SHA256 fingerprint, and removing one (after a server is reinstalled,
//! the old key must be forgotten before the new one can be trusted).

use std::path::Path;

use russh::keys::{HashAlg, PublicKey};

/// One trusted key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownHost {
    /// Its line in the file, counting from 1.
    pub line: usize,
    /// The host names it is trusted for (`host`, or `[host]:port` for other ports).
    pub hosts: Vec<String>,
    /// The key type, e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// `SHA256:…`, or empty if the key could not be read.
    pub fingerprint: String,
}

/// The entries of a known-hosts file's text. Comments, blank lines, `@` markers and hashed
/// host names (shown as "(hashed)") are handled; lines that are not entries are skipped.
#[must_use]
pub fn parse(text: &str) -> Vec<KnownHost> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let mut first = fields.next().unwrap_or_default();
        if first.starts_with('@') {
            first = fields.next().unwrap_or_default();
        }
        let (Some(algorithm), Some(key)) = (fields.next(), fields.next()) else {
            continue;
        };
        let hosts = if first.starts_with("|1|") {
            vec!["(hashed)".to_owned()]
        } else {
            first.split(',').map(str::to_owned).collect()
        };
        let fingerprint = PublicKey::from_openssh(&format!("{algorithm} {key}"))
            .map(|key| key.fingerprint(HashAlg::Sha256).to_string())
            .unwrap_or_default();
        entries.push(KnownHost {
            line: index + 1,
            hosts,
            algorithm: algorithm.to_owned(),
            fingerprint,
        });
    }
    entries
}

/// The entries of the known-hosts file at `path`; none if it does not exist yet.
#[must_use]
pub fn read(path: &Path) -> Vec<KnownHost> {
    std::fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Removes line `line` (counting from 1) from the file at `path`, atomically.
///
/// # Errors
/// The file cannot be read or written; it is left as it was.
pub fn forget(path: &Path, line: usize) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(index, _)| index + 1 != line)
        .map(|(_, text)| text)
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, out)?;
    std::fs::rename(&temporary, path)
}

/// What [`import`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Entries added.
    pub added: usize,
    /// Entries left out because a key is already trusted for one of their host names (a
    /// different key is never trusted silently), or the same line is already there.
    pub skipped: usize,
}

/// Adds the entries of another known-hosts file (`from`, usually OpenSSH's
/// `~/.ssh/known_hosts`) to the one at `into` (F-KNOWN-005), atomically. Only hosts with no
/// trusted key yet are added; comments and lines that are not entries are left out.
///
/// # Errors
/// A file cannot be read, or `into` cannot be written; it is left as it was.
pub fn import(into: &Path, from: &Path) -> std::io::Result<Imported> {
    let source = std::fs::read_to_string(from)?;
    let mut text = match std::fs::read_to_string(into) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut names: Vec<String> = parse(&text)
        .into_iter()
        .flat_map(|entry| entry.hosts)
        .filter(|name| name != "(hashed)")
        .collect();
    let mut lines: Vec<String> = text.lines().map(normalized).collect();
    let mut result = Imported::default();
    for line in source.lines() {
        // Only real entries: a key this app can read.
        let Some(entry) = parse(line)
            .into_iter()
            .next()
            .filter(|entry| !entry.fingerprint.is_empty())
        else {
            continue;
        };
        let line = normalized(line);
        let known = entry.hosts.iter().any(|host| names.contains(host));
        if known || lines.contains(&line) {
            result.skipped += 1;
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        names.extend(entry.hosts.into_iter().filter(|name| name != "(hashed)"));
        lines.push(line);
        result.added += 1;
    }
    if result.added > 0 {
        let temporary = into.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, into)?;
    }
    Ok(result)
}

/// `line` with its fields separated by single spaces.
fn normalized(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIHhb4m8U3xS0ycLC2nBqGDbB0kPPw8IVPBCWd2HjWcZ1";

    #[test]
    fn entries_list_hosts_types_and_fingerprints() {
        let text = format!(
            "# comment\n\nprod-db,10.0.4.12 ssh-ed25519 {ED25519}\n[jump]:2222 ssh-ed25519 {ED25519}\n|1|abc=|def= ssh-ed25519 {ED25519}\n@cert-authority *.corp ssh-ed25519 {ED25519}\nbroken\n"
        );
        let entries = parse(&text);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].hosts, ["prod-db", "10.0.4.12"]);
        assert_eq!(entries[0].line, 3);
        assert!(entries[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(entries[1].hosts, ["[jump]:2222"]);
        assert_eq!(entries[2].hosts, ["(hashed)"]);
        assert_eq!(entries[3].hosts, ["*.corp"]);
    }

    #[test]
    fn importing_adds_new_hosts_and_never_a_second_key_for_a_trusted_one() {
        const OTHER: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIGtOO8V0K1MT4rhhh35kzjb6OArnlu8NDuGa6nLdDgQS";
        let dir = std::env::temp_dir().join(format!("annulus-known-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("known_hosts");
        let theirs = dir.join("openssh_known_hosts");
        std::fs::write(&ours, format!("web ssh-ed25519 {ED25519}")).unwrap();
        std::fs::write(
            &theirs,
            format!(
                "# OpenSSH\nweb ssh-ed25519 {OTHER}\ndb,10.0.0.5  ssh-ed25519 {OTHER}\n\
                 |1|abc=|def= ssh-ed25519 {OTHER}\nnot an entry\n"
            ),
        )
        .unwrap();
        let result = import(&ours, &theirs).unwrap();
        assert_eq!(
            result,
            Imported {
                added: 2,
                skipped: 1
            }
        );
        let entries = read(&ours);
        let hosts: Vec<Vec<String>> = entries.iter().map(|e| e.hosts.clone()).collect();
        assert_eq!(
            hosts,
            [vec!["web"], vec!["db", "10.0.0.5"], vec!["(hashed)"]]
        );
        assert_eq!(
            entries[0].fingerprint,
            parse(&format!("x ssh-ed25519 {ED25519}"))[0].fingerprint,
            "the key Annulus trusted for web is unchanged"
        );
        // Again: nothing new.
        assert_eq!(
            import(&ours, &theirs).unwrap(),
            Imported {
                added: 0,
                skipped: 3
            }
        );
    }

    #[test]
    fn forgetting_removes_only_that_line() {
        let dir = std::env::temp_dir().join(format!("annulus-known-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        std::fs::write(
            &path,
            format!("a ssh-ed25519 {ED25519}\nb ssh-ed25519 {ED25519}\n"),
        )
        .unwrap();
        forget(&path, 1).unwrap();
        let left = read(&path);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].hosts, ["b"]);
        assert_eq!(read(&dir.join("missing")), []);
    }
}
