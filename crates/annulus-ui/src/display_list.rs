//! The display list: plain drawing commands that every renderer backend consumes. It holds no
//! platform types, so snapshot tests run anywhere (DESIGN §7.2).

use crate::geometry::{Color, Point, Rect};

/// Identifies one of our vector icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconId(pub u16);

/// Identifies an image the app has handed to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// One drawing command. Push/pop pairs are always balanced: the only way to emit them is
/// [`DisplayList::clip`] and [`DisplayList::layer`].
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A filled rectangle.
    Rect {
        /// Area to fill.
        rect: Rect,
        /// Fill color.
        color: Color,
    },
    /// A filled rectangle with rounded corners.
    RoundedRect {
        /// Area to fill.
        rect: Rect,
        /// Corner radius in DIPs.
        radius: f32,
        /// Fill color.
        color: Color,
    },
    /// A stroke drawn inside `rect`.
    Border {
        /// Outer edge of the stroke.
        rect: Rect,
        /// Corner radius in DIPs.
        radius: f32,
        /// Stroke width in DIPs.
        width: f32,
        /// Stroke color.
        color: Color,
    },
    /// A blurred drop shadow under `rect`.
    Shadow {
        /// The shape casting the shadow.
        rect: Rect,
        /// Corner radius of that shape.
        radius: f32,
        /// Shadow offset.
        offset: Point,
        /// Blur radius in DIPs.
        blur: f32,
        /// Shadow color.
        color: Color,
    },
    /// A run of text in one style. Shaping happens in the backend's text service.
    TextRun {
        /// Baseline origin.
        origin: Point,
        /// The text.
        text: String,
        /// Font size in DIPs.
        size: f32,
        /// Text color.
        color: Color,
    },
    /// One of our vector icons, scaled into `rect`.
    Icon {
        /// Which icon.
        icon: IconId,
        /// Where to draw it.
        rect: Rect,
        /// Fill color.
        color: Color,
    },
    /// An image scaled into `rect`.
    Image {
        /// Which image.
        image: ImageId,
        /// Where to draw it.
        rect: Rect,
    },
    /// Start clipping to `rect`, intersected with any outer clip.
    PushClip(Rect),
    /// End the innermost clip.
    PopClip,
    /// Start a group drawn with `opacity` (0.0 to 1.0).
    PushLayer {
        /// Group opacity.
        opacity: f32,
    },
    /// End the innermost layer.
    PopLayer,
}

/// An ordered list of drawing commands, back to front.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    ops: Vec<Op>,
}

impl DisplayList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The commands, back to front.
    #[must_use]
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Appends a drawing command. Clip and layer commands are ignored here, so the list can
    /// never be unbalanced; use [`Self::clip`] and [`Self::layer`] for those.
    pub fn push(&mut self, op: Op) {
        match op {
            Op::PushClip(_) | Op::PopClip | Op::PushLayer { .. } | Op::PopLayer => {}
            op => self.ops.push(op),
        }
    }

    /// Draws whatever `draw` adds, clipped to `rect`.
    pub fn clip(&mut self, rect: Rect, draw: impl FnOnce(&mut Self)) {
        self.ops.push(Op::PushClip(rect));
        draw(self);
        self.ops.push(Op::PopClip);
    }

    /// Draws whatever `draw` adds as one group with `opacity`, clamped to 0.0–1.0. A NaN
    /// opacity draws the group fully opaque.
    pub fn layer(&mut self, opacity: f32, draw: impl FnOnce(&mut Self)) {
        let opacity = if opacity.is_nan() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        self.ops.push(Op::PushLayer { opacity });
        draw(self);
        self.ops.push(Op::PopLayer);
    }

    /// Appends every command of `other`.
    pub fn extend(&mut self, other: DisplayList) {
        self.ops.extend(other.ops);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(255, 0, 0);

    #[test]
    fn nested_clip_and_layer_stay_balanced() {
        let mut list = DisplayList::new();
        list.clip(Rect::new(0.0, 0.0, 10.0, 10.0), |list| {
            list.layer(0.5, |list| {
                list.push(Op::Rect {
                    rect: Rect::default(),
                    color: RED,
                });
            });
        });
        let kinds: Vec<_> = list.ops().iter().map(std::mem::discriminant).collect();
        let expected = [
            Op::PushClip(Rect::default()),
            Op::PushLayer { opacity: 0.0 },
            Op::Rect {
                rect: Rect::default(),
                color: RED,
            },
            Op::PopLayer,
            Op::PopClip,
        ];
        assert_eq!(
            kinds,
            expected
                .iter()
                .map(std::mem::discriminant)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn raw_push_cannot_unbalance_the_list() {
        let mut list = DisplayList::new();
        list.push(Op::PopClip);
        list.push(Op::PushLayer { opacity: 1.0 });
        assert_eq!(list.ops(), &[]);
    }

    #[test]
    fn layer_opacity_is_clamped_and_nan_is_opaque() {
        let mut list = DisplayList::new();
        list.layer(2.0, |_| {});
        list.layer(-1.0, |_| {});
        list.layer(f32::NAN, |_| {});
        let opacities: Vec<_> = list
            .ops()
            .iter()
            .filter_map(|op| match op {
                Op::PushLayer { opacity } => Some(*opacity),
                _ => None,
            })
            .collect();
        assert_eq!(opacities, [1.0, 0.0, 1.0]);
    }
}
