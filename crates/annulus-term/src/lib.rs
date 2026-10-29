//! Terminal emulation: VT parsing, screen and scrollback model, selection, search, reflow and key encoding. Bytes in, state out.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]
