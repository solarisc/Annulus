//! The CPU renderer ([ADR-0017] decision 3): `tiny-skia` for shapes, our text engine for
//! glyphs, into an RGBA buffer. It ships on Linux and renders every golden image on every
//! OS, so its output must be deterministic.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![forbid(unsafe_code)]

mod text;

pub use text::TextEngine;

use annulus_ui::gfx::{Canvas, Color, Font, Point, Rect, TextSize};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path,
    PathBuilder, Pattern, Pixmap, SpreadMode, Stroke, Transform,
};

/// Why a frame could not be made or saved.
#[derive(Debug, thiserror::Error)]
pub enum CpuError {
    /// The pixel size is zero or too large.
    #[error("frame size {width}x{height} is not usable")]
    BadSize {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// PNG encoding failed.
    #[error("could not encode PNG: {0}")]
    Png(String),
}

/// A frame in memory that implements [`Canvas`].
pub struct CpuCanvas {
    frame: Pixmap,
    scale: f32,
    /// Clip rectangles in device pixels with their masks; the last is active.
    clips: Vec<(Rect, Mask)>,
    text: TextEngine,
    /// The groups open, innermost last: the frame under each, and where and how it lands.
    groups: Vec<Group>,
}

/// A group being drawn: `frame` holds it meanwhile, and this holds what is under it.
struct Group {
    under: Pixmap,
    /// Where it lands, in device pixels.
    bounds: Rect,
    alpha: f32,
}

impl CpuCanvas {
    /// A `width` x `height` pixel frame at `scale` device pixels per DIP, cleared to
    /// transparent.
    ///
    /// # Errors
    /// [`CpuError::BadSize`] when either side is zero or too large.
    pub fn new(width: u32, height: u32, scale: f32, text: TextEngine) -> Result<Self, CpuError> {
        let frame = Pixmap::new(width, height).ok_or(CpuError::BadSize { width, height })?;
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Ok(Self {
            frame,
            scale,
            clips: Vec::new(),
            text,
            groups: Vec::new(),
        })
    }

    /// Fills the whole frame with `color`, ignoring clips.
    pub fn clear(&mut self, color: Color) {
        self.frame.fill(tiny_skia::Color::from_rgba8(
            color.r, color.g, color.b, color.a,
        ));
    }

    /// The frame's premultiplied RGBA pixels, row by row.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        self.frame.data()
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.frame.width()
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.frame.height()
    }

    /// The frame as a PNG file.
    ///
    /// # Errors
    /// [`CpuError::Png`] when encoding fails.
    pub fn encode_png(&self) -> Result<Vec<u8>, CpuError> {
        self.frame
            .encode_png()
            .map_err(|error| CpuError::Png(error.to_string()))
    }

    /// Gives back the text engine, keeping its glyph cache for the next frame.
    #[must_use]
    pub fn into_text_engine(self) -> TextEngine {
        self.text
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    fn fill_path(&mut self, path: &Path, color: Color) {
        let paint = paint(color);
        let transform = self.transform();
        let mask = self.clips.last().map(|(_, mask)| mask);
        self.frame
            .fill_path(path, &paint, FillRule::Winding, transform, mask);
    }

    fn stroke_path(&mut self, path: &Path, width: f32, color: Color) {
        if !(width.is_finite() && width > 0.0) {
            return;
        }
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let paint = paint(color);
        let transform = self.transform();
        let mask = self.clips.last().map(|(_, mask)| mask);
        self.frame
            .stroke_path(path, &paint, &stroke, transform, mask);
    }

    /// The active clip in device pixels.
    fn clip_rect(&self) -> Rect {
        self.clips.last().map_or_else(
            || {
                #[allow(clippy::cast_precision_loss)] // Frame sizes are far below 2^24.
                Rect::new(
                    0.0,
                    0.0,
                    self.frame.width() as f32,
                    self.frame.height() as f32,
                )
            },
            |(rect, _)| *rect,
        )
    }
}

impl Canvas for CpuCanvas {
    fn scale(&self) -> f32 {
        self.scale
    }

    fn fill_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if let Some(path) = rounded_rect(rect, radius) {
            self.fill_path(&path, color);
        }
    }

    fn stroke_rect(&mut self, rect: Rect, radius: f32, width: f32, color: Color) {
        // Inset by half the width so the stroke stays inside the rectangle.
        let half = width / 2.0;
        if let Some(path) = rounded_rect(rect.inset(half, half), (radius - half).max(0.0)) {
            self.stroke_path(&path, width, color);
        }
    }

    fn shadow(&mut self, rect: Rect, radius: f32, blur: f32, offset_y: f32, color: Color) {
        // Stacked rings, each a little larger and fainter: deterministic and cheap.
        const RINGS: u8 = 10;
        let blur = if blur.is_finite() { blur.max(0.0) } else { 0.0 };
        let base = rect.offset(0.0, offset_y);
        for ring in (1..=RINGS).rev() {
            let spread = blur * f32::from(ring) / f32::from(RINGS);
            let alpha = 1.0 / f32::from(RINGS) * (1.0 - f32::from(ring - 1) / f32::from(RINGS));
            if let Some(path) = rounded_rect(base.outset(spread - blur / 2.0), radius + spread) {
                self.fill_path(&path, color.alpha(alpha * 1.6));
            }
        }
    }

    fn fill_circle(&mut self, center: Point, radius: f32, color: Color) {
        if let Some(path) = PathBuilder::from_circle(center.x, center.y, radius) {
            self.fill_path(&path, color);
        }
    }

    fn polyline(&mut self, points: &[Point], closed: bool, width: f32, color: Color) {
        let mut path = PathBuilder::new();
        for (index, point) in points.iter().enumerate() {
            if index == 0 {
                path.move_to(point.x, point.y);
            } else {
                path.line_to(point.x, point.y);
            }
        }
        if closed {
            path.close();
        }
        if let Some(path) = path.finish() {
            self.stroke_path(&path, width, color);
        }
    }

    fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, color: Color) {
        if let Some(path) = PathBuilder::from_circle(center.x, center.y, radius) {
            self.stroke_path(&path, width, color);
        }
    }

    fn fill_polygon(&mut self, points: &[Point], color: Color) {
        let mut path = PathBuilder::new();
        for (index, point) in points.iter().enumerate() {
            if index == 0 {
                path.move_to(point.x, point.y);
            } else {
                path.line_to(point.x, point.y);
            }
        }
        path.close();
        if let Some(path) = path.finish() {
            self.fill_path(&path, color);
        }
    }

    fn measure(&mut self, text: &str, font: Font) -> TextSize {
        self.text.measure(text, font)
    }

    fn draw_text(&mut self, origin: Point, text: &str, font: Font, color: Color) {
        let scale = self.scale;
        let clip = self.clip_rect();
        let origin = Point::new(origin.x * scale, origin.y * scale);
        let Self {
            frame,
            text: engine,
            ..
        } = self;
        engine.draw(frame, origin, text, font, font.size * scale, color, clip);
    }

    fn push_clip(&mut self, rect: Rect) {
        let device = Rect::new(
            rect.x * self.scale,
            rect.y * self.scale,
            rect.w * self.scale,
            rect.h * self.scale,
        );
        let device = device.intersect(&self.clip_rect());
        let mut mask = Mask::new(self.frame.width(), self.frame.height());
        if let (Some(mask), Some(bounds)) = (
            mask.as_mut(),
            tiny_skia::Rect::from_xywh(device.x, device.y, device.w, device.h),
        ) {
            let path = PathBuilder::from_rect(bounds);
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        }
        if let Some(mask) = mask {
            self.clips.push((device, mask));
        }
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill_gradient(&mut self, rect: Rect, radius: f32, from: Color, to: Color, across: bool) {
        let Some(path) = rounded_rect(rect, radius) else {
            return;
        };
        let stop = |at: f32, c: Color| {
            GradientStop::new(at, tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a))
        };
        let start = tiny_skia::Point::from_xy(rect.x, rect.y);
        let end = if across {
            tiny_skia::Point::from_xy(rect.r(), rect.y)
        } else {
            tiny_skia::Point::from_xy(rect.x, rect.b())
        };
        let Some(shader) = LinearGradient::new(
            start,
            end,
            vec![stop(0.0, from), stop(1.0, to)],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            // Too small a span for a gradient to be seen.
            self.fill_rect(rect, radius, from.mix(to, 0.5));
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let transform = self.transform();
        let mask = self.clips.last().map(|(_, mask)| mask);
        self.frame
            .fill_path(&path, &paint, FillRule::Winding, transform, mask);
    }

    fn has_groups(&self) -> bool {
        true
    }

    fn push_group(&mut self, bounds: Rect, alpha: f32) {
        // Same size as the frame, so clips and coordinates need no second thought. The memory
        // comes zeroed and lazily from the OS, and only the group's bounds are ever copied.
        let (width, height) = (self.frame.width(), self.frame.height());
        let fresh = Pixmap::new(width, height).unwrap_or_else(|| self.frame.clone());
        let under = std::mem::replace(&mut self.frame, fresh);
        let scale = self.scale;
        self.groups.push(Group {
            under,
            bounds: Rect::new(
                bounds.x * scale,
                bounds.y * scale,
                bounds.w * scale,
                bounds.h * scale,
            ),
            alpha,
        });
    }

    fn pop_group(&mut self) {
        let Some(group) = self.groups.pop() else {
            return;
        };
        let drawn = std::mem::replace(&mut self.frame, group.under);
        let Some(bounds) = tiny_skia::Rect::from_xywh(
            group.bounds.x,
            group.bounds.y,
            group.bounds.w,
            group.bounds.h,
        ) else {
            return;
        };
        let paint = Paint {
            shader: Pattern::new(
                drawn.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Nearest,
                group.alpha,
                Transform::identity(),
            ),
            ..Paint::default()
        };
        let mask = self.clips.last().map(|(_, mask)| mask);
        self.frame
            .fill_rect(bounds, &paint, Transform::identity(), mask);
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// A rectangle path with circular corners, or `None` for an empty or invalid rectangle.
#[allow(clippy::many_single_char_names)] // x, y, w, h, r read like the geometry they are.
fn rounded_rect(rect: Rect, radius: f32) -> Option<Path> {
    let Rect { x, y, w, h } = rect;
    if !(x.is_finite() && y.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let r = if radius.is_finite() {
        radius.clamp(0.0, w.min(h) / 2.0)
    } else {
        0.0
    };
    if r == 0.0 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(
            x, y, w, h,
        )?));
    }
    // Distance of the cubic control points that best approximates a quarter circle.
    let k = r * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    path.close();
    path.finish()
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small test sizes.
mod tests {
    use super::*;

    fn canvas(scale: f32) -> CpuCanvas {
        let side = (40.0 * scale) as u32;
        let Ok(canvas) = CpuCanvas::new(side, side, scale, TextEngine::new()) else {
            unreachable!("a 40x40 frame is valid")
        };
        canvas
    }

    fn pixel(canvas: &CpuCanvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * canvas.width() + x) * 4) as usize;
        let p = canvas.pixels();
        [p[at], p[at + 1], p[at + 2], p[at + 3]]
    }

    #[test]
    fn fills_land_where_asked_and_scale_with_the_dpi() {
        for scale in [1.0, 2.0] {
            let mut canvas = canvas(scale);
            canvas.fill_rect(Rect::new(10.0, 10.0, 10.0, 10.0), 0.0, Color::WHITE);
            let inside = (15.0 * scale) as u32;
            let outside = (25.0 * scale) as u32;
            assert_eq!(pixel(&canvas, inside, inside), [255, 255, 255, 255]);
            assert_eq!(pixel(&canvas, outside, outside), [0, 0, 0, 0]);
        }
    }

    #[test]
    fn a_polygon_fills_inside_its_edges_and_leaves_its_notch_empty() {
        let mut canvas = canvas(1.0);
        canvas.clear(Color::BLACK);
        // A peak with a notch under it, like a letter A without its bar.
        canvas.fill_polygon(&LAMBDA, Color::WHITE);
        assert_eq!(pixel(&canvas, 20, 7), [255, 255, 255, 255], "the peak");
        assert_eq!(pixel(&canvas, 9, 30), [255, 255, 255, 255], "a leg");
        assert_eq!(pixel(&canvas, 20, 30), [0, 0, 0, 255], "the notch");
        assert_eq!(pixel(&canvas, 2, 10), [0, 0, 0, 255], "outside");
    }

    const LAMBDA: [Point; 7] = [
        Point::new(18.0, 5.0),
        Point::new(22.0, 5.0),
        Point::new(35.0, 35.0),
        Point::new(30.0, 35.0),
        Point::new(20.0, 14.0),
        Point::new(10.0, 35.0),
        Point::new(5.0, 35.0),
    ];

    #[test]
    fn a_group_lands_faded_as_one_where_overlaps_do_not_show_through() {
        let mut canvas = canvas(1.0);
        canvas.clear(Color::BLACK);
        let square = Rect::new(10.0, 10.0, 10.0, 10.0);
        canvas.push_group(Rect::new(0.0, 0.0, 40.0, 40.0), 0.5);
        // Red, then white over all of it: as one, only the white shows, at half.
        canvas.fill_rect(square, 0.0, Color::rgba(255, 0, 0, 255));
        canvas.fill_rect(square, 0.0, Color::WHITE);
        canvas.pop_group();
        let [r, g, b, a] = pixel(&canvas, 15, 15);
        assert_eq!(a, 255);
        assert!(r == g && g == b, "no red shows through: {r} {g} {b}");
        assert!((120..=135).contains(&r), "{r}");
        assert_eq!(
            pixel(&canvas, 25, 25),
            [0, 0, 0, 255],
            "the rest is untouched"
        );
    }

    #[test]
    fn a_group_lands_only_inside_its_bounds_and_the_active_clip() {
        let mut canvas = canvas(1.0);
        canvas.clear(Color::BLACK);
        canvas.push_clip(Rect::new(0.0, 0.0, 12.0, 40.0));
        canvas.push_group(Rect::new(5.0, 5.0, 10.0, 10.0), 1.0);
        canvas.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 0.0, Color::WHITE);
        canvas.pop_group();
        canvas.pop_clip();
        assert_eq!(pixel(&canvas, 8, 8), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 2, 2), [0, 0, 0, 255], "outside the bounds");
        assert_eq!(pixel(&canvas, 13, 8), [0, 0, 0, 255], "outside the clip");
    }

    #[test]
    fn a_gradient_runs_from_one_color_to_the_other_in_either_direction() {
        for across in [false, true] {
            let mut canvas = canvas(1.0);
            let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
            canvas.fill_gradient(rect, 0.0, Color::BLACK, Color::WHITE, across);
            let (near, far) = if across {
                (pixel(&canvas, 2, 20), pixel(&canvas, 37, 20))
            } else {
                (pixel(&canvas, 20, 2), pixel(&canvas, 20, 37))
            };
            assert!(near[0] < 40, "{near:?}");
            assert!(far[0] > 215, "{far:?}");
        }
    }

    #[test]
    fn clips_nest_and_pop() {
        let mut canvas = canvas(1.0);
        canvas.push_clip(Rect::new(0.0, 0.0, 20.0, 40.0));
        canvas.push_clip(Rect::new(10.0, 0.0, 30.0, 40.0));
        canvas.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 0.0, Color::WHITE);
        canvas.pop_clip();
        assert_eq!(pixel(&canvas, 5, 5), [0, 0, 0, 0]);
        assert_eq!(pixel(&canvas, 15, 5)[3], 255);
        assert_eq!(pixel(&canvas, 25, 5), [0, 0, 0, 0]);
        canvas.pop_clip();
        canvas.fill_rect(Rect::new(30.0, 30.0, 5.0, 5.0), 0.0, Color::WHITE);
        assert_eq!(pixel(&canvas, 32, 32)[3], 255);
    }

    #[test]
    fn text_draws_inside_its_clip_only() {
        let mut canvas = canvas(1.0);
        canvas.push_clip(Rect::new(0.0, 0.0, 6.0, 40.0));
        canvas.draw_text(Point::new(0.0, 10.0), "WWWW", Font::ui(16.0), Color::WHITE);
        canvas.pop_clip();
        let inked =
            |x0: u32, x1: u32| (x0..x1).any(|x| (0..40).any(|y| pixel(&canvas, x, y)[3] > 0));
        assert!(inked(0, 6));
        assert!(!inked(6, 40));
    }
}
