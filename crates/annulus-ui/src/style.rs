//! Layout style of a node: our flexbox subset (DESIGN §7.2). Visual style (colors, fonts)
//! comes from design tokens in a later step.

use crate::geometry::Size;

/// The direction a container stacks its children in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    /// Left to right.
    Row,
    /// Top to bottom.
    #[default]
    Column,
}

/// How long a node wants to be along one axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Exactly this many DIPs.
    Fixed(f32),
    /// As long as its content (and padding) needs.
    #[default]
    Fit,
    /// A share of the space left after fixed and fit siblings, by weight. Across the parent's
    /// axis, the full cross size. Inside a wrapping container it behaves like [`Length::Fit`].
    Fill(f32),
}

/// Where children sit across the container's axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// At the start (top or left).
    Start,
    /// Centered.
    Center,
    /// At the end (bottom or right).
    End,
    /// Stretched to the full cross size, unless the child has a fixed cross length.
    #[default]
    Stretch,
}

/// How free space along the container's axis is spent when no child fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    /// Children packed at the start.
    #[default]
    Start,
    /// Children packed in the middle.
    Center,
    /// Children packed at the end.
    End,
    /// First and last child at the edges, the rest spread evenly between them.
    SpaceBetween,
}

/// Space on each side of a box, in DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    /// Above.
    pub top: f32,
    /// Right.
    pub right: f32,
    /// Below.
    pub bottom: f32,
    /// Left.
    pub left: f32,
}

impl Edges {
    /// The same amount on every side.
    #[must_use]
    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Total horizontal space.
    #[must_use]
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Total vertical space.
    #[must_use]
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Whether a node takes part in its parent's flow.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Position {
    /// Laid out with its siblings.
    #[default]
    Flow,
    /// Placed at an offset from the parent's top-left corner, above the flow children and
    /// without moving them. Popovers, menus and side panels use this.
    Overlay {
        /// Offset from the parent's left edge.
        x: f32,
        /// Offset from the parent's top edge.
        y: f32,
    },
}

/// The layout style of one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Direction the children stack in.
    pub axis: Axis,
    /// Space between adjacent children (and between wrapped lines).
    pub gap: f32,
    /// Space between the node's edge and its children.
    pub padding: Edges,
    /// Preferred width.
    pub width: Length,
    /// Preferred height.
    pub height: Length,
    /// Smallest allowed size. Wins over `max` if they conflict.
    pub min: Size,
    /// Largest allowed size.
    pub max: Size,
    /// Placement of children across the axis.
    pub align: Align,
    /// Placement of children along the axis.
    pub justify: Justify,
    /// Children that don't fit on a line start a new one (card grids).
    pub wrap: bool,
    /// Children may extend past the node along its axis; they are clipped and scrolled.
    pub scroll: bool,
    /// Flow or overlay.
    pub position: Position,
}

impl Style {
    /// This style with every length made usable: NaN, infinite or negative lengths, gaps,
    /// paddings, offsets and weights become 0, and a NaN or negative `max` becomes unbounded
    /// or 0 respectively. The tree stores styles in this form, so layout never sees them.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let length = |length: Length| match length {
            Length::Fixed(value) => Length::Fixed(usable(value)),
            Length::Fit => Length::Fit,
            Length::Fill(weight) => Length::Fill(usable(weight)),
        };
        let max = |value: f32| {
            if value.is_nan() {
                f32::INFINITY
            } else {
                value.max(0.0)
            }
        };
        Self {
            gap: usable(self.gap),
            padding: Edges {
                top: usable(self.padding.top),
                right: usable(self.padding.right),
                bottom: usable(self.padding.bottom),
                left: usable(self.padding.left),
            },
            width: length(self.width),
            height: length(self.height),
            min: Size::new(usable(self.min.width), usable(self.min.height)),
            max: Size::new(max(self.max.width), max(self.max.height)),
            position: match self.position {
                Position::Flow => Position::Flow,
                Position::Overlay { x, y } => Position::Overlay {
                    x: if x.is_finite() { x } else { 0.0 },
                    y: if y.is_finite() { y } else { 0.0 },
                },
            },
            ..self
        }
    }
}

/// A finite, non-negative value, or 0.
fn usable(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            axis: Axis::Column,
            gap: 0.0,
            padding: Edges::default(),
            width: Length::Fit,
            height: Length::Fit,
            min: Size::default(),
            max: Size::new(f32::INFINITY, f32::INFINITY),
            align: Align::Stretch,
            justify: Justify::Start,
            wrap: false,
            scroll: false,
            position: Position::Flow,
        }
    }
}
