//! The whole text of a terminal, scrollback included, for session logs (F-LOG-001).

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
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
}
