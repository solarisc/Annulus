//! Window structure: a resizable split pane, a tab strip and a side panel. Like every
//! widget they report what the user did; the app's view models change the tree.

use annulus_config::tokens::{radius, space, text};

use super::{centered_baseline, color, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Rect, Size};
use crate::icons::CLOSE;
use crate::input::{Button, Event, Handled, Key};
use crate::style::{Axis, Length, Position, Style};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// Width of the split pane's divider, in DIPs; it is the gap between the two panes.
pub const DIVIDER: f32 = 6.0;

/// Smallest share of a split pane either side may get, in per mille.
const MIN_SHARE: u16 = 100;

/// Keyboard resize step, in per mille.
const KEY_STEP: u16 = 50;

/// Two panes side by side (or stacked) with a draggable divider between them. Dragging
/// the divider, or Left/Right (Up/Down) while it has focus, reports [`Action::Resize`] with
/// the first pane's new share in per mille; the app applies [`SplitPane::pane_styles`].
#[derive(Debug)]
pub struct SplitPane {
    axis: Axis,
    share: u16,
    dragging: bool,
}

impl SplitPane {
    /// A split along `axis` giving the first pane `share` per mille (clamped).
    #[must_use]
    pub fn new(axis: Axis, share: u16) -> Self {
        Self {
            axis,
            share: clamp_share(share),
            dragging: false,
        }
    }

    /// The style of the split container itself.
    #[must_use]
    pub fn style(&self) -> Style {
        Style {
            axis: self.axis,
            gap: DIVIDER,
            width: Length::Fill(1.0),
            height: Length::Fill(1.0),
            ..Style::default()
        }
    }

    /// The styles of the two panes for a first-pane `share` in per mille.
    #[must_use]
    pub fn pane_styles(axis: Axis, share: u16) -> (Style, Style) {
        let share = f32::from(clamp_share(share));
        let pane = |weight: f32| match axis {
            Axis::Row => Style {
                width: Length::Fill(weight),
                ..Style::default()
            },
            Axis::Column => Style {
                height: Length::Fill(weight),
                ..Style::default()
            },
        };
        (pane(share), pane(1000.0 - share))
    }

    fn set_share(&mut self, share: u16, cx: &mut EventCx) {
        let share = clamp_share(share);
        if share != self.share {
            self.share = share;
            cx.emit(Action::Resize(share));
        }
    }

    fn share_at(&self, bounds: Rect, position: Point) -> u16 {
        let (offset, length) = match self.axis {
            Axis::Row => (position.x - bounds.origin.x, bounds.size.width),
            Axis::Column => (position.y - bounds.origin.y, bounds.size.height),
        };
        if length <= 0.0 {
            return self.share;
        }
        let permille = (offset / length * 1000.0).round().clamp(0.0, 1000.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to 0..=1000.
        let permille = permille as u16;
        permille
    }
}

fn clamp_share(share: u16) -> u16 {
    share.clamp(MIN_SHARE, 1000 - MIN_SHARE)
}

impl Widget for SplitPane {
    fn name(&self) -> &'static str {
        "split-pane"
    }

    fn role(&self) -> Role {
        Role::Splitter
    }

    fn label(&self) -> Option<&str> {
        Some("Resize panes")
    }

    fn value(&self) -> Option<String> {
        Some(format!("{}%", self.share / 10))
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        // The divider is the gap between the panes; show a thin line in its middle.
        let bounds = cx.bounds;
        let share = f32::from(self.share) / 1000.0;
        let line = match self.axis {
            Axis::Row => {
                let x = bounds.origin.x + (bounds.size.width - DIVIDER) * share + DIVIDER / 2.0;
                Rect::new(x - 0.5, bounds.origin.y, 1.0, bounds.size.height)
            }
            Axis::Column => {
                let y = bounds.origin.y + (bounds.size.height - DIVIDER) * share + DIVIDER / 2.0;
                Rect::new(bounds.origin.x, y - 0.5, bounds.size.width, 1.0)
            }
        };
        let tone = if self.dragging || cx.state.hovered {
            cx.palette.accent
        } else {
            cx.palette.border_subtle
        };
        list.push(Op::Rect {
            rect: line,
            color: color(tone),
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let (back, forward) = match self.axis {
            Axis::Row => (Key::ArrowLeft, Key::ArrowRight),
            Axis::Column => (Key::ArrowUp, Key::ArrowDown),
        };
        match event {
            Event::PointerDown {
                button: Button::Primary,
                ..
            } => {
                self.dragging = true;
                Handled::Yes
            }
            Event::PointerMove { position } if self.dragging => {
                let share = self.share_at(cx.bounds, *position);
                self.set_share(share, cx);
                Handled::Yes
            }
            Event::PointerUp { .. } if self.dragging => {
                self.dragging = false;
                Handled::Yes
            }
            _ if is_key(event, back) => {
                self.set_share(self.share.saturating_sub(KEY_STEP), cx);
                Handled::Yes
            }
            _ if is_key(event, forward) => {
                self.set_share(self.share.saturating_add(KEY_STEP), cx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// Widest a tab gets, in DIPs.
const MAX_TAB: f32 = 200.0;

/// Width of a tab's close button area.
const TAB_CLOSE: f32 = 24.0;

/// A row of equal-width tabs. A click or Left/Right selects ([`Action::Select`]); the close
/// button, a middle click or Delete closes ([`Action::Close`]); dragging a tab onto another
/// reorders them ([`Action::Move`]).
#[derive(Debug)]
pub struct TabStrip {
    tabs: Vec<String>,
    selected: usize,
    drag_from: Option<usize>,
}

impl TabStrip {
    /// Tabs titled `tabs`, with `selected` (clamped) in front.
    #[must_use]
    pub fn new(tabs: Vec<String>, selected: usize) -> Self {
        let selected = selected.min(tabs.len().saturating_sub(1));
        Self {
            tabs,
            selected,
            drag_from: None,
        }
    }

    #[allow(clippy::cast_precision_loss)] // A strip holds tens of tabs at most.
    fn tab_width(&self, bounds: Rect) -> f32 {
        (bounds.size.width / self.tabs.len().max(1) as f32).min(MAX_TAB)
    }

    #[allow(clippy::cast_precision_loss)] // Same.
    fn tab_rect(&self, bounds: Rect, index: usize) -> Rect {
        let width = self.tab_width(bounds);
        Rect::new(
            bounds.origin.x + width * index as f32,
            bounds.origin.y,
            width,
            bounds.size.height,
        )
    }

    fn tab_at(&self, bounds: Rect, position: Point) -> Option<usize> {
        (0..self.tabs.len()).find(|index| self.tab_rect(bounds, *index).contains(position))
    }

    fn close_rect(tab: Rect) -> Rect {
        Rect::new(
            tab.max_x() - TAB_CLOSE,
            tab.origin.y,
            TAB_CLOSE,
            tab.size.height,
        )
    }
}

impl Widget for TabStrip {
    fn name(&self) -> &'static str {
        "tab-strip"
    }

    fn role(&self) -> Role {
        Role::TabList
    }

    fn label(&self) -> Option<&str> {
        Some("Tabs")
    }

    fn value(&self) -> Option<String> {
        self.tabs.get(self.selected).cloned()
    }

    #[allow(clippy::cast_precision_loss)] // A strip holds tens of tabs at most.
    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let height = measure.measure("Ag", text::BODY, None).height + 2.0 * space::S;
        Size::new(MAX_TAB * self.tabs.len() as f32, height.max(32.0))
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        for (index, title) in self.tabs.iter().enumerate() {
            let tab = self.tab_rect(cx.bounds, index);
            if index == self.selected {
                list.push(Op::RoundedRect {
                    rect: tab,
                    radius: radius::MEDIUM,
                    color: color(palette.bg_surface),
                });
                list.push(Op::Rect {
                    rect: Rect::new(
                        tab.origin.x + space::S,
                        tab.max_y() - 2.0,
                        tab.size.width - 2.0 * space::S,
                        2.0,
                    ),
                    color: color(palette.accent),
                });
            }
            let ink = if index == self.selected {
                palette.text_primary
            } else {
                palette.text_secondary
            };
            let text_area = Rect::new(
                tab.origin.x + space::M,
                tab.origin.y,
                tab.size.width - space::M - TAB_CLOSE,
                tab.size.height,
            );
            list.clip(text_area, |list| {
                list.push(Op::TextRun {
                    origin: Point::new(text_area.origin.x, centered_baseline(tab, text::BODY)),
                    text: title.clone(),
                    size: text::BODY,
                    color: color(ink),
                });
            });
            let close = Self::close_rect(tab);
            let icon = Rect::new(
                close.origin.x + 6.0,
                close.origin.y + (close.size.height - 12.0) / 2.0,
                12.0,
                12.0,
            );
            list.push(Op::Icon {
                icon: CLOSE,
                rect: icon,
                color: color(palette.text_secondary),
            });
        }
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        match event {
            Event::PointerDown {
                position,
                button: Button::Middle,
            } => {
                if let Some(index) = self.tab_at(cx.bounds, *position) {
                    cx.emit(Action::Close(index));
                }
                Handled::Yes
            }
            Event::PointerDown {
                position,
                button: Button::Primary,
            } => {
                self.drag_from = self.tab_at(cx.bounds, *position);
                Handled::Yes
            }
            Event::PointerUp {
                position,
                button: Button::Primary,
            } => {
                let from = self.drag_from.take();
                let to = self.tab_at(cx.bounds, *position);
                match (from, to) {
                    (Some(from), Some(to)) if from != to => cx.emit(Action::Move { from, to }),
                    (Some(index), Some(_))
                        if Self::close_rect(self.tab_rect(cx.bounds, index))
                            .contains(*position) =>
                    {
                        cx.emit(Action::Close(index));
                    }
                    (Some(index), Some(_)) if index != self.selected => {
                        self.selected = index;
                        cx.emit(Action::Select(index));
                    }
                    _ => {}
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowLeft) || is_key(event, Key::ArrowRight) => {
                let next = if is_key(event, Key::ArrowLeft) {
                    self.selected.checked_sub(1)
                } else {
                    Some(self.selected + 1).filter(|next| *next < self.tabs.len())
                };
                if let Some(next) = next {
                    self.selected = next;
                    cx.emit(Action::Select(next));
                }
                Handled::Yes
            }
            _ if is_key(event, Key::Delete) && !self.tabs.is_empty() => {
                cx.emit(Action::Close(self.selected));
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// A panel sliding in from the right edge, such as the host editor (F-HOST-001). It does not
/// block the rest of the window. Escape reports [`Action::Dismiss`].
#[derive(Debug)]
pub struct SidePanel {
    title: String,
}

impl SidePanel {
    /// A panel headed `title`.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    /// The style for a panel `width` DIPs wide along the right edge of a `parent_width`
    /// parent, with room for the title.
    #[must_use]
    pub fn style(parent_width: f32, width: f32) -> Style {
        let mut padding = crate::style::Edges::all(space::L);
        padding.top = space::L + text::HEADING + space::M;
        Style {
            position: Position::Overlay {
                x: (parent_width - width).max(0.0),
                y: 0.0,
            },
            width: Length::Fixed(width),
            height: Length::Fill(1.0),
            padding,
            gap: space::M,
            ..Style::default()
        }
    }
}

impl Widget for SidePanel {
    fn name(&self) -> &'static str {
        "side-panel"
    }

    fn role(&self) -> Role {
        Role::Group
    }

    fn label(&self) -> Option<&str> {
        Some(&self.title)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::Rect {
            rect: bounds,
            color: color(palette.bg_surface),
        });
        list.push(Op::Rect {
            rect: Rect::new(bounds.origin.x, bounds.origin.y, 1.0, bounds.size.height),
            color: color(palette.border_subtle),
        });
        list.push(Op::TextRun {
            origin: Point::new(
                bounds.origin.x + space::L,
                bounds.origin.y + space::L + text::HEADING,
            ),
            text: self.title.clone(),
            size: text::HEADING,
            color: color(palette.text_primary),
        });
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        if is_key(event, Key::Escape) {
            cx.emit(Action::Dismiss);
            return Handled::Yes;
        }
        Handled::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;
    use crate::test_support::FixedAdvance;
    use crate::tree::{NodeId, Tree};
    use crate::widgets::{Button as PushButton, ButtonKind, FieldStyle, Group, TextField};

    fn mount(widget: Box<dyn Widget>, style: Style) -> (Tree, NodeId) {
        let mut tree = Tree::new(Box::new(Group));
        let id = tree.append_child(tree.root(), widget).unwrap();
        tree.set_style(id, style).unwrap();
        tree.layout(Size::new(1000.0, 400.0), &mut FixedAdvance);
        (tree, id)
    }

    fn pointer(tree: &mut Tree, event: fn(Point) -> Event, x: f32, y: f32) {
        tree.dispatch(&event(Point::new(x, y)));
    }

    fn down(at: Point) -> Event {
        Event::PointerDown {
            position: at,
            button: Button::Primary,
        }
    }

    fn up(at: Point) -> Event {
        Event::PointerUp {
            position: at,
            button: Button::Primary,
        }
    }

    fn moved(at: Point) -> Event {
        Event::PointerMove { position: at }
    }

    #[test]
    fn dragging_the_divider_resizes_within_limits() {
        let split = SplitPane::new(Axis::Row, 500);
        let style = split.style();
        let (mut tree, id) = mount(Box::new(split), style);
        pointer(&mut tree, down, 500.0, 50.0);
        pointer(&mut tree, moved, 300.0, 50.0);
        pointer(&mut tree, moved, 5.0, 50.0);
        pointer(&mut tree, up, 5.0, 50.0);
        assert_eq!(
            tree.take_actions(),
            [(id, Action::Resize(300)), (id, Action::Resize(MIN_SHARE))]
        );
    }

    #[test]
    fn arrow_keys_resize_a_focused_split() {
        let split = SplitPane::new(Axis::Column, 500);
        let style = split.style();
        let (mut tree, id) = mount(Box::new(split), style);
        tree.set_focus(Some(id), true);
        tree.dispatch(&Event::KeyDown {
            key: Key::ArrowDown,
            modifiers: Modifiers::default(),
        });
        assert_eq!(tree.take_actions(), [(id, Action::Resize(550))]);
    }

    #[test]
    fn pane_styles_split_the_space_by_share() {
        let mut tree = Tree::new(Box::new(Group));
        let split = tree
            .append_child(tree.root(), Box::new(SplitPane::new(Axis::Row, 250)))
            .unwrap();
        tree.set_style(split, SplitPane::new(Axis::Row, 250).style())
            .unwrap();
        let (left, right) = SplitPane::pane_styles(Axis::Row, 250);
        let a = tree.append_child(split, Box::new(Group)).unwrap();
        let b = tree.append_child(split, Box::new(Group)).unwrap();
        tree.set_style(a, left).unwrap();
        tree.set_style(b, right).unwrap();
        tree.layout(Size::new(806.0, 100.0), &mut FixedAdvance);
        // 800 DIPs after the divider, a quarter and three quarters.
        assert!((tree.bounds(a).unwrap().size.width - 200.0).abs() < 0.01);
        assert!((tree.bounds(b).unwrap().size.width - 600.0).abs() < 0.01);
    }

    fn strip() -> (Tree, NodeId) {
        let tabs = vec!["prod-db".into(), "web-01".into(), "Local".into()];
        mount(
            Box::new(TabStrip::new(tabs, 0)),
            Style {
                width: Length::Fixed(600.0),
                ..Style::default()
            },
        )
    }

    #[test]
    fn clicking_selects_dragging_reorders_and_the_close_button_closes() {
        let (mut tree, id) = strip();
        pointer(&mut tree, down, 250.0, 10.0);
        pointer(&mut tree, up, 250.0, 10.0);
        pointer(&mut tree, down, 50.0, 10.0);
        pointer(&mut tree, up, 450.0, 10.0);
        pointer(&mut tree, down, 590.0, 10.0);
        pointer(&mut tree, up, 590.0, 10.0);
        assert_eq!(
            tree.take_actions(),
            [
                (id, Action::Select(1)),
                (id, Action::Move { from: 0, to: 2 }),
                (id, Action::Close(2))
            ]
        );
    }

    #[test]
    fn middle_click_and_delete_close_tabs() {
        let (mut tree, id) = strip();
        tree.dispatch(&Event::PointerDown {
            position: Point::new(250.0, 10.0),
            button: Button::Middle,
        });
        tree.set_focus(Some(id), true);
        tree.dispatch(&Event::KeyDown {
            key: Key::Delete,
            modifiers: Modifiers::default(),
        });
        assert_eq!(
            tree.take_actions(),
            [(id, Action::Close(1)), (id, Action::Close(0))]
        );
    }

    #[test]
    fn side_panel_sits_at_the_right_edge_and_leaves_the_window_usable() {
        let mut tree = Tree::new(Box::new(Group));
        let button = tree
            .append_child(
                tree.root(),
                Box::new(PushButton::new("Connect", ButtonKind::Primary)),
            )
            .unwrap();
        let panel = tree
            .append_child(tree.root(), Box::new(SidePanel::new("Edit host")))
            .unwrap();
        tree.set_style(panel, SidePanel::style(1000.0, 360.0))
            .unwrap();
        let field = tree
            .append_child(panel, Box::new(TextField::new(FieldStyle::Text, "Label")))
            .unwrap();
        tree.layout(Size::new(1000.0, 400.0), &mut FixedAdvance);
        assert!((tree.bounds(panel).unwrap().origin.x - 640.0).abs() < 0.01);
        assert!(tree.bounds(field).unwrap().origin.x >= 640.0);

        tree.set_focus(Some(button), true);
        tree.dispatch(&Event::KeyDown {
            key: Key::Tab,
            modifiers: Modifiers::default(),
        });
        assert_eq!(
            tree.focused(),
            Some(field),
            "Tab reaches the panel; it is not a trap"
        );
        tree.dispatch(&Event::KeyDown {
            key: Key::Escape,
            modifiers: Modifiers::default(),
        });
        assert_eq!(tree.take_actions(), [(panel, Action::Dismiss)]);
    }
}
