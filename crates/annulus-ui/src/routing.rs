//! Input routing: hit testing, event bubbling, pointer capture, focus and keyboard navigation.
//!
//! Pointer events go to the topmost node under the pointer (or the node that captured the
//! pointer), keyboard events to the focused node, and both bubble to ancestors until a widget
//! handles them. Tab order is paint order, so it matches what the user sees.

use crate::geometry::Point;
use crate::input::{Event, Handled, Key};
use crate::style::Position;
use crate::tree::{Dirty, NodeId, Tree};
use crate::widget::{EventCx, Widget};

impl Tree {
    /// The focused node, if it still exists.
    #[must_use]
    pub fn focused(&self) -> Option<NodeId> {
        self.focused.filter(|id| self.contains(*id))
    }

    /// Whether the focus ring is showing.
    #[must_use]
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Moves focus to `id` (or clears it). `visible` shows the focus ring, as after keyboard
    /// navigation. A node that is gone or not focusable clears focus instead.
    pub fn set_focus(&mut self, id: Option<NodeId>, visible: bool) {
        let target = id.filter(|id| self.widget(*id).is_some_and(Widget::focusable));
        for changed in [self.focused, target].into_iter().flatten() {
            // Old and new focus repaint their ring; a stale id is simply skipped.
            let _ = self.mark_dirty(changed, Dirty::PAINT);
        }
        self.focused = target;
        self.focus_visible = visible && target.is_some();
    }

    /// Focuses the next focusable node in Tab order, wrapping around, and returns it.
    pub fn focus_next(&mut self) -> Option<NodeId> {
        self.focus_step(true)
    }

    /// Focuses the previous focusable node in Tab order, wrapping around, and returns it.
    pub fn focus_previous(&mut self) -> Option<NodeId> {
        self.focus_step(false)
    }

    /// The topmost node whose bounds contain `point`: overlays above flow children, later
    /// siblings above earlier ones, children above parents. A scrolling node's flow children
    /// are only hit inside its bounds, where they are visible.
    #[must_use]
    pub fn hit_test(&self, point: Point) -> Option<NodeId> {
        self.hit(self.root(), point)
    }

    /// Routes `event` and returns the node that handled it, if any.
    ///
    /// - A press focuses the nearest focusable node under the pointer (without a focus ring)
    ///   and the node that handles it captures the pointer until the release.
    /// - A wheel event nobody handles scrolls the nearest scrolling ancestor.
    /// - An unhandled Tab or Shift+Tab moves focus and shows the focus ring.
    pub fn dispatch(&mut self, event: &Event) -> Option<NodeId> {
        match event {
            Event::PointerDown { position, .. } => {
                let target = self.hit_test(*position);
                let focus = target.and_then(|id| self.focusable_ancestor_or_self(id));
                self.set_focus(focus, false);
                let handler = target.and_then(|id| self.bubble(id, event));
                self.captured = handler;
                handler
            }
            Event::PointerUp { position, .. } => {
                let target = self.captured.take().or_else(|| self.hit_test(*position));
                target.and_then(|id| self.bubble(id, event))
            }
            Event::PointerMove { position } => {
                self.update_hover(*position);
                let target = self.captured.filter(|id| self.contains(*id));
                let target = target.or_else(|| self.hit_test(*position));
                target.and_then(|id| self.bubble(id, event))
            }
            Event::Wheel { position, delta } => {
                let target = self.hit_test(*position)?;
                self.bubble(target, event)
                    .or_else(|| self.scroll_ancestor(target, *delta))
            }
            Event::KeyDown { key, modifiers } => {
                let start = self.focused().unwrap_or(self.root());
                self.bubble(start, event).or_else(|| match key {
                    Key::Tab if modifiers.shift => self.focus_previous(),
                    Key::Tab => self.focus_next(),
                    _ => None,
                })
            }
            Event::Text(_) => {
                let start = self.focused().unwrap_or(self.root());
                self.bubble(start, event)
            }
        }
    }

    /// Offers `event` to `start` and then each ancestor until one handles it. The handler is
    /// repainted, since handling usually changes how it looks.
    fn bubble(&mut self, start: NodeId, event: &Event) -> Option<NodeId> {
        let mut current = Some(start);
        while let Some(id) = current {
            let node = self.node_mut(id)?;
            let mut cx = EventCx::new(node.bounds);
            let handled = node.widget.handle(event, &mut cx);
            let parent = node.parent;
            self.actions
                .extend(cx.into_actions().into_iter().map(|action| (id, action)));
            if handled == Handled::Yes {
                let _ = self.mark_dirty(id, Dirty::PAINT);
                return Some(id);
            }
            current = parent;
        }
        None
    }

    /// Tracks the node under the pointer; the nodes it leaves and enters repaint.
    fn update_hover(&mut self, position: Point) {
        let now = self.hit_test(position);
        if now != self.hovered {
            for changed in [self.hovered, now].into_iter().flatten() {
                let _ = self.mark_dirty(changed, Dirty::PAINT);
            }
            self.hovered = now;
        }
    }

    fn scroll_ancestor(&mut self, start: NodeId, delta: f32) -> Option<NodeId> {
        let mut current = Some(start);
        while let Some(id) = current {
            if self.style(id).is_some_and(|style| style.scroll) {
                let before = self.scroll_offset(id);
                let after = self.scroll_by(id, delta).ok();
                if after != before {
                    return Some(id);
                }
            }
            current = self.parent(id);
        }
        None
    }

    fn focusable_ancestor_or_self(&self, start: NodeId) -> Option<NodeId> {
        let mut current = Some(start);
        while let Some(id) = current {
            if self.widget(id).is_some_and(Widget::focusable) {
                return Some(id);
            }
            current = self.parent(id);
        }
        None
    }

    fn focus_step(&mut self, forward: bool) -> Option<NodeId> {
        let order: Vec<NodeId> = self
            .paint_order()
            .into_iter()
            .filter(|id| self.widget(*id).is_some_and(Widget::focusable))
            .collect();
        let current = self
            .focused()
            .and_then(|id| order.iter().position(|o| *o == id));
        let next = match (current, forward) {
            (None, true) => order.first(),
            (None, false) => order.last(),
            (Some(index), true) => order.get(index + 1).or(order.first()),
            (Some(index), false) => index
                .checked_sub(1)
                .and_then(|i| order.get(i))
                .or(order.last()),
        }
        .copied();
        self.set_focus(next, true);
        next
    }

    /// Every node in paint order: a node, its flow children, then its overlay children.
    fn paint_order(&self) -> Vec<NodeId> {
        let mut order = Vec::new();
        let mut pending = vec![self.root()];
        while let Some(id) = pending.pop() {
            order.push(id);
            let (overlays, flow) = self.split_children(id);
            pending.extend(overlays.iter().rev());
            pending.extend(flow.iter().rev());
        }
        order
    }

    fn hit(&self, id: NodeId, point: Point) -> Option<NodeId> {
        let bounds = self.bounds(id)?;
        let (overlays, flow) = self.split_children(id);
        if let Some(hit) = overlays
            .iter()
            .rev()
            .find_map(|child| self.hit(*child, point))
        {
            return Some(hit);
        }
        let clipped = self.style(id).is_some_and(|style| style.scroll);
        if (!clipped || bounds.contains(point))
            && let Some(hit) = flow.iter().rev().find_map(|child| self.hit(*child, point))
        {
            return Some(hit);
        }
        bounds.contains(point).then_some(id)
    }

    /// The children of `id` split into (overlays, flow), each in order.
    pub(crate) fn split_children(&self, id: NodeId) -> (Vec<NodeId>, Vec<NodeId>) {
        self.children(id).iter().copied().partition(|child| {
            self.style(*child)
                .is_some_and(|style| matches!(style.position, Position::Overlay { .. }))
        })
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
