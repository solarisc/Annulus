//! The first widget set: buttons, toggles, text fields, choices, labels and
//! tooltips. Each draws from the theme palette, reports [`Action`](crate::Action)s and
//! declares an accessibility role.

mod button;
mod choice;
mod label;
mod text_field;
mod toggle;

#[cfg(test)]
mod tests;

pub use button::{Button, ButtonKind, IconButton, SplitButton};
pub use choice::{Segmented, Select};
pub use label::{Label, Tooltip};
pub use text_field::{FieldStyle, TextField};
pub use toggle::{Checkbox, Radio, Switch};

use annulus_config::tokens::{Palette, Rgb};

use crate::geometry::{Color, Rect};
use crate::input::{Event, Key, Modifiers};

pub(crate) fn color(rgb: Rgb) -> Color {
    Color::rgb(rgb.r, rgb.g, rgb.b)
}

/// The palette color with the best contrast on `background`, for text on filled controls.
pub(crate) fn text_on(background: Rgb, palette: &Palette) -> Rgb {
    [palette.text_primary, palette.bg_app, palette.bg_surface]
        .into_iter()
        .max_by(|a, b| a.contrast(background).total_cmp(&b.contrast(background)))
        .unwrap_or(palette.text_primary)
}

/// The baseline that centers one line of `size` text vertically in `bounds`. 0.7 is a
/// typical cap-height ratio for UI fonts.
pub(crate) fn centered_baseline(bounds: Rect, size: f32) -> f32 {
    bounds.origin.y + (bounds.size.height + size * 0.7) / 2.0
}

/// Whether `event` is a key press of `key` with no modifiers.
pub(crate) fn is_key(event: &Event, key: Key) -> bool {
    matches!(event, Event::KeyDown { key: pressed, modifiers } if *pressed == key && *modifiers == Modifiers::default())
}

/// Whether `event` presses Enter or Space, the keys that activate buttons and toggles.
pub(crate) fn is_activation_key(event: &Event) -> bool {
    is_key(event, Key::Enter) || is_key(event, Key::Space)
}

/// Whether `event` releases the primary button inside `bounds`: a completed click.
pub(crate) fn is_click(event: &Event, bounds: Rect) -> bool {
    matches!(event, Event::PointerUp { position, button: crate::input::Button::Primary } if bounds.contains(*position))
}
