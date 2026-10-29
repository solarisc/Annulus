//! Client sync engine: push and pull, conflict resolution, cursors and server-sent events.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

pub mod account;
pub mod clock;
pub mod engine;
pub mod http;
pub mod pin;
pub mod replica;

#[cfg(test)]
mod tests;
