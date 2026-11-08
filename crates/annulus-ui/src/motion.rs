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
    /// Quick to start, then a long soft landing: surfaces and pages arriving.
    Emphasized,
    /// Slow to start, then gone: things leaving, which should never hold up what comes next.
    Exit,
}

/// The y of a cubic Bezier easing from (0, 0) to (1, 1) through (`x1`, `y1`) and (`x2`, `y2`)
/// at x = `t`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let along = |a: f32, b: f32, s: f32| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    // x grows with s here, so halving finds it to well under a pixel of any move on screen.
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..14 {
        let middle = f32::midpoint(low, high);
        if along(x1, x2, middle) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    along(y1, y2, f32::midpoint(low, high))
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
            Curve::Emphasized | Curve::Exit if t <= 0.0 || t >= 1.0 => t,
            Curve::Emphasized => bezier(0.2, 0.0, 0.0, 1.0, t),
            Curve::Exit => bezier(0.4, 0.0, 1.0, 1.0, t),
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
    /// Small fades and focus feedback.
    pub const FAST: Motion = Motion::new(motion::FAST, Curve::Out);
    /// Something answering the pointer: the moment it arrives.
    pub const ANSWER: Motion = Motion::new(motion::ANSWER, Curve::Out);
    /// Something letting the pointer go: slower than it answered.
    pub const LET_GO: Motion = Motion::new(motion::LET_GO, Curve::Out);
    /// Panels, menus, page and tab changes.
    pub const BASE: Motion = Motion::new(motion::BASE, Curve::Emphasized);
    /// Large surfaces.
    pub const SLOW: Motion = Motion::new(motion::SLOW, Curve::Emphasized);
    /// Menus, cards and dialogs growing into place.
    pub const POP: Motion = Motion::new(motion::SLOW, Curve::Overshoot);
    /// Whatever leaves: gone quickly so what comes next is not held up.
    pub const EXIT: Motion = Motion::new(motion::EXIT, Curve::Exit);
    /// A side panel or the like leaving: it has further to go.
    pub const EXIT_SLOW: Motion = Motion::new(motion::EXIT_SLOW, Curve::Exit);

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
        self.delayed(self.delay + index * motion::STAGGER)
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
/// A soft spring's natural frequency, per second: settles in about 0.4 s.
const SOFT_FREQUENCY: f32 = 19.0;
/// A soft spring's damping ratio: past its target by about 1 %, then back.
const SOFT_DAMPING: f32 = 0.82;

/// How a position follows its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feel {
    /// Critically damped: quick and exact, never passing the target (tabs, a list's offset).
    Firm,
    /// Under-damped: a little slower, landing with a touch of life (a selection, cards
    /// moving to new places).
    Soft,
}

/// A spring: positions that follow their target and keep their speed when the target moves
/// again mid-way.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spring {
    pub(crate) value: f32,
    velocity: f32,
    pub(crate) target: f32,
    pub(crate) seen: f64,
    feel: Feel,
}

impl Spring {
    /// Resting at `value`.
    pub(crate) fn at(value: f32, now: f64) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            seen: now,
            feel: Feel::Firm,
        }
    }

    /// The same, following with `feel`.
    pub(crate) fn feeling(self, feel: Feel) -> Self {
        Self { feel, ..self }
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
        let gap = match self.feel {
            Feel::Firm => {
                let decay = (-STIFFNESS * dt).exp();
                let lead = self.velocity + STIFFNESS * gap;
                self.velocity = (self.velocity - STIFFNESS * lead * dt) * decay;
                (gap + lead * dt) * decay
            }
            Feel::Soft => {
                // The closed form, so the path does not depend on the frame rate.
                let rate = SOFT_DAMPING * SOFT_FREQUENCY;
                let ring = SOFT_FREQUENCY * (1.0 - SOFT_DAMPING * SOFT_DAMPING).sqrt();
                let (sin, cos) = (ring * dt).sin_cos();
                let decay = (-rate * dt).exp();
                let speed = self.velocity;
                self.velocity = decay
                    * (speed * cos
                        - (speed * rate + SOFT_FREQUENCY * SOFT_FREQUENCY * gap) / ring * sin);
                decay * (gap * cos + (speed + rate * gap) / ring * sin)
            }
        };
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
    /// Fade everything in here as one, inside this rectangle (in the layer's own units),
    /// instead of shape by shape. For surfaces whose shapes overlap: a card's fill would show
    /// its shadow through it. Costs an offscreen pass while it fades.
    pub group: Option<Rect>,
}

impl Layer {
    /// Changes nothing.
    pub const NONE: Layer = Layer {
        shift: Point::new(0.0, 0.0),
        scale: 1.0,
        origin: Point::new(0.0, 0.0),
        alpha: 1.0,
        group: None,
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

    /// This layer, faded as one inside `bounds`: see [`Layer::group`].
    #[must_use]
    pub fn grouped(self, bounds: Rect) -> Self {
        Self {
            group: Some(bounds),
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
    /// What each color is multiplied by: the layers faded shape by shape.
    alpha: f32,
    /// How much of what is drawn shows in the end, groups included.
    shown: f32,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        scale: 1.0,
        shift: Point::new(0.0, 0.0),
        alpha: 1.0,
        shown: 1.0,
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
            shown: self.shown * layer.alpha,
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
    /// The transform around each layer, and whether the layer opened a group.
    stack: Vec<(Transform, bool)>,
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
        let before = self.now;
        self.now = before.then(layer);
        // Nothing to fade at full opacity, and nothing to see at none.
        let group = layer
            .group
            .filter(|_| layer.alpha < 0.999 && layer.alpha > 0.002 && self.inner.has_groups());
        if let Some(bounds) = group {
            // The group lands faded, so its shapes are not faded again one by one.
            self.now.alpha = before.alpha;
            self.inner.push_group(self.now.rect(bounds), layer.alpha);
        }
        self.stack.push((before, group.is_some()));
    }

    pub(crate) fn pop(&mut self) {
        if let Some((before, grouped)) = self.stack.pop() {
            self.now = before;
            if grouped {
                self.inner.pop_group();
            }
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
        self.now.shown
    }

    /// Clips to `rect`, which is already in screen coordinates.
    pub(crate) fn push_screen_clip(&mut self, rect: Rect) {
        self.inner.push_clip(rect);
    }

    /// Nothing drawn now would show.
    fn invisible(&self) -> bool {
        self.now.shown <= 0.002
    }
}

impl Canvas for Layers<'_> {
    fn set_font_families(&mut self, ui: Option<&str>, mono: Option<&str>) {
        self.inner.set_font_families(ui, mono);
    }

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

    fn fill_polygon(&mut self, points: &[Point], color: Color) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        let mapped: Vec<Point> = points.iter().map(|p| t.point(*p)).collect();
        self.inner.fill_polygon(&mapped, t.color(color));
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
        self.inner
            .draw_text_zoomed(t.point(origin), text, font, t.scale, t.color(color));
    }

    fn push_clip(&mut self, rect: Rect) {
        let rect = self.now.rect(rect);
        self.inner.push_clip(rect);
    }

    fn pop_clip(&mut self) {
        self.inner.pop_clip();
    }

    fn fill_gradient(&mut self, rect: Rect, radius: f32, from: Color, to: Color, across: bool) {
        if self.invisible() {
            return;
        }
        let t = self.now;
        self.inner.fill_gradient(
            t.rect(rect),
            radius * t.scale,
            t.color(from),
            t.color(to),
            across,
        );
    }

    fn has_groups(&self) -> bool {
        self.inner.has_groups()
    }

    fn push_group(&mut self, bounds: Rect, alpha: f32) {
        let bounds = self.now.rect(bounds);
        self.inner.push_group(bounds, alpha);
    }

    fn pop_group(&mut self) {
        self.inner.pop_group();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::Recorder;

    #[test]
    fn curves_start_at_zero_and_end_at_one() {
        for curve in [
            Curve::Linear,
            Curve::Out,
            Curve::InOut,
            Curve::Overshoot,
            Curve::Emphasized,
            Curve::Exit,
        ] {
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
    fn arrivals_land_softly_and_exits_leave_late() {
        // A page arriving has done most of its move early, and creeps in at the end.
        let early = Curve::Emphasized.at(0.3);
        assert!(early > 0.6, "{early}");
        assert!(Curve::Emphasized.at(0.9) > 0.98);
        // Something leaving stays near where it was, then goes.
        let late = Curve::Exit.at(0.3);
        assert!(late < 0.15, "{late}");
        // Neither ever runs backwards.
        for curve in [Curve::Emphasized, Curve::Exit] {
            let mut last = 0.0_f32;
            for step in 0_u8..=100 {
                let now = curve.at(f32::from(step) / 100.0);
                assert!(now >= last - 1e-3, "{curve:?} at {step}");
                last = now;
            }
        }
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
        assert_eq!(tween.sample(1.33), (10.0, false));
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
    fn a_soft_spring_passes_its_target_a_little_settles_and_does_not_mind_the_frame_rate() {
        let run = |step: f64| {
            let mut spring = Spring::at(0.0, 0.0).feeling(Feel::Soft);
            let (mut time, mut peak, mut moving) = (0.0, 0.0_f32, true);
            let mut settled_at = 0.0;
            while moving {
                time += step;
                moving = spring.advance(100.0, time);
                peak = peak.max(spring.value);
                settled_at = time;
                assert!(time < 2.0, "it settles");
            }
            assert_eq!(spring.value, 100.0);
            (peak, settled_at)
        };
        let (peak, settled_at) = run(FRAME);
        assert!(peak > 100.0 && peak < 103.0, "{peak}");
        assert!(settled_at < 0.7, "{settled_at}");
        // 144 Hz takes the same path as 60 Hz, near enough to see no difference.
        let (fine_peak, _) = run(1.0 / 144.0);
        assert!((peak - fine_peak).abs() < 0.5, "{peak} vs {fine_peak}");
    }

    /// A canvas with groups: what it was asked to do, in order.
    #[derive(Default)]
    struct Grouping {
        inner: Recorder,
        calls: Vec<String>,
    }

    impl Canvas for Grouping {
        fn scale(&self) -> f32 {
            1.0
        }
        fn fill_rect(&mut self, rect: Rect, radius: f32, color: Color) {
            self.calls.push(format!("fill {}", color.a));
            self.inner.fill_rect(rect, radius, color);
        }
        fn stroke_rect(&mut self, _: Rect, _: f32, _: f32, _: Color) {}
        fn shadow(&mut self, _: Rect, _: f32, _: f32, _: f32, _: Color) {}
        fn fill_circle(&mut self, _: Point, _: f32, _: Color) {}
        fn polyline(&mut self, _: &[Point], _: bool, _: f32, _: Color) {}
        fn stroke_circle(&mut self, _: Point, _: f32, _: f32, _: Color) {}
        fn fill_polygon(&mut self, _: &[Point], _: Color) {}
        fn measure(&mut self, text: &str, font: Font) -> TextSize {
            self.inner.measure(text, font)
        }
        fn draw_text(&mut self, _: Point, _: &str, _: Font, _: Color) {}
        fn push_clip(&mut self, _: Rect) {}
        fn pop_clip(&mut self) {}
        fn has_groups(&self) -> bool {
            true
        }
        fn push_group(&mut self, bounds: Rect, alpha: f32) {
            self.calls.push(format!("group {} {alpha}", bounds.x));
        }
        fn pop_group(&mut self) {
            self.calls.push("end".to_owned());
        }
        fn draw_text_zoomed(&mut self, _: Point, _: &str, font: Font, zoom: f32, _: Color) {
            self.calls.push(format!("text {} {zoom}", font.size));
        }
    }

    #[test]
    fn a_grouped_layer_fades_as_one_and_only_while_it_is_fading() {
        let mut canvas = Grouping::default();
        let rect = Rect::new(10.0, 10.0, 20.0, 20.0);
        {
            let mut layers = Layers::new(&mut canvas);
            layers.push(Layer::fade(0.5).shifted(5.0, 0.0).grouped(rect));
            layers.fill_rect(rect, 0.0, Color::WHITE);
            let shows = layers.opacity();
            assert!((shows - 0.5).abs() < 1e-6, "it still says how much shows");
            layers.pop();
            // At full opacity it is just a layer: no offscreen pass at rest.
            layers.push(Layer::fade(1.0).grouped(rect));
            layers.fill_rect(rect, 0.0, Color::WHITE);
            layers.pop();
        }
        assert_eq!(
            canvas.calls,
            ["group 15 0.5", "fill 255", "end", "fill 255"],
            "the shape keeps its own color; the group lands faded"
        );
    }

    #[test]
    fn without_groups_a_grouped_layer_fades_shape_by_shape() {
        let mut recorder = Recorder::default();
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut layers = Layers::new(&mut recorder);
        layers.push(Layer::fade(0.0).grouped(rect));
        layers.fill_rect(rect, 0.0, Color::WHITE);
        layers.pop();
        layers.push(Layer::fade(0.5).grouped(rect));
        layers.fill_rect(rect, 0.0, Color::WHITE);
        layers.pop();
        assert_eq!(
            recorder.fills,
            [rect],
            "invisible draws nothing; faded still draws"
        );
    }

    #[test]
    fn a_scaled_layer_zooms_its_text_instead_of_changing_the_font() {
        let mut canvas = Grouping::default();
        {
            let mut layers = Layers::new(&mut canvas);
            layers.push(Layer::NONE.scaled(Point::new(0.0, 0.0), 0.9));
            layers.draw_text(
                Point::new(10.0, 10.0),
                "web-1",
                Font::ui(13.0),
                Color::WHITE,
            );
            layers.pop();
        }
        assert_eq!(canvas.calls, ["text 13 0.9"]);
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
