//! Password and passphrase fields whose text lives only in zeroizing memory (SECURITY.md).
//!
//! [`SecretText`] is an egui text buffer: every edit builds a new zeroizing string and drops
//! the old one, which wipes it, so no stale copy of the secret stays behind in the buffer.
//! [`secret_field`] shows it masked and clears egui's undo history after every frame, since
//! that history would otherwise keep plain copies.

use std::fmt;
use std::ops::Range;

use egui::text::CharIndex;
use egui::{Response, TextBuffer, TextEdit, Ui};
use zeroize::Zeroizing;

/// A secret being typed.
#[derive(Default)]
pub struct SecretText(Zeroizing<String>);

impl SecretText {
    /// An empty secret.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The secret itself. Hand it only to the code that needs it, and never log it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether nothing has been typed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn rebuild(&mut self, before: &str, middle: &str, after: &str) {
        let mut next = Zeroizing::new(String::with_capacity(
            before.len() + middle.len() + after.len(),
        ));
        next.push_str(before);
        next.push_str(middle);
        next.push_str(after);
        // Dropping the old buffer wipes it.
        self.0 = next;
    }

    fn byte_at(&self, index: CharIndex) -> usize {
        self.0
            .char_indices()
            .nth(index.0)
            .map_or(self.0.len(), |(byte, _)| byte)
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText([redacted])")
    }
}

impl TextBuffer for SecretText {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn insert_text(&mut self, text: &str, char_index: CharIndex) -> usize {
        let at = self.byte_at(char_index);
        let current = self.0.clone();
        self.rebuild(&current[..at], text, &current[at..]);
        text.chars().count()
    }

    fn delete_char_range(&mut self, char_range: Range<CharIndex>) {
        let (start, end) = (self.byte_at(char_range.start), self.byte_at(char_range.end));
        if start < end {
            let current = self.0.clone();
            self.rebuild(&current[..start], "", &current[end..]);
        }
    }

    fn clear(&mut self) {
        self.0 = Zeroizing::new(String::new());
    }

    fn replace_with(&mut self, text: &str) {
        self.rebuild("", text, "");
    }

    /// Never hands the secret out as a plain `String`: returns nothing and clears the field.
    fn take(&mut self) -> String {
        self.clear();
        String::new()
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

/// A one-line masked field for `secret`, with `hint` shown while it is empty (and as its
/// accessible name).
pub fn secret_field(ui: &mut Ui, secret: &mut SecretText, hint: &str) -> Response {
    let output = TextEdit::singleline(secret)
        .password(true)
        .hint_text(hint)
        .show(ui);
    let mut state = output.state;
    state.clear_undoer();
    state.store(ui.ctx(), output.response.response.id);
    output.response.response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_land_at_character_positions_with_multibyte_text() {
        let mut secret = SecretText::new();
        secret.insert_text("pässwörd", CharIndex(0));
        secret.insert_text("!", CharIndex(4));
        assert_eq!(secret.expose(), "päss!wörd");
        secret.delete_char_range(CharIndex(1)..CharIndex(2));
        assert_eq!(secret.expose(), "pss!wörd");
    }

    #[test]
    fn the_secret_never_leaves_as_a_plain_string_or_in_debug_output() {
        let mut secret = SecretText::new();
        secret.replace_with("hunter2");
        assert!(!format!("{secret:?}").contains("hunter2"));
        assert_eq!(secret.take(), "");
        assert!(secret.is_empty());
    }
}
