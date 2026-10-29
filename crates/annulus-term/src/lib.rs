//! Terminal emulation on `alacritty_terminal` ([ADR-0016](../../docs/adr/0016-alacritty-terminal-core.md)):
//! our palette, a drawable snapshot of the screen, and key encoding. Bytes in, state out; no
//! I/O here.

#![forbid(unsafe_code)]

pub mod keys;
pub mod palette;
pub mod select;
pub mod snapshot;

pub use alacritty_terminal;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

pub use palette::Palette;
pub use snapshot::{Cell, Screen, snapshot};

/// Lines of scrollback kept per terminal.
pub const SCROLLBACK_LINES: usize = 10_000;

/// A terminal size in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// Columns.
    pub columns: usize,
    /// Rows.
    pub rows: usize,
}

impl Size {
    /// A size of at least one cell each way.
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns: columns.max(1),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// The terminal configuration Annulus uses.
#[must_use]
pub fn config() -> Config {
    Config {
        scrolling_history: SCROLLBACK_LINES,
        ..Config::default()
    }
}

/// A terminal with no program attached: bytes in, screen out. Sessions use
/// `alacritty_terminal`'s event loop instead; this drives tests and session-log replay.
pub struct Emulator {
    term: Term<VoidListener>,
    parser: Processor,
    palette: Palette,
}

impl Emulator {
    /// An empty terminal of `columns` by `rows`.
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Self {
        let term = Term::new(config(), &Size::new(columns, rows), VoidListener);
        Self {
            term,
            parser: Processor::new(),
            palette: Palette::annulus_dark(),
        }
    }

    /// Feeds program output.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Changes the size; text reflows.
    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.term.resize(Size::new(columns, rows));
    }

    /// What the terminal shows now.
    #[must_use]
    pub fn screen(&self) -> Screen {
        snapshot(&self.term, &self.palette)
    }

    /// The terminal itself, for selection and search.
    pub fn term_mut(&mut self) -> &mut Term<VoidListener> {
        &mut self.term
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_selects_text_and_highlights_it() {
        let mut term = Emulator::new(20, 3);
        term.advance(b"hello world\r\nsecond line");
        select::start(term.term_mut(), 0, 6, select::SelectionKind::Characters);
        select::extend(term.term_mut(), 1, 5);
        assert_eq!(
            select::text(term.term_mut()).as_deref(),
            Some("world\nsecond")
        );
        let screen = term.screen();
        assert!(screen.rows[0][6].selected && !screen.rows[0][5].selected);
        assert_eq!(screen.rows[0][6].bg, Palette::annulus_dark().selection);
        select::clear(term.term_mut());
        assert_eq!(select::text(term.term_mut()), None);
    }

    #[test]
    fn double_click_selects_the_word_under_the_pointer() {
        let mut term = Emulator::new(30, 2);
        term.advance(b"ssh admin@prod-db -p 22");
        select::start(term.term_mut(), 0, 6, select::SelectionKind::Words);
        select::extend(term.term_mut(), 0, 6);
        assert_eq!(
            select::text(term.term_mut()).as_deref(),
            Some("admin@prod-db")
        );
    }

    #[test]
    fn resizing_reflows_and_hostile_input_never_breaks_the_grid() {
        let mut term = Emulator::new(10, 3);
        term.advance(b"0123456789abcdef");
        term.resize(20, 3);
        let screen = term.screen();
        let first: String = screen.rows[0].iter().map(|cell| cell.ch).collect();
        assert!(first.starts_with("0123456789abcdef"), "{first:?}");

        // Garbage, truncated escapes and absurd parameters.
        term.advance(b"\x1b[999999;999999H\x1b[38;5;9999m\x1b]8;;\x07\x1b[?1049h\xff\xfe\x1b[");
        term.resize(1, 1);
        term.resize(300, 100);
        let screen = term.screen();
        assert_eq!(screen.rows.len(), 100);
        assert!(screen.rows.iter().all(|row| row.len() == 300));
    }
}
