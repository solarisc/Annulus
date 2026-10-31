//! The whole text of a terminal, scrollback included, for session logs (F-LOG-001), and
//! its screen rows, to move the view to one (an agent's command, F-AUTO-004; the scroll bar).

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point};

/// Every line from the oldest scrollback line to the bottom of the screen, as plain text
/// (no colors or escape sequences), without trailing blank lines.
#[must_use]
pub fn transcript<T>(term: &Term<T>) -> String {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(
        term.bottommost_line(),
        Column(term.columns().saturating_sub(1)),
    );
    let text = term.bounds_to_string(start, end);
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Rows below count screen rows from the oldest row of the scrollback: a line that wrapped
/// takes a row for each piece, as on screen (unlike [`transcript`] lines).
///
/// The row shown at the top of the screen now.
#[must_use]
pub fn top_row<T>(term: &Term<T>) -> usize {
    let grid = term.grid();
    grid.history_size().saturating_sub(grid.display_offset())
}

/// The row the cursor is on: where typing goes in.
#[must_use]
pub fn cursor_row<T>(term: &Term<T>) -> usize {
    let line = usize::try_from(term.grid().cursor.point.line.0).unwrap_or(0);
    term.grid().history_size() + line
}

/// All rows: the scrollback and the screen.
#[must_use]
pub fn total_rows<T>(term: &Term<T>) -> usize {
    term.grid().history_size() + term.screen_lines()
}

/// Scrolls the view so `row` is at the top of the screen, or as near as the scrollback
/// allows (a row on the newest screen leaves the view at the bottom).
pub fn scroll_to_row<T: EventListener>(term: &mut Term<T>, row: usize) {
    let back = term.grid().history_size().saturating_sub(row);
    term.scroll_display(Scroll::Bottom);
    term.scroll_display(Scroll::Delta(i32::try_from(back).unwrap_or(i32::MAX)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    #[test]
    fn the_transcript_includes_scrollback_and_drops_colors() {
        let mut emulator = Emulator::new(20, 3);
        let mut text = String::new();
        for line in 0..10 {
            text += "\x1b[31mline ";
            text += &line.to_string();
            text += "\x1b[0m\r\n";
        }
        emulator.advance(text.as_bytes());
        let transcript = transcript(emulator.term_mut());
        assert!(transcript.starts_with("line 0\n"), "{transcript:?}");
        assert!(transcript.ends_with("line 9\n"));
        assert!(!transcript.contains('\x1b'));
    }

    #[test]
    fn rows_count_wrapped_pieces_and_a_row_scrolls_to_the_top_and_back() {
        let mut emulator = Emulator::new(10, 3);
        emulator.advance(b"short\r\n");
        // 25 characters on a 10-column screen: three rows, one line of transcript.
        emulator.advance(b"0123456789abcdefghijKLMNO\r\n");
        let marked = cursor_row(emulator.term_mut());
        assert_eq!(marked, 4, "1 + 3 rows above the cursor");
        emulator.advance(b"$ typed here\r\n");
        for line in 0..6 {
            emulator.advance(format!("out {line}\r\n").as_bytes());
        }
        assert_eq!(
            transcript(emulator.term_mut())
                .lines()
                .position(|line| line.starts_with("$ typed")),
            Some(2),
            "the transcript joins the wrapped line, so its numbers are not rows"
        );
        let bottom = top_row(emulator.term_mut());
        assert_eq!(total_rows(emulator.term_mut()), bottom + 3);

        scroll_to_row(emulator.term_mut(), marked);
        assert_eq!(top_row(emulator.term_mut()), marked);
        let first: String = emulator.screen().rows[0]
            .iter()
            .map(|cell| cell.ch)
            .collect();
        assert!(first.starts_with("$ typed he"), "{first:?}");

        scroll_to_row(emulator.term_mut(), 99);
        assert_eq!(
            top_row(emulator.term_mut()),
            bottom,
            "past the end is the bottom"
        );
    }
}
