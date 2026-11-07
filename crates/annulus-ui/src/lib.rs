//! Annulus's own UI toolkit ([ADR-0017](../../docs/adr/0017-native-ui.md)): the
//! drawing interface the renderers implement, the immediate-mode kit of zones, focus, input and
//! animation, the widgets, our icons and mark, and the accessibility tree built from the zones.

#![forbid(unsafe_code)]

pub mod access;
pub mod gfx;
pub mod icons;
pub mod kit;
pub mod mark;
pub mod motion;
pub mod widgets;
