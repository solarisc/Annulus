//! The command palette (Ctrl+Shift+K, F-APP-005): one search over hosts, tabs, snippets and
//! commands, with quick connect for a typed `user@host`. Ranking is shared with the egui
//! palette (`command_palette`).

use annulus_config::tokens::space;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::widgets::{BODY, CONTROL, FieldStyle, SMALL};

use crate::command_palette::{Action, Item, rank, typed_target};

/// How many results show at once.
const SHOWN: usize = 8;
const ROW: f32 = 40.0;

/// What the palette ended with.
pub enum Outcome {
    /// Run this.
    Run(Action),
    /// Closed without a choice.
    Close,
}

/// The palette while open.
#[derive(Default)]
pub struct PaletteView {
    query: String,
    selected: usize,
    opened: bool,
}

impl PaletteView {
    /// Draws the palette over the window; returns how it ended, once it has.
    #[allow(clippy::too_many_lines)] // Field, keys and results, top to bottom.
    pub fn draw(&mut self, ui: &mut Ui<'_>, items: &[Item]) -> Option<Outcome> {
        let t = ui.theme;
        let window = ui.viewport;
        ui.gfx.fill_rect(window, 0.0, t.scrim);
        let backdrop = ui.zone(Id::new("palette.backdrop"), window, Role::Backdrop, "");
        if backdrop.clicked {
            return Some(Outcome::Close);
        }
        let mut results: Vec<Item> = Vec::new();
        if let Some(target) = typed_target(&self.query) {
            results.push(Item {
                label: format!("Connect to {target}"),
                kind: "Quick connect",
                action: Action::QuickConnect(target.to_owned()),
            });
        }
        // As many rows as fit under the field in this window, up to eight.
        let top = if window.h < 560.0 { space::XL } else { 72.0 };
        let room = window.h - top - space::L - CONTROL - 2.0 * space::M - space::S;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // A few rows.
        let fits = (room / ROW).floor().max(1.0) as usize;
        results.extend(rank(&self.query, items).into_iter().take(SHOWN).cloned());
        results.truncate(SHOWN.min(fits));
        self.selected = self.selected.min(results.len().saturating_sub(1));

        #[allow(clippy::cast_precision_loss)] // At most eight rows.
        let list_h = results.len().max(1) as f32 * ROW;
        let width = window.w.clamp(320.0, 600.0) - 2.0 * space::L;
        let card = Rect::new(
            window.center().x - width / 2.0,
            window.y + top,
            width,
            CONTROL + 2.0 * space::M + list_h + space::S,
        );
        ui.gfx.shadow(card, 14.0, 32.0, 12.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        ui.zone(Id::new("palette.card"), card, Role::Backdrop, "");
        let mut inner = card.inset(space::M, space::M);
        let field_id = Id::new("palette.query");
        if !self.opened {
            ui.state.set_focus(Some(field_id), false);
            self.opened = true;
        }
        let field = ui.text_field(
            field_id,
            inner.take_top(CONTROL),
            &mut self.query,
            FieldStyle {
                placeholder: "Search hosts, tabs, snippets and commands, or type user@host",
                icon: Some(icons::COMMAND),
                ..FieldStyle::default()
            },
        );
        if field.changed {
            self.selected = 0;
        }
        inner.take_top(space::S);
        if ui.key(Key::Down, Modifiers::NONE) && self.selected + 1 < results.len() {
            self.selected += 1;
        }
        if ui.key(Key::Up, Modifiers::NONE) {
            self.selected = self.selected.saturating_sub(1);
        }
        if ui.key(Key::Escape, Modifiers::NONE) {
            return Some(Outcome::Close);
        }
        if results.is_empty() {
            ui.gfx.text(
                inner.take_top(ROW),
                "Nothing matches.",
                BODY,
                t.text2,
                Align::Center,
            );
            return None;
        }
        let mut chosen = None;
        for (index, item) in results.iter().enumerate() {
            let row = inner.take_top(ROW);
            let id = Id::new(("palette.item", index));
            let response = ui.zone(id, row, Role::MenuItem, &item.label);
            if response.hovered && ui.pointer().is_some() {
                ui.set_cursor(Cursor::Pointer);
            }
            if index == self.selected || response.hovered {
                ui.gfx.fill_rect(
                    row,
                    8.0,
                    if index == self.selected {
                        t.selected
                    } else {
                        t.hover
                    },
                );
            }
            let mut line = row.inset(space::M, 0.0);
            let icon = match item.action {
                Action::Connect(_) | Action::QuickConnect(_) => icons::SERVER,
                Action::Tab(_) => icons::TERMINAL,
                Action::Snippet(_) => icons::CODE,
                Action::NewLocal => icons::PLUS,
                Action::Lock => icons::LOCK,
                Action::Settings => icons::SETTINGS,
            };
            let slot = line.take_left(16.0).centered(16.0, 16.0);
            ui.gfx.icon(
                icon,
                slot,
                if index == self.selected {
                    t.accent
                } else {
                    t.text2
                },
            );
            line.take_left(space::M);
            let kind_w = ui.gfx.measure(item.kind, SMALL).width + 2.0 * space::S;
            let kind = line.take_right(kind_w);
            ui.gfx.fill_rect(kind.centered(kind.w, 20.0), 10.0, t.hover);
            ui.gfx.text(kind, item.kind, SMALL, t.text2, Align::Center);
            line.take_right(space::S);
            ui.gfx.text(line, &item.label, BODY, t.text, Align::Start);
            if response.clicked {
                chosen = Some(item.action.clone());
            }
        }
        if field.submitted {
            chosen = results.get(self.selected).map(|item| item.action.clone());
        }
        chosen.map(Outcome::Run)
    }
}
