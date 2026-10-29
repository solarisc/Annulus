//! A tree of groups, drawn as a flat list of its visible rows (collapsed branches are not
//! there at all), with the usual tree keys.

use annulus_config::tokens::{radius, space, text};

use super::{centered_baseline, color, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Rect, Size};
use crate::icons::{CHEVRON_DOWN, CHEVRON_RIGHT};
use crate::input::{Button, Event, Handled, Key};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// Height of one row, in DIPs.
pub const ROW_HEIGHT: f32 = 28.0;

/// Indent per level, in DIPs.
const INDENT: f32 = 16.0;

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// What it shows.
    pub label: String,
    /// Nesting level, 0 at the top.
    pub depth: usize,
    /// Whether it has children (and so can expand).
    pub has_children: bool,
    /// Whether its children are shown (the rows after it).
    pub expanded: bool,
}

/// The visible rows of a tree. Up and Down move the selection; Right expands a collapsed
/// row ([`Action::Expand`]) or steps into an expanded one; Left collapses an expanded row
/// ([`Action::Collapse`]) or steps to its parent; Enter or a click reports
/// [`Action::Select`]. The app owns the tree and rebuilds the rows after expand/collapse.
#[derive(Debug)]
pub struct TreeView {
    label: String,
    rows: Vec<TreeRow>,
    selected: usize,
}

impl TreeView {
    /// A tree named `label` showing `rows`, with `selected` (clamped) selected.
    #[must_use]
    pub fn new(label: impl Into<String>, rows: Vec<TreeRow>, selected: usize) -> Self {
        let selected = selected.min(rows.len().saturating_sub(1));
        Self {
            label: label.into(),
            rows,
            selected,
        }
    }

    /// The selected row.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn parent_of(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        self.rows[..index].iter().rposition(|row| row.depth < depth)
    }

    fn move_to(&mut self, index: usize, cx: &mut EventCx) {
        if index < self.rows.len() && index != self.selected {
            self.selected = index;
            cx.emit(Action::Select(index));
        }
    }
}

impl Widget for TreeView {
    fn name(&self) -> &'static str {
        "tree-view"
    }

    fn role(&self) -> Role {
        Role::Tree
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn value(&self) -> Option<String> {
        self.rows.get(self.selected).map(|row| row.label.clone())
    }

    #[allow(clippy::cast_precision_loss)] // Visible rows are a screenful.
    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let widest = self
            .rows
            .iter()
            .map(|row| {
                INDENT * row.depth as f32
                    + INDENT
                    + measure.measure(&row.label, text::BODY, None).width
            })
            .fold(0.0, f32::max);
        Size::new(widest + 2.0 * space::S, ROW_HEIGHT * self.rows.len() as f32)
    }

    #[allow(clippy::cast_precision_loss)] // Visible rows are a screenful.
    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        for (index, row) in self.rows.iter().enumerate() {
            let rect = Rect::new(
                bounds.origin.x,
                bounds.origin.y + ROW_HEIGHT * index as f32,
                bounds.size.width,
                ROW_HEIGHT,
            );
            if index == self.selected {
                list.push(Op::RoundedRect {
                    rect,
                    radius: radius::SMALL,
                    color: color(palette.bg_selected),
                });
            }
            let x = rect.origin.x + space::S + INDENT * row.depth as f32;
            if row.has_children {
                let icon = if row.expanded {
                    CHEVRON_DOWN
                } else {
                    CHEVRON_RIGHT
                };
                let at = Rect::new(x, rect.origin.y + (ROW_HEIGHT - 12.0) / 2.0, 12.0, 12.0);
                list.push(Op::Icon {
                    icon,
                    rect: at,
                    color: color(palette.text_secondary),
                });
            }
            list.push(Op::TextRun {
                origin: Point::new(x + INDENT, centered_baseline(rect, text::BODY)),
                text: row.label.clone(),
                size: text::BODY,
                color: color(palette.text_primary),
            });
        }
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let Some(row) = self.rows.get(self.selected).cloned() else {
            return Handled::No;
        };
        match event {
            Event::PointerDown {
                position,
                button: Button::Primary,
            } => {
                let offset = (position.y - cx.bounds.origin.y) / ROW_HEIGHT;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                // Floored, range-checked.
                let index = offset.floor().max(0.0) as usize;
                if index < self.rows.len() {
                    self.selected = index;
                    cx.emit(Action::Select(index));
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowDown) => {
                self.move_to(self.selected + 1, cx);
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowUp) => {
                if let Some(previous) = self.selected.checked_sub(1) {
                    self.move_to(previous, cx);
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowRight) => {
                if row.has_children && !row.expanded {
                    cx.emit(Action::Expand(self.selected));
                } else if row.has_children {
                    self.move_to(self.selected + 1, cx);
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowLeft) => {
                if row.has_children && row.expanded {
                    cx.emit(Action::Collapse(self.selected));
                } else if let Some(parent) = self.parent_of(self.selected) {
                    self.move_to(parent, cx);
                }
                Handled::Yes
            }
            _ if is_key(event, Key::Enter) => {
                cx.emit(Action::Select(self.selected));
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
    use crate::test_support::FixedAdvance;
    use crate::tree::Tree;
    use crate::widgets::Group;

    fn row(label: &str, depth: usize, has_children: bool, expanded: bool) -> TreeRow {
        TreeRow {
            label: label.into(),
            depth,
            has_children,
            expanded,
        }
    }

    /// Production (expanded) > web-01, db (collapsed); Staging (collapsed).
    fn groups(selected: usize) -> (Tree, crate::tree::NodeId) {
        let rows = vec![
            row("Production", 0, true, true),
            row("web-01", 1, false, false),
            row("db", 1, true, false),
            row("Staging", 0, true, false),
        ];
        let mut tree = Tree::new(Box::new(Group));
        let view = tree
            .append_child(
                tree.root(),
                Box::new(TreeView::new("Groups", rows, selected)),
            )
            .unwrap();
        tree.layout(Size::new(300.0, 300.0), &mut FixedAdvance);
        tree.set_focus(Some(view), true);
        (tree, view)
    }

    fn key(tree: &mut Tree, key: Key) -> Vec<Action> {
        tree.dispatch(&Event::KeyDown {
            key,
            modifiers: Modifiers::default(),
        });
        tree.take_actions()
            .into_iter()
            .map(|(_, action)| action)
            .collect()
    }

    #[test]
    fn right_expands_a_collapsed_group_and_enters_an_open_one() {
        let (mut tree, _) = groups(2);
        assert_eq!(key(&mut tree, Key::ArrowRight), [Action::Expand(2)]);
        let (mut tree, _) = groups(0);
        assert_eq!(key(&mut tree, Key::ArrowRight), [Action::Select(1)]);
    }

    #[test]
    fn left_collapses_an_open_group_or_jumps_to_the_parent() {
        let (mut tree, _) = groups(0);
        assert_eq!(key(&mut tree, Key::ArrowLeft), [Action::Collapse(0)]);
        let (mut tree, _) = groups(2);
        assert_eq!(key(&mut tree, Key::ArrowLeft), [Action::Select(0)]);
    }

    #[test]
    fn up_and_down_stop_at_the_ends() {
        let (mut tree, _) = groups(0);
        assert_eq!(key(&mut tree, Key::ArrowUp), []);
        let (mut tree, _) = groups(3);
        assert_eq!(key(&mut tree, Key::ArrowDown), []);
        assert_eq!(key(&mut tree, Key::ArrowUp), [Action::Select(2)]);
    }

    #[test]
    fn clicking_a_row_selects_it() {
        let (mut tree, view) = groups(0);
        let bounds = tree.bounds(view).unwrap();
        let third = Point::new(bounds.origin.x + 40.0, bounds.origin.y + ROW_HEIGHT * 2.5);
        tree.dispatch(&Event::PointerDown {
            position: third,
            button: Button::Primary,
        });
        assert_eq!(tree.take_actions(), [(view, Action::Select(2))]);
    }
}
