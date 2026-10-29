//! Self-hosted Annulus sync server: accounts, OPAQUE login, devices, record store, history and trash.
//!
//! The server keeps ciphertext it cannot read ([ADR-0008]): records are opaque blobs with a
//! per-account sequence number, the account key is stored only wrapped, and login is OPAQUE,
//! so no password or password-equivalent ever arrives here. [`Server`] holds the logic;
//! [`router`] puts HTTP in front of it.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.
//!
//! [ADR-0008]: ../../docs/adr/0008-self-hosted-sync-server.md

#![forbid(unsafe_code)]

mod http;
mod store;
pub mod tls;

pub use http::service;
pub use store::{AccountInfo, Caller, Server, ServerError, Signups};

#[cfg(test)]
mod tests;
