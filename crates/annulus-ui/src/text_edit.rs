//! The editing core shared by every text field: grapheme-aware movement, selection, undo and
//! redo, IME composition, and secret fields (DESIGN §7.2, SECURITY.md).
//!
//! Positions are byte offsets that always sit on a grapheme cluster boundary, so the cursor
//! can never split an emoji or a letter with its accents.
//!
//! Secret text lives only in zeroizing buffers. Every edit builds a new buffer and drops the
//! old one, which wipes it; growing a `String` in place would leave stale copies of the
//! password in freed memory. Secret fields also keep no undo history, refuse to copy, and
//! show bullets.

use std::fmt;

use unicode_segmentation::UnicodeSegmentation;
use zeroize::Zeroizing;

/// The most bytes a field holds. Pasting more is cut at a grapheme boundary, so a hostile
/// clipboard cannot exhaust memory.
pub const MAX_BYTES: usize = 64 * 1024;

/// Undo steps kept per plain field.
const MAX_UNDO: usize = 100;

/// What a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Ordinary text.
    Plain,
    /// A password, passphrase or PIN.
    Secret,
}

/// Text being composed by an input method, shown at the cursor but not yet part of the text.
#[derive(Clone, PartialEq, Eq)]
struct Composition {
    text: Zeroizing<String>,
}

/// One snapshot for undo.
struct Snapshot {
    text: Zeroizing<String>,
    cursor: usize,
    anchor: usize,
}

/// The state of one text field.
pub struct TextEditor {
    kind: FieldKind,
    text: Zeroizing<String>,
    /// Where the caret is.
    cursor: usize,
    /// The other end of the selection; equal to `cursor` when nothing is selected.
    anchor: usize,
    composition: Option<Composition>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl fmt::Debug for TextEditor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("TextEditor");
        debug.field("kind", &self.kind);
        match self.kind {
            FieldKind::Plain => debug.field("text", &self.text.as_str()),
            FieldKind::Secret => debug.field("text", &"[redacted]"),
        };
        debug
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .finish()
    }
}

impl TextEditor {
    /// An empty field.
    #[must_use]
    pub fn new(kind: FieldKind) -> Self {
        Self {
            kind,
            text: Zeroizing::new(String::new()),
            cursor: 0,
            anchor: 0,
            composition: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The field's text. For a secret field this is the secret itself; pass it on only to
    /// the code that needs it, and never log it.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What the field shows: the text with any IME composition at the cursor, and every
    /// grapheme of a secret replaced by a bullet.
    #[must_use]
    pub fn display_text(&self) -> String {
        let shown = match &self.composition {
            Some(composition) => {
                let (before, after) = self.text.split_at(self.cursor);
                format!("{before}{}{after}", composition.text.as_str())
            }
            None => self.text.to_string(),
        };
        match self.kind {
            FieldKind::Plain => shown,
            FieldKind::Secret => "\u{2022}".repeat(shown.graphemes(true).count()),
        }
    }

    /// The caret position, as a byte offset.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected byte range, start before end. Empty when nothing is selected.
    #[must_use]
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    /// The selected text for the clipboard. Always `None` for a secret field.
    #[must_use]
    pub fn selected_text(&self) -> Option<&str> {
        let range = self.selection();
        (self.kind == FieldKind::Plain && !range.is_empty()).then(|| &self.text[range])
    }

    /// Replaces the selection (or inserts at the caret) with `text`, cut to fit
    /// [`MAX_BYTES`]. Control characters other than tab are dropped, since a field is one line.
    pub fn insert(&mut self, text: &str) {
        let range = self.selection();
        let room = MAX_BYTES.saturating_sub(self.text.len() - range.len());
        let clean: Zeroizing<String> = Zeroizing::new(
            text.chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect(),
        );
        let mut take = 0;
        for (start, grapheme) in clean.grapheme_indices(true) {
            if start + grapheme.len() > room {
                break;
            }
            take = start + grapheme.len();
        }
        if range.is_empty() && take == 0 {
            return;
        }
        self.replace(range, &clean[..take]);
    }

    /// Deletes the selection, or the grapheme before the caret.
    pub fn backspace(&mut self) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace(range, "");
        } else if let Some(start) = self.previous_boundary(self.cursor) {
            self.replace(start..self.cursor, "");
        }
    }

    /// Deletes the selection, or the grapheme after the caret.
    pub fn delete(&mut self) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace(range, "");
        } else if let Some(end) = self.next_boundary(self.cursor) {
            self.replace(self.cursor..end, "");
        }
    }

    /// Moves the caret one grapheme left, extending the selection if `extend`. Without
    /// `extend`, a selection collapses to its start instead.
    pub fn move_left(&mut self, extend: bool) {
        let range = self.selection();
        let target = if !extend && !range.is_empty() {
            range.start
        } else {
            self.previous_boundary(self.cursor).unwrap_or(self.cursor)
        };
        self.move_to(target, extend);
    }

    /// Moves the caret one grapheme right; see [`Self::move_left`].
    pub fn move_right(&mut self, extend: bool) {
        let range = self.selection();
        let target = if !extend && !range.is_empty() {
            range.end
        } else {
            self.next_boundary(self.cursor).unwrap_or(self.cursor)
        };
        self.move_to(target, extend);
    }

    /// Moves the caret to the start of the previous word. In a secret field, words are not
    /// revealed: the caret goes to the start of the field.
    pub fn move_word_left(&mut self, extend: bool) {
        let target = match self.kind {
            FieldKind::Secret => 0,
            FieldKind::Plain => self
                .text
                .unicode_word_indices()
                .map(|(start, _)| start)
                .filter(|start| *start < self.cursor)
                .last()
                .unwrap_or(0),
        };
        self.move_to(target, extend);
    }

    /// Moves the caret to the end of the next word; see [`Self::move_word_left`].
    pub fn move_word_right(&mut self, extend: bool) {
        let target = match self.kind {
            FieldKind::Secret => self.text.len(),
            FieldKind::Plain => self
                .text
                .unicode_word_indices()
                .map(|(start, word)| start + word.len())
                .find(|end| *end > self.cursor)
                .unwrap_or(self.text.len()),
        };
        self.move_to(target, extend);
    }

    /// Moves the caret to the start of the field.
    pub fn move_home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    /// Moves the caret to the end of the field.
    pub fn move_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    /// Shows `text` as the input method's in-progress composition, or clears it with an
    /// empty string. The field's text does not change until [`Self::commit_composition`].
    pub fn set_composition(&mut self, text: &str) {
        self.composition = (!text.is_empty()).then(|| Composition {
            text: Zeroizing::new(text.to_owned()),
        });
    }

    /// Inserts the composition as typed text.
    pub fn commit_composition(&mut self) {
        if let Some(composition) = self.composition.take() {
            self.insert(&composition.text);
        }
    }

    /// Undoes the last edit. Returns whether there was one. Secret fields have no history.
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.redo.push(current);
        self.restore(snapshot);
        true
    }

    /// Redoes the last undone edit. Returns whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.undo.push(current);
        self.restore(snapshot);
        true
    }

    /// Wipes the text and history, as when a dialog with a password field closes.
    pub fn clear(&mut self) {
        self.text = Zeroizing::new(String::new());
        self.cursor = 0;
        self.anchor = 0;
        self.composition = None;
        self.undo.clear();
        self.redo.clear();
    }

    fn replace(&mut self, range: std::ops::Range<usize>, with: &str) {
        if self.kind == FieldKind::Plain {
            let snapshot = self.snapshot();
            self.undo.push(snapshot);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        let mut next = Zeroizing::new(String::with_capacity(
            self.text.len() - range.len() + with.len(),
        ));
        next.push_str(&self.text[..range.start]);
        next.push_str(with);
        next.push_str(&self.text[range.end..]);
        // Dropping the old buffer wipes it.
        self.text = next;
        self.cursor = range.start + with.len();
        self.anchor = self.cursor;
    }

    fn move_to(&mut self, target: usize, extend: bool) {
        self.cursor = target;
        if !extend {
            self.anchor = target;
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
    }

    fn previous_boundary(&self, from: usize) -> Option<usize> {
        self.text[..from]
            .grapheme_indices(true)
            .next_back()
            .map(|(start, _)| start)
    }

    fn next_boundary(&self, from: usize) -> Option<usize> {
        self.text[from..]
            .graphemes(true)
            .next()
            .map(|grapheme| from + grapheme.len())
    }
}

#[cfg(test)]
#[path = "text_edit_tests.rs"]
mod tests;
