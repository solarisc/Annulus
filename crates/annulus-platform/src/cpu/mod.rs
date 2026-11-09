//! The CPU renderer ([ADR-0017] decision 3): `tiny-skia` for shapes, our text engine for
//! glyphs, into an RGBA buffer. It ships on Linux and renders every golden image on every
//! OS, so its output must be deterministic.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![forbid(unsafe_code)]

pub(crate) mod text;

pub use text::TextEngine;

use annulus_ui::gfx::{Canvas, Color, Font, Point, Rect, TextSize};
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint, Path, PathBuilder,
    Pixmap, SpreadMode, Stroke, Transform,
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
    /// Device pixels per DIP reported to layout and terminal sizing.
    scale: f32,
    /// Device pixels per DIP in the frame being painted, smaller during backdrop capture.
    paint_scale: f32,
    /// Clip rectangles in device pixels with their masks; the last is active.
    clips: Vec<(Rect, Mask)>,
    text: TextEngine,
    /// The groups open, innermost last: the frame under each, and where and how it lands.
    groups: Vec<Group>,
    /// The full-size frame held while a backdrop is drawn at blur resolution.
    backdrop: Option<Backdrop>,
}

/// A group being drawn: `frame` holds it meanwhile, and this holds what is under it.
struct Group {
    under: Pixmap,
    /// Where it lands, in device pixels.
    bounds: Rect,
    alpha: f32,
}

struct Backdrop {
    under: Pixmap,
    paint_scale: f32,
    bounds: Rect,
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
            paint_scale: scale,
            clips: Vec::new(),
            text,
            groups: Vec::new(),
            backdrop: None,
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
        Transform::from_scale(self.paint_scale, self.paint_scale)
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
    fn set_font_families(&mut self, ui: Option<&str>, mono: Option<&str>) {
        self.text.set_families(ui, mono);
    }

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
        const RINGS: u8 = 10;
        let clip = self.clip_rect();
        let mask = self.clips.last().map(|(_, mask)| mask.data());
        if draw_shadow(
            &mut self.frame,
            clip,
            mask,
            rect,
            radius,
            blur,
            offset_y,
            color,
            self.paint_scale,
        ) {
            return;
        }
        // Very small shapes can collapse inside a wide blur. Keep the path version for those.
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
        let scale = self.paint_scale;
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
            rect.x * self.paint_scale,
            rect.y * self.paint_scale,
            rect.w * self.paint_scale,
            rect.h * self.paint_scale,
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

    fn begin_backdrop_capture(&mut self, bounds: Rect, radius: f32) {
        if self.backdrop.is_some()
            || !self.groups.is_empty()
            || !self.clips.is_empty()
            || bounds.x != 0.0
            || bounds.y != 0.0
            || bounds.w <= 0.0
            || bounds.h <= 0.0
        {
            return;
        }
        let device_pixel = 1.0 / self.scale.max(1.0);
        let sample = if radius <= 0.0 {
            device_pixel
        } else {
            (radius / 2.0).clamp(device_pixel, 8.0)
        };
        let capture_scale = 1.0 / sample;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Window pixels.
        let width = (bounds.w * capture_scale).ceil().max(1.0) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Window pixels.
        let height = (bounds.h * capture_scale).ceil().max(1.0) as u32;
        let Some(frame) = Pixmap::new(width, height) else {
            return;
        };
        let under = std::mem::replace(&mut self.frame, frame);
        let paint_scale = std::mem::replace(&mut self.paint_scale, capture_scale);
        self.backdrop = Some(Backdrop {
            under,
            paint_scale,
            bounds,
        });
    }

    fn end_backdrop_capture(&mut self, bounds: Rect, radius: f32) {
        let Some(capture) = self.backdrop.take() else {
            self.blur_backdrop(bounds, radius);
            return;
        };
        let drawn = std::mem::replace(&mut self.frame, capture.under);
        self.paint_scale = capture.paint_scale;
        let bounds = if bounds.w > 0.0 && bounds.h > 0.0 {
            bounds
        } else {
            capture.bounds
        };
        scale_backdrop(&drawn, &mut self.frame, bounds, self.scale);
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Pixel bounds.
    fn blur_backdrop(&mut self, rect: Rect, radius: f32) {
        let frame_w = self.frame.width() as usize;
        let frame_h = self.frame.height() as usize;
        let x0 = (rect.x * self.paint_scale).floor().max(0.0) as usize;
        let y0 = (rect.y * self.paint_scale).floor().max(0.0) as usize;
        let x1 = (rect.r() * self.paint_scale).ceil().max(0.0) as usize;
        let y1 = (rect.b() * self.paint_scale).ceil().max(0.0) as usize;
        let x1 = x1.min(frame_w);
        let y1 = y1.min(frame_h);
        let radius = (radius * self.paint_scale).round().max(0.0) as usize;
        if radius == 0 || x0 >= x1 || y0 >= y1 {
            return;
        }
        box_blur(
            self.frame.data_mut(),
            frame_w,
            (x0, y0, x1 - x0, y1 - y0),
            radius,
        );
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
        let scale = self.paint_scale;
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
        let bounds = group.bounds.intersect(&self.clip_rect());
        blend_group(&mut self.frame, &drawn, bounds, group.alpha);
    }
}

/// Draws the same nested shadow rings in one bounded pixel pass. Each ring is an outset of the
/// same rounded core, so one signed distance says which rings cover a pixel.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines
)] // Clamped pixel bounds and the Canvas shadow contract.
fn draw_shadow(
    frame: &mut Pixmap,
    clip: Rect,
    mask: Option<&[u8]>,
    rect: Rect,
    radius: f32,
    blur: f32,
    offset_y: f32,
    color: Color,
    scale: f32,
) -> bool {
    const RINGS: usize = 10;
    if color.a == 0
        || !rect.x.is_finite()
        || !rect.y.is_finite()
        || rect.w <= 0.0
        || rect.h <= 0.0
        || !radius.is_finite()
        || !blur.is_finite()
        || !offset_y.is_finite()
    {
        return true;
    }
    let blur = blur.max(0.0) * scale;
    let radius = radius.max(0.0) * scale;
    let base = Rect::new(
        rect.x * scale,
        (rect.y + offset_y) * scale,
        rect.w * scale,
        rect.h * scale,
    );
    let core = Rect::new(
        base.x + blur / 2.0,
        base.y + blur / 2.0,
        base.w - blur,
        base.h - blur,
    );
    // The shared-core shortcut holds only while none of the rounded paths clamp its radius.
    if core.w <= 0.0
        || core.h <= 0.0
        || radius > core.w.min(core.h) / 2.0
        || radius + blur > (core.w + 2.0 * blur).min(core.h + 2.0 * blur) / 2.0
    {
        return false;
    }

    let frame_w = frame.width() as usize;
    let frame_h = frame.height() as usize;
    let outer = core.outset(blur).intersect(&clip);
    let x0 = outer.x.floor().max(0.0) as usize;
    let y0 = outer.y.floor().max(0.0) as usize;
    let x1 = outer.r().ceil().min(frame_w as f32).max(0.0) as usize;
    let y1 = outer.b().ceil().min(frame_h as f32).max(0.0) as usize;
    let width = x1.saturating_sub(x0);
    let height = y1.saturating_sub(y0);
    if width == 0 || height == 0 {
        return true;
    }

    let ring_alpha: [f32; RINGS] = std::array::from_fn(|index| {
        let ring = (index + 1) as f32;
        let alpha = 1.0 / RINGS as f32 * (1.0 - (ring - 1.0) / RINGS as f32) * 1.6;
        f32::from(color.alpha(alpha).a) / 255.0
    });
    let center = core.center();
    let half_w = core.w / 2.0;
    let half_h = core.h / 2.0;
    let flat_w = half_w - radius;
    let flat_h = half_h - radius;
    let stride = frame_w * 4;
    let pixels = &mut frame.data_mut()[y0 * stride..y1 * stride];
    let workers = blur_workers(width * height, height);
    std::thread::scope(|scope| {
        let mut first = 0;
        let mut unclaimed_rows = pixels;
        for worker in 0..workers {
            let left = workers - worker;
            let take_rows = (height - first) / left;
            let (band, tail) = unclaimed_rows.split_at_mut(take_rows * stride);
            unclaimed_rows = tail;
            let start = first;
            first += take_rows;
            let ring_alpha = &ring_alpha;
            scope.spawn(move || {
                for (local_y, row) in band.chunks_exact_mut(stride).enumerate() {
                    let y = y0 + start + local_y;
                    let py = y as f32 + 0.5;
                    for (local_x, pixel) in row[x0 * 4..x1 * 4]
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .enumerate()
                    {
                        let x = x0 + local_x;
                        let px = x as f32 + 0.5;
                        let qx = (px - center.x).abs() - flat_w;
                        let qy = (py - center.y).abs() - flat_h;
                        let outside = qx.max(0.0).hypot(qy.max(0.0));
                        let distance = outside + qx.max(qy).min(0.0) - radius;
                        let mut transparent = 1.0_f32;
                        for (index, alpha) in ring_alpha.iter().enumerate() {
                            let spread = blur * (index + 1) as f32 / RINGS as f32;
                            let coverage = (spread + 0.5 - distance).clamp(0.0, 1.0);
                            transparent *= 1.0 - alpha * coverage;
                        }
                        let mut alpha = ((1.0 - transparent) * 255.0).round() as u32;
                        if let Some(mask) = mask {
                            alpha = (alpha * u32::from(mask[y * frame_w + x]) + 127) / 255;
                        }
                        if alpha == 0 {
                            continue;
                        }
                        let keep = 255 - alpha;
                        let source = |channel: u8| (u32::from(channel) * alpha + 127) / 255;
                        for (out, channel) in pixel[..3].iter_mut().zip([color.r, color.g, color.b])
                        {
                            *out = (source(channel) + (u32::from(*out) * keep + 127) / 255).min(255)
                                as u8;
                        }
                        pixel[3] =
                            (alpha + (u32::from(pixel[3]) * keep + 127) / 255).min(255) as u8;
                    }
                }
            });
        }
    });
    true
}

/// Scales a small blurred backdrop into its full-size bounds. The generic vector sampler costs
/// more than a Retina frame for this large bitmap, while this fixed bilinear pass has no paths,
/// masks or blend modes to check and can split its rows safely.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)] // Clamped pixel bounds; window sizes fit in f32 exactly enough.
fn scale_backdrop(source: &Pixmap, target: &mut Pixmap, bounds: Rect, scale: f32) {
    let target_w = target.width() as usize;
    let target_h = target.height() as usize;
    let x0 = (bounds.x * scale).floor().max(0.0) as usize;
    let y0 = (bounds.y * scale).floor().max(0.0) as usize;
    let x1 = (bounds.r() * scale).ceil().min(target_w as f32).max(0.0) as usize;
    let y1 = (bounds.b() * scale).ceil().min(target_h as f32).max(0.0) as usize;
    let width = x1.saturating_sub(x0);
    let height = y1.saturating_sub(y0);
    if width == 0 || height == 0 {
        return;
    }
    let source_w = source.width() as usize;
    let source_h = source.height() as usize;
    let axis = |at: usize, from: usize, to: usize| {
        let point = ((at as f32 + 0.5) * from as f32 / to as f32 - 0.5)
            .clamp(0.0, from.saturating_sub(1) as f32);
        let low = point.floor() as usize;
        let high = (low + 1).min(from - 1);
        let share = ((point - low as f32) * 256.0).round() as u32;
        (low, high, share)
    };
    let x_map: Vec<_> = (0..width)
        .map(|x| {
            let (low, high, share) = axis(x, source_w, width);
            (low, high, share)
        })
        .collect();
    let source = source.data().as_chunks::<4>().0;
    let wide_stride = width * 4;
    let mut wide = vec![0_u8; wide_stride * source_h];
    let workers = blur_workers(width * source_h, source_h);
    std::thread::scope(|scope| {
        let mut first = 0;
        let mut rest = wide.as_mut_slice();
        for worker in 0..workers {
            let left = workers - worker;
            let take_rows = (source_h - first) / left;
            let (band, tail) = rest.split_at_mut(take_rows * wide_stride);
            rest = tail;
            let start = first;
            first += take_rows;
            let x_map = &x_map;
            scope.spawn(move || {
                for (local_y, row) in band.chunks_exact_mut(wide_stride).enumerate() {
                    let source_row = (start + local_y) * source_w;
                    for (pixel, &(left, right, across)) in
                        row.as_chunks_mut::<4>().0.iter_mut().zip(x_map)
                    {
                        *pixel = lerp_rgba(
                            source[source_row + left],
                            source[source_row + right],
                            across,
                        );
                    }
                }
            });
        }
    });
    let wide = wide.as_chunks::<4>().0;
    let target_stride = target_w * 4;
    let target = &mut target.data_mut()[y0 * target_stride..y1 * target_stride];
    let workers = blur_workers(width * height, height);
    std::thread::scope(|scope| {
        let mut first = 0;
        let mut rest = target;
        for worker in 0..workers {
            let left = workers - worker;
            let take_rows = (height - first) / left;
            let (band, tail) = rest.split_at_mut(take_rows * target_stride);
            rest = tail;
            let start = first;
            first += take_rows;
            scope.spawn(move || {
                for (local_y, row) in band.chunks_exact_mut(target_stride).enumerate() {
                    let y = start + local_y;
                    let (top, bottom, down) = axis(y, source_h, height);
                    let top = top * width;
                    let bottom = bottom * width;
                    let row = &mut row[x0 * 4..x1 * 4];
                    for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        *pixel = lerp_rgba(wide[top + x], wide[bottom + x], down);
                    }
                }
            });
        }
    });
}

fn lerp_rgba(from: [u8; 4], to: [u8; 4], share: u32) -> [u8; 4] {
    const PAIRS: u32 = 0x00ff_00ff;
    const ROUND: u32 = 0x0080_0080;
    let from = u32::from_le_bytes(from);
    let to = u32::from_le_bytes(to);
    let keep = 256 - share;
    let rb = (((from & PAIRS) * keep + (to & PAIRS) * share + ROUND) >> 8) & PAIRS;
    let ga = (((((from >> 8) & PAIRS) * keep + ((to >> 8) & PAIRS) * share + ROUND) >> 8) << 8)
        & 0xff00_ff00;
    (rb | ga).to_le_bytes()
}

/// Lands one premultiplied RGBA layer on another. A group already has the same size and pixel
/// places as its frame, so a direct source-over pass is much faster than a vector pattern fill.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)] // Clamped pixel bounds; frame sizes fit exactly enough in f32.
fn blend_group(under: &mut Pixmap, drawn: &Pixmap, bounds: Rect, alpha: f32) {
    let width = under.width() as usize;
    let x0 = bounds.x.floor().max(0.0) as usize;
    let y0 = bounds.y.floor().max(0.0) as usize;
    let x1 = bounds.r().ceil().min(under.width() as f32).max(0.0) as usize;
    let y1 = bounds.b().ceil().min(under.height() as f32).max(0.0) as usize;
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    let source = drawn.data();
    let target = under.data_mut();
    let stride = width * 4;
    let rows = y1.saturating_sub(y0);
    let pixels = rows * x1.saturating_sub(x0);
    let workers = if pixels >= 256 * 1024 {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(6)
            .min(rows)
    } else {
        1
    };
    let target = &mut target[y0 * stride..y1 * stride];
    let source = &source[y0 * stride..y1 * stride];
    std::thread::scope(|scope| {
        let mut target = target;
        let mut source = source;
        for worker in 0..workers {
            let left = workers - worker;
            let take = target.len() / stride / left * stride;
            let (target_band, rest) = target.split_at_mut(take);
            let (source_band, source_rest) = source.split_at(take);
            target = rest;
            source = source_rest;
            scope.spawn(move || {
                blend_group_rows(target_band, source_band, stride, x0, x1, alpha);
            });
        }
    });
}

fn blend_group_rows(
    target: &mut [u8],
    source: &[u8],
    stride: usize,
    x0: usize,
    x1: usize,
    alpha: u32,
) {
    for (target, source) in target
        .chunks_exact_mut(stride)
        .zip(source.chunks_exact(stride))
    {
        let target = target[x0 * 4..x1 * 4].as_chunks_mut::<4>().0;
        let source = source[x0 * 4..x1 * 4].as_chunks::<4>().0;
        for (target, source) in target.iter_mut().zip(source) {
            let source_alpha = (u32::from(source[3]) * alpha + 127) / 255;
            let keep = 255 - source_alpha;
            for channel in 0..4 {
                let over = (u32::from(source[channel]) * alpha + 127) / 255;
                let below = (u32::from(target[channel]) * keep + 127) / 255;
                target[channel] = u8::try_from((over + below).min(255)).unwrap_or(255);
            }
        }
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// A two-pass box blur over one part of a premultiplied RGBA frame.
fn box_blur(
    pixels: &mut [u8],
    frame_w: usize,
    (x0, y0, width, height): (usize, usize, usize, usize),
    radius: usize,
) {
    // One row and one column are enough: finish a whole line before replacing its source.
    // At 4K this stays in kilobytes instead of making another frame-sized buffer.
    let mut line = vec![0_u8; width.max(height) * 4];
    for y in 0..height {
        let mut sum = [0_u32; 4];
        let first_end = radius.min(width - 1);
        for x in 0..=first_end {
            let at = ((y0 + y) * frame_w + x0 + x) * 4;
            for channel in 0..4 {
                sum[channel] += u32::from(pixels[at + channel]);
            }
        }
        let mut count = first_end + 1;
        for x in 0..width {
            let out = x * 4;
            for channel in 0..4 {
                line[out + channel] =
                    u8::try_from(sum[channel] / u32::try_from(count).unwrap_or(1))
                        .unwrap_or(u8::MAX);
            }
            if x >= radius {
                let remove = ((y0 + y) * frame_w + x0 + x - radius) * 4;
                for channel in 0..4 {
                    sum[channel] -= u32::from(pixels[remove + channel]);
                }
                count -= 1;
            }
            if x + radius + 1 < width {
                let add = ((y0 + y) * frame_w + x0 + x + radius + 1) * 4;
                for channel in 0..4 {
                    sum[channel] += u32::from(pixels[add + channel]);
                }
                count += 1;
            }
        }
        let at = ((y0 + y) * frame_w + x0) * 4;
        pixels[at..at + width * 4].copy_from_slice(&line[..width * 4]);
    }
    for x in 0..width {
        let mut sum = [0_u32; 4];
        let first_end = radius.min(height - 1);
        for y in 0..=first_end {
            let at = ((y0 + y) * frame_w + x0 + x) * 4;
            for channel in 0..4 {
                sum[channel] += u32::from(pixels[at + channel]);
            }
        }
        let mut count = first_end + 1;
        for y in 0..height {
            let out = y * 4;
            for channel in 0..4 {
                line[out + channel] =
                    u8::try_from(sum[channel] / u32::try_from(count).unwrap_or(1))
                        .unwrap_or(u8::MAX);
            }
            if y >= radius {
                let remove = ((y0 + y - radius) * frame_w + x0 + x) * 4;
                for channel in 0..4 {
                    sum[channel] -= u32::from(pixels[remove + channel]);
                }
                count -= 1;
            }
            if y + radius + 1 < height {
                let add = ((y0 + y + radius + 1) * frame_w + x0 + x) * 4;
                for channel in 0..4 {
                    sum[channel] += u32::from(pixels[add + channel]);
                }
                count += 1;
            }
        }
        for y in 0..height {
            let at = ((y0 + y) * frame_w + x0 + x) * 4;
            let from = y * 4;
            pixels[at..at + 4].copy_from_slice(&line[from..from + 4]);
        }
    }
}

fn blur_workers(pixels: usize, rows: usize) -> usize {
    if pixels < 256 * 1024 {
        return 1;
    }
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(6)
        .min(rows)
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
    fn backdrop_blur_softens_an_edge_and_stays_inside_its_bounds() {
        let mut canvas = canvas(1.0);
        canvas.clear(Color::BLACK);
        canvas.fill_rect(Rect::new(0.0, 0.0, 20.0, 40.0), 0.0, Color::WHITE);
        let untouched = pixel(&canvas, 2, 20);
        canvas.blur_backdrop(Rect::new(5.0, 0.0, 30.0, 40.0), 4.0);
        let near = pixel(&canvas, 19, 20)[0];
        let over = pixel(&canvas, 21, 20)[0];
        assert!((20..235).contains(&near), "{near}");
        assert!((20..235).contains(&over), "{over}");
        assert_eq!(pixel(&canvas, 2, 20), untouched, "outside the blur");
    }

    #[test]
    fn backdrop_capture_draws_small_then_fills_the_retina_frame() {
        let mut canvas = CpuCanvas::new(400, 240, 2.0, TextEngine::new()).unwrap();
        let bounds = Rect::new(0.0, 0.0, 200.0, 120.0);
        canvas.begin_backdrop_capture(bounds, 12.0);
        assert!(canvas.width() < 100, "the backdrop stayed Retina-sized");
        assert_eq!(canvas.scale(), 2.0, "layout still sees the display scale");
        canvas.fill_rect(bounds, 0.0, Color::WHITE);
        canvas.end_backdrop_capture(bounds, 12.0);
        assert_eq!((canvas.width(), canvas.height()), (400, 240));
        assert_eq!(pixel(&canvas, 200, 120), [255, 255, 255, 255]);
    }

    #[test]
    fn backdrop_scaling_blends_all_four_channels_together() {
        assert_eq!(
            lerp_rgba([0, 64, 128, 255], [255, 128, 0, 0], 128),
            [128, 96, 64, 128]
        );
        assert_eq!(lerp_rgba([1, 2, 3, 4], [9, 8, 7, 6], 0), [1, 2, 3, 4]);
        assert_eq!(lerp_rgba([1, 2, 3, 4], [9, 8, 7, 6], 256), [9, 8, 7, 6]);
    }

    #[test]
    fn a_shadow_fades_outward_and_stays_inside_its_clip() {
        let mut canvas = canvas(1.0);
        let card = Rect::new(12.0, 12.0, 16.0, 16.0);
        canvas.push_clip(Rect::new(0.0, 0.0, 20.0, 40.0));
        canvas.shadow(card, 4.0, 4.0, 0.0, Color::BLACK.alpha(0.8));
        canvas.pop_clip();
        let middle = pixel(&canvas, 16, 16)[3];
        let edge = pixel(&canvas, 10, 16)[3];
        assert!(middle > edge && edge > 0, "shadow alpha: {middle} {edge}");
        assert_eq!(pixel(&canvas, 22, 16), [0, 0, 0, 0], "outside the clip");
        assert_eq!(pixel(&canvas, 3, 16), [0, 0, 0, 0], "past the blur");
    }

    /// Wall-clock checks vary across machines; run this on the Retina Mac after shadow work.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "manual Retina Mac speed check"]
    fn retina_palette_shadows_draw_within_one_frame() {
        use std::time::{Duration, Instant};

        let mut canvas = CpuCanvas::new(2_200, 1_440, 2.0, TextEngine::new()).unwrap();
        let card = Rect::new(274.0, 72.0, 552.0, 424.0);
        let began = Instant::now();
        canvas.shadow(card, 18.0, 34.0, 14.0, Color::BLACK.alpha(0.75));
        canvas.shadow(card, 18.0, 7.0, 2.0, Color::BLACK.alpha(0.55));
        let elapsed = began.elapsed();
        assert!(
            elapsed <= Duration::from_micros(16_667),
            "Retina palette shadows took {elapsed:?}"
        );
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
