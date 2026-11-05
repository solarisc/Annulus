//! Terminal colors: our own "Annulus Dark" scheme for the 16 ANSI colors (UI-SPEC §8,
//! ADR-0010), the standard xterm 256-color cube and grays, and resolution of a cell's color.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// The colors a terminal draws with when the program does not override them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// ANSI colors 0–15: black, red, green, yellow, blue, magenta, cyan, white, then the
    /// bright versions.
    pub ansi: [Rgb; 16],
    /// Default text.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// The cursor block.
    pub cursor: Rgb,
    /// Behind selected text.
    pub selection: Rgb,
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

impl Palette {
    /// "Annulus Dark": graphite background, teal accent, readable on every ANSI color.
    #[must_use]
    pub fn annulus_dark() -> Self {
        Self {
            ansi: [
                rgb(0x1C, 0x1F, 0x22),
                rgb(0xE5, 0x53, 0x4B),
                rgb(0x4C, 0xC2, 0x7A),
                rgb(0xE0, 0xA4, 0x3A),
                rgb(0x4D, 0x9D, 0xE0),
                rgb(0xB7, 0x7E, 0xDB),
                rgb(0x2B, 0xB3, 0xA3),
                rgb(0xC9, 0xCE, 0xD3),
                rgb(0x5C, 0x64, 0x6C),
                rgb(0xF0, 0x7A, 0x72),
                rgb(0x72, 0xD8, 0x98),
                rgb(0xF2, 0xC2, 0x6B),
                rgb(0x7A, 0xB8, 0xEE),
                rgb(0xCF, 0xA2, 0xEC),
                rgb(0x5F, 0xD3, 0xC4),
                rgb(0xF4, 0xF6, 0xF8),
            ],
            foreground: rgb(0xEE, 0xEC, 0xE8),
            background: rgb(0x16, 0x15, 0x13),
            cursor: rgb(0x2E, 0xB5, 0xA5),
            selection: rgb(0x1F, 0x5C, 0x56),
        }
    }

    /// The color of indexed color `index` (0–255) when the program did not redefine it.
    #[must_use]
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                // The 6x6x6 cube: levels 0, 95, 135, 175, 215, 255.
                let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
                let n = index - 16;
                Rgb {
                    r: level(n / 36),
                    g: level((n / 6) % 6),
                    b: level(n % 6),
                }
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb {
                    r: gray,
                    g: gray,
                    b: gray,
                }
            }
        }
    }

    /// Resolves a cell color: the program's overrides (OSC 4/10/11) in `overrides` first,
    /// then this palette.
    #[must_use]
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Indexed(index) => {
                overrides[usize::from(index)].unwrap_or_else(|| self.indexed(index))
            }
            Color::Named(named) => overrides[named].unwrap_or_else(|| self.named(named)),
        }
    }

    fn named(&self, named: NamedColor) -> Rgb {
        let index = named as usize;
        if let Ok(ansi) = u8::try_from(index)
            && ansi < 16
        {
            return self.ansi[usize::from(ansi)];
        }
        match named {
            NamedColor::Foreground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => dim(self.foreground),
            NamedColor::BrightForeground => self.ansi[15],
            other => {
                // DimBlack..DimWhite: faint versions of the 8 normal colors.
                match (other as usize).checked_sub(NamedColor::DimBlack as usize) {
                    Some(index) if index < 8 => dim(self.ansi[index]),
                    _ => self.foreground,
                }
            }
        }
    }
}

/// Two thirds of a color's brightness, for faint (SGR 2) text.
fn dim(color: Rgb) -> Rgb {
    let scale = |c: u8| u8::try_from(u16::from(c) * 2 / 3).unwrap_or(c);
    Rgb {
        r: scale(color.r),
        g: scale(color.g),
        b: scale(color.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_color_table_matches_xterm() {
        let palette = Palette::annulus_dark();
        assert_eq!(palette.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(palette.indexed(21), Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(palette.indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            palette.indexed(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(palette.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            palette.indexed(255),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
        assert_eq!(palette.indexed(1), palette.ansi[1]);
    }

    #[test]
    fn program_overrides_win_over_the_palette() {
        let palette = Palette::annulus_dark();
        let mut overrides = Colors::default();
        let pink = Rgb {
            r: 255,
            g: 105,
            b: 180,
        };
        overrides[1] = Some(pink);
        assert_eq!(palette.resolve(Color::Indexed(1), &overrides), pink);
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Red), &overrides),
            pink
        );
        assert_eq!(
            palette.resolve(Color::Indexed(2), &overrides),
            palette.ansi[2]
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Foreground), &Colors::default()),
            palette.foreground
        );
    }
}
