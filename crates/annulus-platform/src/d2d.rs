//! The Windows renderer ([ADR-0017] decision 3): Direct2D shapes and DirectWrite text with
//! the system fonts (Segoe UI Variable, Cascadia Mono). It draws on any `ID2D1RenderTarget`:
//! a window's, or a WIC bitmap with no window at all, which is how CI photographs the
//! Windows look of every scene.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![allow(unsafe_code)]
// The bindings take raw pointers to locals that outlive each call.
#![allow(clippy::borrow_as_ptr)]

use std::collections::HashMap;

use annulus_ui::gfx::{Canvas, Color, Family, Font, Point, Rect, TextSize, Weight};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BRUSH_PROPERTIES, D2D1_CAP_STYLE_ROUND,
    D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_GAMMA_2_2,
    D2D1_LAYER_OPTIONS_INITIALIZE_FOR_CLEARTYPE, D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_ROUND,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
    ID2D1Factory, ID2D1Factory1, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};
use windows_numerics::{Matrix3x2, Vector2};

/// The Direct2D and DirectWrite factories and the fonts they found; make once and keep.
pub struct D2d {
    factory: ID2D1Factory1,
    write: IDWriteFactory,
    ui_family: HSTRING,
    display_family: HSTRING,
    mono_family: HSTRING,
}

impl D2d {
    /// The factories, with the best UI and monospace fonts installed.
    ///
    /// # Errors
    /// When Direct2D or DirectWrite is unavailable.
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: plain factory creation; the results are reference-counted COM objects.
        let (factory, write) = unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            (factory, write)
        };
        let pick = |names: &[&str]| {
            let found = names
                .iter()
                .copied()
                .find(|name| has_family(&write, name))
                .unwrap_or(names[names.len() - 1]);
            HSTRING::from(found)
        };
        let ui_family = pick(&["Segoe UI Variable Text", "Segoe UI"]);
        let display_family = pick(&["Segoe UI Variable Display", "Segoe UI"]);
        let mono_family = pick(&["Cascadia Mono", "Consolas"]);
        Ok(Self {
            factory,
            write,
            ui_family,
            display_family,
            mono_family,
        })
    }

    /// The Direct2D factory, for render targets.
    pub(crate) fn factory(&self) -> &ID2D1Factory1 {
        &self.factory
    }

    /// Draws a `width` x `height` pixel frame at `scale` into memory with `draw`, and returns
    /// it as a PNG file.
    ///
    /// # Errors
    /// When WIC or Direct2D refuse, or encoding fails.
    pub fn render_png(
        &self,
        width: u32,
        height: u32,
        scale: f32,
        draw: impl FnOnce(&mut D2dCanvas<'_>),
    ) -> Result<Vec<u8>, String> {
        // SAFETY: COM is initialized for this thread before WIC is used; a second call on a
        // thread that already initialized COM only returns S_FALSE.
        let wic: IWICImagingFactory = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(fail("WIC"))?
        };
        // SAFETY: the bitmap and target are created and used on this thread only; the
        // properties outlive the call.
        let (bitmap, target): (IWICBitmap, ID2D1RenderTarget) = unsafe {
            let bitmap = wic
                .CreateBitmap(
                    width,
                    height,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
                .map_err(fail("bitmap"))?;
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0 * scale,
                dpiY: 96.0 * scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = self
                .factory
                .CreateWicBitmapRenderTarget(&bitmap, &properties)
                .map_err(fail("render target"))?;
            // A bitmap with alpha cannot take ClearType.
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            (bitmap, target)
        };
        {
            let mut canvas = D2dCanvas::new(self, target.clone(), scale).map_err(fail("canvas"))?;
            // SAFETY: drawing calls are bracketed by BeginDraw and EndDraw on this thread.
            unsafe {
                target.BeginDraw();
            }
            draw(&mut canvas);
            canvas.finish();
            // SAFETY: as above.
            unsafe { target.EndDraw(None, None) }.map_err(fail("draw"))?;
        }
        let stride = width * 4;
        let mut pixels = vec![0u8; (stride * height) as usize];
        // SAFETY: the buffer holds exactly `height` rows of `stride` bytes.
        unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels) }
            .map_err(fail("read back"))?;
        // BGRA to RGBA, both premultiplied, as tiny-skia stores them.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let size = tiny_skia::IntSize::from_wh(width, height).ok_or("empty frame")?;
        let pixmap = tiny_skia::Pixmap::from_vec(pixels, size).ok_or("frame size")?;
        pixmap.encode_png().map_err(|e| format!("png: {e}"))
    }
}

/// Turns a Windows error into a message naming the step that failed.
fn fail(what: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |e| format!("{what}: {e}")
}

/// Whether DirectWrite knows the font family `name`.
fn has_family(write: &IDWriteFactory, name: &str) -> bool {
    let mut collection = None;
    let name = HSTRING::from(name);
    // SAFETY: out-parameters are valid locals; the collection is a COM object.
    unsafe {
        if write
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return false;
        }
        let Some(collection) = collection else {
            return false;
        };
        let mut index = 0;
        let mut exists = BOOL(0);
        collection
            .FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)
            .is_ok()
            && exists.as_bool()
    }
}

/// A [`Canvas`] over a Direct2D render target, between its `BeginDraw` and `EndDraw`.
pub struct D2dCanvas<'a> {
    d2d: &'a D2d,
    target: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    round: ID2D1StrokeStyle,
    scale: f32,
    /// What is open on the target, innermost last: Direct2D wants them closed in that order.
    open: Vec<Open>,
    /// Clips a group's end had to close early, whose own ends still come.
    closed_early: u32,
    formats: HashMap<(Family, u32, Weight), IDWriteTextFormat>,
    layouts: HashMap<(String, Family, u32, Weight), IDWriteTextLayout>,
}

/// A clip or a group open on the target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Clip,
    Group,
}

impl<'a> D2dCanvas<'a> {
    /// A canvas on `target`, whose DPI is `96 * scale`.
    ///
    /// # Errors
    /// When the brush or stroke style cannot be made.
    pub fn new(d2d: &'a D2d, target: ID2D1RenderTarget, scale: f32) -> windows::core::Result<Self> {
        // SAFETY: target-owned resources made on the target's thread.
        let (brush, round) = unsafe {
            let brush = target.CreateSolidColorBrush(&color(Color::BLACK), None)?;
            let base: &ID2D1Factory = &d2d.factory;
            let round = base.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            (brush, round)
        };
        Ok(Self {
            d2d,
            target,
            brush,
            round,
            scale,
            open: Vec::new(),
            closed_early: 0,
            formats: HashMap::new(),
            layouts: HashMap::new(),
        })
    }

    /// Closes whatever is left open, before `EndDraw`.
    pub fn finish(&mut self) {
        while let Some(open) = self.open.pop() {
            self.close(open);
        }
    }

    fn close(&mut self, open: Open) {
        // SAFETY: `open` is the innermost thing open on the target.
        unsafe {
            match open {
                Open::Clip => self.target.PopAxisAlignedClip(),
                Open::Group => self.target.PopLayer(),
            }
        }
    }

    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        // SAFETY: the brush belongs to this canvas's target.
        unsafe { self.brush.SetColor(&color(c)) };
        &self.brush
    }

    fn format(&mut self, font: Font) -> Option<IDWriteTextFormat> {
        let key = (font.family, font.size.to_bits(), font.weight);
        if let Some(format) = self.formats.get(&key) {
            return Some(format.clone());
        }
        let family = match font.family {
            Family::Ui => &self.d2d.ui_family,
            Family::Display => &self.d2d.display_family,
            Family::Mono => &self.d2d.mono_family,
        };
        #[allow(clippy::cast_possible_truncation)] // Weights are 400 to 700.
        let weight = DWRITE_FONT_WEIGHT(font.weight.value() as i32);
        // SAFETY: the family string lives in `D2d`; the format is a COM object.
        let format = unsafe {
            let format = self
                .d2d
                .write
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font.size,
                    w!("en-us"),
                )
                .ok()?;
            let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            format
        };
        self.formats.insert(key, format.clone());
        Some(format)
    }

    fn layout(&mut self, text: &str, font: Font) -> Option<IDWriteTextLayout> {
        let key = (
            text.to_owned(),
            font.family,
            font.size.to_bits(),
            font.weight,
        );
        if let Some(layout) = self.layouts.get(&key) {
            return Some(layout.clone());
        }
        let format = self.format(font)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` outlives the call; the layout copies it.
        let layout = unsafe {
            self.d2d
                .write
                .CreateTextLayout(&wide, &format, 100_000.0, 10_000.0)
                .ok()?
        };
        self.layouts.insert(key, layout.clone());
        Some(layout)
    }
}

fn color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    }
}

fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.r(),
        bottom: r.b(),
    }
}

fn point(p: Point) -> Vector2 {
    Vector2 { X: p.x, Y: p.y }
}

impl Canvas for D2dCanvas<'_> {
    fn scale(&self) -> f32 {
        self.scale
    }

    fn fill_rect(&mut self, r: Rect, radius: f32, c: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        // SAFETY: drawing between BeginDraw and EndDraw with this target's brush.
        unsafe {
            if radius == 0.0 {
                self.target.FillRectangle(&rect(r), self.brush(c));
            } else {
                let rounded = D2D1_ROUNDED_RECT {
                    rect: rect(r),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.target.FillRoundedRectangle(&rounded, self.brush(c));
            }
        }
    }

    fn stroke_rect(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        let half = width / 2.0;
        let inner = r.inset(half, half);
        if inner.w <= 0.0 || inner.h <= 0.0 {
            return;
        }
        let radius = (radius - half).clamp(0.0, inner.w.min(inner.h) / 2.0);
        let rounded = D2D1_ROUNDED_RECT {
            rect: rect(inner),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `fill_rect`.
        unsafe {
            self.target
                .DrawRoundedRectangle(&rounded, self.brush(c), width, None);
        }
    }

    fn shadow(&mut self, r: Rect, radius: f32, blur: f32, offset_y: f32, c: Color) {
        // The same stacked rings as the CPU renderer, so both look alike.
        const RINGS: u8 = 10;
        let blur = if blur.is_finite() { blur.max(0.0) } else { 0.0 };
        let base = r.offset(0.0, offset_y);
        for ring in (1..=RINGS).rev() {
            let spread = blur * f32::from(ring) / f32::from(RINGS);
            let alpha = 1.0 / f32::from(RINGS) * (1.0 - f32::from(ring - 1) / f32::from(RINGS));
            self.fill_rect(
                base.outset(spread - blur / 2.0),
                radius + spread,
                c.alpha(alpha * 1.6),
            );
        }
    }

    fn fill_circle(&mut self, center: Point, radius: f32, c: Color) {
        let ellipse = D2D1_ELLIPSE {
            point: point(center),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `fill_rect`.
        unsafe { self.target.FillEllipse(&ellipse, self.brush(c)) };
    }

    fn polyline(&mut self, points: &[Point], closed: bool, width: f32, c: Color) {
        let [first, rest @ ..] = points else { return };
        // SAFETY: the geometry is built and closed before it is drawn; all COM objects.
        unsafe {
            let Ok(path) = self.d2d.factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(point(*first), D2D1_FIGURE_BEGIN_HOLLOW);
            let rest: Vec<Vector2> = rest.iter().map(|p| point(*p)).collect();
            sink.AddLines(&rest);
            sink.EndFigure(if closed {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            if sink.Close().is_err() {
                return;
            }
            self.target
                .DrawGeometry(&path, self.brush(c), width, &self.round);
        }
    }

    fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, c: Color) {
        let ellipse = D2D1_ELLIPSE {
            point: point(center),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `fill_rect`.
        unsafe {
            self.target
                .DrawEllipse(&ellipse, self.brush(c), width, &self.round);
        }
    }

    fn measure(&mut self, text: &str, font: Font) -> TextSize {
        let Some(layout) = self.layout(if text.is_empty() { " " } else { text }, font) else {
            return TextSize::default();
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        // SAFETY: `metrics` is a valid out-parameter.
        let _ = unsafe { layout.GetMetrics(&mut metrics) };
        TextSize {
            width: if text.is_empty() {
                0.0
            } else {
                metrics.widthIncludingTrailingWhitespace
            },
            height: metrics.height,
        }
    }

    fn draw_text(&mut self, origin: Point, text: &str, font: Font, c: Color) {
        if text.is_empty() {
            return;
        }
        let Some(layout) = self.layout(text, font) else {
            return;
        };
        // SAFETY: as in `fill_rect`.
        unsafe {
            self.target.DrawTextLayout(
                point(origin),
                &layout,
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    fn push_clip(&mut self, r: Rect) {
        // SAFETY: every push is matched by a pop (`finish` pops what is left).
        unsafe {
            self.target
                .PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        self.open.push(Open::Clip);
    }

    fn pop_clip(&mut self) {
        if self.closed_early > 0 {
            self.closed_early -= 1;
            return;
        }
        if self.open.last() == Some(&Open::Clip) {
            self.open.pop();
            self.close(Open::Clip);
        }
    }

    fn fill_gradient(&mut self, r: Rect, radius: f32, from: Color, to: Color, across: bool) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: color(from),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: color(to),
            },
        ];
        let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: point(Point::new(r.x, r.y)),
            endPoint: point(if across {
                Point::new(r.r(), r.y)
            } else {
                Point::new(r.x, r.b())
            }),
        };
        let radius = radius.clamp(0.0, r.w.min(r.h) / 2.0);
        // SAFETY: target-owned resources made and used on the target's thread, between
        // BeginDraw and EndDraw; the stops and properties outlive the calls.
        unsafe {
            let Ok(collection) = self.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let properties: Option<*const D2D1_BRUSH_PROPERTIES> = None;
            let Ok(brush) = self
                .target
                .CreateLinearGradientBrush(&line, properties, &collection)
            else {
                return;
            };
            let rounded = D2D1_ROUNDED_RECT {
                rect: rect(r),
                radiusX: radius,
                radiusY: radius,
            };
            self.target.FillRoundedRectangle(&rounded, &brush);
        }
    }

    fn has_groups(&self) -> bool {
        true
    }

    fn push_group(&mut self, bounds: Rect, alpha: f32) {
        let parameters = D2D1_LAYER_PARAMETERS {
            contentBounds: rect(bounds),
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: alpha,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            // Text in the group stays ClearType, so it does not change look when the fade ends.
            layerOptions: D2D1_LAYER_OPTIONS_INITIALIZE_FOR_CLEARTYPE,
        };
        // SAFETY: drawing between BeginDraw and EndDraw; Direct2D makes the layer itself when
        // none is given (Windows 8 and later); `parameters` outlives the call and owns nothing.
        unsafe {
            self.target.PushLayer(&raw const parameters, None);
        }
        self.open.push(Open::Group);
    }

    fn pop_group(&mut self) {
        // Direct2D closes in order: a clip opened inside the group and still open goes first.
        while self.open.last() == Some(&Open::Clip) {
            self.open.pop();
            self.close(Open::Clip);
            self.closed_early += 1;
        }
        if self.open.last() == Some(&Open::Group) {
            self.open.pop();
            self.close(Open::Group);
        }
    }

    fn draw_text_zoomed(&mut self, origin: Point, text: &str, font: Font, zoom: f32, c: Color) {
        if (zoom - 1.0).abs() < 1e-3 {
            self.draw_text(origin, text, font, c);
            return;
        }
        if text.is_empty() {
            return;
        }
        let Some(layout) = self.layout(text, font) else {
            return;
        };
        // The run as it is shaped at rest, scaled about where it starts.
        let grown = Matrix3x2 {
            M11: zoom,
            M12: 0.0,
            M21: 0.0,
            M22: zoom,
            M31: origin.x * (1.0 - zoom),
            M32: origin.y * (1.0 - zoom),
        };
        // SAFETY: as in `fill_rect`; the transform is put back before anything else is drawn.
        unsafe {
            let mut before = Matrix3x2::default();
            self.target.GetTransform(&raw mut before);
            self.target.SetTransform(&raw const grown);
            self.target.DrawTextLayout(
                point(origin),
                &layout,
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            self.target.SetTransform(&raw const before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_renders_to_png_with_text_and_shapes() {
        let Ok(d2d) = D2d::new() else {
            unreachable!("Direct2D exists on every supported Windows")
        };
        let mut width = 0.0;
        let png = d2d.render_png(80, 40, 1.0, |canvas| {
            canvas.fill_rect(Rect::new(0.0, 0.0, 80.0, 40.0), 0.0, Color::WHITE);
            canvas.fill_rect(Rect::new(10.0, 10.0, 20.0, 20.0), 4.0, Color::BLACK);
            width = canvas.measure("Hosts", Font::ui(13.0)).width;
            canvas.draw_text(Point::new(40.0, 10.0), "Hi", Font::ui(13.0), Color::BLACK);
        });
        let Ok(png) = png else {
            unreachable!("the frame renders: {png:?}")
        };
        assert_eq!(&png[1..4], b"PNG");
        assert!(width > 20.0 && width < 60.0, "{width}");
    }

    /// The pixels of a PNG this renderer made: RGBA, row by row.
    fn pixels(png: &[u8]) -> tiny_skia::Pixmap {
        let Ok(pixmap) = tiny_skia::Pixmap::decode_png(png) else {
            unreachable!("our own PNG decodes")
        };
        pixmap
    }

    fn render(draw: impl FnOnce(&mut D2dCanvas<'_>)) -> tiny_skia::Pixmap {
        let Ok(d2d) = D2d::new() else {
            unreachable!("Direct2D exists on every supported Windows")
        };
        let Ok(png) = d2d.render_png(40, 40, 1.0, draw) else {
            unreachable!("the frame renders")
        };
        pixels(&png)
    }

    #[test]
    fn a_group_lands_faded_as_one_where_overlaps_do_not_show_through() {
        let frame = render(|canvas| {
            canvas.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 0.0, Color::BLACK);
            canvas.push_group(Rect::new(0.0, 0.0, 40.0, 40.0), 0.5);
            let square = Rect::new(10.0, 10.0, 10.0, 10.0);
            canvas.fill_rect(square, 0.0, Color::rgba(255, 0, 0, 255));
            canvas.fill_rect(square, 0.0, Color::WHITE);
            canvas.pop_group();
        });
        let Some(p) = frame.pixel(15, 15) else {
            unreachable!("inside the frame")
        };
        assert!(p.red() == p.green() && p.green() == p.blue(), "{p:?}");
        assert!((110..=145).contains(&p.red()), "{p:?}");
    }

    #[test]
    fn clips_opened_inside_a_group_close_with_it_and_the_frame_still_renders() {
        let frame = render(|canvas| {
            canvas.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 0.0, Color::BLACK);
            canvas.push_group(Rect::new(0.0, 0.0, 40.0, 40.0), 0.5);
            canvas.push_clip(Rect::new(0.0, 0.0, 20.0, 40.0));
            canvas.pop_group();
            // Its own end comes after the group's, and closes nothing else.
            canvas.pop_clip();
            canvas.fill_rect(Rect::new(30.0, 30.0, 10.0, 10.0), 0.0, Color::WHITE);
        });
        let Some(p) = frame.pixel(35, 35) else {
            unreachable!("inside the frame")
        };
        assert_eq!(p.red(), 255);
    }

    #[test]
    fn a_gradient_runs_from_one_color_to_the_other() {
        let frame = render(|canvas| {
            let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
            canvas.fill_gradient(rect, 0.0, Color::BLACK, Color::WHITE, false);
        });
        let (Some(top), Some(bottom)) = (frame.pixel(20, 2), frame.pixel(20, 37)) else {
            unreachable!("inside the frame")
        };
        assert!(top.red() < 60 && bottom.red() > 200, "{top:?} {bottom:?}");
    }

    #[test]
    fn zoomed_text_scales_from_where_it_starts() {
        let frame = render(|canvas| {
            canvas.fill_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 0.0, Color::BLACK);
            canvas.draw_text_zoomed(
                Point::new(2.0, 2.0),
                "MM",
                Font::ui(13.0),
                2.0,
                Color::WHITE,
            );
            // The transform was put back: this lands where it says.
            canvas.fill_rect(Rect::new(38.0, 38.0, 2.0, 2.0), 0.0, Color::WHITE);
        });
        let lit = frame.pixels().iter().filter(|p| p.red() > 128).count();
        assert!(lit > 60, "the letters show large: {lit} pixels");
        let Some(corner) = frame.pixel(39, 39) else {
            unreachable!("inside the frame")
        };
        assert_eq!(corner.red(), 255);
    }
}
