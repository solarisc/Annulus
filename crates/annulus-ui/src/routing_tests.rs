use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Rect, Size};
use crate::input::{Button, Modifiers};
use crate::style::{Axis, Length, Style};
use crate::widget::Widget;

type Log = Rc<RefCell<Vec<(&'static str, &'static str)>>>;

/// Records the events it receives and handles the kinds it is told to.
struct Probe {
    tag: &'static str,
    focusable: bool,
    handles: &'static [&'static str],
    log: Log,
}

fn kind(event: &Event) -> &'static str {
    match event {
        Event::PointerDown { .. } => "down",
        Event::PointerUp { .. } => "up",
        Event::PointerMove { .. } => "move",
        Event::Wheel { .. } => "wheel",
        Event::KeyDown { .. } => "key",
        Event::Text(_) => "text",
    }
}

impl Widget for Probe {
    fn name(&self) -> &'static str {
        self.tag
    }

    fn paint(&self, _bounds: Rect, _list: &mut DisplayList) {}

    fn focusable(&self) -> bool {
        self.focusable
    }

    fn handle(&mut self, event: &Event) -> Handled {
        self.log.borrow_mut().push((self.tag, kind(event)));
        if self.handles.contains(&kind(event)) {
            Handled::Yes
        } else {
            Handled::No
        }
    }
}

struct Fixture {
    tree: Tree,
    log: Log,
}

impl Fixture {
    fn new() -> Self {
        let log = Log::default();
        let root = Probe {
            tag: "root",
            focusable: false,
            handles: &[],
            log: log.clone(),
        };
        Self {
            tree: Tree::new(Box::new(root)),
            log,
        }
    }

    fn add(
        &mut self,
        parent: NodeId,
        tag: &'static str,
        focusable: bool,
        handles: &'static [&'static str],
        style: Style,
    ) -> NodeId {
        let probe = Probe {
            tag,
            focusable,
            handles,
            log: self.log.clone(),
        };
        let id = self.tree.append_child(parent, Box::new(probe)).unwrap();
        self.tree.set_style(id, style).unwrap();
        id
    }

    fn received(&self) -> Vec<(&'static str, &'static str)> {
        self.log.borrow_mut().drain(..).collect()
    }
}

fn height(h: f32) -> Style {
    Style {
        height: Length::Fixed(h),
        ..Style::default()
    }
}

fn down(x: f32, y: f32) -> Event {
    Event::PointerDown {
        position: Point::new(x, y),
        button: Button::Primary,
    }
}

fn tab(shift: bool) -> Event {
    Event::KeyDown {
        key: Key::Tab,
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
    }
}

#[test]
fn overlay_wins_the_hit_test_over_the_flow_child_below_it() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let row = f.add(root, "row", false, &[], height(50.0));
    let overlay = f.add(
        root,
        "menu",
        false,
        &[],
        Style {
            position: Position::Overlay { x: 0.0, y: 0.0 },
            width: Length::Fixed(20.0),
            height: Length::Fixed(20.0),
            ..Style::default()
        },
    );
    f.tree.layout(Size::new(100.0, 100.0));
    assert_eq!(f.tree.hit_test(Point::new(5.0, 5.0)), Some(overlay));
    assert_eq!(f.tree.hit_test(Point::new(50.0, 10.0)), Some(row));
    assert_eq!(f.tree.hit_test(Point::new(50.0, 90.0)), Some(root));
    assert_eq!(f.tree.hit_test(Point::new(500.0, 500.0)), None);
}

#[test]
fn children_scrolled_out_of_view_cannot_be_hit() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    f.tree
        .set_style(
            root,
            Style {
                axis: Axis::Column,
                ..Style::default()
            },
        )
        .unwrap();
    let list = f.add(
        root,
        "list",
        false,
        &[],
        Style {
            scroll: true,
            ..height(100.0)
        },
    );
    let first = f.add(list, "first", false, &[], height(80.0));
    let second = f.add(list, "second", false, &[], height(80.0));
    f.tree.layout(Size::new(100.0, 300.0));
    // The second item ends at y=160, outside the 100-high list.
    assert_eq!(f.tree.hit_test(Point::new(10.0, 150.0)), Some(root));
    f.tree.scroll_by(list, 60.0).unwrap();
    f.tree.layout(Size::new(100.0, 300.0));
    assert_eq!(f.tree.hit_test(Point::new(10.0, 50.0)), Some(second));
    assert_ne!(f.tree.hit_test(Point::new(10.0, 10.0)), Some(second));
    let _ = first;
}

#[test]
fn tab_walks_focusable_nodes_in_visual_order_and_wraps() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let a = f.add(root, "a", true, &[], height(10.0));
    let group = f.add(root, "group", false, &[], height(10.0));
    let b = f.add(group, "b", true, &[], height(10.0));
    let c = f.add(root, "c", true, &[], height(10.0));
    f.tree.layout(Size::new(100.0, 100.0));

    let mut order = Vec::new();
    for _ in 0..4 {
        f.tree.dispatch(&tab(false));
        order.push(f.tree.focused().unwrap());
    }
    assert_eq!(order, [a, b, c, a]);
    f.tree.dispatch(&tab(true));
    assert_eq!(f.tree.focused(), Some(c));
    assert!(f.tree.focus_visible());
}

#[test]
fn focus_ring_shows_after_keyboard_focus_but_not_after_a_click() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let button = f.add(root, "button", true, &[], height(20.0));
    f.tree.layout(Size::new(100.0, 100.0));
    let has_ring = |tree: &mut Tree| {
        tree.paint()
            .ops()
            .iter()
            .any(|op| matches!(op, Op::Border { .. }))
    };

    f.tree.dispatch(&down(5.0, 5.0));
    assert_eq!(f.tree.focused(), Some(button));
    assert!(!has_ring(&mut f.tree));

    f.tree.dispatch(&tab(false));
    assert!(has_ring(&mut f.tree));
}

#[test]
fn removed_focused_node_is_no_longer_focused() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let a = f.add(root, "a", true, &[], height(10.0));
    f.tree.set_focus(Some(a), true);
    f.tree.remove(a).unwrap();
    assert_eq!(f.tree.focused(), None);
}

#[test]
fn keys_bubble_from_the_focused_node_to_the_first_handler() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let dialog = f.add(root, "dialog", false, &["key"], height(50.0));
    let field = f.add(dialog, "field", true, &["text"], height(20.0));
    f.tree.set_focus(Some(field), false);
    let escape = Event::KeyDown {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    };
    assert_eq!(f.tree.dispatch(&escape), Some(dialog));
    assert_eq!(f.received(), [("field", "key"), ("dialog", "key")]);
    assert_eq!(f.tree.dispatch(&Event::Text("x".into())), Some(field));
}

#[test]
fn pressed_widget_keeps_the_pointer_until_release() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let slider = f.add(root, "slider", false, &["down", "move", "up"], height(10.0));
    f.tree.layout(Size::new(100.0, 100.0));
    assert_eq!(f.tree.dispatch(&down(5.0, 5.0)), Some(slider));
    // Dragged far outside the slider: it still gets the moves and the release.
    let far = Point::new(90.0, 90.0);
    assert_eq!(
        f.tree.dispatch(&Event::PointerMove { position: far }),
        Some(slider)
    );
    assert_eq!(
        f.tree.dispatch(&Event::PointerUp {
            position: far,
            button: Button::Primary
        }),
        Some(slider)
    );
    assert_eq!(f.tree.dispatch(&Event::PointerMove { position: far }), None);
}

#[test]
fn unhandled_wheel_scrolls_the_nearest_scrolling_ancestor() {
    let mut f = Fixture::new();
    let root = f.tree.root();
    let list = f.add(
        root,
        "list",
        false,
        &[],
        Style {
            scroll: true,
            ..height(100.0)
        },
    );
    for tag in ["one", "two", "three"] {
        f.add(list, tag, false, &[], height(80.0));
    }
    f.tree.layout(Size::new(100.0, 300.0));
    let wheel = Event::Wheel {
        position: Point::new(10.0, 10.0),
        delta: 30.0,
    };
    assert_eq!(f.tree.dispatch(&wheel), Some(list));
    assert_eq!(f.tree.scroll_offset(list), Some(30.0));
}
