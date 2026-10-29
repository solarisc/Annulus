//! The portable CPU backend, also the reference renderer (ADR-0012): it draws a display list
//! into an RGBA pixel buffer with `tiny-skia`. It ships on macOS and Linux, and renders every
//! golden pixel test on every OS, so its output must be deterministic.

#![forbid(unsafe_code)]

use tiny_skia::{FillRule, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};
use annulus_ui::{Color, DisplayList, Op, Rect};

/// Why a canvas could not be made or saved.
#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    /// Width or height was zero or too large.
    #[error("canvas size {width}x{height} is not usable")]
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

/// A pixel buffer that display lists are drawn into.
pub struct Canvas {
    /// Finished layers; the last one is being drawn into. The first is the frame itself.
    layers: Vec<(Pixmap, f32)>,
    /// Active clip rectangles in device pixels, innermost last, each already intersected
    /// with the ones before it.
    clips: Vec<Option<Mask>>,
    /// DIPs to device pixels.
    scale: f32,
}

impl Canvas {
    /// A transparent canvas of `width` by `height` device pixels, where one DIP is `scale`
    /// pixels.
    ///
    /// # Errors
    /// [`CanvasError::BadSize`] for a zero or oversized canvas.
    pub fn new(width: u32, height: u32, scale: f32) -> Result<Self, CanvasError> {
        let pixmap = Pixmap::new(width, height).ok_or(CanvasError::BadSize { width, height })?;
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        Ok(Self {
            layers: vec![(pixmap, 1.0)],
            clips: Vec::new(),
            scale,
        })
    }

    /// Fills the whole canvas with `color`.
    pub fn clear(&mut self, color: Color) {
        if let Some((frame, _)) = self.layers.first_mut() {
            frame.fill(tiny_skia::Color::from_rgba8(
                color.r, color.g, color.b, color.a,
            ));
        }
    }

    /// Draws `list` on top of what is there.
    pub fn draw(&mut self, list: &DisplayList) {
        for op in list.ops() {
            self.draw_op(op);
        }
        // A well-formed list leaves nothing open; this only guards against a truncated one.
        while self.layers.len() > 1 {
            self.pop_layer();
        }
        self.clips.clear();
    }

    /// The frame as premultiplied RGBA bytes, row by row.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        self.layers.first().map_or(&[], |(frame, _)| frame.data())
    }

    /// Width in device pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.layers.first().map_or(0, |(frame, _)| frame.width())
    }

    /// Height in device pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.layers.first().map_or(0, |(frame, _)| frame.height())
    }

    /// The frame as a PNG file.
    ///
    /// # Errors
    /// [`CanvasError::Png`] if encoding fails.
    pub fn encode_png(&self) -> Result<Vec<u8>, CanvasError> {
        let (frame, _) = self
            .layers
            .first()
            .ok_or(CanvasError::Png("no frame".into()))?;
        frame
            .encode_png()
            .map_err(|error| CanvasError::Png(error.to_string()))
    }

    fn draw_op(&mut self, op: &Op) {
        match op {
            Op::Rect { rect, color } => self.fill(rounded_rect(*rect, 0.0), *color),
            Op::RoundedRect {
                rect,
                radius,
                color,
            } => self.fill(rounded_rect(*rect, *radius), *color),
            Op::Border {
                rect,
                radius,
                width,
                color,
            } => self.border(*rect, *radius, *width, *color),
            Op::Shadow {
                rect,
                radius,
                offset,
                blur,
                color,
            } => {
                self.shadow(*rect, *radius, offset.x, offset.y, *blur, *color);
            }
            // TODO: text runs need the text service (bundled font, swash, parley).
            Op::TextRun { .. } => {}
            // TODO: icons need the icon set.
            Op::Icon { .. } => {}
            // TODO: images need an image store; none are used before the host grid.
            Op::Image { .. } => {}
            Op::PushClip(rect) => self.push_clip(*rect),
            Op::PopClip => {
                self.clips.pop();
            }
            Op::PushLayer { opacity } => self.push_layer(*opacity),
            Op::PopLayer => self.pop_layer(),
        }
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    fn fill(&mut self, path: Option<Path>, color: Color) {
        let Some(path) = path else { return };
        let paint = paint(color);
        let transform = self.transform();
        let mask = self.clips.last().and_then(Option::as_ref);
        if let Some((target, _)) = self.layers.last_mut() {
            target.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    fn border(&mut self, rect: Rect, radius: f32, width: f32, color: Color) {
        if !(width.is_finite() && width > 0.0) {
            return;
        }
        // The stroke is centered on the path, so inset by half the width to stay inside.
        let half = width / 2.0;
        let inset = Rect::new(
            rect.origin.x + half,
            rect.origin.y + half,
            rect.size.width - width,
            rect.size.height - width,
        );
        let Some(path) = rounded_rect(inset, (radius - half).max(0.0)) else {
            return;
        };
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        let paint = paint(color);
        let transform = self.transform();
        let mask = self.clips.last().and_then(Option::as_ref);
        if let Some((target, _)) = self.layers.last_mut() {
            target.stroke_path(&path, &paint, &stroke, transform, mask);
        }
    }

    /// tiny-skia has no blur, so a shadow is drawn as rings of decreasing opacity around the
    /// shape: a close, deterministic approximation of a Gaussian falloff.
    fn shadow(&mut self, rect: Rect, radius: f32, dx: f32, dy: f32, blur: f32, color: Color) {
        const RINGS: u8 = 8;
        let blur = if blur.is_finite() { blur.max(0.0) } else { 0.0 };
        let base = Rect::new(
            rect.origin.x + dx,
            rect.origin.y + dy,
            rect.size.width,
            rect.size.height,
        );
        for ring in (0..RINGS).rev() {
            let spread = blur * f32::from(ring) / f32::from(RINGS);
            let grown = Rect::new(
                base.origin.x - spread,
                base.origin.y - spread,
                base.size.width + 2.0 * spread,
                base.size.height + 2.0 * spread,
            );
            let alpha = u16::from(color.a) / u16::from(RINGS);
            let ring_color = Color {
                a: u8::try_from(alpha).unwrap_or(u8::MAX),
                ..color
            };
            self.fill(rounded_rect(grown, radius + spread), ring_color);
        }
    }

    fn push_clip(&mut self, rect: Rect) {
        let (width, height) = (self.width(), self.height());
        let outer = self.clips.last().cloned().flatten();
        let mask = Mask::new(width, height).and_then(|mut mask| {
            let path = rounded_rect(rect, 0.0)?;
            mask.fill_path(&path, FillRule::Winding, false, self.transform());
            if let Some(outer) = outer {
                intersect(&mut mask, &outer);
            }
            Some(mask)
        });
        self.clips.push(mask);
    }

    fn push_layer(&mut self, opacity: f32) {
        if let Some(layer) = Pixmap::new(self.width(), self.height()) {
            self.layers.push((layer, opacity));
        }
    }

    fn pop_layer(&mut self) {
        if self.layers.len() < 2 {
            return;
        }
        let Some((layer, opacity)) = self.layers.pop() else {
            return;
        };
        let paint = PixmapPaint {
            opacity,
            ..PixmapPaint::default()
        };
        if let Some((target, _)) = self.layers.last_mut() {
            target.draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), None);
        }
    }
}

/// Keeps only the pixels covered by both masks.
fn intersect(mask: &mut Mask, outer: &Mask) {
    for (inner, outer) in mask.data_mut().iter_mut().zip(outer.data()) {
        *inner = u8::try_from(u16::from(*inner) * u16::from(*outer) / 255).unwrap_or(u8::MAX);
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// A rectangle path with circular corners, or `None` for an empty or invalid rectangle.
fn rounded_rect(rect: Rect, radius: f32) -> Option<Path> {
    if rect.is_empty() || !rect.origin.x.is_finite() || !rect.origin.y.is_finite() {
        return None;
    }
    let (x, y, w, h) = (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    );
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
