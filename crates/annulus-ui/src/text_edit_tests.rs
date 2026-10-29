use super::*;

fn plain(text: &str) -> TextEditor {
    let mut editor = TextEditor::new(FieldKind::Plain);
    editor.insert(text);
    editor
}

/// A family emoji (four people joined by zero-width joiners) and an "e" with a combining
/// accent: each is one grapheme made of several code points.
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
const E_ACUTE: &str = "e\u{301}";

#[test]
fn backspace_removes_a_whole_grapheme_cluster() {
    let mut editor = plain(&format!("a{FAMILY}{E_ACUTE}"));
    editor.backspace();
    assert_eq!(editor.text(), format!("a{FAMILY}"));
    editor.backspace();
    assert_eq!(editor.text(), "a");
}

#[test]
fn arrows_step_over_clusters_and_the_cursor_stays_on_boundaries() {
    let text = format!("{FAMILY}x{E_ACUTE}");
    let mut editor = plain(&text);
    editor.move_home(false);
    let mut stops = vec![editor.cursor()];
    for _ in 0..5 {
        editor.move_right(false);
        stops.push(editor.cursor());
    }
    assert_eq!(
        stops,
        [
            0,
            FAMILY.len(),
            FAMILY.len() + 1,
            text.len(),
            text.len(),
            text.len()
        ]
    );
    assert!(stops.iter().all(|stop| text.is_char_boundary(*stop)));
}

#[test]
fn typing_replaces_the_selection() {
    let mut editor = plain("hello world");
    editor.move_word_left(false);
    editor.move_end(true);
    assert_eq!(editor.selected_text(), Some("world"));
    editor.insert("there");
    assert_eq!(editor.text(), "hello there");
    assert_eq!(editor.cursor(), "hello there".len());
}

#[test]
fn word_movement_jumps_between_words() {
    let mut editor = plain("ssh user@host -p 22");
    editor.move_home(false);
    editor.move_word_right(false);
    assert_eq!(editor.cursor(), 3);
    editor.move_end(false);
    editor.move_word_left(false);
    assert_eq!(editor.cursor(), "ssh user@host -p ".len());
}

#[test]
fn arrow_without_shift_collapses_a_selection_to_its_edge() {
    let mut editor = plain("abcdef");
    editor.select_all();
    editor.move_left(false);
    assert_eq!((editor.cursor(), editor.selection().len()), (0, 0));
    editor.select_all();
    editor.move_right(false);
    assert_eq!(editor.cursor(), 6);
}

#[test]
fn undo_and_redo_restore_text_and_caret() {
    let mut editor = plain("abc");
    editor.backspace();
    assert!(editor.undo());
    assert_eq!((editor.text(), editor.cursor()), ("abc", 3));
    assert!(editor.redo());
    assert_eq!(editor.text(), "ab");
    editor.insert("x");
    assert!(!editor.redo(), "a new edit clears the redo history");
}

#[test]
fn composition_shows_at_the_caret_until_committed() {
    let mut editor = plain("ab");
    editor.move_left(false);
    editor.set_composition("\u{306B}");
    assert_eq!(editor.text(), "ab");
    assert_eq!(editor.display_text(), "a\u{306B}b");
    editor.commit_composition();
    assert_eq!(editor.text(), "a\u{306B}b");
    assert_eq!(editor.display_text(), "a\u{306B}b");
}

#[test]
fn secret_field_hides_its_text_everywhere_but_text() {
    let mut editor = TextEditor::new(FieldKind::Secret);
    editor.insert(&format!("hunter2{E_ACUTE}"));
    assert_eq!(editor.display_text(), "\u{2022}".repeat(8));
    editor.select_all();
    assert_eq!(editor.selected_text(), None);
    assert!(!format!("{editor:?}").contains("hunter2"));
    editor.backspace();
    assert!(!editor.undo(), "secret fields keep no history");
    assert_eq!(editor.text(), "");
}

#[test]
fn secret_field_word_jumps_reveal_nothing() {
    let mut editor = TextEditor::new(FieldKind::Secret);
    editor.insert("two words");
    editor.move_word_left(false);
    assert_eq!(editor.cursor(), 0);
}

#[test]
fn pasting_too_much_is_cut_on_a_grapheme_boundary() {
    let mut editor = plain("");
    let huge = FAMILY.repeat(MAX_BYTES / FAMILY.len() + 10);
    editor.insert(&huge);
    assert!(editor.text().len() <= MAX_BYTES);
    assert_eq!(editor.text().len() % FAMILY.len(), 0);
    editor.insert("more");
    assert!(editor.text().len() <= MAX_BYTES);
}

#[test]
fn control_characters_are_dropped_from_input() {
    let editor = plain("a\nb\u{1b}[31mc\td");
    assert_eq!(editor.text(), "ab[31mc\td");
}

#[test]
fn clear_wipes_text_and_history() {
    let mut editor = plain("secret-ish");
    editor.clear();
    assert_eq!(editor.text(), "");
    assert!(!editor.undo());
}
