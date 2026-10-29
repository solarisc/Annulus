//! The widget trait: what a node draws. Layout, input and accessibility
//! add their methods here as they arrive.

use crate::display_list::DisplayList;
use crate::geometry::{Rect, Size};

/// The behavior of one node in the tree. Widgets hold only their own visual state; the view
/// models in `annulus-app` own app state (DESIGN §7.2).
pub trait Widget {
    /// A short name for debugging and test output, e.g. `"button"`.
    fn name(&self) -> &'static str;

    /// The size this widget's own content needs (text, icon), not counting padding or
    /// children, when at most `available` is free (either side may be infinite). Containers
    /// keep the default of zero.
    fn content_size(&self, available: Size) -> Size {
        let _ = available;
        Size::default()
    }

    /// Appends this widget's own drawing (not its children's) for the given bounds.
    fn paint(&self, bounds: Rect, list: &mut DisplayList);
}
