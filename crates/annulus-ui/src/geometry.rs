//! Plain geometry in device-independent pixels (DIPs): points, sizes, rectangles and colors.

/// A point in DIPs, origin at the window's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Distance from the left edge.
    pub x: f32,
    /// Distance from the top edge.
    pub y: f32,
}

impl Point {
    /// A point at `(x, y)`.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A width and height in DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Horizontal extent.
    pub width: f32,
    /// Vertical extent.
    pub height: f32,
}

impl Size {
    /// A size of `width` by `height`.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in DIPs. Negative sizes are treated as empty.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Top-left corner.
    pub origin: Point,
    /// Extent from the origin.
    pub size: Size,
}

impl Rect {
    /// A rectangle at `(x, y)` with the given width and height.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    /// The right edge.
    #[must_use]
    pub fn max_x(&self) -> f32 {
        self.origin.x + self.size.width
    }

    /// The bottom edge.
    #[must_use]
    pub fn max_y(&self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Whether the rectangle covers no area (including negative or NaN sizes).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.size.width > 0.0 && self.size.height > 0.0)
    }

    /// The smallest rectangle that contains both. An empty rectangle contributes nothing.
    #[must_use]
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.origin.x.min(other.origin.x);
        let y = self.origin.y.min(other.origin.y);
        Rect::new(
            x,
            y,
            self.max_x().max(other.max_x()) - x,
            self.max_y().max(other.max_y()) - y,
        )
    }

    /// Whether `point` lies inside. The left and top edges are inside, the right and bottom
    /// edges are not, so adjacent rectangles never both contain a point.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.y >= self.origin.y
            && point.x < self.max_x()
            && point.y < self.max_y()
    }
}

/// A straight (not premultiplied) sRGB color with alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha; 0 is fully transparent.
    pub a: u8,
}

impl Color {
    /// An opaque color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A color with alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_covers_both_and_ignores_empty_rects() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 5.0, 5.0, 20.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 25.0, 25.0));
        let empty = Rect::new(100.0, 100.0, 0.0, 0.0);
        assert_eq!(a.union(&empty), a);
        assert_eq!(empty.union(&a), a);
    }

    #[test]
    fn nan_or_negative_size_is_empty() {
        assert!(Rect::new(0.0, 0.0, f32::NAN, 5.0).is_empty());
        assert!(Rect::new(0.0, 0.0, -1.0, 5.0).is_empty());
        assert!(!Rect::new(0.0, 0.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn adjacent_rects_never_share_a_point() {
        let left = Rect::new(0.0, 0.0, 10.0, 10.0);
        let right = Rect::new(10.0, 0.0, 10.0, 10.0);
        let edge = Point::new(10.0, 5.0);
        assert!(!left.contains(edge));
        assert!(right.contains(edge));
    }
}
