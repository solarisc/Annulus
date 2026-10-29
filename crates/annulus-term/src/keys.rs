//! Key encoding: the bytes a key press sends to the program, as xterm does. Text input
//! (letters, symbols, IME results) is sent as typed; this covers the keys that are not text.

use alacritty_terminal::term::TermMode;

/// A key that is not plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Enter / Return.
    Enter,
    /// Backspace.
    Backspace,
    /// Tab.
    Tab,
    /// Escape.
    Escape,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow right.
    Right,
    /// Arrow left.
    Left,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Insert.
    Insert,
    /// Forward delete.
    Delete,
    /// F1 to F12.
    F(u8),
    /// A letter or symbol pressed with Ctrl, e.g. Ctrl+C.
    Ctrl(char),
}

/// Modifier keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Shift.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// Control.
    pub ctrl: bool,
}

impl Mods {
    /// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl, or `None` without modifiers.
    fn param(self) -> Option<u8> {
        let value = u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (value > 0).then_some(value + 1)
    }
}

/// The bytes for `key` with `mods`, in terminal `mode` (application cursor keys change the
/// arrows). `None` for combinations that send nothing.
#[must_use]
pub fn encode(key: Key, mods: Mods, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let bytes = match key {
        Key::Enter => alt_prefixed(mods, b"\r"),
        Key::Backspace => alt_prefixed(mods, if mods.ctrl { b"\x08" } else { b"\x7f" }),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefixed(mods, b"\t"),
        Key::Escape => alt_prefixed(mods, b"\x1b"),
        Key::Up => cursor(b'A', mods, app_cursor),
        Key::Down => cursor(b'B', mods, app_cursor),
        Key::Right => cursor(b'C', mods, app_cursor),
        Key::Left => cursor(b'D', mods, app_cursor),
        Key::Home => cursor(b'H', mods, app_cursor),
        Key::End => cursor(b'F', mods, app_cursor),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => match mods.param() {
            None => vec![0x1b, b'O', b'P' + n - 1],
            Some(param) => format!("\x1b[1;{param}{}", char::from(b'P' + n - 1)).into_bytes(),
        },
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)], mods),
        Key::F(_) => return None,
        Key::Ctrl(c) => {
            let control = control_byte(c)?;
            alt_prefixed(mods, &[control])
        }
    };
    Some(bytes)
}

/// The C0 control byte for Ctrl+`c`: letters and `@[\]^_` map to 0x00–0x1F, Space to NUL,
/// `?` to DEL.
fn control_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_uppercase();
    match c {
        '@'..='_' => u8::try_from(c).ok().map(|b| b - b'@'),
        ' ' | '2' => Some(0),
        '?' => Some(0x7f),
        '3' => Some(0x1b),
        '4' => Some(0x1c),
        '5' => Some(0x1d),
        '6' => Some(0x1e),
        '7' | '/' => Some(0x1f),
        '8' => Some(0x7f),
        _ => None,
    }
}

fn alt_prefixed(mods: Mods, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn cursor(letter: u8, mods: Mods, app_cursor: bool) -> Vec<u8> {
    match mods.param() {
        Some(param) => format!("\x1b[1;{param}{}", char::from(letter)).into_bytes(),
        None if app_cursor => vec![0x1b, b'O', letter],
        None => vec![0x1b, b'[', letter],
    }
}

fn tilde(code: u8, mods: Mods) -> Vec<u8> {
    match mods.param() {
        Some(param) => format!("\x1b[{code};{param}~").into_bytes(),
        None => format!("\x1b[{code}~").into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    fn enc(key: Key, mods: Mods) -> Vec<u8> {
        encode(key, mods, TermMode::empty()).unwrap()
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(enc(Key::Up, NONE), b"\x1b[A");
        assert_eq!(
            encode(Key::Up, NONE, TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(enc(Key::Left, Mods { ctrl: true, ..NONE }), b"\x1b[1;5D");
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        assert_eq!(enc(Key::Ctrl('c'), NONE), [0x03]);
        assert_eq!(enc(Key::Ctrl('d'), NONE), [0x04]);
        assert_eq!(enc(Key::Ctrl('['), NONE), [0x1b]);
        assert_eq!(enc(Key::Ctrl(' '), NONE), [0x00]);
        assert_eq!(
            enc(Key::Ctrl('a'), Mods { alt: true, ..NONE }),
            [0x1b, 0x01]
        );
        assert_eq!(encode(Key::Ctrl('\u{e9}'), NONE, TermMode::empty()), None);
    }

    #[test]
    fn editing_and_function_keys_use_xterm_sequences() {
        assert_eq!(enc(Key::Enter, NONE), b"\r");
        assert_eq!(enc(Key::Backspace, NONE), b"\x7f");
        assert_eq!(
            enc(
                Key::Tab,
                Mods {
                    shift: true,
                    ..NONE
                }
            ),
            b"\x1b[Z"
        );
        assert_eq!(enc(Key::Delete, NONE), b"\x1b[3~");
        assert_eq!(
            enc(
                Key::PageUp,
                Mods {
                    shift: true,
                    ..NONE
                }
            ),
            b"\x1b[5;2~"
        );
        assert_eq!(enc(Key::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE), b"\x1b[24~");
        assert_eq!(encode(Key::F(13), NONE, TermMode::empty()), None);
    }
}
