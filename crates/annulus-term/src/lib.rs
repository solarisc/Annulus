//! Terminal emulation on `alacritty_terminal` ([ADR-0016](../../docs/adr/0016-alacritty-terminal-core.md)):
//! our palette, a drawable snapshot of the screen, and key encoding. Bytes in, state out; no
//! I/O here.

#![forbid(unsafe_code)]

pub mod find;
pub mod keys;
pub mod links;
pub mod mouse;
pub mod palette;
pub mod select;
pub mod snapshot;
pub mod themes;
pub mod transcript;

pub use alacritty_terminal;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

pub use palette::Palette;
pub use snapshot::{Cell, Screen, snapshot};

/// Lines of scrollback a terminal keeps until it is told another number.
pub const SCROLLBACK_LINES: usize = 10_000;
/// The most lines of scrollback a terminal keeps: `alacritty_terminal`'s own limit.
pub const SCROLLBACK_MAX: usize = 100_000;

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
    config_keeping(SCROLLBACK_LINES)
}

/// The configuration of a terminal that keeps `lines` of scrollback, at most
/// [`SCROLLBACK_MAX`].
fn config_keeping(lines: usize) -> Config {
    Config {
        scrolling_history: lines.min(SCROLLBACK_MAX),
        ..Config::default()
    }
}

/// Keeps `lines` of scrollback from now on (F-TERM-004): fewer than it holds drops the oldest
/// at once.
pub fn keep_scrollback<T: alacritty_terminal::event::EventListener>(
    term: &mut Term<T>,
    lines: usize,
) {
    term.set_options(config_keeping(lines));
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

    /// Draws with `palette` from now on (the terminal theme chosen in Settings).
    pub fn set_palette(&mut self, palette: &Palette) {
        if &self.palette != palette {
            self.palette = palette.clone();
        }
    }

    /// The terminal itself, for selection and search.
    pub fn term_mut(&mut self) -> &mut Term<VoidListener> {
        &mut self.term
    }
}

/// Forgets the scrollback; the screen stays as it is (F-TERM-014).
pub fn clear_scrollback<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>) {
    term.grid_mut().clear_history();
}

/// Resets the terminal as `reset` (RIS) does: modes, colors, the screen and the
/// scrollback, for a program that left it in a mess (F-TERM-014).
pub fn reset<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>) {
    use alacritty_terminal::vte::ansi::Handler as _;
    term.reset_state();
    term.grid_mut().clear_history();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_drops_the_scrollback_and_reset_also_the_screen_and_modes() {
        use alacritty_terminal::grid::Dimensions as _;
        let mut emulator = Emulator::new(20, 4);
        for n in 0..30 {
            emulator.advance(format!("line {n}\r\n").as_bytes());
        }
        emulator.advance(b"\x1b[?1049h");
        emulator.advance(b"\x1b[?1049l");
        emulator.advance(b"kept");
        assert!(emulator.term_mut().grid().history_size() > 0);
        clear_scrollback(emulator.term_mut());
        assert_eq!(emulator.term_mut().grid().history_size(), 0);
        assert!(
            crate::transcript::transcript(emulator.term_mut()).contains("kept"),
            "the screen stays"
        );

        emulator.advance(b"\x1b[?25l\x1b[31mred");
        reset(emulator.term_mut());
        assert!(
            !crate::transcript::transcript(emulator.term_mut()).contains("kept"),
            "the screen is cleared"
        );
        assert!(
            emulator
                .term_mut()
                .mode()
                .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR),
            "modes are back to their defaults"
        );
    }

    #[test]
    fn the_scrollback_keeps_as_many_lines_as_asked_and_a_smaller_number_drops_the_oldest() {
        use alacritty_terminal::grid::Dimensions as _;
        let mut emulator = Emulator::new(20, 4);
        let print = |emulator: &mut Emulator, from: usize| {
            for n in from..from + 200 {
                emulator.advance(format!("line {n}\r\n").as_bytes());
            }
        };
        keep_scrollback(emulator.term_mut(), 50);
        print(&mut emulator, 0);
        assert_eq!(emulator.term_mut().grid().history_size(), 50);
        keep_scrollback(emulator.term_mut(), 10);
        assert_eq!(emulator.term_mut().grid().history_size(), 10, "cut at once");
        let text = crate::transcript::transcript(emulator.term_mut());
        assert!(
            !text.contains("line 150\n") && text.contains("line 199"),
            "the newest stay: {text}"
        );
        // Asked for more than alacritty keeps, it keeps its most.
        keep_scrollback(emulator.term_mut(), usize::MAX);
        print(&mut emulator, 200);
        assert!(emulator.term_mut().grid().history_size() >= 200);
    }

    #[test]
    fn a_replayed_terminal_draws_in_the_chosen_theme() {
        let mut emulator = Emulator::new(10, 2);
        emulator.advance(b"\x1b[31mred");
        let nord = themes::theme("Nord").unwrap();
        assert_eq!(
            emulator.screen().background,
            Palette::annulus_dark().background
        );
        emulator.set_palette(&nord);
        let screen = emulator.screen();
        assert_eq!(screen.background, nord.background);
        assert_eq!(screen.rows[0][0].fg, nord.ansi[1]);
    }

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
