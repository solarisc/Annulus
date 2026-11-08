//! The command palette (Ctrl+Shift+P, F-APP-005): one search over hosts, tabs, snippets and
//! commands, with quick connect for a typed `user@host`. Search and ranking live in
//! `command_palette` so every route follows one rule.

use std::sync::Arc;

use annulus_config::tokens::space;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::{Curve, Layer, Motion};
use annulus_ui::widgets::{BODY, CONTROL, FieldStyle, SMALL};

use crate::command_palette::{Action, Item, search, typed_target};

/// How many results show at once.
const SHOWN: usize = 8;
const ROW: f32 = 40.0;
/// Height of the line at the palette's foot that says what the keys do.
const HINTS_H: f32 = 28.0;

/// What the palette ended with.
#[derive(Clone)]
pub enum Outcome {
    /// Run this.
    Run(Action),
    /// Closed without a choice.
    Close(Option<Id>),
}

/// The palette while open.
#[derive(Default)]
pub struct PaletteView {
    items: Arc<[Item]>,
    query: String,
    selected: usize,
    opened: bool,
    closing: Option<(Outcome, f64)>,
    return_focus: Option<Id>,
    focus_captured: bool,
}

struct ResultRow {
    label: String,
    kind: &'static str,
    action: Action,
}

impl PaletteView {
    /// Opens over one stable search index. It is rebuilt the next time the palette opens.
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            items: items.into(),
            ..Self::default()
        }
    }

    /// Draws the palette over the window; returns how it ended, once it has.
    #[allow(clippy::too_many_lines)] // Field, keys and results, top to bottom.
    pub fn draw(&mut self, ui: &mut Ui<'_>) -> Option<Outcome> {
        let window = ui.viewport;
        let mut shown = ui.entrance(Id::new("palette.drop.open"), Motion::SLOW);
        if let Some((_, since)) = self.closing.as_ref() {
            let share =
                ((ui.time() - *since) / (f64::from(Motion::EXIT.ms) / 1000.0)).clamp(0.0, 1.0);
            if ui.state.reduce_motion || share >= 1.0 {
                return self.closing.take().map(|(outcome, _)| match outcome {
                    Outcome::Close(_) => Outcome::Close(self.return_focus),
                    run @ Outcome::Run(_) => run,
                });
            }
            #[allow(clippy::cast_possible_truncation)] // A share from zero to one.
            let share = share as f32;
            shown = 1.0 - Curve::Exit.at(share);
            ui.push_inert();
            ui.request_repaint();
        }
        ui.gfx
            .fill_rect(window, 0.0, ui.theme.scrim.alpha(shown.clamp(0.0, 1.0)));
        let backdrop = ui.zone(Id::new("palette.backdrop"), window, Role::Backdrop, "");
        let requested = if backdrop.clicked {
            Some(Outcome::Close(None))
        } else {
            let items = Arc::clone(&self.items);
            self.list(ui, &items, shown)
        };
        if self.closing.is_some() {
            ui.pop_inert();
        } else if let Some(outcome) = requested {
            self.closing = Some((outcome, ui.time()));
            ui.state.set_focus(None, false);
            ui.request_repaint();
        }
        None
    }

    /// Starts the close motion when the palette shortcut is pressed again.
    pub fn close(&mut self, now: f64) {
        if self.closing.is_none() {
            self.closing = Some((Outcome::Close(None), now));
        }
    }

    /// The field and the results, inside the card's motion.
    #[allow(clippy::too_many_lines)] // Field, keys and results, top to bottom.
    fn list(&mut self, ui: &mut Ui<'_>, items: &[Item], shown: f32) -> Option<Outcome> {
        let t = ui.theme;
        let window = ui.viewport;
        let mut results: Vec<ResultRow> = Vec::new();
        if let Some(target) = typed_target(&self.query) {
            results.push(ResultRow {
                label: format!("Connect to {target}"),
                kind: "Quick connect",
                action: Action::QuickConnect(target.to_owned()),
            });
        }
        // As many rows as fit under the field in this window, up to eight.
        let top = if window.h < 560.0 { space::XL } else { 72.0 };
        let room = window.h - top - space::L - CONTROL - 2.0 * space::M - space::S - HINTS_H;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // A few rows.
        let fits = (room / ROW).floor().max(1.0) as usize;
        results.extend(
            search(&self.query, items)
                .into_iter()
                .take(SHOWN)
                .map(|found| ResultRow {
                    label: found.label().into_owned(),
                    kind: found.item.kind,
                    action: found.action,
                }),
        );
        results.truncate(SHOWN.min(fits));
        self.selected = self.selected.min(results.len().saturating_sub(1));

        #[allow(clippy::cast_precision_loss)] // At most eight rows.
        let list_h = results.len().max(1) as f32 * ROW;
        let width = window.w.clamp(320.0, 600.0) - 2.0 * space::L;
        let card = Rect::new(
            window.center().x - width / 2.0,
            window.y + top,
            width,
            CONTROL + 2.0 * space::M + list_h + space::S + HINTS_H,
        );
        let shape = fluid_rect(window, card, shown);
        fluid_surface(ui, window, shape, shown);
        ui.zone(
            Id::new("palette.card"),
            shape,
            Role::Dialog,
            "Command palette",
        );
        ui.push_clip(shape);
        let reveal = ((shown - 0.28) / 0.72).clamp(0.0, 1.0);
        ui.push_layer(
            Layer::fade(reveal)
                .shifted(0.0, 10.0 * (1.0 - reveal))
                .grouped(card.outset(32.0)),
        );
        let mut inner = card.inset(space::M, space::M);
        // What the keys do, at its foot.
        let hints = Rect::new(inner.x + space::M, inner.b() - HINTS_H + 8.0, inner.w, 20.0);
        ui.gfx.text(
            hints,
            "Up and Down to move  \u{b7}  Enter to open  \u{b7}  Esc to close",
            SMALL,
            t.text3,
            Align::Start,
        );
        let field_id = Id::new("palette.query");
        if !self.opened {
            if !self.focus_captured {
                self.return_focus = ui.state.focus();
                self.focus_captured = true;
            }
            ui.state.set_focus(Some(field_id), false);
            // The first frames clip the field out with the small drop. Keep asking for focus
            // until the field exists in the frame's zones and can keep it.
            self.opened = shown >= 0.45;
        }
        let field = ui.text_field(
            field_id,
            inner.take_top(CONTROL),
            &mut self.query,
            FieldStyle {
                placeholder: "Search hosts, pages, settings and commands, or type user@host",
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
        if ui.key(Key::Home, Modifiers::NONE) {
            self.selected = 0;
        }
        if ui.key(Key::End, Modifiers::NONE) {
            self.selected = results.len().saturating_sub(1);
        }
        if ui.key(Key::Escape, Modifiers::NONE) {
            ui.pop_layer();
            ui.pop_clip();
            return Some(Outcome::Close(None));
        }
        if results.is_empty() {
            ui.gfx.text(
                inner.take_top(ROW),
                "Nothing matches.",
                BODY,
                t.text2,
                Align::Center,
            );
            ui.pop_layer();
            ui.pop_clip();
            return None;
        }
        let mut chosen = None;
        // The selection glides from row to row as the arrows move it.
        #[allow(clippy::cast_precision_loss)] // At most eight rows.
        let target = inner.y + self.selected as f32 * ROW;
        let glide = ui.spring(Id::new("palette.glide"), target);
        ui.gfx
            .fill_rect(Rect::new(inner.x, glide, inner.w, ROW), 8.0, t.selected);
        for (index, item) in results.iter().enumerate() {
            let row = inner.take_top(ROW);
            let id = Id::new(("palette.item", index));
            let response = ui.zone(id, row, Role::MenuItem, &item.label);
            if response.hovered && ui.pointer().is_some() {
                ui.set_cursor(Cursor::Pointer);
                if index != self.selected {
                    self.selected = index;
                    ui.request_repaint();
                }
            }
            if response.hovered && index != self.selected {
                ui.gfx.fill_rect(row, 8.0, t.hover);
            }
            let mut line = row.inset(space::M, 0.0);
            let icon = match item.action {
                Action::Connect(_) | Action::QuickConnect(_) | Action::EditHost(_) => icons::SERVER,
                Action::HostSftp(_) | Action::EditGroup(_) => icons::FOLDER,
                Action::EditKey(_) => icons::KEY,
                Action::EditSnippet(_) | Action::Snippet(_) => icons::CODE,
                Action::EditForward(_) => icons::FORWARD,
                Action::Tab(_) => icons::TERMINAL,
                Action::NewLocal => icons::PLUS,
                Action::Lock => icons::LOCK,
                Action::Navigate(section) => section.icon(),
                Action::Settings(_) => icons::SETTINGS,
            };
            let slot = line.take_left(16.0).centered(16.0, 16.0);
            ui.gfx.icon(
                icon,
                slot,
                if index == self.selected {
                    t.text
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
        ui.pop_layer();
        ui.pop_clip();
        chosen.map(Outcome::Run)
    }
}

/// The growing command surface. Height falls first; width follows and settles with a small
/// pass beyond its final size.
fn fluid_rect(window: Rect, target: Rect, shown: f32) -> Rect {
    let shown = shown.clamp(0.0, 1.0);
    let fall = Curve::Emphasized.at(shown);
    let spread = Curve::Overshoot.at(((shown - 0.12) / 0.88).clamp(0.0, 1.0));
    let high = Curve::Emphasized.at(((shown - 0.04) / 0.96).clamp(0.0, 1.0));
    let width = 22.0 + (target.w - 22.0) * spread;
    let height = 30.0 + (target.h - 30.0) * high;
    let start_y = window.y - 18.0;
    let y = start_y + (target.y - start_y) * fall;
    Rect::new(window.center().x - width / 2.0, y, width, height)
}

/// Draws the drop, its short link to the top edge, and the raised surface it becomes.
fn fluid_surface(ui: &mut Ui<'_>, window: Rect, shape: Rect, shown: f32) {
    let t = ui.theme;
    let link = (1.0 - shown / 0.46).clamp(0.0, 1.0);
    if link > 0.0 {
        let width = 5.0 + 5.0 * link;
        let neck = Rect::new(
            window.center().x - width / 2.0,
            window.y,
            width,
            (shape.y + shape.h * 0.46 - window.y).max(0.0),
        );
        ui.gfx.fill_rect(neck, width / 2.0, t.surface.alpha(link));
    }
    let radius = (shape.w.min(shape.h) / 2.0).min(14.0);
    ui.raised_surface(shape, radius);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drop_starts_small_above_the_window_and_finishes_at_the_card() {
        let window = Rect::new(0.0, 0.0, 1_100.0, 720.0);
        let card = Rect::new(250.0, 72.0, 600.0, 420.0);
        let start = fluid_rect(window, card, 0.0);
        assert_eq!(start, Rect::new(539.0, -18.0, 22.0, 30.0));
        let middle = fluid_rect(window, card, 0.45);
        assert!(middle.y > start.y && middle.y < card.y);
        assert!(middle.w > start.w && middle.w < card.w * 1.05);
        assert!(middle.h > start.h && middle.h < card.h);
        assert_eq!(fluid_rect(window, card, 1.0), card);
    }
}
