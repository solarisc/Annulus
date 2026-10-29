//! Links in terminal output (F-TERM-008): OSC 8 hyperlinks, and URLs found in the text. Only
//! schemes on the allow-list are ever opened.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Point};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::viewport_to_point;

/// Schemes a click may open; anything else (`file:`, `javascript:`, custom handlers) is shown
/// but not opened.
const ALLOWED: [&str; 3] = ["https://", "http://", "mailto:"];

/// URLs in plain text: an allowed scheme, then anything but spaces, quotes, brackets that
/// usually end a URL, and control characters.
const URL: &str = "(?i)(https?://|mailto:)[^\\x00-\\x1F\\x7F-\\x9F<>\"\\s{}^`\u{27E8}\u{27E9}]+";

/// Whether `url` may be opened.
#[must_use]
pub fn allowed(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ALLOWED.iter().any(|scheme| lower.starts_with(scheme)) && url.len() > "mailto:".len()
}

/// A link under the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Where it points: the real target for OSC 8 links, the text for plain URLs.
    pub url: String,
    /// It came from an OSC 8 escape, so the visible text may differ from the target.
    pub explicit: bool,
}

/// Punctuation that ends a sentence more often than a URL.
fn trim_trailing(url: &str) -> &str {
    let mut url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
    // A closing parenthesis stays only when the URL opened one.
    while url.ends_with(')') && url.matches('(').count() < url.matches(')').count() {
        url = &url[..url.len() - 1];
    }
    url
}

/// The link at screen cell (`row`, `column`), if any.
#[must_use]
pub fn link_at<T>(term: &Term<T>, row: usize, column: usize) -> Option<Link> {
    if row >= term.screen_lines() || column >= term.columns() {
        return None;
    }
    let point = viewport_to_point(
        term.grid().display_offset(),
        Point::new(row, Column(column)),
    );
    if let Some(hyperlink) = term.grid()[point].hyperlink() {
        return Some(Link {
            url: hyperlink.uri().to_owned(),
            explicit: true,
        });
    }
    let mut regex = RegexSearch::new(URL).ok()?;
    let line = point.line;
    let start = Point::new(line, Column(0));
    let end = Point::new(line, Column(term.columns() - 1));
    let found = RegexIter::new(start, end, Direction::Right, term, &mut regex)
        .find(|found| found.contains(&point))?;
    let text = term.bounds_to_string(*found.start(), *found.end());
    let url = trim_trailing(text.trim_end());
    // The click must land on the URL, not on punctuation trimmed from its end.
    let end_column = found.start().column.0 + url.chars().count();
    (point.column.0 < end_column).then(|| Link {
        url: url.to_owned(),
        explicit: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Emulator;

    fn emulator_with(text: &str) -> Emulator {
        let mut emulator = Emulator::new(60, 4);
        emulator.advance(text.as_bytes());
        emulator
    }

    #[test]
    fn plain_urls_are_found_without_trailing_punctuation() {
        let mut emulator = emulator_with("see https://example.com/a_(b). ok");
        let term = emulator.term_mut();
        let link = link_at(term, 0, 10).unwrap();
        assert_eq!(link.url, "https://example.com/a_(b)");
        assert!(!link.explicit);
        assert_eq!(link_at(term, 0, 0), None, "plain words are not links");
        assert_eq!(
            link_at(term, 0, 30),
            None,
            "the trimmed period is not part of it"
        );
    }

    #[test]
    fn osc8_links_report_their_real_target() {
        let mut emulator =
            emulator_with("\x1b]8;;https://real.example/\x1b\\click me\x1b]8;;\x1b\\");
        let link = link_at(emulator.term_mut(), 0, 2).unwrap();
        assert_eq!(link.url, "https://real.example/");
        assert!(link.explicit);
    }

    #[test]
    fn only_web_and_mail_schemes_may_open() {
        assert!(allowed("https://example.com"));
        assert!(allowed("HTTP://example.com"));
        assert!(allowed("mailto:a@example.com"));
        assert!(!allowed("file:///etc/passwd"));
        assert!(!allowed("javascript:alert(1)"));
        assert!(!allowed("mailto:"));
    }
}
