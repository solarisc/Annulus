//! Layout: turns each node's [`Style`] plus the viewport into bounds. It is a pure function of
//! the tree (no window, no fonts beyond what widgets report), so it is tested with tables.
//!
//! Two passes: `measure` asks how big a subtree wants to be, `arrange` hands out rectangles
//! top-down and records them with [`Tree::set_bounds`], which keeps damage exact.

use crate::geometry::{Point, Rect, Size};
use crate::style::{Align, Axis, Justify, Length, Position, Style};
use crate::tree::{Dirty, NodeId, Tree};
use crate::widget::TextMeasure;

impl Tree {
    /// Lays out the tree to fill `viewport`. Does nothing when no node needs layout and the
    /// viewport did not change, so idle frames cost nothing.
    pub fn layout(&mut self, viewport: Size, text: &mut dyn TextMeasure) {
        let root = self.root();
        let needed = self
            .dirty(root)
            .is_some_and(|dirty| dirty.contains(Dirty::LAYOUT));
        let resized = self
            .bounds(root)
            .is_some_and(|bounds| bounds.size != viewport);
        if needed || resized {
            self.arrange(
                root,
                Rect {
                    origin: Point::default(),
                    size: viewport,
                },
                text,
            );
        }
    }

    /// The size `id` wants when at most `available` is free.
    fn measure(&self, id: NodeId, available: Size, text: &mut dyn TextMeasure) -> Size {
        let Some(node) = self.node(id) else {
            return Size::default();
        };
        let style = &node.style;
        let pad = style.padding;
        let width = match style.width {
            Length::Fixed(width) => width,
            _ => available.width,
        };
        let height = match style.height {
            Length::Fixed(height) => height,
            _ => available.height,
        };
        let inner = Size::new(
            non_negative(width - pad.horizontal()),
            non_negative(height - pad.vertical()),
        );
        let flow = self.flow_children(id);
        let content = if flow.is_empty() {
            node.widget.content_size(inner, text)
        } else {
            self.measure_children(style, &flow, inner, text)
        };
        let natural = Size::new(
            content.width + pad.horizontal(),
            content.height + pad.vertical(),
        );
        Size::new(
            clamp(
                resolve(style.width, natural.width, available.width),
                style.min.width,
                style.max.width,
            ),
            clamp(
                resolve(style.height, natural.height, available.height),
                style.min.height,
                style.max.height,
            ),
        )
    }

    /// The size the flow children of a container with `style` need inside `inner`.
    fn measure_children(
        &self,
        style: &Style,
        flow: &[NodeId],
        inner: Size,
        text: &mut dyn TextMeasure,
    ) -> Size {
        let axis = style.axis;
        let mut child_space = inner;
        if style.scroll {
            set_main(axis, &mut child_space, f32::INFINITY);
        }
        if style.wrap {
            let lines = self.wrap_lines(style, flow, child_space, text);
            let main = lines.iter().map(|line| line.main).fold(0.0, f32::max);
            let cross =
                lines.iter().map(|line| line.cross).sum::<f32>() + style.gap * gaps(lines.len());
            return from_axes(axis, main, cross);
        }
        let mut main = style.gap * gaps(flow.len());
        let mut cross = 0.0_f32;
        for &child in flow {
            let size = self.measure(child, child_space, text);
            let child_style = self.child_style(child);
            main += match main_length(axis, &child_style) {
                Length::Fill(_) => main_of(axis, child_style.min),
                _ => main_of(axis, size),
            };
            cross = cross.max(cross_of(axis, size));
        }
        from_axes(axis, main, cross)
    }

    /// Gives `id` the rectangle `rect` and lays out its children inside it.
    fn arrange(&mut self, id: NodeId, rect: Rect, text: &mut dyn TextMeasure) {
        if self.set_bounds(id, rect).is_err() {
            return;
        }
        let Some(style) = self.style(id).copied() else {
            return;
        };
        let inner = Rect::new(
            rect.origin.x + style.padding.left,
            rect.origin.y + style.padding.top,
            non_negative(rect.size.width - style.padding.horizontal()),
            non_negative(rect.size.height - style.padding.vertical()),
        );
        let flow = self.flow_children(id);
        let placed = if style.wrap {
            self.place_wrapped(&style, &flow, inner, text)
        } else {
            self.place_line(id, &style, &flow, inner, text)
        };
        for (child, child_rect) in placed {
            self.arrange(child, child_rect, text);
        }
        for child in self.children(id).to_vec() {
            let child_style = self.child_style(child);
            if let Position::Overlay { x, y } = child_style.position {
                let space = Size::new(
                    non_negative(rect.size.width - x),
                    non_negative(rect.size.height - y),
                );
                let size = self.measure(child, space, text);
                let origin = Point::new(rect.origin.x + x, rect.origin.y + y);
                self.arrange(child, Rect { origin, size }, text);
            }
        }
    }

    /// Places flow children on one line along the axis: fixed and fit children first, then
    /// fill children share what is left by weight, then `justify` spends any remainder.
    fn place_line(
        &mut self,
        id: NodeId,
        style: &Style,
        flow: &[NodeId],
        inner: Rect,
        text: &mut dyn TextMeasure,
    ) -> Vec<(NodeId, Rect)> {
        let axis = style.axis;
        let inner_main = main_of(axis, inner.size);
        let inner_cross = cross_of(axis, inner.size);
        let mut child_space = inner.size;
        if style.scroll {
            set_main(axis, &mut child_space, f32::INFINITY);
        }

        let measured: Vec<(NodeId, Style, Size)> = flow
            .iter()
            .map(|&child| {
                (
                    child,
                    self.child_style(child),
                    self.measure(child, child_space, text),
                )
            })
            .collect();
        let gap_total = style.gap * gaps(flow.len());
        let mut fixed_total = 0.0;
        let mut weight_total = 0.0;
        for (_, child_style, size) in &measured {
            match main_length(axis, child_style) {
                Length::Fill(weight) => weight_total += valid_weight(weight),
                _ => fixed_total += main_of(axis, *size),
            }
        }
        let free = if style.scroll {
            0.0
        } else {
            non_negative(inner_main - fixed_total - gap_total)
        };

        let mains: Vec<f32> = measured
            .iter()
            .map(
                |(_, child_style, size)| match main_length(axis, child_style) {
                    Length::Fill(weight) => {
                        let share = if weight_total > 0.0 {
                            free * valid_weight(weight) / weight_total
                        } else {
                            0.0
                        };
                        clamp(
                            share,
                            main_of(axis, child_style.min),
                            main_of(axis, child_style.max),
                        )
                    }
                    _ => main_of(axis, *size),
                },
            )
            .collect();
        let used = mains.iter().sum::<f32>() + gap_total;
        let leftover = if style.scroll {
            0.0
        } else {
            non_negative(inner_main - used)
        };
        let (mut cursor, extra_gap) = match style.justify {
            Justify::Center => (leftover / 2.0, 0.0),
            Justify::End => (leftover, 0.0),
            Justify::SpaceBetween if flow.len() > 1 => (0.0, leftover / gaps(flow.len())),
            Justify::Start | Justify::SpaceBetween => (0.0, 0.0),
        };

        let scroll_offset = self.record_scroll(id, style.scroll, used, inner_main);
        cursor -= scroll_offset;

        let mut placed = Vec::with_capacity(flow.len());
        for ((child, child_style, size), main) in measured.iter().zip(mains) {
            let cross = match cross_length(axis, child_style) {
                Length::Fill(_) => inner_cross,
                Length::Fit if style.align == Align::Stretch => inner_cross,
                Length::Fixed(_) | Length::Fit => cross_of(axis, *size),
            };
            let cross = clamp(
                cross,
                cross_of(axis, child_style.min),
                cross_of(axis, child_style.max),
            );
            let cross_pos = match style.align {
                Align::Start | Align::Stretch => 0.0,
                Align::Center => (inner_cross - cross) / 2.0,
                Align::End => inner_cross - cross,
            };
            placed.push((
                *child,
                rect_on_axes(axis, inner.origin, cursor, cross_pos, main, cross),
            ));
            cursor += main + style.gap + extra_gap;
        }
        placed
    }

    /// Places flow children in lines that wrap when the next child would not fit.
    fn place_wrapped(
        &self,
        style: &Style,
        flow: &[NodeId],
        inner: Rect,
        text: &mut dyn TextMeasure,
    ) -> Vec<(NodeId, Rect)> {
        let axis = style.axis;
        let mut placed = Vec::with_capacity(flow.len());
        let mut cross_pos = 0.0;
        for line in self.wrap_lines(style, flow, inner.size, text) {
            let mut cursor = 0.0;
            for (child, size) in line.items {
                let cross = cross_of(axis, size);
                let offset = match style.align {
                    Align::Start | Align::Stretch => 0.0,
                    Align::Center => (line.cross - cross) / 2.0,
                    Align::End => line.cross - cross,
                };
                let main = main_of(axis, size);
                placed.push((
                    child,
                    rect_on_axes(axis, inner.origin, cursor, cross_pos + offset, main, cross),
                ));
                cursor += main + style.gap;
            }
            cross_pos += line.cross + style.gap;
        }
        placed
    }

    /// Splits flow children into lines no longer than the inner main size.
    fn wrap_lines(
        &self,
        style: &Style,
        flow: &[NodeId],
        inner: Size,
        text: &mut dyn TextMeasure,
    ) -> Vec<Line> {
        let axis = style.axis;
        let limit = main_of(axis, inner);
        let mut lines: Vec<Line> = Vec::new();
        for &child in flow {
            let size = self.measure(child, inner, text);
            let main = main_of(axis, size);
            let starts_new = lines
                .last()
                .is_none_or(|line| !line.items.is_empty() && line.main + style.gap + main > limit);
            if starts_new {
                lines.push(Line {
                    items: Vec::new(),
                    main: 0.0,
                    cross: 0.0,
                });
            }
            if let Some(line) = lines.last_mut() {
                if !line.items.is_empty() {
                    line.main += style.gap;
                }
                line.main += main;
                line.cross = line.cross.max(cross_of(axis, size));
                line.items.push((child, size));
            }
        }
        lines
    }

    /// Stores how far `id` can scroll and returns its offset, clamped to the new content.
    fn record_scroll(&mut self, id: NodeId, scrolls: bool, content: f32, viewport: f32) -> f32 {
        let Some(node) = self.node_mut(id) else {
            return 0.0;
        };
        node.scroll_max = if scrolls {
            non_negative(content - viewport)
        } else {
            0.0
        };
        node.scroll_offset = node.scroll_offset.clamp(0.0, node.scroll_max);
        node.scroll_offset
    }

    fn flow_children(&self, id: NodeId) -> Vec<NodeId> {
        self.children(id)
            .iter()
            .copied()
            .filter(|&child| self.child_style(child).position == Position::Flow)
            .collect()
    }

    fn child_style(&self, id: NodeId) -> Style {
        self.style(id).copied().unwrap_or_default()
    }
}

/// One line of a wrapping container.
struct Line {
    items: Vec<(NodeId, Size)>,
    main: f32,
    cross: f32,
}

/// The length a node gets along one axis, before min/max.
fn resolve(length: Length, natural: f32, available: f32) -> f32 {
    match length {
        Length::Fixed(value) => value,
        Length::Fill(_) if available.is_finite() => available,
        Length::Fit | Length::Fill(_) => natural,
    }
}

/// Clamps to `[min, max]`; `min` wins a conflict, and the result is never negative or NaN.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    non_negative(value.min(max).max(min))
}

/// `value`, or 0 if it is negative or NaN.
fn non_negative(value: f32) -> f32 {
    value.max(0.0)
}

/// A fill weight, or 0 if it is negative or not finite.
fn valid_weight(weight: f32) -> f32 {
    if weight.is_finite() {
        weight.max(0.0)
    } else {
        0.0
    }
}

/// The number of gaps between `count` items, as a float.
#[allow(clippy::cast_precision_loss)] // Child counts are far below 2^24.
fn gaps(count: usize) -> f32 {
    count.saturating_sub(1) as f32
}

fn main_length(axis: Axis, style: &Style) -> Length {
    match axis {
        Axis::Row => style.width,
        Axis::Column => style.height,
    }
}

fn cross_length(axis: Axis, style: &Style) -> Length {
    match axis {
        Axis::Row => style.height,
        Axis::Column => style.width,
    }
}

fn main_of(axis: Axis, size: Size) -> f32 {
    match axis {
        Axis::Row => size.width,
        Axis::Column => size.height,
    }
}

fn cross_of(axis: Axis, size: Size) -> f32 {
    match axis {
        Axis::Row => size.height,
        Axis::Column => size.width,
    }
}

fn set_main(axis: Axis, size: &mut Size, value: f32) {
    match axis {
        Axis::Row => size.width = value,
        Axis::Column => size.height = value,
    }
}

fn from_axes(axis: Axis, main: f32, cross: f32) -> Size {
    match axis {
        Axis::Row => Size::new(main, cross),
        Axis::Column => Size::new(cross, main),
    }
}

fn rect_on_axes(
    axis: Axis,
    origin: Point,
    main_pos: f32,
    cross_pos: f32,
    main: f32,
    cross: f32,
) -> Rect {
    match axis {
        Axis::Row => Rect::new(origin.x + main_pos, origin.y + cross_pos, main, cross),
        Axis::Column => Rect::new(origin.x + cross_pos, origin.y + main_pos, cross, main),
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
