//! A plain copy of what the terminal shows, with colors resolved, for the view to draw. Taken
//! under the terminal lock, then drawn without holding it.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use crate::palette::Palette;

/// One visible cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character (a space for blanks; wide characters are followed by a spacer cell
    /// with `spacer` set).
    pub ch: char,
    /// Text color, after inverse and dim are applied.
    pub fg: Rgb,
    /// Background color, after inverse is applied.
    pub bg: Rgb,
    /// Bold text.
    pub bold: bool,
    /// Italic text.
    pub italic: bool,
    /// Any kind of underline.
    pub underline: bool,
    /// Strike-through.
    pub strikeout: bool,
    /// The right half of a wide character: draw nothing but the background.
    pub spacer: bool,
    /// Part of the mouse selection.
    pub selected: bool,
}

/// The visible screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    /// Rows top to bottom, each exactly `columns` cells.
    pub rows: Vec<Vec<Cell>>,
    /// Columns per row.
    pub columns: usize,
    /// Where to draw the cursor (row, column), or `None` when it is hidden or scrolled away.
    pub cursor: Option<(usize, usize)>,
    /// The background behind everything.
    pub background: Rgb,
    /// The cursor color.
    pub cursor_color: Rgb,
}

/// Copies the visible screen of `term`, resolving colors with `palette`.
#[must_use]
pub fn snapshot<T: EventListener>(term: &Term<T>, palette: &Palette) -> Screen {
    use alacritty_terminal::grid::Dimensions as _;
    let content = term.renderable_content();
    let columns = term.columns();
    let lines = term.screen_lines();
    let blank_bg = palette.resolve(Color::Named(NamedColor::Background), content.colors);
    let blank = Cell {
        ch: ' ',
        fg: palette.foreground,
        bg: blank_bg,
        bold: false,
        italic: false,
        underline: false,
        strikeout: false,
        spacer: false,
        selected: false,
    };
    let mut rows = vec![vec![blank; columns]; lines];
    let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
    for indexed in content.display_iter {
        let line = indexed.point.line.0 + offset;
        let (Ok(row), column) = (usize::try_from(line), indexed.point.column.0) else {
            continue;
        };
        let Some(slot) = rows.get_mut(row).and_then(|row| row.get_mut(column)) else {
            continue;
        };
        let cell = indexed.cell;
        let flags = cell.flags;
        let mut fg = palette.resolve(cell.fg, content.colors);
        let mut bg = palette.resolve(cell.bg, content.colors);
        if flags.contains(Flags::DIM) {
            fg = Rgb {
                r: fg.r / 3 * 2,
                g: fg.g / 3 * 2,
                b: fg.b / 3 * 2,
            };
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        let selected = content
            .selection
            .is_some_and(|range| range.contains(indexed.point));
        if selected {
            bg = palette.selection;
        }
        *slot = Cell {
            ch: if cell.c == '\0' { ' ' } else { cell.c },
            fg,
            bg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: flags.contains(Flags::STRIKEOUT),
            spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
            selected,
        };
    }
    let cursor_point = content.cursor.point;
    let cursor_visible =
        content.mode.contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden;
    let cursor = usize::try_from(cursor_point.line.0 + offset)
        .ok()
        .filter(|row| cursor_visible && *row < lines)
        .map(|row| (row, cursor_point.column.0));
    Screen {
        rows,
        columns,
        cursor,
        background: blank_bg,
        cursor_color: palette.resolve(Color::Named(NamedColor::Cursor), content.colors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    fn text(screen: &Screen, row: usize) -> String {
        screen.rows[row]
            .iter()
            .filter(|cell| !cell.spacer)
            .map(|cell| cell.ch)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn text_colors_and_cursor_land_where_the_program_put_them() {
        let mut term = Emulator::new(20, 4);
        term.advance(b"hello\r\n\x1b[31mred\x1b[0m");
        let screen = term.screen();
        assert_eq!(text(&screen, 0), "hello");
        assert_eq!(text(&screen, 1), "red");
        assert_eq!(screen.rows[1][0].fg, Palette::annulus_dark().ansi[1]);
        assert_eq!(screen.cursor, Some((1, 3)));
    }

    #[test]
    fn inverse_swaps_colors_and_hidden_cursor_is_not_drawn() {
        let mut term = Emulator::new(10, 2);
        term.advance(b"\x1b[7mX\x1b[?25l");
        let screen = term.screen();
        let palette = Palette::annulus_dark();
        assert_eq!(
            (screen.rows[0][0].fg, screen.rows[0][0].bg),
            (palette.background, palette.foreground)
        );
        assert_eq!(screen.cursor, None);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut term = Emulator::new(10, 2);
        term.advance("\u{4f60}a".as_bytes());
        let screen = term.screen();
        assert_eq!(screen.rows[0][0].ch, '\u{4f60}');
        assert!(screen.rows[0][1].spacer);
        assert_eq!(screen.rows[0][2].ch, 'a');
    }

    #[test]
    fn truecolor_and_scrolling_keep_every_row_full_width() {
        let mut term = Emulator::new(8, 3);
        for line in 0..10 {
            term.advance(format!("\x1b[38;2;1;2;3mline {line}\r\n").as_bytes());
        }
        let screen = term.screen();
        assert!(screen.rows.iter().all(|row| row.len() == 8));
        assert_eq!(text(&screen, 1), "line 9");
        assert_eq!(screen.rows[1][0].fg, Rgb { r: 1, g: 2, b: 3 });
    }
}
