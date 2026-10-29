//! The command palette (F-APP-005, Ctrl+Shift+K): one box to reach saved hosts first, then
//! open tabs, snippets and commands. Typing narrows the list with a fuzzy match; the
//! arrows choose and Enter runs, so it never needs the mouse.

use eframe::egui::{self, Key, RichText, Ui};

/// What choosing an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Connect to the saved host with this id.
    Connect(u64),
    /// Bring the tab at this index to the front.
    Tab(usize),
    /// Type the snippet with this id into the terminal in front.
    Snippet(u64),
    /// Open a local terminal.
    NewLocal,
    /// Lock the vault.
    Lock,
    /// Show the Settings screen.
    Settings,
}

/// One line of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it is called.
    pub label: String,
    /// Its kind, shown dimmed: "Host", "Tab", "Snippet" or "Command".
    pub kind: &'static str,
    /// What choosing it does.
    pub action: Action,
}

/// How well `query` matches `text`: its letters in order (any case), higher for a match at
/// the start, at word starts and in runs; `None` when it does not match at all.
#[must_use]
pub fn score(query: &str, text: &str) -> Option<i64> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let (mut at, mut total, mut previous) = (0, 0i64, None::<usize>);
    for wanted in query {
        let found = (at..text.len()).find(|&index| text[index] == wanted)?;
        total += 1;
        if found == 0 {
            total += 8;
        } else if !text[found - 1].is_alphanumeric() {
            total += 5;
        }
        if previous.is_some_and(|last| last + 1 == found) {
            total += 3;
        }
        previous = Some(found);
        at = found + 1;
    }
    // Shorter texts win ties: "db" beats "db-backup-old" for "db".
    Some(total * 100 - i64::try_from(text.len()).unwrap_or(i64::MAX).min(99))
}

/// `items` that match `query`, best first; with no query, in their own order (hosts first).
#[must_use]
pub fn rank<'a>(query: &str, items: &'a [Item]) -> Vec<&'a Item> {
    let mut scored: Vec<(i64, usize, &Item)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((score(query, &item.label)?, index, item)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// The palette while open.
#[derive(Default)]
pub struct CommandPalette {
    query: String,
    selected: usize,
}

/// What the palette did this frame.
pub enum Outcome {
    /// Still open.
    Open,
    /// Closed without choosing.
    Closed,
    /// This was chosen (and the palette closed).
    Chosen(Action),
}

impl CommandPalette {
    /// Draws the palette over the window.
    pub fn ui(&mut self, ctx: &egui::Context, items: &[Item]) -> Outcome {
        let ranked = rank(&self.query, items);
        let (up, down, enter, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                input.consume_key(egui::Modifiers::NONE, Key::Enter),
                input.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            return Outcome::Closed;
        }
        if down {
            self.selected = (self.selected + 1).min(ranked.len().saturating_sub(1));
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(ranked.len().saturating_sub(1));
        let mut chosen = None;
        egui::Window::new("Go to")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .fixed_size(egui::vec2(480.0, 0.0))
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Hosts, tabs, snippets, commands")
                        .desired_width(f32::INFINITY),
                );
                field.request_focus();
                if field.changed() {
                    self.selected = 0;
                }
                ui.separator();
                chosen = self.list(ui, &ranked);
            });
        if enter {
            chosen = ranked.get(self.selected).map(|item| item.action);
        }
        chosen.map_or(Outcome::Open, Outcome::Chosen)
    }

    fn list(&mut self, ui: &mut Ui, ranked: &[&Item]) -> Option<Action> {
        if ranked.is_empty() {
            ui.label("Nothing matches.");
            return None;
        }
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for (index, item) in ranked.iter().enumerate().take(50) {
                    let selected = index == self.selected;
                    let row = ui.selectable_label(
                        selected,
                        RichText::new(format!("{}    {}", item.label, item.kind)),
                    );
                    if selected {
                        row.scroll_to_me(None);
                    }
                    if row.clicked() {
                        chosen = Some(item.action);
                    }
                }
            });
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> Item {
        Item {
            label: label.into(),
            kind: "Host",
            action: Action::NewLocal,
        }
    }

    #[test]
    fn letters_in_order_match_and_better_matches_come_first() {
        assert!(score("pdb", "prod-db").is_some());
        assert_eq!(score("dbp", "prod-db"), None, "order matters");
        assert!(score("PROD", "prod-db").is_some(), "any case");
        let items = [
            item("db-backup-old"),
            item("web-prod"),
            item("db"),
            item("staging"),
        ];
        let labels: Vec<&str> = rank("db", &items)
            .iter()
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(labels, ["db", "db-backup-old"]);
        let all: Vec<&str> = rank("", &items).iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            all,
            ["db-backup-old", "web-prod", "db", "staging"],
            "no query: as given"
        );
        let pair = [item("web-prod"), item("ape")];
        let word: Vec<&str> = rank("p", &pair).iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            word,
            ["web-prod", "ape"],
            "a word start beats the middle of a word"
        );
    }
}
