//! Animation: time-based tweens on paint-only properties (DESIGN §7.2).
//!
//! Time is passed in as milliseconds from any fixed origin, never read from a clock, so tests
//! are deterministic. The app asks for a new frame only while [`Animator::needs_frame`] is
//! true, so a still window does no work. With "reduce motion" on, every change is immediate.

use crate::tree::{Dirty, NodeId, Tree};

/// A property an animation may change. Only paint-only properties animate, so animations
/// never trigger layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Property {
    /// Group opacity, 0.0 to 1.0.
    Opacity,
    /// Horizontal paint offset in DIPs (slide-in panels).
    OffsetX,
    /// Vertical paint offset in DIPs (popovers, toasts).
    OffsetY,
}

/// How progress maps to the value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Easing {
    /// Constant speed.
    Linear,
    /// Fast start, gentle stop (cubic); UI-SPEC's default for panels and popovers.
    #[default]
    EaseOut,
}

impl Easing {
    fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
        }
    }
}

/// One running animation of one property.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tween {
    node: NodeId,
    property: Property,
    from: f32,
    to: f32,
    start_ms: u64,
    duration_ms: u32,
    easing: Easing,
}

impl Tween {
    fn value_at(&self, now_ms: u64) -> f32 {
        if self.duration_ms == 0 {
            return self.to;
        }
        let elapsed = now_ms.saturating_sub(self.start_ms);
        #[allow(clippy::cast_precision_loss)] // Durations are seconds long, far below 2^24 ms.
        let t = elapsed as f32 / self.duration_ms as f32;
        self.from + (self.to - self.from) * self.easing.apply(t)
    }

    fn finished_at(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.start_ms) >= u64::from(self.duration_ms)
    }
}

/// A property's value at this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Update {
    /// Which node.
    pub node: NodeId,
    /// Which property.
    pub property: Property,
    /// Its value now.
    pub value: f32,
}

/// All running animations of one window.
#[derive(Debug, Default)]
pub struct Animator {
    tweens: Vec<Tween>,
    /// Final values of animations that ended (or were skipped) since the last tick.
    settled: Vec<Update>,
    reduce_motion: bool,
}

impl Animator {
    /// An animator with no animations.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows the OS "reduce motion" setting: when on, animations jump to their end.
    pub fn set_reduce_motion(&mut self, reduce: bool) {
        self.reduce_motion = reduce;
        if reduce {
            for tween in self.tweens.drain(..) {
                self.settled.push(Update {
                    node: tween.node,
                    property: tween.property,
                    value: tween.to,
                });
            }
        }
    }

    /// Animates `property` of `node` from `from` to `to` over `duration_ms`, starting at
    /// `now_ms`. If that property is already animating, the new animation starts from its
    /// current value instead of `from`, so retargeting never jumps.
    #[allow(clippy::too_many_arguments)] // Each argument is a distinct part of the request.
    pub fn start(
        &mut self,
        node: NodeId,
        property: Property,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        now_ms: u64,
    ) {
        let running = self
            .tweens
            .iter()
            .position(|t| t.node == node && t.property == property);
        let from = running.map_or(from, |index| self.tweens[index].value_at(now_ms));
        if let Some(index) = running {
            self.tweens.remove(index);
        }
        if self.reduce_motion || duration_ms == 0 {
            self.settled.push(Update {
                node,
                property,
                value: to,
            });
            return;
        }
        self.tweens.push(Tween {
            node,
            property,
            from,
            to,
            start_ms: now_ms,
            duration_ms,
            easing,
        });
    }

    /// Whether another frame is needed to continue an animation.
    #[must_use]
    pub fn needs_frame(&self) -> bool {
        !self.tweens.is_empty() || !self.settled.is_empty()
    }

    /// The values at `now_ms`, with every affected node marked for repaint. Finished
    /// animations report their final value once and stop. Animations of removed nodes stop
    /// silently.
    pub fn tick(&mut self, now_ms: u64, tree: &mut Tree) -> Vec<Update> {
        self.tweens.retain(|tween| tree.contains(tween.node));
        let mut updates: Vec<Update> = self
            .settled
            .drain(..)
            .filter(|u| tree.contains(u.node))
            .collect();
        for tween in &self.tweens {
            updates.push(Update {
                node: tween.node,
                property: tween.property,
                value: tween.value_at(now_ms),
            });
        }
        self.tweens.retain(|tween| !tween.finished_at(now_ms));
        for update in &updates {
            let _ = tree.mark_dirty(update.node, Dirty::PAINT);
        }
        updates
    }
}

#[cfg(test)]
mod tests {
    // Animation values are computed from exact inputs; endpoints must be exact.
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::display_list::DisplayList;
    use crate::widget::{PaintCx, Widget};

    struct Blank;

    impl Widget for Blank {
        fn name(&self) -> &'static str {
            "blank"
        }

        fn paint(&self, _cx: &PaintCx<'_>, _list: &mut DisplayList) {}
    }

    fn setup() -> (Tree, NodeId, Animator) {
        let mut tree = Tree::new(Box::new(Blank));
        let node = tree.append_child(tree.root(), Box::new(Blank)).unwrap();
        (tree, node, Animator::new())
    }

    fn value(updates: &[Update]) -> f32 {
        updates.last().unwrap().value
    }

    #[test]
    fn tween_runs_from_start_to_end_then_stops_requesting_frames() {
        let (mut tree, node, mut animator) = setup();
        animator.start(node, Property::Opacity, 0.0, 1.0, 100, Easing::Linear, 1000);
        assert_eq!(value(&animator.tick(1000, &mut tree)), 0.0);
        assert_eq!(value(&animator.tick(1050, &mut tree)), 0.5);
        assert!(animator.needs_frame());
        assert_eq!(value(&animator.tick(1200, &mut tree)), 1.0);
        assert!(!animator.needs_frame());
        assert_eq!(animator.tick(1300, &mut tree), Vec::<Update>::new());
    }

    #[test]
    fn ease_out_moves_faster_at_the_start() {
        let (mut tree, node, mut animator) = setup();
        animator.start(node, Property::OffsetX, 0.0, 100.0, 100, Easing::EaseOut, 0);
        assert!(value(&animator.tick(50, &mut tree)) > 50.0);
    }

    #[test]
    fn reduce_motion_jumps_to_the_end_in_one_frame() {
        let (mut tree, node, mut animator) = setup();
        animator.set_reduce_motion(true);
        animator.start(node, Property::OffsetY, -20.0, 0.0, 120, Easing::EaseOut, 0);
        assert_eq!(value(&animator.tick(0, &mut tree)), 0.0);
        assert!(!animator.needs_frame());
    }

    #[test]
    fn turning_on_reduce_motion_finishes_running_animations() {
        let (mut tree, node, mut animator) = setup();
        animator.start(node, Property::Opacity, 0.0, 1.0, 1000, Easing::Linear, 0);
        animator.set_reduce_motion(true);
        assert_eq!(value(&animator.tick(10, &mut tree)), 1.0);
        assert!(!animator.needs_frame());
    }

    #[test]
    fn retargeting_continues_from_the_current_value() {
        let (mut tree, node, mut animator) = setup();
        animator.start(node, Property::OffsetX, 0.0, 100.0, 100, Easing::Linear, 0);
        // Halfway (at 50) the panel is told to go back; it must not jump to the new `from`.
        animator.start(node, Property::OffsetX, 999.0, 0.0, 100, Easing::Linear, 50);
        assert_eq!(value(&animator.tick(50, &mut tree)), 50.0);
        assert_eq!(value(&animator.tick(100, &mut tree)), 25.0);
    }

    #[test]
    fn animation_of_a_removed_node_stops() {
        let (mut tree, node, mut animator) = setup();
        animator.start(node, Property::Opacity, 0.0, 1.0, 100, Easing::Linear, 0);
        tree.remove(node).unwrap();
        assert_eq!(animator.tick(10, &mut tree), Vec::<Update>::new());
        assert!(!animator.needs_frame());
    }

    #[test]
    fn tick_marks_animated_nodes_for_repaint() {
        let (mut tree, node, mut animator) = setup();
        tree.paint();
        animator.start(node, Property::Opacity, 0.0, 1.0, 100, Easing::Linear, 0);
        animator.tick(10, &mut tree);
        assert!(tree.dirty(node).unwrap().contains(Dirty::PAINT));
    }
}
