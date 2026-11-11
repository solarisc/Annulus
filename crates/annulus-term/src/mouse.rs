//! Mouse reports: the bytes a click, drag or wheel turn sends to a program that asked for the
//! mouse (`htop`, `btop`, `vim` with `mouse=a`, `tmux`), as xterm does.

use alacritty_terminal::term::TermMode;

use crate::keys::Mods;

/// A mouse button, the wheel counted as two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Left, usually.
    Left,
    /// The wheel button.
    Middle,
    /// Right, usually.
    Right,
    /// The wheel turned away from the user.
    WheelUp,
    /// The wheel turned toward the user.
    WheelDown,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }

    fn wheel(self) -> bool {
        matches!(self, Self::WheelUp | Self::WheelDown)
    }
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A button went down (a wheel turn is a press alone).
    Press(Button),
    /// A button came up.
    Release(Button),
    /// The pointer moved to another cell, with this button held, if any.
    Motion(Option<Button>),
}

/// Whether the program asked for mouse reports.
#[must_use]
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// The bytes that report `action` at `row` and `column` (from 0, top left) with `held` down,
/// or `None` when the program did not ask for it: no mouse mode, motion it did not ask for,
/// or, in the oldest encoding, a cell too far right or down to say.
#[must_use]
pub fn encode(
    action: Action,
    (row, column): (usize, usize),
    held: Mods,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !reporting(mode) {
        return None;
    }
    let (code, release) = match action {
        Action::Press(button) => (button.code(), false),
        // A wheel turn has no release.
        Action::Release(button) if button.wheel() => return None,
        Action::Release(button) => (button.code(), true),
        Action::Motion(button) => {
            let asked = match button {
                Some(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
                None => mode.contains(TermMode::MOUSE_MOTION),
            };
            if !asked {
                return None;
            }
            // No button held reads as "release" in the motion code, as xterm sends it.
            (button.map_or(3, Button::code) + 32, false)
        }
    };
    let code = code
        + if held.shift { 4 } else { 0 }
        + if held.alt { 8 } else { 0 }
        + if held.ctrl { 16 } else { 0 };
    let (x, y) = (column + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // The older encodings cannot name a released button: every release is button 3.
    let code = if release { 3 + (code & !3) } else { code };
    let mut bytes = b"\x1b[M".to_vec();
    if mode.contains(TermMode::UTF8_MOUSE) {
        for value in [code + 32, coordinate(x)?, coordinate(y)?] {
            let ch = char::from_u32(value)?;
            let mut buffer = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
        }
    } else {
        for value in [code + 32, coordinate(x)?, coordinate(y)?] {
            bytes.push(u8::try_from(value).ok()?);
        }
    }
    Some(bytes)
}

/// A 1-based position as the older encodings send it: offset by 32, at most 2047 (UTF-8) or
/// 255 (one byte, checked by the caller).
fn coordinate(position: usize) -> Option<u32> {
    let value = u32::try_from(position).ok()? + 32;
    (value <= 2047).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLICKS: TermMode = TermMode::MOUSE_REPORT_CLICK;
    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    fn sgr(mode: TermMode) -> TermMode {
        mode | TermMode::SGR_MOUSE
    }

    #[test]
    fn without_a_mouse_mode_nothing_is_reported() {
        let press = Action::Press(Button::Left);
        assert_eq!(encode(press, (0, 0), NONE, TermMode::empty()), None);
        assert_eq!(encode(press, (0, 0), NONE, TermMode::SGR_MOUSE), None);
    }

    #[test]
    fn sgr_reports_one_based_cells_and_names_the_released_button() {
        let mode = sgr(CLICKS);
        assert_eq!(
            encode(Action::Press(Button::Left), (4, 9), NONE, mode).unwrap(),
            b"\x1b[<0;10;5M"
        );
        assert_eq!(
            encode(Action::Release(Button::Right), (4, 9), NONE, mode).unwrap(),
            b"\x1b[<2;10;5m"
        );
        // Far past the old encoding's limit.
        assert_eq!(
            encode(Action::Press(Button::Middle), (300, 999), NONE, mode).unwrap(),
            b"\x1b[<1;1000;301M"
        );
    }

    #[test]
    fn the_old_encoding_sends_offset_bytes_and_release_as_button_three() {
        assert_eq!(
            encode(Action::Press(Button::Left), (0, 0), NONE, CLICKS).unwrap(),
            b"\x1b[M\x20\x21\x21"
        );
        assert_eq!(
            encode(Action::Release(Button::Right), (1, 2), NONE, CLICKS).unwrap(),
            b"\x1b[M\x23\x23\x22"
        );
        // Column 224 and on cannot fit in a byte: nothing rather than garbage.
        assert_eq!(
            encode(Action::Press(Button::Left), (0, 223), NONE, CLICKS),
            None
        );
        assert_eq!(
            encode(Action::Press(Button::Left), (0, 222), NONE, CLICKS).unwrap(),
            b"\x1b[M\x20\xff\x21"
        );
    }

    #[test]
    fn utf8_mode_writes_wide_coordinates_as_characters() {
        let mode = CLICKS | TermMode::UTF8_MOUSE;
        let bytes = encode(Action::Press(Button::Left), (0, 299), NONE, mode).unwrap();
        let mut expected = b"\x1b[M\x20".to_vec();
        expected.extend("\u{14c}\u{21}".as_bytes()); // 300 + 32 = 332 = U+014C.
        assert_eq!(bytes, expected);
    }

    #[test]
    fn the_wheel_presses_buttons_64_and_65_and_never_releases() {
        let mode = sgr(CLICKS);
        assert_eq!(
            encode(Action::Press(Button::WheelUp), (0, 0), NONE, mode).unwrap(),
            b"\x1b[<64;1;1M"
        );
        assert_eq!(
            encode(Action::Press(Button::WheelDown), (0, 0), NONE, mode).unwrap(),
            b"\x1b[<65;1;1M"
        );
        assert_eq!(
            encode(Action::Release(Button::WheelUp), (0, 0), NONE, mode),
            None
        );
    }

    #[test]
    fn modifiers_add_their_bits() {
        let mods = Mods {
            shift: true,
            alt: true,
            ctrl: true,
        };
        assert_eq!(
            encode(Action::Press(Button::Left), (0, 0), mods, sgr(CLICKS)).unwrap(),
            b"\x1b[<28;1;1M"
        );
    }

    #[test]
    fn motion_is_reported_only_in_the_modes_that_ask_for_it() {
        let drag = Action::Motion(Some(Button::Left));
        let hover = Action::Motion(None);
        assert_eq!(encode(drag, (0, 0), NONE, sgr(CLICKS)), None);
        assert_eq!(
            encode(drag, (2, 3), NONE, sgr(TermMode::MOUSE_DRAG)).unwrap(),
            b"\x1b[<32;4;3M"
        );
        assert_eq!(encode(hover, (0, 0), NONE, sgr(TermMode::MOUSE_DRAG)), None);
        assert_eq!(
            encode(hover, (2, 3), NONE, sgr(TermMode::MOUSE_MOTION)).unwrap(),
            b"\x1b[<35;4;3M"
        );
    }
}
