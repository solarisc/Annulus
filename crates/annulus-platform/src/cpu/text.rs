//! Text for the CPU renderer: parley shapes and lays out, swash rasterizes glyphs into a
//! coverage cache, and glyphs are blended straight into the frame.
//!
//! The engine knows only the fonts it is given, never system fonts, so golden images render
//! identically on every OS.

use std::collections::HashMap;

use annulus_ui::gfx::{Color, Family, Font, Point, Rect, TextSize};
use parley::fontique::{Collection, CollectionOptions, SourceCache};
use parley::{
    FontContext, FontFamily, FontFamilyName, FontWeight, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Vector};
use tiny_skia::Pixmap;

/// Fira Sans Regular, SIL OFL 1.1 (`fonts/FiraSans-OFL.txt`).
pub const UI_REGULAR: &[u8] = include_bytes!("../../fonts/FiraSans-Regular.ttf");
/// Fira Sans Bold, SIL OFL 1.1.
pub const UI_BOLD: &[u8] = include_bytes!("../../fonts/FiraSans-Bold.ttf");
/// Fira Mono Medium, SIL OFL 1.1 (`fonts/FiraMono-OFL.txt`).
pub const MONO: &[u8] = include_bytes!("../../fonts/FiraMono-Medium.ttf");

const UI_FAMILY: &str = "Fira Sans";
const MONO_FAMILY: &str = crate::fonts::DEFAULT_MONO;

/// A rasterized glyph: its coverage and where it sits relative to the pen.
struct Glyph {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    coverage: Vec<u8>,
}

/// Which glyph image: font, glyph, size and quarter-pixel horizontal phase.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u16,
    size: u32,
    phase: u8,
}

/// Shapes, measures and draws text with the bundled fonts.
pub struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    scaler: ScaleContext,
    glyphs: HashMap<GlyphKey, Option<Glyph>>,
    ui_family: Option<String>,
    mono_family: Option<String>,
    revision: u64,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// An engine with the bundled UI and monospace fonts.
    #[must_use]
    pub fn new() -> Self {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let (revision, data) = crate::fonts::snapshot();
        for font in data {
            collection.register_fonts(font, None);
        }
        Self {
            fonts: FontContext {
                collection,
                source_cache: SourceCache::default(),
            },
            layouts: LayoutContext::new(),
            scaler: ScaleContext::new(),
            glyphs: HashMap::new(),
            ui_family: None,
            mono_family: None,
            revision,
        }
    }

    /// Applies family preferences and any font registry changes without reading the disk.
    pub fn set_families(&mut self, ui: Option<&str>, mono: Option<&str>) {
        let revision = crate::fonts::revision();
        if self.ui_family.as_deref() == ui
            && self.mono_family.as_deref() == mono
            && revision == self.revision
        {
            return;
        }
        self.ui_family = ui.map(str::to_owned);
        self.mono_family = mono.map(str::to_owned);
        if revision != self.revision {
            let (_, data) = crate::fonts::snapshot();
            let mut collection = Collection::new(CollectionOptions {
                shared: false,
                system_fonts: false,
            });
            for font in data {
                collection.register_fonts(font, None);
            }
            self.fonts.collection = collection;
            self.glyphs.clear();
            self.revision = revision;
        }
    }

    /// The size of `text` on one line, in the units of `font.size`.
    pub fn measure(&mut self, text: &str, font: Font) -> TextSize {
        let layout = self.layout(text, font, font.size);
        TextSize {
            width: if text.is_empty() { 0.0 } else { layout.width() },
            height: layout.height(),
        }
    }

    /// Draws one line of `text` with its line box's top-left at `origin` (device pixels), at
    /// `size` device pixels, inside `clip` (device pixels).
    #[allow(clippy::too_many_arguments)] // One glyph run's every input.
    pub(crate) fn draw(
        &mut self,
        target: &mut Pixmap,
        origin: Point,
        text: &str,
        font: Font,
        size: f32,
        color: Color,
        clip: Rect,
    ) {
        if text.is_empty() || !(size.is_finite() && size > 0.0) {
            return;
        }
        let layout = self.layout(text, font, size);
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let data = run.font();
                let Some(font_ref) = FontRef::from_index(data.data.as_ref(), data.index as usize)
                else {
                    continue;
                };
                let run_size = run.font_size();
                for glyph in glyph_run.positioned_glyphs() {
                    let x = origin.x + glyph.x;
                    let y = origin.y + glyph.y;
                    let Ok(id) = u16::try_from(glyph.id) else {
                        continue;
                    };
                    let x_floor = x.floor();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // A quarter-pixel phase, 0..=3.
                    let phase = (((x - x_floor) * 4.0).round() as u8) % 4;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // Sizes are small positive numbers.
                    let key = GlyphKey {
                        font: data.data.id(),
                        index: data.index,
                        glyph: id,
                        size: (run_size * 64.0).round() as u32,
                        phase,
                    };
                    let scaler = &mut self.scaler;
                    let cached = self.glyphs.entry(key).or_insert_with(|| {
                        rasterize(scaler, font_ref, run_size, id, f32::from(phase) / 4.0)
                    });
                    if let Some(image) = cached {
                        #[allow(clippy::cast_possible_truncation)] // Pixel positions fit.
                        let (left, top) =
                            (x_floor as i32 + image.left, y.round() as i32 - image.top);
                        blend(target, image, left, top, color, clip);
                    }
                }
            }
        }
    }

    fn layout(&mut self, text: &str, font: Font, size: f32) -> Layout<()> {
        let mut builder = self
            .layouts
            .ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(size));
        let family = match font.family {
            Family::Ui | Family::Display => self.ui_family.as_deref().unwrap_or(UI_FAMILY),
            Family::Mono => self.mono_family.as_deref().unwrap_or(MONO_FAMILY),
        };
        let fallback = if font.family == Family::Mono {
            MONO_FAMILY
        } else {
            UI_FAMILY
        };
        let families = [
            FontFamilyName::named(family),
            FontFamilyName::named(fallback),
            FontFamilyName::named(crate::fonts::SYMBOL_FAMILY),
        ];
        builder.push_default(StyleProperty::FontFamily(FontFamily::List(
            std::borrow::Cow::Borrowed(&families),
        )));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(
            font.weight.value(),
        )));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout
    }
}

fn rasterize(
    scaler: &mut ScaleContext,
    font: FontRef<'_>,
    size: f32,
    glyph: u16,
    phase: f32,
) -> Option<Glyph> {
    let mut scaler = scaler.builder(font).size(size).hint(false).build();
    let image = Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .offset(Vector::new(phase, 0.0))
        .render(&mut scaler, glyph)?;
    Some(Glyph {
        left: image.placement.left,
        top: image.placement.top,
        width: image.placement.width,
        height: image.placement.height,
        coverage: image.data,
    })
}

/// Blends `color` into `target` through the glyph's coverage, source-over on premultiplied
/// pixels, inside `clip`.
fn blend(target: &mut Pixmap, glyph: &Glyph, left: i32, top: i32, color: Color, clip: Rect) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    // Pixmap sizes and clip edges fit in i32.
    let (x0, y0, x1, y1) = (
        left.max(clip.x.floor() as i32).max(0),
        top.max(clip.y.floor() as i32).max(0),
        (left + glyph.width as i32)
            .min(clip.r().ceil() as i32)
            .min(target.width() as i32),
        (top + glyph.height as i32)
            .min(clip.b().ceil() as i32)
            .min(target.height() as i32),
    );
    let stride = target.width() as usize;
    let data = target.data_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            #[allow(clippy::cast_sign_loss)] // x >= left and y >= top here.
            let cover =
                glyph.coverage[((y - top) as usize) * glyph.width as usize + (x - left) as usize];
            if cover == 0 {
                continue;
            }
            // Slightly heavier coverage reads better for light text on dark backgrounds.
            let cover = f32::from(cover) / 255.0;
            let alpha = cover.powf(0.85) * f32::from(color.a) / 255.0;
            #[allow(clippy::cast_sign_loss)] // Clipped to the pixmap above.
            let at = (y as usize * stride + x as usize) * 4;
            let source = [color.r, color.g, color.b];
            for channel in 0..3 {
                let dst = f32::from(data[at + channel]);
                data[at + channel] =
                    to_byte(f32::from(source[channel]) * alpha + dst * (1.0 - alpha));
            }
            let dst_alpha = f32::from(data[at + 3]);
            data[at + 3] = to_byte(255.0 * alpha + dst_alpha * (1.0 - alpha));
        }
    }
}

fn to_byte(value: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped first.
    let byte = value.round().clamp(0.0, 255.0) as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::*;
    use annulus_ui::gfx::Weight;

    #[test]
    fn longer_text_is_wider_bold_is_wider_still_and_empty_has_no_width() {
        let mut engine = TextEngine::new();
        let font = Font::ui(13.0);
        let short = engine.measure("ssh", font);
        let long = engine.measure("ssh admin@example.com", font);
        assert!(long.width > short.width && short.width > 0.0);
        let bold = engine.measure("ssh admin@example.com", font.weight(Weight::Bold));
        assert!(bold.width > long.width);
        let empty = engine.measure("", font);
        assert!(empty.width.abs() < f32::EPSILON && empty.height > 10.0);
    }

    #[test]
    fn nerd_symbols_shape_to_real_glyphs_with_both_bundled_terminal_families() {
        for family in ["JetBrains Mono", "Fira Mono"] {
            let mut engine = TextEngine::new();
            engine.set_families(None, Some(family));
            for symbol in ["\u{e0b0}", "\u{f120}", "\u{f0001}"] {
                let layout = engine.layout(symbol, Font::mono(14.0), 14.0);
                let mut ids = Vec::new();
                for line in layout.lines() {
                    for item in line.items() {
                        if let PositionedLayoutItem::GlyphRun(run) = item {
                            ids.extend(run.positioned_glyphs().map(|glyph| glyph.id));
                        }
                    }
                }
                assert!(
                    !ids.is_empty() && ids.iter().all(|id| *id != 0),
                    "{family}: {symbol:?} = {ids:?}"
                );
            }
        }
    }

    #[test]
    fn changing_the_selected_family_changes_measurement_and_missing_fonts_fall_back() {
        let mut engine = TextEngine::new();
        let text = "The quick brown fox";
        let ui = engine.measure(text, Font::ui(14.0)).width;
        engine.set_families(Some("JetBrains Mono"), None);
        let mono = engine.measure(text, Font::ui(14.0)).width;
        assert!((ui - mono).abs() > 1.0);
        engine.set_families(Some("No Such Font"), None);
        assert!((ui - engine.measure(text, Font::ui(14.0)).width).abs() < 0.01);
    }

    #[test]
    fn monospace_characters_share_one_width() {
        let mut engine = TextEngine::new();
        let font = Font::mono(13.0);
        let narrow = engine.measure("iiii", font).width;
        let wide = engine.measure("WWWW", font).width;
        assert!((narrow - wide).abs() < 0.01, "{narrow} vs {wide}");
    }
}
