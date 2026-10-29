//! The Windows renderer ([ADR-0017] decision 3): Direct2D shapes and DirectWrite text with
//! the system fonts (Segoe UI Variable, Cascadia Mono). It draws on any `ID2D1RenderTarget`:
//! a window's, or a WIC bitmap with no window at all, which is how CI photographs the
//! Windows look of every scene.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![allow(unsafe_code)]

use std::collections::HashMap;

use annulus_ui::gfx::{Canvas, Color, Family, Font, Point, Rect, TextSize, Weight};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES,
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
use windows_numerics::Vector2;

/// The Direct2D and DirectWrite factories and the fonts they found; make once and keep.
pub struct D2d {
    factory: ID2D1Factory1,
    write: IDWriteFactory,
    ui_family: HSTRING,
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
        let mono_family = pick(&["Cascadia Mono", "Consolas"]);
        Ok(Self {
            factory,
            write,
            ui_family,
            mono_family,
        })
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
    clips: u32,
    formats: HashMap<(Family, u32, Weight), IDWriteTextFormat>,
    layouts: HashMap<(String, Family, u32, Weight), IDWriteTextLayout>,
}

impl<'a> D2dCanvas<'a> {
    /// A canvas on `target`, whose DPI is `96 * scale`.
    ///
    /// # Errors
    /// When the brush or stroke style cannot be made.
    pub fn new(d2d: &'a D2d, target: ID2D1RenderTarget, scale: f32) -> windows::core::Result<Self> {
        // SAFETY: target-owned resources made on the target's thread.
        let (brush, round) = unsafe {
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
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
            clips: 0,
            formats: HashMap::new(),
            layouts: HashMap::new(),
        })
    }

    /// Pops any clip left open, before `EndDraw`.
    pub fn finish(&mut self) {
        while self.clips > 0 {
            self.pop_clip();
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
        self.clips += 1;
    }

    fn pop_clip(&mut self) {
        if self.clips > 0 {
            // SAFETY: a clip is open.
            unsafe { self.target.PopAxisAlignedClip() };
            self.clips -= 1;
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
}
