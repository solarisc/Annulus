//! Platform traits and their per-OS implementations: PTYs, secret stores, biometrics, FIDO2. The only crate allowed to use unsafe.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4. Windows, input and drawing
//! come from eframe ([ADR-0015](../../docs/adr/0015-egui-instead-of-hand-built-ui.md)).
