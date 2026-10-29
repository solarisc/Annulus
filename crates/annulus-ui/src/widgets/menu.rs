//! Popup menus: the list a select opens, context menus and overflow menus.

use annulus_config::tokens::{radius, space, text};

use super::{centered_baseline, color, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Color, Point, Rect, Size};
use crate::input::{Button, Event, Handled, Key};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// Height of one item, in DIPs.
pub const ITEM_HEIGHT: f32 = 28.0;

/// A popup list of items. Arrows move the highlight (wrapping), Home and End jump, Enter,
/// Space or a click on an item report [`Action::Select`] with its index, and Escape or a
/// click outside report [`Action::Dismiss`]. It holds keyboard focus like a dialog, but an
/// outside click closes it instead of being ignored.
#[derive(Debug)]
pub struct Menu {
    label: String,
    items: Vec<String>,
    highlighted: usize,
}

impl Menu {
    /// A menu named `label` showing `items`, with `highlighted` (clamped) highlighted.
    #[must_use]
    pub fn new(label: impl Into<String>, items: Vec<String>, highlighted: usize) -> Self {
        let highlighted = highlighted.min(items.len().saturating_sub(1));
        Self {
            label: label.into(),
            items,
            highlighted,
        }
    }

    /// The highlighted item.
    #[must_use]
    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    fn item_at(&self, bounds: Rect, position: Point) -> Option<usize> {
        if !bounds.contains(position) {
            return None;
        }
        let offset = (position.y - bounds.origin.y - space::XS) / ITEM_HEIGHT;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // Floored, range-checked.
        let index = offset.floor().max(0.0) as usize;
        (index < self.items.len()).then_some(index)
    }

    fn step(&mut self, forward: bool) {
        let count = self.items.len();
        if count > 0 {
            self.highlighted = if forward {
                (self.highlighted + 1) % count
            } else {
                (self.highlighted + count - 1) % count
            };
        }
    }
}

impl Widget for Menu {
    fn name(&self) -> &'static str {
        "menu"
    }

    fn role(&self) -> Role {
        Role::Menu
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn value(&self) -> Option<String> {
        self.items.get(self.highlighted).cloned()
    }

    fn modal(&self) -> bool {
        true
    }

    #[allow(clippy::cast_precision_loss)] // A menu has a handful of items.
    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let widest = self
            .items
            .iter()
            .map(|item| measure.measure(item, text::BODY, None).width)
            .fold(0.0, f32::max);
        Size::new(
            widest + 2.0 * space::M,
            ITEM_HEIGHT * self.items.len() as f32 + 2.0 * space::XS,
        )
    }

    #[allow(clippy::cast_precision_loss)] // A menu has a handful of items.
    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::Shadow {
            rect: bounds,
            radius: radius::MEDIUM,
            offset: Point::new(0.0, 4.0),
            blur: 12.0,
            color: Color::rgba(0, 0, 0, 70),
        });
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::MEDIUM,
            color: color(palette.bg_raised),
        });
        list.push(Op::Border {
            rect: bounds,
            radius: radius::MEDIUM,
            width: 1.0,
            color: color(palette.border_subtle),
        });
        for (index, item) in self.items.iter().enumerate() {
            let row = Rect::new(
                bounds.origin.x + space::XS,
                bounds.origin.y + space::XS + ITEM_HEIGHT * index as f32,
                bounds.size.width - 2.0 * space::XS,
                ITEM_HEIGHT,
            );
            if index == self.highlighted {
                list.push(Op::RoundedRect {
                    rect: row,
                    radius: radius::SMALL,
                    color: color(palette.bg_selected),
                });
            }
            list.push(Op::TextRun {
                origin: Point::new(row.origin.x + space::S, centered_baseline(row, text::BODY)),
                text: item.clone(),
                size: text::BODY,
                color: color(palette.text_primary),
            });
        }
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        match event {
            Event::PointerMove { position } => {
                if let Some(index) = self.item_at(cx.bounds, *position) {
                    self.highlighted = index;
                }
                Handled::Yes
            }
            Event::PointerDown {
                position,
                button: Button::Primary,
            } => {
                match self.item_at(cx.bounds, *position) {
                    Some(index) => cx.emit(Action::Select(index)),
                    None if !cx.bounds.contains(*position) => cx.emit(Action::Dismiss),
                    None => {}
                }
                Handled::Yes
            }
            Event::PointerDown { .. } | Event::PointerUp { .. } | Event::Wheel { .. } => {
                Handled::Yes
            }
            _ if is_key(event, Key::Escape) => {
                cx.emit(Action::Dismiss);
                Handled::Yes
            }
            _ if is_key(event, Key::Enter) || is_key(event, Key::Space) => {
                if !self.items.is_empty() {
                    cx.emit(Action::Select(self.highlighted));
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowDown) || is_key(event, Key::ArrowUp) => {
                self.step(is_key(event, Key::ArrowDown));
                Handled::Yes
            }
            _ if is_key(event, Key::Home) => {
                self.highlighted = 0;
                Handled::Yes
            }
            _ if is_key(event, Key::End) => {
                self.highlighted = self.items.len().saturating_sub(1);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::style::{Position, Style};
    use crate::test_support::FixedAdvance;
    use crate::tree::{NodeId, Tree};
    use crate::widgets::{Button as PushButton, ButtonKind, Group};

    fn open_menu() -> (Tree, NodeId, NodeId) {
        let mut tree = Tree::new(Box::new(Group));
        let behind = tree
            .append_child(
                tree.root(),
                Box::new(PushButton::new("Behind", ButtonKind::Primary)),
            )
            .unwrap();
        let items = vec![
            "Connect".into(),
            "Edit".into(),
            "Duplicate".into(),
            "Delete".into(),
        ];
        let menu = tree
            .append_child(tree.root(), Box::new(Menu::new("Host actions", items, 0)))
            .unwrap();
        tree.set_style(
            menu,
            Style {
                position: Position::Overlay { x: 100.0, y: 100.0 },
                ..Style::default()
            },
        )
        .unwrap();
        tree.layout(Size::new(600.0, 400.0), &mut FixedAdvance);
        (tree, menu, behind)
    }

    fn key(tree: &mut Tree, key: Key) {
        tree.dispatch(&Event::KeyDown {
            key,
            modifiers: Modifiers::default(),
        });
    }

    #[test]
    fn arrows_wrap_and_enter_selects_the_highlighted_item() {
        let (mut tree, menu, _) = open_menu();
        key(&mut tree, Key::ArrowUp);
        key(&mut tree, Key::Enter);
        key(&mut tree, Key::Home);
        key(&mut tree, Key::ArrowDown);
        key(&mut tree, Key::Space);
        assert_eq!(
            tree.take_actions(),
            [(menu, Action::Select(3)), (menu, Action::Select(1))]
        );
    }

    #[test]
    fn clicking_an_item_selects_it_and_clicking_outside_dismisses() {
        let (mut tree, menu, behind) = open_menu();
        let bounds = tree.bounds(menu).unwrap();
        let third = Point::new(
            bounds.origin.x + 10.0,
            bounds.origin.y + space::XS + ITEM_HEIGHT * 2.5,
        );
        tree.dispatch(&Event::PointerDown {
            position: third,
            button: Button::Primary,
        });
        let outside = Point::new(5.0, 5.0);
        assert!(tree.bounds(behind).unwrap().contains(outside));
        tree.dispatch(&Event::PointerDown {
            position: outside,
            button: Button::Primary,
        });
        assert_eq!(
            tree.take_actions(),
            [(menu, Action::Select(2)), (menu, Action::Dismiss)]
        );
    }

    #[test]
    fn escape_dismisses() {
        let (mut tree, menu, _) = open_menu();
        key(&mut tree, Key::Escape);
        assert_eq!(tree.take_actions(), [(menu, Action::Dismiss)]);
    }
}
