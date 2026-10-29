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

/// Removes `entry` from the file at `path`, atomically. It is found by what it says (host
/// names, kind, fingerprint), not by a line number that may have moved since it was read.
///
/// # Errors
/// The file cannot be read or written, or no longer has that entry; it is left as it was.
pub fn forget(path: &Path, entry: &KnownHost) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let line = parse(&text)
        .into_iter()
        .find(|found| {
            found.hosts == entry.hosts
                && found.algorithm == entry.algorithm
                && found.fingerprint == entry.fingerprint
        })
        .map(|found| found.line)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "that key is no longer in the file",
            )
        })?;
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
    replace(path, out.as_bytes())
}

/// Replaces `path` with `bytes`: written to a temporary file (owner-only on Unix: the file says
/// which servers this user reaches), flushed to disk, then renamed over it.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
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
/// A host that already has a key is never given a second one, hashed (`|1|…`) lines on
/// either side included: the host-key check treats a second key of the same type as a
/// changed key, so it would grant nothing and lock the host out.
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
    // Only real entries: a key this app can read.
    let candidates: Vec<(String, KnownHost)> = source
        .lines()
        .filter_map(|line| {
            let entry = parse(line).into_iter().next()?;
            (!entry.fingerprint.is_empty()).then(|| (normalized(line), entry))
        })
        .collect();
    let hashed_known = hashed_matches(into, &names, &candidates)?;
    let mut result = Imported::default();
    for (index, (line, entry)) in candidates.into_iter().enumerate() {
        let known = hashed_known.contains(&(index + 1))
            || entry.hosts.iter().any(|host| names.contains(host))
            || entry
                .hosts
                .iter()
                .filter(|host| *host != "(hashed)")
                .any(|host| has_key(into, host));
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
        replace(into, text.as_bytes())?;
    }
    Ok(result)
}

/// Splits `host` or `[host]:port` into the name and port the host-key check looks up.
fn host_and_port(name: &str) -> (&str, u16) {
    name.strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
        .and_then(|(host, port)| Some((host, port.parse().ok()?)))
        .unwrap_or((name, 22))
}

/// Whether the file at `path` already holds a key for host name `name` (hashed lines
/// too). A file the check cannot read counts as holding one: better skip than add.
fn has_key(path: &Path, name: &str) -> bool {
    let (host, port) = host_and_port(name);
    russh::keys::known_hosts::known_host_keys_path(host, port, path)
        .map_or(true, |keys| !keys.is_empty())
}

/// The candidates (by position, from 1) whose hashed host names match one of `names`,
/// found with the host-key check's own matching through a scratch file next to `into`.
fn hashed_matches(
    into: &Path,
    names: &[String],
    candidates: &[(String, KnownHost)],
) -> std::io::Result<Vec<usize>> {
    if names.is_empty() || candidates.is_empty() {
        return Ok(Vec::new());
    }
    let scratch = into.with_extension("import");
    let mut body = candidates
        .iter()
        .map(|(line, _)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    body.push('\n');
    std::fs::write(&scratch, body)?;
    let mut matched = Vec::new();
    for name in names {
        let (host, port) = host_and_port(name);
        if let Ok(keys) = russh::keys::known_hosts::known_host_keys_path(host, port, &scratch) {
            matched.extend(keys.into_iter().map(|(line, _)| line));
        }
    }
    let _ = std::fs::remove_file(&scratch);
    Ok(matched)
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

    /// An OpenSSH hashed host field for `host`: `|1|salt|HMAC-SHA1(salt, host)`.
    fn hashed(host: &str) -> String {
        use base64::Engine as _;
        use hmac::{KeyInit, Mac};
        let salt = [7u8; 20];
        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt).unwrap();
        mac.update(host.as_bytes());
        let b64 = base64::engine::general_purpose::STANDARD;
        format!(
            "|1|{}|{}",
            b64.encode(salt),
            b64.encode(mac.finalize().into_bytes())
        )
    }

    #[test]
    fn a_hashed_line_for_a_host_with_a_key_is_left_out_and_the_check_still_passes() {
        const OTHER: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIGtOO8V0K1MT4rhhh35kzjb6OArnlu8NDuGa6nLdDgQS";
        let dir = std::env::temp_dir().join(format!("annulus-known-hashed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("known_hosts");
        let theirs = dir.join("openssh_known_hosts");
        std::fs::write(
            &ours,
            format!("web ssh-ed25519 {ED25519}\n[jump]:2222 ssh-ed25519 {ED25519}\n"),
        )
        .unwrap();
        std::fs::write(
            &theirs,
            format!(
                "{} ssh-ed25519 {OTHER}\n{} ssh-ed25519 {OTHER}\n{} ssh-ed25519 {OTHER}\n",
                hashed("web"),
                hashed("[jump]:2222"),
                hashed("new-host")
            ),
        )
        .unwrap();
        assert_eq!(
            import(&ours, &theirs).unwrap(),
            Imported {
                added: 1,
                skipped: 2
            }
        );
        // The key Annulus pinned for web still passes the host-key check.
        let pinned = PublicKey::from_openssh(&format!("ssh-ed25519 {ED25519}")).unwrap();
        assert_eq!(
            russh::keys::check_known_hosts_path("web", 22, &pinned, &ours).ok(),
            Some(true)
        );
        let new = PublicKey::from_openssh(&format!("ssh-ed25519 {OTHER}")).unwrap();
        assert_eq!(
            russh::keys::check_known_hosts_path("new-host", 22, &new, &ours).ok(),
            Some(true),
            "the new host's hashed key works"
        );
        assert!(
            !dir.join("known_hosts.import").exists(),
            "no scratch file left"
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
        let a = read(&path).remove(0);
        // A line added above it since it was read: the right entry still goes.
        std::fs::write(
            &path,
            format!("c ssh-ed25519 {ED25519}\na ssh-ed25519 {ED25519}\nb ssh-ed25519 {ED25519}\n"),
        )
        .unwrap();
        forget(&path, &a).unwrap();
        let left: Vec<_> = read(&path).into_iter().map(|entry| entry.hosts).collect();
        assert_eq!(left, [["c"], ["b"]]);
        assert!(forget(&path, &a).is_err(), "gone already");
        assert_eq!(read(&dir.join("missing")), []);
    }
}
