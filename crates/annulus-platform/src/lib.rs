//! Platform code and the per-OS implementations behind it: the renderers and windows of our
//! own UI ([ADR-0017](../../docs/adr/0017-native-ui.md)), PTYs, secret stores,
//! biometrics, FIDO2. The only crate allowed to use `unsafe`.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

pub mod cpu;
#[cfg(windows)]
pub mod d2d;
#[cfg(windows)]
pub mod install;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod menu_bar;
pub mod process;
#[cfg(windows)]
mod win32;
pub mod window;
