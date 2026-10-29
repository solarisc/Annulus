//! Importers for OpenSSH config, `known_hosts` and keys, and PuTTY sessions.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

pub mod ssh_config;
