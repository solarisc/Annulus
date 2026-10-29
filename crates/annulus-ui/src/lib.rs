//! The egui look of Annulus: the theme from our design tokens, our own icons, and password
//! fields kept in zeroizing memory ([ADR-0015](../../docs/adr/0015-egui-instead-of-hand-built-ui.md)).

#![forbid(unsafe_code)]

pub mod gfx;
pub mod icons;
pub mod kit;
pub mod secret;
pub mod theme;
pub mod widgets;
