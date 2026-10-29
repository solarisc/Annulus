//! Helpers shared by this crate's tests.

use crate::geometry::Size;
use crate::widget::TextMeasure;

/// Measures text with fixed advances: every character is half the font size wide and a line
/// is 1.25 times the font size high, so layout tests don't depend on fonts (ADR-0012 §6).
pub(crate) struct FixedAdvance;

impl TextMeasure for FixedAdvance {
    fn measure(&mut self, text: &str, size: f32, _max_width: Option<f32>) -> Size {
        #[allow(clippy::cast_precision_loss)] // Test strings are short.
        let chars = text.chars().count() as f32;
        Size::new(chars * size * 0.5, size * 1.25)
    }
}
