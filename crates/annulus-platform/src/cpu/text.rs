//! Text for the CPU backend: parley shapes and lays out, swash rasterizes glyphs, and the
//! glyphs are blended into the frame through the active clip.
//!
//! The engine knows only the fonts it is given, never system fonts, so golden tests render
//! identically on every OS. System font discovery (fontique) joins when the app needs it.

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{FontContext, FontFamily, Layout, LayoutContext, PositionedLayoutItem, StyleProperty};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Vector};
use tiny_skia::{Mask, Pixmap, PixmapPaint, Transform};
use annulus_ui::{Color, Point, Size};

/// The UI font bundled for golden tests: Fira Sans Regular, SIL OFL 1.1 (see
/// `fonts/FiraSans-OFL.txt`).
pub const TEST_FONT: &[u8] = include_bytes!("../../fonts/FiraSans-Regular.ttf");

/// The family name of [`TEST_FONT`].
pub const TEST_FONT_FAMILY: &str = "Fira Sans";

/// Shapes, measures and draws text with a fixed set of fonts.
pub struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    scaler: ScaleContext,
    family: String,
}

impl TextEngine {
    /// An engine that knows only `fonts` (TrueType or OpenType files) and uses `family` for
    /// every run.
    #[must_use]
    pub fn with_fonts(fonts: &[&'static [u8]], family: &str) -> Self {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        for font in fonts {
            collection.register_fonts(Blob::new(std::sync::Arc::new(*font)), None);
        }
        Self {
            fonts: FontContext {
                collection,
                source_cache: SourceCache::default(),
            },
            layouts: LayoutContext::new(),
            scaler: ScaleContext::new(),
            family: family.to_owned(),
        }
    }

    /// The size `text` takes at font `size` (DIPs), wrapped at `max_width` if given.
    pub fn measure(&mut self, text: &str, size: f32, max_width: Option<f32>) -> Size {
        let layout = self.layout(text, size, 1.0, max_width);
        Size::new(layout.width(), layout.height())
    }

    /// Draws one line of `text` with its baseline starting at `baseline` (device pixels) at
    /// `size` device pixels, clipped by `clip`.
    pub(crate) fn draw(
        &mut self,
        target: &mut Pixmap,
        baseline: Point,
        text: &str,
        size: f32,
        color: Color,
        clip: Option<&Mask>,
    ) {
        let layout = self.layout(text, size, 1.0, None);
        let first_baseline = layout
            .lines()
            .next()
            .map_or(0.0, |line| line.metrics().baseline);
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font = run.font();
                let Some(font_ref) = FontRef::from_index(font.data.as_ref(), font.index as usize)
                else {
                    continue;
                };
                let mut scaler = self
                    .scaler
                    .builder(font_ref)
                    .size(run.font_size())
                    .hint(false)
                    .build();
                for glyph in glyph_run.positioned_glyphs() {
                    let x = baseline.x + glyph.x;
                    let y = baseline.y - first_baseline + glyph.y;
                    let (x_floor, y_floor) = (x.floor(), y.floor());
                    let Ok(glyph_id) = u16::try_from(glyph.id) else {
                        continue;
                    };
                    let Some(image) = Render::new(&[Source::Outline])
                        .format(Format::Alpha)
                        .offset(Vector::new(x - x_floor, y - y_floor))
                        .render(&mut scaler, glyph_id)
                    else {
                        continue;
                    };
                    let Some(coverage) = tint(
                        &image.data,
                        image.placement.width,
                        image.placement.height,
                        color,
                    ) else {
                        continue;
                    };
                    #[allow(clippy::cast_possible_truncation)] // Pixel positions fit in i32.
                    let (left, top) = (
                        x_floor as i32 + image.placement.left,
                        y_floor as i32 - image.placement.top,
                    );
                    target.draw_pixmap(
                        left,
                        top,
                        coverage.as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        clip,
                    );
                }
            }
        }
    }

    fn layout(&mut self, text: &str, size: f32, scale: f32, max_width: Option<f32>) -> Layout<()> {
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, scale, true);
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(
            self.family.as_str().into(),
        )));
        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        layout
    }
}

/// A glyph image: `color` with each pixel's alpha scaled by the glyph's coverage,
/// premultiplied as tiny-skia expects.
fn tint(coverage: &[u8], width: u32, height: u32, color: Color) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    for (pixel, cover) in pixmap.data_mut().chunks_exact_mut(4).zip(coverage) {
        let alpha = u16::from(color.a) * u16::from(*cover) / 255;
        let premultiply =
            |channel: u8| u8::try_from(u16::from(channel) * alpha / 255).unwrap_or(u8::MAX);
        pixel[0] = premultiply(color.r);
        pixel[1] = premultiply(color.g);
        pixel[2] = premultiply(color.b);
        pixel[3] = u8::try_from(alpha).unwrap_or(u8::MAX);
    }
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_text_measures_wider_and_wrapping_makes_it_taller() {
        let mut engine = TextEngine::with_fonts(&[TEST_FONT], TEST_FONT_FAMILY);
        let short = engine.measure("ssh", 13.0, None);
        let long = engine.measure("ssh admin@example.com", 13.0, None);
        assert!(long.width > short.width && short.width > 0.0);
        let wrapped = engine.measure("ssh admin@example.com -p 2222", 13.0, Some(60.0));
        assert!(wrapped.height > long.height);
    }

    #[test]
    fn empty_text_has_no_width() {
        let mut engine = TextEngine::with_fonts(&[TEST_FONT], TEST_FONT_FAMILY);
        assert!(engine.measure("", 13.0, None).width.abs() < f32::EPSILON);
    }
}
