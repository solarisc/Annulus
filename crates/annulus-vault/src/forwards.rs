//! Port-forwarding rules (F-FWD-001): what to forward and through which saved host. They are
//! stored in the encrypted vault ([`crate::vault`]); `annulus-ssh` runs them.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// What a rule forwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// A local port reaches a destination as seen from the server.
    #[default]
    Local,
    /// A local SOCKS5 proxy; each client names its destination.
    Dynamic,
    /// The server listens; its connections reach a destination as seen from this computer.
    Remote,
}

/// One port-forwarding rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    /// Stable identifier within the vault.
    pub id: u64,
    /// The name shown in the list.
    pub label: String,
    /// The saved host whose connection carries the traffic.
    pub host_id: u64,
    /// Local or dynamic.
    #[serde(default)]
    pub kind: ForwardKind,
    /// The address to listen on: local for local and SOCKS rules, on the server for remote
    /// rules. Loopback unless the user opens it up.
    #[serde(default = "loopback")]
    pub bind_address: String,
    /// The port to listen on.
    pub bind_port: u16,
    /// Local rules: the destination host, resolved by the server. Remote rules: resolved
    /// here.
    #[serde(default)]
    pub destination_host: String,
    /// Local and remote rules: the destination port.
    #[serde(default)]
    pub destination_port: u16,
    /// Start the rule when the vault is unlocked (F-FWD-004).
    #[serde(default)]
    pub auto_start: bool,
    /// Start the rule when a terminal to its host opens (F-FWD-004). Like `auto_start`,
    /// this device's choice only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub with_host: bool,
}

fn loopback() -> String {
    Ipv4Addr::LOCALHOST.to_string()
}

impl PortForward {
    /// A new local rule through `host_id`, listening on loopback.
    #[must_use]
    pub fn new(id: u64, host_id: u64) -> Self {
        Self {
            id,
            label: String::new(),
            host_id,
            kind: ForwardKind::Local,
            bind_address: loopback(),
            bind_port: 0,
            destination_host: String::new(),
            destination_port: 0,
            auto_start: false,
            with_host: false,
        }
    }

    /// Why the rule cannot be saved, if it cannot.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        if self.label.trim().is_empty() {
            Some("Give the rule a name.")
        } else if self.kind == ForwardKind::Remote && self.bind_address.trim().is_empty() {
            Some("Enter the address the server listens on, such as localhost.")
        } else if self.kind != ForwardKind::Remote
            && self.bind_address.trim().parse::<IpAddr>().is_err()
        {
            Some("The bind address must be an IP address, such as 127.0.0.1.")
        } else if self.bind_port == 0 {
            Some("Choose a port to listen on, from 1 to 65535.")
        } else if self.has_destination() && self.destination_host.trim().is_empty() {
            Some("Enter the destination host.")
        } else if self.has_destination() && self.destination_port == 0 {
            Some("Choose a destination port from 1 to 65535.")
        } else {
            None
        }
    }

    /// Whether the rule has a fixed destination (all but SOCKS rules).
    #[must_use]
    pub fn has_destination(&self) -> bool {
        self.kind != ForwardKind::Dynamic
    }

    /// The local address to listen on, if the rule listens here and is valid.
    #[must_use]
    pub fn bind(&self) -> Option<std::net::SocketAddr> {
        if self.kind == ForwardKind::Remote {
            return None;
        }
        let ip: IpAddr = self.bind_address.trim().parse().ok()?;
        Some(std::net::SocketAddr::new(ip, self.bind_port))
    }

    /// One line for the list: `127.0.0.1:8080 → db:5432`, or `SOCKS on 127.0.0.1:1080`.
    #[must_use]
    pub fn summary(&self) -> String {
        let bind = match self.bind() {
            Some(bind) => bind.to_string(),
            None => format!("{}:{}", self.bind_address, self.bind_port),
        };
        match self.kind {
            ForwardKind::Local => format!(
                "{bind} \u{2192} {}:{}",
                self.destination_host.trim(),
                self.destination_port
            ),
            ForwardKind::Dynamic => format!("SOCKS on {bind}"),
            ForwardKind::Remote => format!(
                "server {bind} \u{2192} {}:{} here",
                self.destination_host.trim(),
                self.destination_port
            ),
        }
    }

    /// Whether the rule listens beyond one computer: this one for local and SOCKS rules,
    /// the server for remote rules.
    #[must_use]
    pub fn is_exposed(&self) -> bool {
        match self.kind {
            ForwardKind::Remote => {
                let address = self.bind_address.trim();
                !(address.eq_ignore_ascii_case("localhost")
                    || address.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
            }
            _ => self.bind().is_some_and(|bind| !bind.ip().is_loopback()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PortForward {
        PortForward {
            label: "db".into(),
            bind_port: 15432,
            destination_host: "db.internal".into(),
            destination_port: 5432,
            ..PortForward::new(1, 7)
        }
    }

    #[test]
    fn a_rule_needs_a_name_ports_and_for_local_rules_a_destination() {
        assert_eq!(rule().problem(), None);
        assert!(
            PortForward {
                label: " ".into(),
                ..rule()
            }
            .problem()
            .is_some()
        );
        assert!(
            PortForward {
                bind_port: 0,
                ..rule()
            }
            .problem()
            .is_some()
        );
        assert!(
            PortForward {
                bind_address: "localhost".into(),
                ..rule()
            }
            .problem()
            .is_some()
        );
        assert!(
            PortForward {
                destination_host: String::new(),
                ..rule()
            }
            .problem()
            .is_some()
        );
        let socks = PortForward {
            kind: ForwardKind::Dynamic,
            destination_host: String::new(),
            destination_port: 0,
            ..rule()
        };
        assert_eq!(socks.problem(), None);
    }

    #[test]
    fn summaries_and_exposure_read_from_the_bind_address() {
        assert_eq!(
            rule().summary(),
            "127.0.0.1:15432 \u{2192} db.internal:5432"
        );
        assert!(!rule().is_exposed());
        let open = PortForward {
            kind: ForwardKind::Dynamic,
            bind_address: "0.0.0.0".into(),
            bind_port: 1080,
            ..rule()
        };
        assert_eq!(open.summary(), "SOCKS on 0.0.0.0:1080");
        assert!(open.is_exposed());
        let remote = PortForward {
            kind: ForwardKind::Remote,
            bind_address: "localhost".into(),
            bind_port: 9000,
            destination_host: "127.0.0.1".into(),
            destination_port: 3000,
            ..rule()
        };
        assert_eq!(
            remote.problem(),
            None,
            "remote rules may name the server's address"
        );
        assert_eq!(
            remote.summary(),
            "server localhost:9000 \u{2192} 127.0.0.1:3000 here"
        );
        assert!(!remote.is_exposed());
        assert!(
            PortForward {
                bind_address: "*".into(),
                ..remote
            }
            .is_exposed()
        );
        let v6 = PortForward {
            bind_address: "::1".into(),
            ..rule()
        };
        assert_eq!(v6.bind().unwrap().to_string(), "[::1]:15432");
    }
}
