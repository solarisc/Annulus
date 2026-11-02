//! The Keychain screen (F-KEY-001…003): SSH keys kept in the vault. Generate an Ed25519
//! key, import one by pasting it (with its passphrase if it has one), copy a public key for
//! `authorized_keys`, install it on a saved host (F-KEY-004), and delete keys. Hosts pick
//! a key in the host editor.

use annulus_ssh::keys::{self, KeyPair};
use annulus_ssh::remote::without_clixml;
use annulus_vault::hosts::Secret;
use annulus_vault::keychain::SshKey;
use annulus_vault::vault::Vault;

use crate::ssh_task::Outcome;

/// What a key command printed, on one line for the message: PowerShell's CLIXML and our
/// own error prefix left out. `None` when it printed nothing.
fn said(output: &str) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(|line| {
            let line = line.trim();
            line.strip_prefix(keys::FAILED.trim_end())
                .map_or(line, str::trim_start)
        })
        .filter(|line| !line.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn status_text(status: Option<u32>) -> String {
    status.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
}

/// What an install's outcome means: done, or why not. Done only when the command said so
/// ([`keys::INSTALLED`]): a shell that ran nothing of ours can still end with status 0.
pub(crate) fn install_result(outcome: Outcome) -> Result<(), String> {
    let exec = outcome?;
    let output = without_clixml(&String::from_utf8_lossy(&exec.output));
    let installed = output.lines().any(|line| line.trim() == keys::INSTALLED);
    if exec.status == Some(0) && installed {
        return Ok(());
    }
    Err(match said(&output) {
        Some(reason) => format!("The host could not add the key: {reason}"),
        None if exec.status == Some(0) => {
            "The host could not add the key: its shell ran something else.".to_owned()
        }
        None => format!(
            "The host could not add the key (exit status {}).",
            status_text(exec.status)
        ),
    })
}

/// What taking a key off a server came to ([`keys::remove_commands`]): how many lines of its
/// `authorized_keys` held the key (0 when none did), or why it is still there.
pub(crate) fn removal_result(outcome: Outcome) -> Result<usize, String> {
    let exec = outcome?;
    let output = without_clixml(&String::from_utf8_lossy(&exec.output));
    let count = output.lines().find_map(|line| {
        line.trim()
            .strip_prefix(keys::REMOVED.trim_end())
            .and_then(|count| count.trim().parse().ok())
    });
    // A Windows server whose shell is PowerShell turns every failing status into 1, so
    // the command's own word counts, not only its status.
    let still_there = output.lines().any(|line| line.trim() == keys::STILL_THERE);
    match (exec.status, count) {
        (Some(0), Some(count)) => Ok(count),
        (Some(3), _) => Err("the key was still there afterwards".into()),
        _ if still_there => Err("the key was still there afterwards".into()),
        _ => {
            Err(said(&output)
                .unwrap_or_else(|| format!("exit status {}", status_text(exec.status))))
        }
    }
}

/// What deleting key `key` came to, as `(problem, message)`: each server it was to come off
/// (`servers`, by name, with how that went), and those left alone (`kept`). Any server that
/// still has it keeps the key in the vault too, so the way in it gives is never lost track of.
pub(crate) fn removal_summary(
    key: &str,
    servers: &[(String, Result<usize, String>)],
    kept: &[String],
) -> (bool, String) {
    let failed: Vec<String> = servers
        .iter()
        .filter_map(|(name, result)| result.as_ref().err().map(|why| format!("{name} ({why})")))
        .collect();
    if !failed.is_empty() {
        return (
            true,
            format!(
                "Kept {key}: could not take it off {}. Delete again to retry, or untick the \
                 servers to delete it here only.",
                failed.join(", ")
            ),
        );
    }
    let names = |found: bool| -> Vec<&str> {
        servers
            .iter()
            .filter(|(_, result)| matches!(result, Ok(count) if (*count > 0) == found))
            .map(|(name, _)| name.as_str())
            .collect()
    };
    let mut message = vec![format!("Deleted {key}.")];
    let (taken, absent) = (names(true), names(false));
    if !taken.is_empty() {
        message.push(format!("Taken off {}.", taken.join(", ")));
    }
    if !absent.is_empty() {
        message.push(format!("Not found on {}.", absent.join(", ")));
    }
    if !kept.is_empty() {
        message.push(format!(
            "Still in the authorized_keys of {}.",
            kept.join(", ")
        ));
    }
    (false, message.join(" "))
}

/// The key as stored, from a generated or imported pair.
pub(crate) fn stored(vault: &Vault, label: &str, pair: &KeyPair) -> SshKey {
    SshKey {
        id: vault.next_key_id(),
        label: label.trim().to_owned(),
        private: Secret::new(pair.private.as_str()),
        public: pair.public.clone(),
        fingerprint: pair.fingerprint.clone(),
    }
}

/// The default comment of a generated key: `user@computer`.
pub(crate) fn comment() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "annulus".into());
    let computer = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "device".into());
    format!("{user}@{computer}")
}

/// Sets the host to log in with `key` after it was installed there; the message to show.
pub(crate) fn link_key(vault: &mut Vault, host_id: Option<u64>, key: &SshKey) -> String {
    let Some(mut host) =
        host_id.and_then(|id| vault.hosts().iter().find(|host| host.id == id).cloned())
    else {
        return format!("Installed {}.", key.label);
    };
    host.key_id = Some(key.id);
    // The key went to this device's user there: other devices must log in as them too.
    crate::connect::pin_user(vault, &mut host, &annulus_session::local_user());
    let label = host.label.clone();
    match vault.upsert_host(host) {
        Ok(()) => format!(
            "Installed {} on {label}; it now logs in with this key.",
            key.label
        ),
        Err(error) => format!(
            "Installed {} on {label}, but could not save the host: {error}",
            key.label
        ),
    }
}

#[cfg(test)]
mod tests {
    use annulus_ssh::keys;

    use super::*;

    #[test]
    fn an_install_succeeds_only_when_the_command_says_so_with_status_zero() {
        let exec = |status, output: &[u8]| {
            Ok(annulus_ssh::Exec {
                status,
                output: output.to_vec(),
            })
        };
        assert_eq!(
            install_result(exec(Some(0), b"annulus-installed\n")),
            Ok(())
        );
        assert_eq!(
            install_result(exec(Some(0), b"annulus-installed\r\n")),
            Ok(()),
            "from Windows"
        );
        // A shell that ran nothing of ours (a menu, another kind of shell) can end with 0.
        assert_eq!(
            install_result(exec(Some(0), b"")),
            Err("The host could not add the key: its shell ran something else.".into())
        );
        assert_eq!(
            install_result(exec(Some(1), b"touch: Read-only file system\n")),
            Err("The host could not add the key: touch: Read-only file system".into())
        );
        // cmd's lines and PowerShell's CLIXML come back as one plain line.
        assert_eq!(
            install_result(exec(
                Some(1),
                b"'umask' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n"
            )),
            Err("The host could not add the key: 'umask' is not recognized as an internal or \
                 external command, operable program or batch file."
                .into())
        );
        assert_eq!(
            install_result(exec(
                Some(1),
                b"annulus-error: Access to the path is denied.\r\n#< CLIXML\r\n<Objs Version=\"1.1.0.1\"></Objs>\r\n"
            )),
            Err("The host could not add the key: Access to the path is denied.".into())
        );
        assert_eq!(
            install_result(exec(None, b"")),
            Err("The host could not add the key (exit status unknown).".into())
        );
        assert_eq!(install_result(Err("refused".into())), Err("refused".into()));
    }

    #[test]
    fn a_removal_counts_only_on_its_own_word_and_a_server_that_kept_it_keeps_the_key() {
        let exec = |status, output: &[u8]| {
            Ok(annulus_ssh::Exec {
                status,
                output: output.to_vec(),
            })
        };
        assert_eq!(removal_result(exec(Some(0), b"annulus-removed 1\n")), Ok(1));
        assert_eq!(removal_result(exec(Some(0), b"annulus-removed 0\n")), Ok(0));
        // A shell that printed nothing of ours did not run the command (Windows, a menu).
        assert!(removal_result(exec(Some(0), b"Microsoft Windows\n")).is_err());
        assert_eq!(
            removal_result(exec(Some(3), b"")),
            Err("the key was still there afterwards".into())
        );
        // A Windows server whose shell is PowerShell ends a failing command with 1.
        assert_eq!(
            removal_result(exec(Some(1), b"annulus-still-there\r\n")),
            Err("the key was still there afterwards".into())
        );
        assert_eq!(
            removal_result(exec(Some(1), b"annulus-error: Access is denied.\r\n")),
            Err("Access is denied.".into())
        );
        assert_eq!(
            removal_result(exec(Some(2), b"sh: cannot create: Read-only file system\n")),
            Err("sh: cannot create: Read-only file system".into())
        );
        assert_eq!(removal_result(Err("refused".into())), Err("refused".into()));

        let done = |name: &str, result| (name.to_owned(), result);
        assert_eq!(
            removal_summary("work", &[done("web", Ok(1)), done("db", Ok(0))], &[]),
            (
                false,
                "Deleted work. Taken off web. Not found on db.".to_owned()
            )
        );
        let (problem, message) = removal_summary(
            "work",
            &[done("web", Ok(1)), done("db", Err("refused".into()))],
            &[],
        );
        assert!(problem);
        assert!(message.starts_with("Kept work: could not take it off db (refused)."));
        assert_eq!(
            removal_summary("work", &[], &["web".to_owned()]),
            (
                false,
                "Deleted work. Still in the authorized_keys of web.".to_owned()
            )
        );
    }

    fn vault(name: &str) -> Vault {
        let dir =
            std::env::temp_dir().join(format!("annulus-keychain-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Vault::create(
            dir.join("vault.anv"),
            "pw",
            annulus_vault::crypto::KdfParams {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
        )
        .unwrap()
        .0
    }

    #[test]
    fn an_installed_key_becomes_the_hosts_key() {
        let mut vault = vault("link");
        let pair = keys::generate(keys::KeyKind::Ed25519, "me@test").unwrap();
        let key = stored(&vault, "work", &pair);
        vault.upsert_key(key.clone()).unwrap();
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.1".into());
        vault.upsert_host(host.clone()).unwrap();
        assert_eq!(
            link_key(&mut vault, Some(host.id), &key),
            "Installed work on web; it now logs in with this key."
        );
        assert_eq!(vault.hosts()[0].key_id, Some(key.id));
        assert_eq!(link_key(&mut vault, Some(999), &key), "Installed work.");
    }

    #[test]
    fn a_generated_key_is_stored_with_its_public_half_and_a_fresh_id() {
        let mut vault = vault("generate");
        let pair = keys::generate(keys::KeyKind::Ed25519, "me@test").unwrap();
        let key = stored(&vault, "  work  ", &pair);
        assert_eq!(key.label, "work");
        assert_eq!(key.public, pair.public);
        vault.upsert_key(key).unwrap();
        assert_eq!(stored(&vault, "next", &pair).id, 2);
    }
}
