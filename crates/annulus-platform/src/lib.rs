//! Platform traits and their per-OS implementations: windowing, renderers, PTYs, secret stores, biometrics, FIDO2. The only crate allowed to use unsafe.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

pub mod cpu;
pub mod window;
