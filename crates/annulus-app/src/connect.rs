//! What a connection runs: a typed `user@host:port` or a saved host turned into a
//! [`Program`] for a session, with what the host inherits from its groups, its keychain key,
//! saved password and jump hosts. Shared by both UIs.

use annulus_session::{Hop, Password, Program, SshFiles, Target, local_user};
use annulus_vault::hosts::Host;
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
