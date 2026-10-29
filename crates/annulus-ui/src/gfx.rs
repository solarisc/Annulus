//! Drawing primitives shared by every view and both renderers ([ADR-0017]): geometry in
//! DIPs, colors, fonts, and the [`Canvas`] a renderer implements.
//!
//! [ADR-0017]: ../../docs/adr/0017-native-ui.md

use annulus_config::tokens::Rgb;

use crate::icons::{self, IconId, Segment};

/// A point in DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal, growing right.
    pub x: f32,
    /// Vertical, growing down.
    pub y: f32,
}

impl Point {
    /// The point at `x`, `y`.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle in DIPs. Width and height are never negative.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// The empty rectangle at the origin.
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    /// A rectangle; a negative width or height becomes zero.
    #[must_use]
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }

    /// Right edge.
    #[must_use]
    pub fn r(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub fn b(&self) -> f32 {
        self.y + self.h
    }

    /// The center point.
    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `point` lies inside; the right and bottom edges are outside.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.r() && point.y < self.b()
    }

    /// Whether the two rectangles share any area.
    #[must_use]
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.r() && other.x < self.r() && self.y < other.b() && other.y < self.b()
    }

    /// The shared area, or an empty rectangle at this one's origin.
    #[must_use]
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect::new(
            x,
            y,
            self.r().min(other.r()) - x,
            self.b().min(other.b()) - y,
        )
    }

    /// Shrunk by `dx` on the left and right and `dy` on the top and bottom.
    #[must_use]
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.w - 2.0 * dx,
            self.h - 2.0 * dy,
        )
    }

    /// Grown by `d` on every side.
    #[must_use]
    pub fn outset(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    /// Moved by `dx`, `dy`.
    #[must_use]
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// A `w` x `h` rectangle centered in this one.
    #[must_use]
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect::new(
            self.x + (self.w - w) / 2.0,
            self.y + (self.h - h) / 2.0,
            w,
            h,
        )
    }

    /// Cuts `h` off the top and returns it.
    #[allow(clippy::return_self_not_must_use)] // Often called just to skip space.
    pub fn take_top(&mut self, h: f32) -> Rect {
        let h = h.min(self.h);
        let top = Rect::new(self.x, self.y, self.w, h);
        self.y += h;
        self.h -= h;
        top
    }

    /// Cuts `h` off the bottom and returns it.
    #[allow(clippy::return_self_not_must_use)] // Often called just to skip space.
    pub fn take_bottom(&mut self, h: f32) -> Rect {
        let h = h.min(self.h);
        self.h -= h;
        Rect::new(self.x, self.y + self.h, self.w, h)
    }

    /// Cuts `w` off the left and returns it.
    #[allow(clippy::return_self_not_must_use)] // Often called just to skip space.
    pub fn take_left(&mut self, w: f32) -> Rect {
        let w = w.min(self.w);
        let left = Rect::new(self.x, self.y, w, self.h);
        self.x += w;
        self.w -= w;
        left
    }

    /// Cuts `w` off the right and returns it.
    #[allow(clippy::return_self_not_must_use)] // Often called just to skip space.
    pub fn take_right(&mut self, w: f32) -> Rect {
        let w = w.min(self.w);
        self.w -= w;
        Rect::new(self.x + self.w, self.y, w, self.h)
    }
}

/// An sRGB color with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Opacity, 255 is opaque.
    pub a: u8,
}

impl Color {
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Color = Color::rgba(0, 0, 0, 255);
    /// Opaque white.
    pub const WHITE: Color = Color::rgba(255, 255, 255, 255);

    /// A color from its channels.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// This color with opacity `alpha` (0.0 to 1.0) times its own.
    #[must_use]
    pub fn alpha(self, alpha: f32) -> Self {
        Self {
            a: unit_to_byte(f32::from(self.a) / 255.0 * alpha),
            ..self
        }
    }

    /// The color `t` of the way from this one to `other` (0.0 is this, 1.0 is `other`).
    #[must_use]
    pub fn mix(self, other: Color, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let channel =
            |a: u8, b: u8| unit_to_byte((f32::from(a) + (f32::from(b) - f32::from(a)) * t) / 255.0);
        Self {
            r: channel(self.r, other.r),
            g: channel(self.g, other.g),
            b: channel(self.b, other.b),
            a: channel(self.a, other.a),
        }
    }
}

impl From<Rgb> for Color {
    fn from(rgb: Rgb) -> Self {
        Color::rgba(rgb.r, rgb.g, rgb.b, 255)
    }
}

/// 0.0..=1.0 to 0..=255, rounded.
fn unit_to_byte(unit: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Clamped to 0..=255 first.
    let byte = (unit.clamp(0.0, 1.0) * 255.0).round() as u8;
    byte
}

/// How heavy a font's strokes are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Weight {
    /// 400.
    #[default]
    Regular,
    /// 500.
    Medium,
    /// 600.
    Semibold,
    /// 700.
    Bold,
}

impl Weight {
    /// The CSS / OpenType weight number.
    #[must_use]
    pub fn value(self) -> f32 {
        match self {
            Weight::Regular => 400.0,
            Weight::Medium => 500.0,
            Weight::Semibold => 600.0,
            Weight::Bold => 700.0,
        }
    }
}

/// Which font family a run uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Family {
    /// The UI font: the system's on Windows, the bundled one elsewhere.
    #[default]
    Ui,
    /// The monospace font: terminals, fingerprints, scripts.
    Mono,
}

/// A font: family, size in DIPs and weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    /// The family.
    pub family: Family,
    /// Size in DIPs.
    pub size: f32,
    /// Stroke weight.
    pub weight: Weight,
}

impl Font {
    /// The UI font at `size`, regular weight.
    #[must_use]
    pub const fn ui(size: f32) -> Self {
        Self {
            family: Family::Ui,
            size,
            weight: Weight::Regular,
        }
    }

    /// The monospace font at `size`, regular weight.
    #[must_use]
    pub const fn mono(size: f32) -> Self {
        Self {
            family: Family::Mono,
            size,
            weight: Weight::Regular,
        }
    }

    /// This font at `weight`.
    #[must_use]
    pub const fn weight(self, weight: Weight) -> Self {
        Self { weight, ..self }
    }
}

/// Where text sits inside its rectangle, horizontally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Left.
    #[default]
    Start,
    /// Centered.
    Center,
    /// Right.
    End,
}

/// The size of one line of text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextSize {
    /// Advance width of the run.
    pub width: f32,
    /// Line height: ascent plus descent plus line gap.
    pub height: f32,
}

/// What a renderer draws with. Coordinates are DIPs; the renderer scales them to device
/// pixels. Every call is clipped to the innermost clip pushed.
pub trait Canvas {
    /// Device pixels per DIP.
    fn scale(&self) -> f32;

    /// Fills `rect` with `color`, its corners rounded by `radius`.
    fn fill_rect(&mut self, rect: Rect, radius: f32, color: Color);

    /// Strokes the outline of `rect`, centered on its edge, `width` wide.
    fn stroke_rect(&mut self, rect: Rect, radius: f32, width: f32, color: Color);

    /// A soft shadow under `rect`: `color` fading out over `blur` DIPs, moved down by
    /// `offset_y`.
    fn shadow(&mut self, rect: Rect, radius: f32, blur: f32, offset_y: f32, color: Color);

    /// Fills a circle.
    fn fill_circle(&mut self, center: Point, radius: f32, color: Color);

    /// Strokes a polyline through `points` with round caps and joins, closed back to the first
    /// point when `closed`.
    fn polyline(&mut self, points: &[Point], closed: bool, width: f32, color: Color);

    /// Strokes a circle.
    fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, color: Color);

    /// The size of `text` on one line in `font`.
    fn measure(&mut self, text: &str, font: Font) -> TextSize;

    /// Draws one line of `text` with its line box's top-left corner at `origin`.
    fn draw_text(&mut self, origin: Point, text: &str, font: Font, color: Color);

    /// Clips everything drawn until the matching [`Canvas::pop_clip`] to `rect`, intersected
    /// with the clip already active.
    fn push_clip(&mut self, rect: Rect);

    /// Ends the innermost clip.
    fn pop_clip(&mut self);

    /// Draws icon `id` scaled into `rect` in `color`, with the icon set's stroke width.
    fn icon(&mut self, id: IconId, rect: Rect, color: Color) {
        let Some(icon) = icons::icon(id) else { return };
        let unit = rect.w.min(rect.h) / icons::GRID;
        let width = icons::STROKE * unit;
        let at = |x: f32, y: f32| Point::new(rect.x + x * unit, rect.y + y * unit);
        let mut line: Vec<Point> = Vec::new();
        let mut closed = false;
        for segment in icon.segments {
            match *segment {
                Segment::Move(x, y) => {
                    if line.len() > 1 {
                        self.polyline(&line, closed, width, color);
                    }
                    line.clear();
                    closed = false;
                    line.push(at(x, y));
                }
                Segment::Line(x, y) => line.push(at(x, y)),
                Segment::Close => closed = true,
                Segment::Circle(x, y, r) => {
                    if line.len() > 1 {
                        self.polyline(&line, closed, width, color);
                    }
                    line.clear();
                    closed = false;
                    self.stroke_circle(at(x, y), r * unit, width, color);
                }
                Segment::Dot(x, y, r) => self.fill_circle(at(x, y), r * unit, color),
                Segment::Arc(x, y, r, from, to) => {
                    if line.len() > 1 {
                        self.polyline(&line, closed, width, color);
                    }
                    line.clear();
                    closed = false;
                    let arc: Vec<Point> = Segment::arc_points(x, y, r, from, to)
                        .into_iter()
                        .map(|(px, py)| at(px, py))
                        .collect();
                    self.polyline(&arc, false, width, color);
                }
            }
        }
        if line.len() > 1 {
            self.polyline(&line, closed, width, color);
        }
    }

    /// Draws `text` on one line inside `rect`, vertically centered and aligned by `align`.
    /// Text wider than `rect` is cut at a character boundary and ends in an ellipsis.
    /// Returns whether it had to be cut.
    fn text(&mut self, rect: Rect, text: &str, font: Font, color: Color, align: Align) -> bool {
        let fitted = fit(self, text, font, rect.w);
        let size = self.measure(&fitted, font);
        let x = match align {
            Align::Start => rect.x,
            Align::Center => rect.x + (rect.w - size.width) / 2.0,
            Align::End => rect.r() - size.width,
        };
        let y = rect.y + (rect.h - size.height) / 2.0;
        self.draw_text(Point::new(x, y), &fitted, font, color);
        fitted.len() != text.len()
    }
}

/// `text`, or as much of it as fits in `width` followed by an ellipsis.
pub fn fit<C: Canvas + ?Sized>(canvas: &mut C, text: &str, font: Font, width: f32) -> String {
    const ELLIPSIS: &str = "\u{2026}";
    if canvas.measure(text, font).width <= width + 0.5 {
        return text.to_owned();
    }
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(text.len()))
        .collect();
    // The longest prefix that fits with the ellipsis: binary search over character ends.
    let (mut low, mut high) = (0usize, ends.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}{ELLIPSIS}", text[..ends[mid - 1]].trim_end());
        if canvas.measure(&candidate, font).width <= width + 0.5 {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low == 0 {
        return if canvas.measure(ELLIPSIS, font).width <= width + 0.5 {
            ELLIPSIS.to_owned()
        } else {
            String::new()
        };
    }
    format!("{}{ELLIPSIS}", text[..ends[low - 1]].trim_end())
}

/// `text`, or its start and its end around an ellipsis, as much of both as fits in `width`
/// (a long file name keeps its extension: `a_5ffc28…62fbd2de.webm`).
pub fn fit_middle<C: Canvas + ?Sized>(
    canvas: &mut C,
    text: &str,
    font: Font,
    width: f32,
) -> String {
    const ELLIPSIS: &str = "\u{2026}";
    if canvas.measure(text, font).width <= width + 0.5 {
        return text.to_owned();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // The most characters kept, split about evenly with a little more at the start.
    let (mut low, mut high) = (0usize, chars.len());
    let candidate = |keep: usize| {
        let head = keep.div_ceil(2);
        let tail = keep - head;
        let head_end = chars.get(head).map_or(text.len(), |(at, _)| *at);
        let tail_start = chars
            .get(chars.len() - tail)
            .map_or(text.len(), |(at, _)| *at);
        format!("{}{ELLIPSIS}{}", &text[..head_end], &text[tail_start..])
    };
    while low < high {
        let mid = (low + high).div_ceil(2);
        if canvas.measure(&candidate(mid), font).width <= width + 0.5 {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low == 0 {
        return fit(canvas, text, font, width);
    }
    candidate(low)
}

/// A path shortened to fit in `width`: its first part (the drive, `~` or the root), an
/// ellipsis, then as many of its last parts as fit; the file name is never dropped, and is
/// shortened in its middle only when nothing else is left to take out.
/// `C:\Users\alice\Downloads\build.log` becomes `C:\…\Downloads\build.log`.
pub fn fit_path<C: Canvas + ?Sized>(canvas: &mut C, path: &str, font: Font, width: f32) -> String {
    const ELLIPSIS: &str = "\u{2026}";
    if canvas.measure(path, font).width <= width + 0.5 {
        return path.to_owned();
    }
    let sep = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let parts: Vec<&str> = path.split(sep).collect();
    if parts.len() > 2 {
        let first = parts[0];
        // The most last parts that fit after "first/…/".
        for keep in (1..parts.len() - 1).rev() {
            let tail = parts[parts.len() - keep..].join(&sep.to_string());
            let candidate = format!("{first}{sep}{ELLIPSIS}{sep}{tail}");
            if canvas.measure(&candidate, font).width <= width + 0.5 {
                return candidate;
            }
        }
        // Only the file name after the ellipsis.
        let name = parts[parts.len() - 1];
        let lead = format!("{ELLIPSIS}{sep}");
        let room = width - canvas.measure(&lead, font).width;
        if canvas.measure(name, font).width <= room + 0.5 {
            return format!("{lead}{name}");
        }
        return fit_middle(canvas, name, font, width);
    }
    fit_middle(canvas, path, font, width)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A canvas that draws nothing and measures every character as 7 DIPs wide (the
    /// ellipsis as 5), with a 16 DIP line height; it records what was drawn.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub texts: Vec<(Point, String)>,
        pub fills: Vec<Rect>,
        pub clips: Vec<Rect>,
    }

    impl Canvas for Recorder {
        fn scale(&self) -> f32 {
            1.0
        }
        fn fill_rect(&mut self, rect: Rect, _: f32, _: Color) {
            self.fills.push(rect);
        }
        fn stroke_rect(&mut self, _: Rect, _: f32, _: f32, _: Color) {}
        fn shadow(&mut self, _: Rect, _: f32, _: f32, _: f32, _: Color) {}
        fn fill_circle(&mut self, _: Point, _: f32, _: Color) {}
        fn polyline(&mut self, _: &[Point], _: bool, _: f32, _: Color) {}
        fn stroke_circle(&mut self, _: Point, _: f32, _: f32, _: Color) {}
        #[allow(clippy::cast_precision_loss)] // Test strings are short.
        fn measure(&mut self, text: &str, _: Font) -> TextSize {
            let width = text
                .chars()
                .map(|c| if c == '\u{2026}' { 5.0 } else { 7.0 })
                .sum();
            TextSize {
                width,
                height: 16.0,
            }
        }
        fn draw_text(&mut self, origin: Point, text: &str, _: Font, _: Color) {
            self.texts.push((origin, text.to_owned()));
        }
        fn push_clip(&mut self, rect: Rect) {
            self.clips.push(rect);
        }
        fn pop_clip(&mut self) {
            self.clips.pop();
        }
    }

    #[test]
    fn rect_cuts_hand_out_strips_and_keep_the_rest() {
        let mut rect = Rect::new(0.0, 0.0, 100.0, 50.0);
        let top = rect.take_top(10.0);
        let right = rect.take_right(30.0);
        assert_eq!(top, Rect::new(0.0, 0.0, 100.0, 10.0));
        assert_eq!(right, Rect::new(70.0, 10.0, 30.0, 40.0));
        assert_eq!(rect, Rect::new(0.0, 10.0, 70.0, 40.0));
        // Asking for more than is left takes what is left.
        assert_eq!(rect.take_left(500.0).w, 70.0);
        assert_eq!(rect.w, 0.0);
    }

    #[test]
    fn overlap_needs_shared_area_not_a_shared_edge() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(a.overlaps(&Rect::new(9.0, 9.0, 5.0, 5.0)));
        assert!(!a.overlaps(&Rect::new(10.0, 0.0, 5.0, 5.0)));
        assert_eq!(
            a.intersect(&Rect::new(5.0, 5.0, 10.0, 10.0)),
            Rect::new(5.0, 5.0, 5.0, 5.0)
        );
    }

    #[test]
    fn long_text_ends_in_an_ellipsis_that_fits() {
        let mut canvas = Recorder::default();
        let font = Font::ui(13.0);
        assert_eq!(fit(&mut canvas, "prod-db", font, 49.0), "prod-db");
        // 4 characters (28) plus the ellipsis (5) fit in 35; a 5th would not.
        assert_eq!(fit(&mut canvas, "prod-db", font, 35.0), "prod\u{2026}");
        // A trailing space before the ellipsis is dropped.
        assert_eq!(fit(&mut canvas, "ab cd", font, 19.0), "ab\u{2026}");
        assert_eq!(fit(&mut canvas, "prod-db", font, 5.0), "\u{2026}");
        assert_eq!(fit(&mut canvas, "prod-db", font, 2.0), "");
        // Multi-byte characters are never split.
        assert_eq!(
            fit(&mut canvas, "\u{e9}\u{e9}\u{e9}\u{e9}", font, 20.0),
            "\u{e9}\u{e9}\u{2026}"
        );
    }

    #[test]
    fn a_long_path_keeps_its_start_and_its_file_name() {
        let mut canvas = Recorder::default();
        let font = Font::ui(13.0);
        let windows = r"C:\Users\alice\Downloads\build.log";
        // 7 per character: 34 characters fit in 238.
        assert_eq!(fit_path(&mut canvas, windows, font, 238.0), windows);
        assert_eq!(
            fit_path(&mut canvas, windows, font, 170.0),
            "C:\\\u{2026}\\Downloads\\build.log"
        );
        assert_eq!(
            fit_path(&mut canvas, windows, font, 110.0),
            "C:\\\u{2026}\\build.log"
        );
        assert_eq!(
            fit_path(
                &mut canvas,
                "/home/alex/projects/annulus/target/release",
                font,
                150.0
            ),
            "/\u{2026}/target/release"
        );
        // Nothing left to take out: the name itself is shortened in its middle.
        let tight = fit_path(&mut canvas, windows, font, 60.0);
        assert!(
            tight.contains('\u{2026}') && tight.ends_with("og"),
            "{tight}"
        );
        assert!(canvas.measure(&tight, font).width <= 60.5);
    }

    #[test]
    fn a_long_name_keeps_both_ends() {
        let mut canvas = Recorder::default();
        let font = Font::ui(13.0);
        let name = "a_5ffc28ba1084d56be94b8ad62fbd2de.webm";
        let fitted = fit_middle(&mut canvas, name, font, 110.0);
        assert!(
            fitted.starts_with("a_5ffc") && fitted.ends_with("de.webm"),
            "{fitted}"
        );
        assert!(canvas.measure(&fitted, font).width <= 110.5);
        assert_eq!(fit_middle(&mut canvas, "short", font, 100.0), "short");
    }

    #[test]
    fn text_reports_when_it_was_cut_and_aligns_what_fits() {
        let mut canvas = Recorder::default();
        let font = Font::ui(13.0);
        let rect = Rect::new(10.0, 0.0, 70.0, 30.0);
        assert!(!canvas.text(rect, "abc", font, Color::WHITE, Align::End));
        assert_eq!(canvas.texts[0].0, Point::new(80.0 - 21.0, 7.0));
        assert!(canvas.text(rect, "a very long label", font, Color::WHITE, Align::Start));
    }

    #[test]
    fn colors_mix_and_fade() {
        let black = Color::BLACK;
        assert_eq!(
            black.mix(Color::WHITE, 0.5),
            Color::rgba(128, 128, 128, 255)
        );
        assert_eq!(Color::WHITE.alpha(0.5).a, 128);
        assert_eq!(black.mix(Color::WHITE, 2.0), Color::WHITE);
    }
}
