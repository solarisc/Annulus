//! Find in scrollback (F-TERM-007): plain text or regex, case-sensitive or not, stepping
//! through matches across the whole history and highlighting those on screen.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

/// Characters with a meaning in regex syntax, escaped for plain-text searches.
const REGEX_META: &str = "\\.+*?()|[]{}^$#&-~";

/// The regex for `query`: escaped unless `regex`, with case sensitivity set explicitly
/// (Alacritty would otherwise switch on smart case).
#[must_use]
pub fn pattern(query: &str, regex: bool, case_sensitive: bool) -> String {
    let flags = if case_sensitive { "(?-i)" } else { "(?i)" };
    if regex {
        return format!("{flags}{query}");
    }
    let mut escaped = String::with_capacity(query.len() * 2);
    for c in query.chars() {
        if REGEX_META.contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    format!("{flags}{escaped}")
}

/// Part of a match on one visible row, in screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highlight {
    /// Row from the top of what is shown.
    pub row: usize,
    /// First column.
    pub start: usize,
    /// Last column, inclusive.
    pub end: usize,
    /// Part of the match the last step landed on.
    pub current: bool,
}

/// A search over one terminal.
pub struct Find {
    regex: RegexSearch,
    current: Option<Match>,
}

impl Find {
    /// A search for `query`.
    ///
    /// # Errors
    /// The query is not a valid regex (or, for plain text, too large); the message says why.
    pub fn new(query: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        let regex = RegexSearch::new(&pattern(query, regex, case_sensitive))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            regex,
            current: None,
        })
    }

    /// Steps to the next match (older text when `backwards`), wrapping around the history,
    /// and scrolls it into view. `None` when nothing matches.
    pub fn step<T: EventListener>(&mut self, term: &mut Term<T>, backwards: bool) -> Option<Match> {
        let (direction, side) = if backwards {
            (Direction::Left, Side::Right)
        } else {
            (Direction::Right, Side::Left)
        };
        let offset = i32::try_from(term.grid().display_offset()).unwrap_or(i32::MAX);
        let origin = match (&self.current, backwards) {
            (Some(found), false) => found.end().add(term, Boundary::None, 1),
            (Some(found), true) => found.start().sub(term, Boundary::None, 1),
            // From the bottom of the view upwards, or from its top downwards.
            (None, true) => Point::new(
                Line(i32::try_from(term.screen_lines()).unwrap_or(i32::MAX) - 1 - offset),
                Column(term.columns() - 1),
            ),
            (None, false) => Point::new(Line(-offset), Column(0)),
        };
        let found = term.search_next(&mut self.regex, origin, direction, side, None)?;
        term.scroll_to_point(*found.start());
        self.current = Some(found.clone());
        Some(found)
    }

    /// The matches on screen, split per row.
    pub fn visible<T>(&mut self, term: &Term<T>) -> Vec<Highlight> {
        let offset = i32::try_from(term.grid().display_offset()).unwrap_or(i32::MAX);
        let lines = i32::try_from(term.screen_lines()).unwrap_or(i32::MAX);
        let last_column = term.columns().saturating_sub(1);
        let top = Point::new(Line(-offset), Column(0));
        let bottom = Point::new(Line(lines - 1 - offset), Column(last_column));
        let current = self.current.clone();
        let mut highlights = Vec::new();
        for found in RegexIter::new(top, bottom, Direction::Right, term, &mut self.regex) {
            let is_current = current.as_ref() == Some(&found);
            for line in found.start().line.0..=found.end().line.0 {
                let Ok(row) = usize::try_from(line + offset) else {
                    continue;
                };
                if row >= term.screen_lines() {
                    continue;
                }
                let start = if line == found.start().line.0 {
                    found.start().column.0
                } else {
                    0
                };
                let end = if line == found.end().line.0 {
                    found.end().column.0
                } else {
                    last_column
                };
                highlights.push(Highlight {
                    row,
                    start,
                    end,
                    current: is_current,
                });
            }
        }
        highlights
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    fn emulator_with(text: &str) -> Emulator {
        let mut emulator = Emulator::new(20, 4);
        emulator.advance(text.as_bytes());
        emulator
    }

    #[test]
    fn plain_text_is_escaped_and_case_is_explicit() {
        assert_eq!(pattern("a.b", false, false), "(?i)a\\.b");
        assert_eq!(pattern("a.b", true, true), "(?-i)a.b");
        assert!(Find::new("(", true, false).is_err());
        assert!(Find::new("(", false, false).is_ok());
    }

    #[test]
    fn matches_on_screen_are_highlighted_and_steps_land_on_them() {
        let mut emulator = emulator_with("foo bar\r\nFOO baz\r\nnothing");
        let mut find = Find::new("foo", false, false).unwrap();
        let rows: Vec<usize> = find
            .visible(emulator.term_mut())
            .iter()
            .map(|h| h.row)
            .collect();
        assert_eq!(rows, [0, 1]);

        let first = find.step(emulator.term_mut(), true).unwrap();
        assert_eq!(
            first.start().line.0,
            1,
            "backwards starts from the newest text"
        );
        let second = find.step(emulator.term_mut(), true).unwrap();
        assert_eq!(second.start().line.0, 0);
        let current: Vec<_> = find
            .visible(emulator.term_mut())
            .into_iter()
            .filter(|h| h.current)
            .collect();
        assert_eq!(
            current,
            [Highlight {
                row: 0,
                start: 0,
                end: 2,
                current: true
            }]
        );

        let mut exact = Find::new("FOO", false, true).unwrap();
        assert_eq!(exact.visible(emulator.term_mut()).len(), 1);
    }

    #[test]
    fn a_match_in_the_scrollback_is_scrolled_into_view() {
        let mut text = String::from("needle\r\n");
        for line in 0..30 {
            text += "line ";
            text += &line.to_string();
            text += "\r\n";
        }
        let mut emulator = emulator_with(&text);
        let mut find = Find::new("needle", false, false).unwrap();
        assert_eq!(find.visible(emulator.term_mut()), []);
        assert!(find.step(emulator.term_mut(), true).is_some());
        assert!(emulator.term_mut().grid().display_offset() > 0);
        assert_eq!(find.visible(emulator.term_mut()).len(), 1);
    }

    #[test]
    fn nothing_found_is_none() {
        let mut emulator = emulator_with("abc");
        let mut find = Find::new("xyz", false, false).unwrap();
        assert!(find.step(emulator.term_mut(), false).is_none());
    }
}
