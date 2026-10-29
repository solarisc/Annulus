//! Translating winit's input events into the toolkit's own events. Pure functions, so the
//! mapping is tested without a window.

use annulus_ui::{Button, Key, Modifiers};
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

/// DIPs one wheel notch scrolls.
pub const LINE_SCROLL: f32 = 40.0;

/// The toolkit key for a winit logical key, or `None` for keys the toolkit ignores.
#[must_use]
pub fn key(logical: &WinitKey) -> Option<Key> {
    match logical {
        WinitKey::Named(named) => Some(match named {
            NamedKey::Tab => Key::Tab,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Space => Key::Space,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowUp => Key::ArrowUp,
            NamedKey::ArrowDown => Key::ArrowDown,
            NamedKey::ArrowLeft => Key::ArrowLeft,
            NamedKey::ArrowRight => Key::ArrowRight,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            _ => return None,
        }),
        WinitKey::Character(text) => {
            let mut chars = text.chars();
            let first = chars.next()?;
            // Shortcuts match on the unshifted, lowercase character.
            chars
                .next()
                .is_none()
                .then(|| Key::Character(first.to_ascii_lowercase()))
        }
        _ => None,
    }
}

/// The toolkit modifiers for winit's modifier state.
#[must_use]
pub fn modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// The toolkit button for a winit mouse button; back/forward and others are ignored.
#[must_use]
pub fn button(button: MouseButton) -> Option<Button> {
    match button {
        MouseButton::Left => Some(Button::Primary),
        MouseButton::Right => Some(Button::Secondary),
        MouseButton::Middle => Some(Button::Middle),
        _ => None,
    }
}

/// Whether a key or button state is a press.
#[must_use]
pub fn pressed(state: ElementState) -> bool {
    state == ElementState::Pressed
}

/// Wheel movement in DIPs along the scrolling axis, positive to reveal content further
/// down. Pixel deltas arrive in physical pixels and are divided by `scale`.
#[must_use]
pub fn wheel(delta: MouseScrollDelta, scale: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Scroll deltas are small.
    match delta {
        MouseScrollDelta::LineDelta(_, y) => -y * LINE_SCROLL,
        MouseScrollDelta::PixelDelta(position) => (-position.y / scale.max(0.1)) as f32,
    }
}

/// Text typed by a key press, without control characters (those are keys, not text).
#[must_use]
pub fn typed_text(text: Option<&str>) -> Option<String> {
    let text: String = text?.chars().filter(|c| !c.is_control()).collect();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    #[test]
    fn named_and_character_keys_map_and_shortcut_letters_are_lowercase() {
        assert_eq!(key(&WinitKey::Named(NamedKey::Tab)), Some(Key::Tab));
        assert_eq!(
            key(&WinitKey::Named(NamedKey::ArrowLeft)),
            Some(Key::ArrowLeft)
        );
        assert_eq!(
            key(&WinitKey::Character("A".into())),
            Some(Key::Character('a'))
        );
        assert_eq!(key(&WinitKey::Named(NamedKey::F24)), None);
        assert_eq!(key(&WinitKey::Character("\u{e9}t".into())), None);
    }

    #[test]
    fn modifier_state_maps_one_to_one() {
        let state = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(
            modifiers(state),
            Modifiers {
                shift: true,
                ctrl: true,
                alt: false,
                meta: false
            }
        );
    }

    #[test]
    fn wheel_lines_and_pixels_become_dips_scrolling_down_positive() {
        assert!(
            (wheel(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0) - LINE_SCROLL).abs() < f32::EPSILON
        );
        let pixels = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -60.0));
        assert!((wheel(pixels, 2.0) - 30.0).abs() < f32::EPSILON);
    }

    #[test]
    fn typed_text_drops_control_characters() {
        assert_eq!(typed_text(Some("a")), Some("a".to_owned()));
        assert_eq!(typed_text(Some("\r")), None);
        assert_eq!(typed_text(Some("\u{8}")), None);
        assert_eq!(typed_text(None), None);
    }

    #[test]
    fn only_the_three_main_buttons_map() {
        assert_eq!(button(MouseButton::Left), Some(Button::Primary));
        assert_eq!(button(MouseButton::Back), None);
    }
}
