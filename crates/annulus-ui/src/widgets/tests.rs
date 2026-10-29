//! Widget behavior through the tree: input in, actions and drawing out.

use annulus_config::tokens::{DARK, LIGHT};

use super::*;
use crate::display_list::Op;
use crate::geometry::{Point, Size};
use crate::input::{Button as Mouse, Event, Key, Modifiers};
use crate::style::{Length, Style};
use crate::test_support::FixedAdvance;
use crate::tree::{NodeId, Tree};
use crate::widget::{Action, Role, Widget};

/// A tree whose root column holds `widget` at its natural size, laid out in 400x300.
fn mount(widget: Box<dyn Widget>) -> (Tree, NodeId) {
    let mut tree = Tree::new(Box::new(Label::new("")));
    tree.set_style(
        tree.root(),
        Style {
            align: crate::style::Align::Start,
            ..Style::default()
        },
    )
    .unwrap();
    let id = tree.append_child(tree.root(), widget).unwrap();
    tree.layout(Size::new(400.0, 300.0), &mut FixedAdvance);
    (tree, id)
}

fn click(tree: &mut Tree, at: Point) {
    tree.dispatch(&Event::PointerDown {
        position: at,
        button: Mouse::Primary,
    });
    tree.dispatch(&Event::PointerUp {
        position: at,
        button: Mouse::Primary,
    });
}

fn click_on(tree: &mut Tree, id: NodeId) {
    let at = inside(tree, id);
    click(tree, at);
}

fn press(tree: &mut Tree, key: Key, modifiers: Modifiers) {
    tree.dispatch(&Event::KeyDown { key, modifiers });
}

fn actions(tree: &mut Tree) -> Vec<Action> {
    tree.take_actions()
        .into_iter()
        .map(|(_, action)| action)
        .collect()
}

fn inside(tree: &Tree, id: NodeId) -> Point {
    let bounds = tree.bounds(id).unwrap();
    Point::new(
        bounds.origin.x + 4.0,
        bounds.origin.y + bounds.size.height / 2.0,
    )
}

fn texts(tree: &mut Tree) -> Vec<String> {
    tree.paint(&mut FixedAdvance)
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::TextRun { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    meta: false,
};
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };

#[test]
fn button_activates_on_click_enter_and_space_but_not_on_release_outside() {
    let (mut tree, button) = mount(Box::new(Button::new("Connect", ButtonKind::Primary)));
    click_on(&mut tree, button);
    tree.set_focus(Some(button), true);
    press(&mut tree, Key::Enter, NONE);
    press(&mut tree, Key::Space, NONE);
    assert_eq!(
        actions(&mut tree),
        [Action::Activate, Action::Activate, Action::Activate]
    );

    tree.dispatch(&Event::PointerDown {
        position: inside(&tree, button),
        button: Mouse::Primary,
    });
    tree.dispatch(&Event::PointerUp {
        position: Point::new(399.0, 299.0),
        button: Mouse::Primary,
    });
    assert_eq!(actions(&mut tree), []);
}

#[test]
fn button_sizes_to_its_label() {
    let (tree, button) = mount(Box::new(Button::new("Connect", ButtonKind::Secondary)));
    // 7 characters at 13 DIPs with fixed half-size advances, plus 12 DIPs padding each side.
    assert!((tree.bounds(button).unwrap().size.width - (7.0 * 6.5 + 24.0)).abs() < 0.01);
}

#[test]
fn split_button_menu_part_opens_and_main_part_activates() {
    let (mut tree, split) = mount(Box::new(SplitButton::new("New host", ButtonKind::Primary)));
    let bounds = tree.bounds(split).unwrap();
    click(
        &mut tree,
        Point::new(bounds.max_x() - 5.0, bounds.origin.y + 10.0),
    );
    click(
        &mut tree,
        Point::new(bounds.origin.x + 5.0, bounds.origin.y + 10.0),
    );
    tree.set_focus(Some(split), true);
    press(&mut tree, Key::ArrowDown, Modifiers { alt: true, ..NONE });
    assert_eq!(
        actions(&mut tree),
        [Action::Open, Action::Activate, Action::Open]
    );
}

#[test]
fn checkbox_flips_and_reports_its_new_value() {
    let (mut tree, check) = mount(Box::new(Checkbox::new("Agent forwarding", false)));
    click_on(&mut tree, check);
    tree.set_focus(Some(check), true);
    press(&mut tree, Key::Space, NONE);
    assert_eq!(
        actions(&mut tree),
        [Action::Toggle(true), Action::Toggle(false)]
    );
}

#[test]
fn switch_flips_with_enter() {
    let (mut tree, switch) = mount(Box::new(Switch::new("Auto-reconnect", true)));
    tree.set_focus(Some(switch), true);
    press(&mut tree, Key::Enter, NONE);
    assert_eq!(actions(&mut tree), [Action::Toggle(false)]);
}

#[test]
fn radio_reports_its_index_only_when_not_already_chosen() {
    let (mut tree, chosen) = mount(Box::new(Radio::new("Password", 0, true)));
    click_on(&mut tree, chosen);
    assert_eq!(actions(&mut tree), []);
    let (mut tree, other) = mount(Box::new(Radio::new("Key", 1, false)));
    click_on(&mut tree, other);
    assert_eq!(actions(&mut tree), [Action::Select(1)]);
}

#[test]
fn text_field_edits_reports_changes_and_submits_on_enter() {
    let (mut tree, field) = mount(Box::new(TextField::new(FieldStyle::Text, "Hostname")));
    tree.set_focus(Some(field), true);
    tree.dispatch(&Event::Text("prod-db".into()));
    press(&mut tree, Key::Backspace, NONE);
    press(&mut tree, Key::ArrowLeft, NONE);
    press(&mut tree, Key::Enter, NONE);
    assert_eq!(
        actions(&mut tree),
        [Action::Edit, Action::Edit, Action::Activate]
    );
    assert!(texts(&mut tree).contains(&"prod-d".to_owned()));
}

#[test]
fn tab_is_left_to_the_tree_so_focus_moves_out_of_a_field() {
    let mut tree = Tree::new(Box::new(Label::new("")));
    let field = tree
        .append_child(
            tree.root(),
            Box::new(TextField::new(FieldStyle::Text, "Host")),
        )
        .unwrap();
    let next = tree
        .append_child(
            tree.root(),
            Box::new(Button::new("Save", ButtonKind::Primary)),
        )
        .unwrap();
    tree.layout(Size::new(400.0, 300.0), &mut FixedAdvance);
    tree.set_focus(Some(field), true);
    press(&mut tree, Key::Tab, NONE);
    assert_eq!(tree.focused(), Some(next));
}

#[test]
fn password_field_never_paints_or_reports_its_secret() {
    let (mut tree, field) = mount(Box::new(TextField::new(FieldStyle::Password, "Password")));
    assert_eq!(tree.widget(field).unwrap().role(), Role::PasswordInput);
    tree.set_focus(Some(field), true);
    tree.dispatch(&Event::Text("hunter2".into()));
    press(&mut tree, Key::Character('a'), CTRL);
    let painted = texts(&mut tree);
    assert!(
        painted.iter().all(|text| !text.contains("hunter")),
        "{painted:?}"
    );
    assert!(painted.contains(&"\u{2022}".repeat(7)));
}

#[test]
fn caret_sits_after_the_text_before_it() {
    let (mut tree, field) = mount(Box::new(TextField::new(FieldStyle::Text, "Port")));
    tree.set_focus(Some(field), false);
    tree.dispatch(&Event::Text("2222".into()));
    press(&mut tree, Key::ArrowLeft, NONE);
    let left = tree.bounds(field).unwrap().origin.x + 8.0;
    let caret_x = tree
        .paint(&mut FixedAdvance)
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::Rect { rect, .. } if (rect.size.width - 1.5).abs() < 0.01 => Some(rect.origin.x),
            _ => None,
        })
        .next_back()
        .unwrap();
    // Three characters at 13 DIPs with fixed advances of 6.5.
    assert!((caret_x - (left + 3.0 * 6.5)).abs() < 0.01, "{caret_x}");
}

#[test]
fn search_field_escape_clears_it() {
    let (mut tree, field) = mount(Box::new(TextField::new(FieldStyle::Search, "Search hosts")));
    tree.set_focus(Some(field), true);
    tree.dispatch(&Event::Text("web".into()));
    press(&mut tree, Key::Escape, NONE);
    assert_eq!(actions(&mut tree), [Action::Edit, Action::Edit]);
    assert!(texts(&mut tree).contains(&"Search hosts".to_owned()));
}

#[test]
fn select_opens_on_click_and_steps_with_arrows_within_bounds() {
    let options = vec!["Ed25519".to_owned(), "ECDSA".to_owned(), "RSA".to_owned()];
    let (mut tree, select) = mount(Box::new(Select::new("Key type", options, 0)));
    click_on(&mut tree, select);
    tree.set_focus(Some(select), true);
    press(&mut tree, Key::ArrowUp, NONE);
    press(&mut tree, Key::ArrowDown, NONE);
    press(&mut tree, Key::ArrowDown, NONE);
    press(&mut tree, Key::ArrowDown, NONE);
    assert_eq!(
        actions(&mut tree),
        [Action::Open, Action::Select(1), Action::Select(2)]
    );
}

#[test]
fn segmented_picks_the_segment_under_the_pointer_and_moves_with_arrows() {
    let options = vec!["Grid".to_owned(), "List".to_owned()];
    let (mut tree, segmented) = mount(Box::new(Segmented::new("View", options, 0)));
    tree.set_style(
        segmented,
        Style {
            width: Length::Fixed(200.0),
            ..Style::default()
        },
    )
    .unwrap();
    tree.layout(Size::new(400.0, 300.0), &mut FixedAdvance);
    click(&mut tree, Point::new(150.0, 10.0));
    tree.set_focus(Some(segmented), true);
    press(&mut tree, Key::ArrowLeft, NONE);
    press(&mut tree, Key::ArrowLeft, NONE);
    assert_eq!(actions(&mut tree), [Action::Select(1), Action::Select(0)]);
}

#[test]
fn every_widget_has_a_role_and_an_accessible_name() {
    let widgets: Vec<Box<dyn Widget>> = vec![
        Box::new(Button::new("Connect", ButtonKind::Primary)),
        Box::new(IconButton::new(crate::icons::SETTINGS, "Settings")),
        Box::new(SplitButton::new("New", ButtonKind::Secondary)),
        Box::new(Checkbox::new("Agent forwarding", false)),
        Box::new(Switch::new("Auto-reconnect", true)),
        Box::new(Radio::new("Password", 0, true)),
        Box::new(TextField::new(FieldStyle::Text, "Hostname")),
        Box::new(TextField::new(FieldStyle::Password, "Password")),
        Box::new(TextField::new(FieldStyle::Search, "Search")),
        Box::new(Select::new("Key type", vec!["Ed25519".into()], 0)),
        Box::new(Segmented::new(
            "View",
            vec!["Grid".into(), "List".into()],
            0,
        )),
        Box::new(Label::new("Hosts")),
        Box::new(Tooltip::new("Copy ssh command")),
        Box::new(Modal::new("Add host")),
    ];
    for widget in &widgets {
        assert_ne!(widget.role(), Role::Group, "{}", widget.name());
        assert!(
            widget.label().is_some_and(|label| !label.is_empty()),
            "{}",
            widget.name()
        );
        let interactive = !matches!(widget.role(), Role::Label | Role::Tooltip | Role::Dialog);
        assert_eq!(widget.focusable(), interactive, "{}", widget.name());
    }
}

#[test]
fn text_on_filled_controls_meets_contrast_in_both_themes() {
    for palette in [DARK, LIGHT] {
        for fill in [palette.accent, palette.status_error] {
            let ink = text_on(fill, &palette);
            assert!(
                ink.contrast(fill) >= 4.5,
                "{ink:?} on {fill:?}: {}",
                ink.contrast(fill)
            );
        }
    }
}

/// The vault behind an "Add host" dialog: a button underneath, a field and two buttons
/// inside.
struct DialogFixture {
    tree: Tree,
    behind: NodeId,
    modal: NodeId,
    field: NodeId,
    save: NodeId,
    cancel: NodeId,
}

fn dialog_fixture() -> DialogFixture {
    let mut tree = Tree::new(Box::new(Group));
    let root = tree.root();
    let behind = tree
        .append_child(root, Box::new(Button::new("New host", ButtonKind::Primary)))
        .unwrap();
    let modal = tree
        .append_child(root, Box::new(Modal::new("Add host")))
        .unwrap();
    tree.set_style(modal, Modal::style()).unwrap();
    let panel = tree
        .append_child(modal, Box::new(DialogPanel::new("Add host")))
        .unwrap();
    tree.set_style(panel, DialogPanel::style(300.0)).unwrap();
    let field = tree
        .append_child(
            panel,
            Box::new(TextField::new(FieldStyle::Text, "Hostname")),
        )
        .unwrap();
    let save = tree
        .append_child(panel, Box::new(Button::new("Save", ButtonKind::Primary)))
        .unwrap();
    let cancel = tree
        .append_child(panel, Box::new(Button::new("Cancel", ButtonKind::Ghost)))
        .unwrap();
    tree.layout(Size::new(800.0, 600.0), &mut FixedAdvance);
    DialogFixture {
        tree,
        behind,
        modal,
        field,
        save,
        cancel,
    }
}

#[test]
fn tab_stays_inside_an_open_modal() {
    let DialogFixture {
        mut tree,
        behind,
        field,
        save,
        cancel,
        ..
    } = dialog_fixture();
    tree.set_focus(Some(behind), true);
    let mut order = Vec::new();
    for _ in 0..4 {
        press(&mut tree, Key::Tab, NONE);
        order.push(tree.focused().unwrap());
    }
    assert_eq!(order, [field, save, cancel, field]);
}

#[test]
fn clicks_outside_a_modal_never_reach_the_app_underneath() {
    let DialogFixture {
        mut tree, behind, ..
    } = dialog_fixture();
    let under = inside(&tree, behind);
    assert_eq!(
        tree.hit_test(under),
        Some(behind),
        "the button lies under the scrim"
    );
    click(&mut tree, under);
    assert_eq!(actions(&mut tree), []);
    assert_ne!(tree.focused(), Some(behind));
}

#[test]
fn escape_in_a_modal_dismisses_it_and_keys_never_leak_out() {
    let DialogFixture {
        mut tree,
        behind,
        modal,
        field,
        ..
    } = dialog_fixture();
    tree.set_focus(Some(field), true);
    press(&mut tree, Key::Escape, NONE);
    assert_eq!(tree.take_actions(), [(modal, Action::Dismiss)]);

    // Focus left on the app underneath: Enter must not press that button.
    tree.set_focus(Some(behind), true);
    press(&mut tree, Key::Enter, NONE);
    assert_eq!(actions(&mut tree), []);
}
