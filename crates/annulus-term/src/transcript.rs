//! The whole text of a terminal, scrollback included, for session logs (F-LOG-001), and
//! moving the view to one of its lines (an agent's command, F-AUTO-004).

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

/// The [`transcript`] line shown at the top of the screen now.
#[must_use]
pub fn top_line<T>(term: &Term<T>) -> usize {
    let grid = term.grid();
    grid.history_size().saturating_sub(grid.display_offset())
}

/// Scrolls the view so [`transcript`] line `line` is at the top of the screen, or as near as
/// the scrollback allows (a line on the newest screen leaves the view at the bottom).
pub fn scroll_to_line<T: EventListener>(term: &mut Term<T>, line: usize) {
    let back = term.grid().history_size().saturating_sub(line);
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
    fn a_transcript_line_scrolls_to_the_top_of_the_screen_and_back() {
        let mut emulator = Emulator::new(20, 3);
        for line in 0..10 {
            emulator.advance(format!("line {line}\r\n").as_bytes());
        }
        let lines: Vec<String> = transcript(emulator.term_mut())
            .lines()
            .map(String::from)
            .collect();
        let bottom = top_line(emulator.term_mut());
        assert!(bottom > 2, "the screen shows the newest lines");

        scroll_to_line(emulator.term_mut(), 2);
        assert_eq!(top_line(emulator.term_mut()), 2);
        assert_eq!(lines[2], "line 2", "the index is the transcript's");

        scroll_to_line(emulator.term_mut(), 99);
        assert_eq!(
            top_line(emulator.term_mut()),
            bottom,
            "past the end is the bottom"
        );
    }
}
