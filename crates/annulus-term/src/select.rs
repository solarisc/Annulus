//! Mouse selection on the visible screen, in screen coordinates (row 0 at the top of what is
//! shown, even when scrolled back).

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::viewport_to_point;

/// How a selection grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character by character (a drag).
    Characters,
    /// Whole words (a double-click).
    Words,
    /// Whole lines (a triple-click).
    Lines,
}

fn point<T>(term: &Term<T>, row: usize, column: usize) -> Point {
    viewport_to_point(
        term.grid().display_offset(),
        Point::new(row, Column(column)),
    )
}

/// Starts a selection at screen cell (`row`, `column`).
pub fn start<T>(term: &mut Term<T>, row: usize, column: usize, kind: SelectionKind) {
    let ty = match kind {
        SelectionKind::Characters => SelectionType::Simple,
        SelectionKind::Words => SelectionType::Semantic,
        SelectionKind::Lines => SelectionType::Lines,
    };
    let at = point(term, row, column);
    term.selection = Some(Selection::new(ty, at, Side::Left));
}

/// Extends the selection to screen cell (`row`, `column`), inclusive.
pub fn extend<T>(term: &mut Term<T>, row: usize, column: usize) {
    let at = point(term, row, column);
    if let Some(selection) = term.selection.as_mut() {
        selection.update(at, Side::Right);
    }
}

/// The selected text, or `None` when nothing is selected.
#[must_use]
pub fn text<T: EventListener>(term: &Term<T>) -> Option<String> {
    term.selection_to_string().filter(|text| !text.is_empty())
}

/// Removes the selection.
pub fn clear<T>(term: &mut Term<T>) {
    term.selection = None;
}
