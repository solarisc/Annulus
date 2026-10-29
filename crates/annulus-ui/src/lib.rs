//! Retained widget tree, layout, styling, input routing, focus, animation, accessibility tree and display list.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.

#![forbid(unsafe_code)]

pub mod display_list;
pub mod geometry;
pub mod tree;
pub mod widget;

pub use display_list::{DisplayList, Op};
pub use geometry::{Color, Point, Rect, Size};
pub use tree::{Dirty, NodeId, Tree, TreeError};
pub use widget::Widget;
