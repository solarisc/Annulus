//! A Session over SSH or a local PTY, with reconnect, logging hooks and broadcast fan-out.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]
