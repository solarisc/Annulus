//! Motion (UI-SPEC §8): curves, tweens that retarget without a jump, springs for positions,
//! and layers that move, scale and fade everything drawn inside them. A layer applies to the
//! zones registered inside it too, so what is clicked is what is seen even mid-animation.

use annulus_config::tokens::motion;

use crate::gfx::{Canvas, Color, Font, Point, Rect, TextSize};

/// One frame at 60 Hz, in seconds: a change that arrives between two frames is taken to have
/// started at most this long before the frame that first sees it.
pub(crate) const FRAME: f64 = 1.0 / 60.0;

/// How a value travels from where it is to where it is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    /// Constant speed: progress bars, spinners.
    Linear,
    /// Fast at first, settling softly: most changes.
    Out,
    /// Slow at both ends: something moving across the screen.
    InOut,
    /// Like [`Curve::Out`], passing the target a little and coming back: things that pop in.
    Overshoot,
}

impl Curve {
    /// The share of the way done at time share `t` (both 0.0 to 1.0; overshoot passes 1.0).
    #[must_use]
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::Out => 1.0 - (1.0 - t).powi(3),
            Curve::InOut => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (2.0 - 2.0 * t).powi(3) / 2.0
                }
            }
            Curve::Overshoot => {
                // "Back" easing, gentle: at most about 5 % past the target.
                const C1: f32 = 1.3;
                const C3: f32 = C1 + 1.0;
                1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
            }
        }
    }
}

/// How long a change takes, along which curve, after how long a wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Duration in milliseconds.
    pub ms: u32,
    /// The curve.
    pub curve: Curve,
    /// Wait before starting, in milliseconds (rows of a list filling in one after another).
    pub delay: u32,
}

impl Motion {
    /// Hover, press and focus feedback.
    pub const FAST: Motion = Motion::new(motion::FAST, Curve::Out);
    /// Panels, menus, page and tab changes.
    pub const BASE: Motion = Motion::new(motion::BASE, Curve::Out);
    /// Large surfaces.
    pub const SLOW: Motion = Motion::new(motion::SLOW, Curve::Out);
    /// Menus, cards and dialogs growing into place.
    pub const POP: Motion = Motion::new(motion::BASE, Curve::Overshoot);

    /// `ms` long along `curve`, at once.
    #[must_use]
    pub const fn new(ms: u32, curve: Curve) -> Self {
        Self {
            ms,
            curve,
            delay: 0,
        }
    }

    /// The same, starting `ms` later.
    #[must_use]
    pub const fn delayed(self, ms: u32) -> Self {
        Self { delay: ms, ..self }
    }

    /// The same, starting after row `index` of a list filling in (capped, so a long list is
    /// never slow).
    #[must_use]
    pub fn staggered(self, index: usize) -> Self {
        let index = u32::try_from(index.min(10)).unwrap_or(10);
        self.delayed(index * motion::STAGGER)
    }
}

/// A value on its way from `from` to `to`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tween {
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) start: f64,
    pub(crate) motion: Motion,
    /// The frame that last asked for it.
    pub(crate) seen: f64,
}

impl Tween {
    /// Resting at `value`.
    pub(crate) fn at(value: f32, now: f64) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            motion: Motion::FAST,
            seen: now,
        }
    }

    /// The value at `now`, and whether it is still moving.
    pub(crate) fn sample(&self, now: f64) -> (f32, bool) {
        let duration = f64::from(self.motion.ms) / 1000.0;
        let begin = self.start + f64::from(self.motion.delay) / 1000.0;
        if (self.to - self.from).abs() <= f32::EPSILON || now >= begin + duration {
            return (self.to, false);
        }
        if now <= begin {
            return (self.from, true);
        }
        #[allow(clippy::cast_possible_truncation)] // A share between 0 and 1.
        let share = ((now - begin) / duration) as f32;
        let eased = self.motion.curve.at(share);
        (self.from + (self.to - self.from) * eased, true)
    }
}

/// How quickly a spring closes the gap: settles in about 0.2 s.
const STIFFNESS: f32 = 26.0;

/// A critically damped spring: positions that follow their target without overshooting, and
/// keep their speed when the target moves again mid-way.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spring {
    pub(crate) value: f32,
    velocity: f32,
    pub(crate) target: f32,
    pub(crate) seen: f64,
}

impl Spring {
    /// Resting at `value`.
    pub(crate) fn at(value: f32, now: f64) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            seen: now,
        }
    }

    /// Moves to `now`, toward `target`; returns whether it is still moving.
    pub(crate) fn advance(&mut self, target: f32, now: f64) -> bool {
        let retargeted = (target - self.target).abs() > f32::EPSILON;
        let mut elapsed = (now - self.seen).max(0.0);
        if retargeted {
            // As with tweens: the change started at most a frame ago.
            elapsed = elapsed.min(FRAME);
        }
        self.target = target;
        self.seen = now;
        #[allow(clippy::cast_possible_truncation)] // Frame gaps.
        let dt = elapsed as f32;
        let gap = self.value - target;
        let decay = (-STIFFNESS * dt).exp();
        let lead = self.velocity + STIFFNESS * gap;
        let gap = (gap + lead * dt) * decay;
        self.velocity = (self.velocity - STIFFNESS * lead * dt) * decay;
        self.value = target + gap;
        if gap.abs() < 0.2 && self.velocity.abs() < 2.0 {
            self.value = target;
            self.velocity = 0.0;
            return false;
        }
        true
    }
}

/// What a layer does to everything drawn inside it: scaled about `origin`, then moved by
/// `shift`, then faded by `alpha`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    /// Moved by this much.
    pub shift: Point,
    /// Scaled by this much…
    pub scale: f32,
    /// …about this point.
    pub origin: Point,
    /// Opacity, 0.0 to 1.0, times the opacity of the layers around it.
    pub alpha: f32,
}

impl Layer {
    /// Changes nothing.
    pub const NONE: Layer = Layer {
        shift: Point::new(0.0, 0.0),
        scale: 1.0,
        origin: Point::new(0.0, 0.0),
        alpha: 1.0,
    };

    /// Faded to `alpha`.
    #[must_use]
    pub fn fade(alpha: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.0, 1.0),
            ..Self::NONE
        }
    }

    /// This layer, also moved by `dx`, `dy`.
    #[must_use]
    pub fn shifted(self, dx: f32, dy: f32) -> Self {
        Self {
            shift: Point::new(self.shift.x + dx, self.shift.y + dy),
            ..self
        }
    }

    /// This layer, also scaled by `scale` about `origin`.
    #[must_use]
    pub fn scaled(self, origin: Point, scale: f32) -> Self {
        Self {
            scale: scale.max(0.01),
            origin,
            ..self
        }
    }

    /// A surface growing into place as `t` goes from 0.0 to 1.0: faded in, scaled up from
    /// `from` of its size about `origin` and slid by `rise` DIPs upward.
    #[must_use]
    pub fn appear(t: f32, origin: Point, from: f32, rise: f32) -> Self {
        Self::fade(t)
            .scaled(origin, from + (1.0 - from) * t)
            .shifted(0.0, rise * (1.0 - t))
    }
}

/// A uniform scale and a translation (screen = local * scale + shift) and an opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transform {
    scale: f32,
    shift: Point,
    alpha: f32,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        scale: 1.0,
        shift: Point::new(0.0, 0.0),
        alpha: 1.0,
    };

    /// `layer` applied inside this transform.
    fn then(self, layer: Layer) -> Transform {
        // The layer alone: p * s + (origin * (1 - s) + shift).
        let s = layer.scale;
        let local = Point::new(
            layer.origin.x * (1.0 - s) + layer.shift.x,
            layer.origin.y * (1.0 - s) + layer.shift.y,
        );
        Transform {
            scale: self.scale * s,
            shift: Point::new(
                local.x * self.scale + self.shift.x,
                local.y * self.scale + self.shift.y,
            ),
            alpha: self.alpha * layer.alpha,
        }
    }

    fn point(&self, p: Point) -> Point {
        Point::new(
            p.x * self.scale + self.shift.x,
            p.y * self.scale + self.shift.y,
        )
    }

    fn rect(&self, r: Rect) -> Rect {
        let at = self.point(Point::new(r.x, r.y));
        Rect::new(at.x, at.y, r.w * self.scale, r.h * self.scale)
    }

    fn color(&self, color: Color) -> Color {
        if self.alpha >= 1.0 {
            color
        } else {
            color.alpha(self.alpha)
        }
    }

    fn identity(&self) -> bool {
        *self == Transform::IDENTITY
    }
}

/// The canvas views draw on: everything goes through the layers pushed (see
/// [`crate::kit::Ui::push_layer`]) onto the renderer's canvas.
pub struct Layers<'a> {
    inner: &'a mut dyn Canvas,
    now: Transform,
    stack: Vec<Transform>,
}

impl<'a> Layers<'a> {
    /// Draws onto `inner`, with no layer yet.
    pub fn new(inner: &'a mut dyn Canvas) -> Self {
        Self {
            inner,
            now: Transform::IDENTITY,
            stack: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, layer: Layer) {
        self.stack.push(self.now);
        self.now = self.now.then(layer);
    }

    pub(crate) fn pop(&mut self) {
        if let Some(before) = self.stack.pop() {
            self.now = before;
        }
    }

    /// How many layers are pushed.
    pub(crate) fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Where `rect`, in the current layer, is on screen.
    #[must_use]
    pub fn to_screen(&self, rect: Rect) -> Rect {
        self.now.rect(rect)
    }

    /// The opacity of the current layer.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        self.now.alpha
    }

    /// Clips to `rect`, which is already in screen coordinates.
    pub(crate) fn push_screen_clip(&mut self, rect: Rect) {
        self.inner.push_clip(rect);
    }

    /// Nothing drawn now would show.
    fn invisible(&self) -> bool {
        self.now.alpha <= 0.002
    }
}

impl Canvas for Layers<'_> {
    fn scale(&self) -> f32 {
        self.inner.scale()
    }

    fn fill_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner
            .fill_rect(t.rect(rect), radius * t.scale, t.color(color));
    }

    fn stroke_rect(&mut self, rect: Rect, radius: f32, width: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner.stroke_rect(
            t.rect(rect),
            radius * t.scale,
            width * t.scale,
            t.color(color),
        );
    }

    fn shadow(&mut self, rect: Rect, radius: f32, blur: f32, offset_y: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner.shadow(
            t.rect(rect),
            radius * t.scale,
            blur * t.scale,
            offset_y * t.scale,
            t.color(color),
        );
    }

    fn fill_circle(&mut self, center: Point, radius: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner
            .fill_circle(t.point(center), radius * t.scale, t.color(color));
    }

    fn polyline(&mut self, points: &[Point], closed: bool, width: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        if t.identity() {
            self.inner.polyline(points, closed, width, color);
            return;
        }
        let mapped: Vec<Point> = points.iter().map(|p| t.point(*p)).collect();
        self.inner
            .polyline(&mapped, closed, width * t.scale, t.color(color));
    }

    fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner.stroke_circle(
            t.point(center),
            radius * t.scale,
            width * t.scale,
            t.color(color),
        );
    }

    fn measure(&mut self, text: &str, font: Font) -> TextSize {
        // Layout happens in the layer's own units.
        self.inner.measure(text, font)
    }

    fn draw_text(&mut self, origin: Point, text: &str, font: Font, color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        let font = Font {
            size: font.size * t.scale,
            ..font
        };
        self.inner
            .draw_text(t.point(origin), text, font, t.color(color));
    }

    fn push_clip(&mut self, rect: Rect) {
        let rect = self.now.rect(rect);
        self.inner.push_clip(rect);
    }

    fn pop_clip(&mut self) {
        self.inner.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::Recorder;

    #[test]
    fn curves_start_at_zero_and_end_at_one() {
        for curve in [Curve::Linear, Curve::Out, Curve::InOut, Curve::Overshoot] {
            assert!(curve.at(0.0).abs() < 1e-6, "{curve:?}");
            assert!((curve.at(1.0) - 1.0).abs() < 1e-6, "{curve:?}");
        }
        // Out is ahead of linear; overshoot passes the end a little, never by much.
        assert!(Curve::Out.at(0.3) > 0.3);
        let peak = (0_u8..=100)
            .map(|i| Curve::Overshoot.at(f32::from(i) / 100.0))
            .fold(0.0_f32, f32::max);
        assert!(peak > 1.0 && peak < 1.08, "{peak}");
    }

    #[test]
    fn a_tween_waits_out_its_delay_then_arrives_on_time() {
        let tween = Tween {
            from: 0.0,
            to: 10.0,
            start: 1.0,
            motion: Motion::BASE.delayed(100),
            seen: 1.0,
        };
        assert_eq!(tween.sample(1.05), (0.0, true));
        let (mid, moving) = tween.sample(1.2);
        assert!(moving && mid > 0.0 && mid < 10.0);
        assert_eq!(tween.sample(1.3), (10.0, false));
    }

    #[test]
    fn a_spring_settles_on_its_target_and_keeps_its_speed_when_it_moves() {
        let mut spring = Spring::at(0.0, 0.0);
        let mut time = 0.0;
        let mut moving = true;
        let mut steps = 0;
        while moving {
            time += FRAME;
            moving = spring.advance(100.0, time);
            steps += 1;
            assert!(spring.value <= 100.0 + 1e-3, "never overshoots");
        }
        assert_eq!(spring.value, 100.0);
        assert!(steps < 30, "about half a second at most: {steps} frames");
        // Mid-way to a new target, the target moves again: no jump back.
        spring.advance(200.0, time + FRAME);
        let before = spring.value;
        spring.advance(300.0, time + 2.0 * FRAME);
        assert!(spring.value > before);
        // A long gap with no change finishes the move.
        assert!(!spring.advance(300.0, time + 10.0));
        assert_eq!(spring.value, 300.0);
    }

    #[test]
    fn a_layer_moves_scales_and_fades_what_is_drawn_and_layers_nest() {
        let mut recorder = Recorder::default();
        let mut layers = Layers::new(&mut recorder);
        let rect = Rect::new(10.0, 10.0, 20.0, 20.0);
        layers.push(Layer::NONE.shifted(5.0, 0.0));
        layers.push(Layer::NONE.scaled(Point::new(20.0, 20.0), 0.5));
        // Halved about its own center, then moved right.
        assert_eq!(layers.to_screen(rect), Rect::new(20.0, 15.0, 10.0, 10.0));
        layers.fill_rect(rect, 4.0, Color::WHITE);
        layers.pop();
        layers.pop();
        assert_eq!(layers.to_screen(rect), rect);
        layers.push(Layer::fade(0.0));
        layers.fill_rect(rect, 4.0, Color::WHITE);
        layers.pop();
        assert_eq!(
            recorder.fills,
            [Rect::new(20.0, 15.0, 10.0, 10.0)],
            "a fully faded layer draws nothing"
        );
    }

    #[test]
    fn appearing_ends_as_no_change() {
        let done = Layer::appear(1.0, Point::new(50.0, 50.0), 0.9, 8.0);
        let t = Transform::IDENTITY.then(done);
        assert!(t.identity(), "{t:?}");
    }
}
