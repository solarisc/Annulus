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
        parts.push(format!("-J {}", jump.target()));
    }
    let (user, address) = (host.username.trim(), host.address.trim());
    parts.push(if user.is_empty() {
        address.to_owned()
    } else {
        format!("{user}@{address}")
    });
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use annulus_vault::hosts::Secret;

    use super::*;

    #[test]
    fn the_ssh_command_has_port_jump_and_user_but_never_a_password() {
        let dir = std::env::temp_dir().join(format!("annulus-connect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut vault, _) = Vault::create(
            dir.join("vault.anv"),
            "pw",
            annulus_vault::crypto::KdfParams {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
        )
        .unwrap();
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
}
