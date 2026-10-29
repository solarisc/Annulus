//! Vault entities, inheritance resolution, encryption, SQLite storage, history and migrations.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

pub mod crypto;
pub mod forwards;
pub mod hosts;
pub mod keychain;
pub mod sync;
pub mod vault;
