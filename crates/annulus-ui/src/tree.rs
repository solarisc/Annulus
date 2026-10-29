//! The retained widget tree: an arena of nodes with generational ids, per-node dirty bits, and
//! painting into a display list.
//!
//! Generational ids mean a [`NodeId`] kept after its node was removed never reaches a new node
//! that reuses the slot. Dirty bits keep idle frames free: nothing is laid out or painted
//! unless something changed (DESIGN §2, "idle means idle").

use std::ops::{BitOr, BitOrAssign};

use crate::display_list::{DisplayList, Op};
use crate::geometry::{Color, Rect};
use annulus_config::tokens::{self, Palette, Rgb};

use crate::style::Style;
use crate::widget::{Action, PaintCx, TextMeasure, Widget, WidgetState};

/// A handle to a node. It stays valid until the node is removed, and never refers to another
/// node afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// A number unique among the live and removed nodes of a tree, for identifiers outside
    /// it (the accessibility tree).
    #[must_use]
    pub fn to_raw(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

/// What must be redone for a node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dirty(u8);

impl Dirty {
    /// Nothing to redo.
    pub const NONE: Dirty = Dirty(0);
    /// Size or position may change. Marking a node propagates this to its ancestors, since
    /// their layout depends on their children.
    pub const LAYOUT: Dirty = Dirty(1);
    /// Only the node's pixels change.
    pub const PAINT: Dirty = Dirty(1 << 1);

    /// Whether every bit of `other` is set.
    #[must_use]
    pub fn contains(self, other: Dirty) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no bit is set.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Dirty {
    type Output = Dirty;

    fn bitor(self, rhs: Dirty) -> Dirty {
        Dirty(self.0 | rhs.0)
    }
}

impl BitOrAssign for Dirty {
    fn bitor_assign(&mut self, rhs: Dirty) {
        self.0 |= rhs.0;
    }
}

/// Why a tree operation was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    /// The id refers to a node that was removed.
    #[error("the node no longer exists")]
    StaleNode,
    /// The root can't be removed; replace the tree instead.
    #[error("the root node cannot be removed")]
    RemoveRoot,
}

pub(crate) struct Node {
    pub(crate) widget: Box<dyn Widget>,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) bounds: Rect,
    pub(crate) dirty: Dirty,
    pub(crate) style: Style,
    /// How far the content is scrolled along the node's axis, in DIPs.
    pub(crate) scroll_offset: f32,
    /// How far scrolling can go, from the last layout.
    pub(crate) scroll_max: f32,
}

struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// The widget tree of one window.
pub struct Tree {
    slots: Vec<Slot>,
    free: Vec<u32>,
    root: NodeId,
    /// Area that must be repainted, collected from nodes marked [`Dirty::PAINT`] and from
    /// nodes that moved or disappeared.
    damage: Rect,
    /// The node keyboard input goes to.
    pub(crate) focused: Option<NodeId>,
    /// Whether the focus ring shows: after keyboard navigation, not after a click.
    pub(crate) focus_visible: bool,
    /// The node that handled a pointer press and receives pointer events until release.
    pub(crate) captured: Option<NodeId>,
    /// The node under the pointer.
    pub(crate) hovered: Option<NodeId>,
    /// Actions widgets reported since the app last took them.
    pub(crate) actions: Vec<(NodeId, Action)>,
    /// The active theme's colors.
    palette: Palette,
}

impl Tree {
    /// A tree holding only `root`.
    #[must_use]
    pub fn new(root: Box<dyn Widget>) -> Self {
        let mut tree = Self {
            slots: Vec::new(),
            free: Vec::new(),
            root: NodeId {
                index: 0,
                generation: 0,
            },
            damage: Rect::default(),
            focused: None,
            focus_visible: false,
            captured: None,
            hovered: None,
            actions: Vec::new(),
            palette: tokens::DARK,
        };
        tree.root = tree.allocate(root, None);
        tree
    }

    /// The root node.
    #[must_use]
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Whether `id` refers to a live node.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.node(id).is_some()
    }

    /// Adds `widget` as the last child of `parent`. The new node and its ancestors need layout.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `parent` was removed.
    pub fn append_child(
        &mut self,
        parent: NodeId,
        widget: Box<dyn Widget>,
    ) -> Result<NodeId, TreeError> {
        if !self.contains(parent) {
            return Err(TreeError::StaleNode);
        }
        let child = self.allocate(widget, Some(parent));
        if let Some(node) = self.node_mut(parent) {
            node.children.push(child);
        }
        self.mark_dirty(child, Dirty::LAYOUT | Dirty::PAINT)?;
        Ok(child)
    }

    /// Removes `id` and its whole subtree. Their area is damaged and the parent needs layout.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `id` was already removed, [`TreeError::RemoveRoot`] for
    /// the root.
    pub fn remove(&mut self, id: NodeId) -> Result<(), TreeError> {
        if id == self.root {
            return Err(TreeError::RemoveRoot);
        }
        let parent = self.node(id).ok_or(TreeError::StaleNode)?.parent;
        if let Some(parent) = parent {
            if let Some(node) = self.node_mut(parent) {
                node.children.retain(|child| *child != id);
            }
            self.mark_dirty(parent, Dirty::LAYOUT)?;
        }
        let mut pending = vec![id];
        while let Some(next) = pending.pop() {
            let Some(node) = self.release(next) else {
                continue;
            };
            self.damage = self.damage.union(&node.bounds);
            pending.extend(node.children);
        }
        Ok(())
    }

    /// The children of `id`, in paint order (back to front). Empty for a removed node.
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.node(id).map_or(&[], |node| node.children.as_slice())
    }

    /// The parent of `id`, or `None` for the root and for removed nodes.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).and_then(|node| node.parent)
    }

    /// The widget of `id`.
    #[must_use]
    pub fn widget(&self, id: NodeId) -> Option<&dyn Widget> {
        self.node(id).map(|node| node.widget.as_ref())
    }

    /// The bounds layout last gave `id`.
    #[must_use]
    pub fn bounds(&self, id: NodeId) -> Option<Rect> {
        self.node(id).map(|node| node.bounds)
    }

    /// What `id` still needs.
    #[must_use]
    pub fn dirty(&self, id: NodeId) -> Option<Dirty> {
        self.node(id).map(|node| node.dirty)
    }

    /// Marks `id` as needing `dirty`. [`Dirty::LAYOUT`] also marks every ancestor for layout.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `id` was removed.
    pub fn mark_dirty(&mut self, id: NodeId, dirty: Dirty) -> Result<(), TreeError> {
        let node = self.node_mut(id).ok_or(TreeError::StaleNode)?;
        node.dirty |= dirty;
        if dirty.contains(Dirty::PAINT) {
            let bounds = node.bounds;
            self.damage = self.damage.union(&bounds);
        }
        if dirty.contains(Dirty::LAYOUT) {
            let mut ancestor = self.parent(id);
            while let Some(next) = ancestor {
                let Some(node) = self.node_mut(next) else {
                    break;
                };
                if node.dirty.contains(Dirty::LAYOUT) {
                    break; // Everything above is already marked.
                }
                node.dirty |= Dirty::LAYOUT;
                ancestor = node.parent;
            }
        }
        Ok(())
    }

    /// Records the bounds layout computed for `id`. Moving or resizing damages both the old
    /// and the new area; unchanged bounds damage nothing. Clears the node's layout bit.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `id` was removed.
    pub fn set_bounds(&mut self, id: NodeId, bounds: Rect) -> Result<(), TreeError> {
        let node = self.node_mut(id).ok_or(TreeError::StaleNode)?;
        node.dirty = Dirty(node.dirty.0 & !Dirty::LAYOUT.0);
        if node.bounds != bounds {
            let old = node.bounds;
            node.bounds = bounds;
            node.dirty |= Dirty::PAINT;
            self.damage = self.damage.union(&old).union(&bounds);
        }
        Ok(())
    }

    /// The layout style of `id`.
    #[must_use]
    pub fn style(&self, id: NodeId) -> Option<&Style> {
        self.node(id).map(|node| &node.style)
    }

    /// Replaces the layout style of `id` with [`Style::sanitized`]; it needs layout and paint
    /// again.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `id` was removed.
    pub fn set_style(&mut self, id: NodeId, style: Style) -> Result<(), TreeError> {
        self.node_mut(id).ok_or(TreeError::StaleNode)?.style = style.sanitized();
        self.mark_dirty(id, Dirty::LAYOUT | Dirty::PAINT)
    }

    /// How far `id`'s content is scrolled.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> Option<f32> {
        self.node(id).map(|node| node.scroll_offset)
    }

    /// Scrolls `id` by `delta` DIPs along its axis, clamped to the content, and returns the
    /// new offset. A non-finite delta is ignored.
    ///
    /// # Errors
    /// [`TreeError::StaleNode`] if `id` was removed.
    pub fn scroll_by(&mut self, id: NodeId, delta: f32) -> Result<f32, TreeError> {
        let node = self.node_mut(id).ok_or(TreeError::StaleNode)?;
        let old = node.scroll_offset;
        if delta.is_finite() {
            node.scroll_offset = (old + delta).clamp(0.0, node.scroll_max);
        }
        let new = node.scroll_offset;
        #[allow(clippy::float_cmp)] // Exact on purpose: did clamping change the offset at all?
        if new != old {
            self.mark_dirty(id, Dirty::LAYOUT | Dirty::PAINT)?;
        }
        Ok(new)
    }

    /// The area to repaint since the last [`Self::paint`]. Empty when nothing changed.
    #[must_use]
    pub fn damage(&self) -> Rect {
        self.damage
    }

    /// The active theme's colors.
    #[must_use]
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Switches the theme; everything repaints.
    pub fn set_palette(&mut self, palette: Palette) {
        if palette != self.palette {
            self.palette = palette;
            let root = self.root;
            let _ = self.mark_dirty(root, Dirty::PAINT);
            self.damage = self.bounds(root).unwrap_or_default();
        }
    }

    /// The node under the pointer, if it still exists.
    #[must_use]
    pub fn hovered(&self) -> Option<NodeId> {
        self.hovered.filter(|id| self.contains(*id))
    }

    /// The actions widgets reported since the last call, in order, with the node each came
    /// from. The app's view models act on them.
    pub fn take_actions(&mut self) -> Vec<(NodeId, Action)> {
        std::mem::take(&mut self.actions)
    }

    /// Paints the whole tree, parents before children, and clears every paint bit and the
    /// damage. Backends redraw only [`Self::damage`] from the result.
    ///
    /// Flow children paint in order, then overlay children above them. A scrolling node clips
    /// its children to its bounds.
    pub fn paint(&mut self, text: &mut dyn TextMeasure) -> DisplayList {
        let mut list = DisplayList::new();
        self.paint_node(self.root, &mut list, text);
        self.damage = Rect::default();
        list
    }

    fn paint_node(&mut self, id: NodeId, list: &mut DisplayList, text: &mut dyn TextMeasure) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        node.dirty = Dirty(node.dirty.0 & !Dirty::PAINT.0);
        let (bounds, clips) = (node.bounds, node.style.scroll);
        let focused = self.focused == Some(id);
        let state = WidgetState {
            hovered: self.hovered == Some(id),
            pressed: self.captured == Some(id),
            focused,
            focus_visible: focused && self.focus_visible,
        };
        let mut cx = PaintCx {
            bounds,
            palette: &self.palette,
            state,
            text,
        };
        if let Some(node) = self.node(id) {
            node.widget.paint(&mut cx, list);
        }
        if state.focus_visible {
            list.push(focus_ring(bounds, self.palette.accent));
        }
        let (overlays, flow) = self.split_children(id);
        if clips {
            list.clip(bounds, |list| {
                for child in flow {
                    self.paint_node(child, list, text);
                }
            });
        } else {
            for child in flow {
                self.paint_node(child, list, text);
            }
        }
        for child in overlays {
            self.paint_node(child, list, text);
        }
    }

    fn allocate(&mut self, widget: Box<dyn Widget>, parent: Option<NodeId>) -> NodeId {
        let node = Node {
            widget,
            parent,
            children: Vec::new(),
            bounds: Rect::default(),
            dirty: Dirty::LAYOUT | Dirty::PAINT,
            style: Style::default(),
            scroll_offset: 0.0,
            scroll_max: 0.0,
        };
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.node = Some(node);
            NodeId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
            self.slots.push(Slot {
                generation: 0,
                node: Some(node),
            });
            NodeId {
                index,
                generation: 0,
            }
        }
    }

    fn release(&mut self, id: NodeId) -> Option<Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let node = slot.node.take()?;
        // Wrapping is harmless: an id would need 2^32 reuses of one slot to collide.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(id.index);
        Some(node)
    }

    pub(crate) fn node(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_mut()
        } else {
            None
        }
    }
}

/// The ring drawn around the focused widget after keyboard navigation (UI-SPEC §8:
/// accent, 2 DIPs, offset 2 DIPs).
fn focus_ring(bounds: Rect, accent: Rgb) -> Op {
    const WIDTH: f32 = tokens::metrics::FOCUS_RING_WIDTH;
    Op::Border {
        rect: Rect::new(
            bounds.origin.x - WIDTH,
            bounds.origin.y - WIDTH,
            bounds.size.width + 2.0 * WIDTH,
            bounds.size.height + 2.0 * WIDTH,
        ),
        radius: WIDTH,
        width: WIDTH,
        color: Color::rgb(accent.r, accent.g, accent.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FixedAdvance;

    /// Paints one rectangle whose color encodes `tag`, so tests can read paint order.
    struct Swatch(u8);

    impl Widget for Swatch {
        fn name(&self) -> &'static str {
            "swatch"
        }

        fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
            list.push(Op::Rect {
                rect: cx.bounds,
                color: Color::rgb(self.0, 0, 0),
            });
        }
    }

    fn swatch(tag: u8) -> Box<dyn Widget> {
        Box::new(Swatch(tag))
    }

    fn painted_tags(list: &DisplayList) -> Vec<u8> {
        list.ops()
            .iter()
            .filter_map(|op| match op {
                Op::Rect { color, .. } => Some(color.r),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn removed_id_never_reaches_the_node_that_reuses_its_slot() {
        let mut tree = Tree::new(swatch(0));
        let old = tree.append_child(tree.root(), swatch(1)).unwrap();
        tree.remove(old).unwrap();
        let new = tree.append_child(tree.root(), swatch(2)).unwrap();
        assert!(!tree.contains(old));
        assert!(tree.widget(old).is_none());
        assert_eq!(tree.remove(old), Err(TreeError::StaleNode));
        assert_eq!(tree.append_child(old, swatch(3)), Err(TreeError::StaleNode));
        assert!(tree.contains(new));
    }

    #[test]
    fn removing_a_node_removes_its_subtree_and_detaches_it() {
        let mut tree = Tree::new(swatch(0));
        let a = tree.append_child(tree.root(), swatch(1)).unwrap();
        let a1 = tree.append_child(a, swatch(2)).unwrap();
        let b = tree.append_child(tree.root(), swatch(3)).unwrap();
        tree.remove(a).unwrap();
        assert!(!tree.contains(a1));
        assert_eq!(tree.children(tree.root()), &[b]);
        assert_eq!(painted_tags(&tree.paint(&mut FixedAdvance)), [0, 3]);
    }

    #[test]
    fn root_cannot_be_removed() {
        let mut tree = Tree::new(swatch(0));
        assert_eq!(tree.remove(tree.root()), Err(TreeError::RemoveRoot));
    }

    #[test]
    fn paint_order_is_parent_first_then_children_in_order() {
        let mut tree = Tree::new(swatch(0));
        let a = tree.append_child(tree.root(), swatch(1)).unwrap();
        tree.append_child(a, swatch(2)).unwrap();
        tree.append_child(tree.root(), swatch(3)).unwrap();
        assert_eq!(painted_tags(&tree.paint(&mut FixedAdvance)), [0, 1, 2, 3]);
    }

    #[test]
    fn layout_dirt_reaches_ancestors_but_paint_dirt_does_not() {
        let mut tree = Tree::new(swatch(0));
        let a = tree.append_child(tree.root(), swatch(1)).unwrap();
        let leaf = tree.append_child(a, swatch(2)).unwrap();
        for id in [tree.root(), a, leaf] {
            tree.set_bounds(id, Rect::new(0.0, 0.0, 1.0, 1.0)).unwrap();
        }
        tree.paint(&mut FixedAdvance);

        tree.mark_dirty(leaf, Dirty::PAINT).unwrap();
        assert_eq!(tree.dirty(a), Some(Dirty::NONE));

        tree.mark_dirty(leaf, Dirty::LAYOUT).unwrap();
        assert!(tree.dirty(a).unwrap().contains(Dirty::LAYOUT));
        assert!(tree.dirty(tree.root()).unwrap().contains(Dirty::LAYOUT));
    }

    #[test]
    fn idle_tree_has_no_damage_after_paint() {
        let mut tree = Tree::new(swatch(0));
        tree.set_bounds(tree.root(), Rect::new(0.0, 0.0, 100.0, 100.0))
            .unwrap();
        tree.paint(&mut FixedAdvance);
        tree.set_bounds(tree.root(), Rect::new(0.0, 0.0, 100.0, 100.0))
            .unwrap();
        assert!(tree.damage().is_empty());
        assert_eq!(tree.dirty(tree.root()), Some(Dirty::NONE));
    }

    #[test]
    fn moving_a_node_damages_its_old_and_new_area() {
        let mut tree = Tree::new(swatch(0));
        let child = tree.append_child(tree.root(), swatch(1)).unwrap();
        tree.set_bounds(child, Rect::new(0.0, 0.0, 10.0, 10.0))
            .unwrap();
        tree.paint(&mut FixedAdvance);
        tree.set_bounds(child, Rect::new(50.0, 0.0, 10.0, 10.0))
            .unwrap();
        assert_eq!(tree.damage(), Rect::new(0.0, 0.0, 60.0, 10.0));
    }

    #[test]
    fn removing_a_node_damages_its_area() {
        let mut tree = Tree::new(swatch(0));
        let child = tree.append_child(tree.root(), swatch(1)).unwrap();
        tree.set_bounds(child, Rect::new(5.0, 5.0, 10.0, 10.0))
            .unwrap();
        tree.paint(&mut FixedAdvance);
        tree.remove(child).unwrap();
        assert_eq!(tree.damage(), Rect::new(5.0, 5.0, 10.0, 10.0));
        assert!(tree.dirty(tree.root()).unwrap().contains(Dirty::LAYOUT));
    }
}
