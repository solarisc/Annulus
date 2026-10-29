//! Retained widget tree, layout, styling, input routing, focus, animation, accessibility tree and display list.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

mod accessibility;
pub mod animation;
pub mod display_list;
pub mod geometry;
pub mod icons;
pub mod input;
mod layout;
mod routing;
pub mod style;
#[cfg(test)]
mod test_support;
pub mod text_edit;
pub mod tree;
pub mod widget;
pub mod widgets;

pub use animation::{Animator, Easing, Property, Update};
pub use display_list::{DisplayList, Op};
pub use geometry::{Color, Point, Rect, Size};
pub use input::{Button, Event, Handled, Key, Modifiers};
pub use style::{Align, Axis, Edges, Justify, Length, Position, Style};
pub use tree::{Dirty, NodeId, Tree, TreeError};
pub use widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget, WidgetState};

/// The design tokens, re-exported so backends and the app share one source of truth.
pub use annulus_config::tokens;
