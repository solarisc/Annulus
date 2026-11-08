//! The accessibility tree (UI-SPEC §Accessibility): every zone of a frame as an
//! AccessKit node, nested the way the screen nests them, for the window's platform adapter
//! (UI Automation on Windows, AT-SPI on Linux, `NSAccessibility` on macOS).
//!
//! A zone drawn inside an earlier list row, scroll view or tab is that zone's child, so a
//! screen reader hears "Delete, in Work laptop" rather than one of many "Delete" buttons. A
//! backdrop starts a new layer: the menu or card above it hangs off the window, not off the
//! row it happens to cover. Secret fields are password inputs and never carry their text.

use std::collections::{HashMap, HashSet};

use accesskit::{
    Action, ActionRequest, Node, NodeId, Rect as AxRect, Role as AxRole, Toggled, TreeId, TreeInfo,
    TreeUpdate,
};

use crate::gfx::Rect;
use crate::kit::{Id, Role, Zone};

/// The AccessKit crate, for the platform adapters and tests that read the tree.
pub use accesskit;

/// The window's node, the root of the tree.
pub const ROOT: NodeId = NodeId(0);

/// What assistive technology asked for, in the toolkit's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Move keyboard focus to the zone.
    Focus(Id),
    /// Activate it, as a click or Enter would.
    Click(Id),
}

impl Request {
    /// The request for `action`, if the toolkit does that action.
    #[must_use]
    pub fn from_action(action: &ActionRequest) -> Option<Self> {
        let id = Id(action.target_node.0);
        match action.action {
            Action::Focus => Some(Request::Focus(id)),
            Action::Click => Some(Request::Click(id)),
            _ => None,
        }
    }
}

fn role(zone: &Zone) -> AxRole {
    match zone.role {
        Role::Button | Role::WindowButton => AxRole::Button,
        Role::Tab => AxRole::Tab,
        Role::TextInput if zone.secret => AxRole::PasswordInput,
        Role::TextInput => AxRole::TextInput,
        Role::CheckBox => AxRole::CheckBox,
        Role::ComboBox => AxRole::ComboBox,
        Role::ListItem => AxRole::ListItem,
        Role::MenuItem => AxRole::MenuItem,
        Role::ScrollView => AxRole::ScrollView,
        Role::Dialog => AxRole::Dialog,
        Role::Splitter => AxRole::Splitter,
        Role::Terminal => AxRole::Terminal,
        Role::Backdrop => AxRole::GenericContainer,
    }
}

/// Zones that hold the zones drawn inside them: a card's buttons, a list's rows, a tab's
/// close button.
fn holds_others(role: Role) -> bool {
    matches!(
        role,
        Role::ListItem | Role::ScrollView | Role::Tab | Role::Dialog
    )
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.x >= outer.x - 0.5
        && inner.y >= outer.y - 0.5
        && inner.r() <= outer.r() + 0.5
        && inner.b() <= outer.b() + 0.5
}

fn bounds(rect: Rect, scale: f32) -> AxRect {
    let scale = f64::from(scale);
    AxRect {
        x0: f64::from(rect.x) * scale,
        y0: f64::from(rect.y) * scale,
        x1: f64::from(rect.r()) * scale,
        y1: f64::from(rect.b()) * scale,
    }
}

fn node(zone: &Zone, scale: f32) -> Node {
    let mut node = Node::new(role(zone));
    if !zone.label.is_empty() {
        node.set_label(zone.label.clone());
    }
    node.set_bounds(bounds(zone.rect, scale));
    match zone.role {
        Role::TextInput | Role::Terminal => node.add_action(Action::Focus),
        // Clickable, but never in the Tab order.
        Role::WindowButton | Role::MenuItem => node.add_action(Action::Click),
        Role::ScrollView | Role::Dialog | Role::Splitter | Role::Backdrop => {}
        Role::Button | Role::Tab | Role::CheckBox | Role::ComboBox | Role::ListItem => {
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
        }
    }
    if let Some(on) = zone.toggled {
        node.set_toggled(Toggled::from(on));
    }
    // A secret's text never leaves the toolkit; its role says it is one.
    if !zone.secret
        && let Some(value) = &zone.value
    {
        node.set_value(value.clone());
    }
    node
}

/// The full tree of a frame's `zones` (top-most last) in a window titled `title`, with
/// keyboard focus on `focus` and `scale` device pixels per DIP.
#[must_use]
pub fn tree(zones: &[Zone], focus: Option<Id>, scale: f32, title: &str) -> TreeUpdate {
    let mut nodes: Vec<(NodeId, Node)> = Vec::with_capacity(zones.len() + 1);
    let mut children: Vec<Vec<NodeId>> = Vec::with_capacity(zones.len() + 1);
    let mut root = Node::new(AxRole::Window);
    root.set_label(title.to_owned());
    nodes.push((ROOT, root));
    children.push(Vec::new());
    let mut seen: HashSet<NodeId> = HashSet::from([ROOT]);
    // The open holders, innermost last: (index in `nodes`, rect).
    let mut holders: Vec<(usize, Rect)> = Vec::new();
    for zone in zones {
        if zone.role == Role::Backdrop {
            holders.clear();
            continue;
        }
        if zone.role == Role::Dialog {
            // A modal dialog is the only part of the window a screen reader can reach.
            nodes.truncate(1);
            children.truncate(1);
            children[0].clear();
            seen.clear();
            seen.insert(ROOT);
            holders.clear();
        }
        let id = NodeId(zone.id.0);
        if !seen.insert(id) {
            continue;
        }
        while holders
            .last()
            .is_some_and(|(_, rect)| !inside(zone.rect, *rect))
        {
            holders.pop();
        }
        let parent = holders.last().map_or(0, |(index, _)| *index);
        children[parent].push(id);
        let index = nodes.len();
        nodes.push((id, node(zone, scale)));
        children.push(Vec::new());
        if holds_others(zone.role) {
            holders.push((index, zone.rect));
        }
    }
    for ((_, node), kids) in nodes.iter_mut().zip(children) {
        if !kids.is_empty() {
            node.set_children(kids);
        }
    }
    let focus = focus
        .map(|id| NodeId(id.0))
        .filter(|id| seen.contains(id))
        .unwrap_or(ROOT);
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo {
            root: ROOT,
            toolkit_name: Some("Annulus".into()),
            toolkit_version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        tree_id: TreeId::ROOT,
        focus,
    }
}

/// Problems with a tree (empty when it is sound): a node that is not the child of exactly
/// one other, a child outside its parent's bounds, a focus on no node. The scene tests run
/// it over every screen.
#[must_use]
pub fn faults(update: &TreeUpdate) -> Vec<String> {
    let mut faults = Vec::new();
    let by_id: HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
    if by_id.len() != update.nodes.len() {
        faults.push("two nodes share an id".to_owned());
    }
    let mut parents: HashMap<NodeId, usize> = HashMap::new();
    for (id, node) in &update.nodes {
        for child in node.children() {
            *parents.entry(*child).or_default() += 1;
            match (by_id.get(child), node.bounds()) {
                (None, _) => faults.push(format!("{id:?} has a child {child:?} that is no node")),
                (Some(kid), Some(outer)) if *id != ROOT => {
                    if let Some(inner) = kid.bounds()
                        && (inner.x0 < outer.x0 - 1.0
                            || inner.y0 < outer.y0 - 1.0
                            || inner.x1 > outer.x1 + 1.0
                            || inner.y1 > outer.y1 + 1.0)
                    {
                        faults.push(format!(
                            "{:?} lies outside its parent {:?}",
                            kid.label(),
                            node.label()
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    for (id, node) in &update.nodes {
        let count = parents.get(id).copied().unwrap_or(0);
        if *id == ROOT && count != 0 {
            faults.push("the window is someone's child".to_owned());
        } else if *id != ROOT && count != 1 {
            faults.push(format!("{:?} has {count} parents", node.label()));
        }
    }
    if !by_id.contains_key(&update.focus) {
        faults.push(format!("focus {:?} is no node", update.focus));
    }
    faults
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(label: &str, role: Role, rect: Rect) -> Zone {
        Zone::new(Id::new(label), rect, role, label)
    }

    fn by_label<'a>(update: &'a TreeUpdate, label: &str) -> (NodeId, &'a Node) {
        let (id, node) = update
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap_or_else(|| panic!("no node {label:?}"));
        (*id, node)
    }

    #[test]
    fn a_rows_buttons_are_its_children_and_a_menu_above_a_backdrop_is_the_windows() {
        let zones = [
            zone("Search", Role::TextInput, Rect::new(0.0, 0.0, 300.0, 32.0)),
            zone(
                "Work laptop",
                Role::ListItem,
                Rect::new(0.0, 40.0, 400.0, 56.0),
            ),
            zone("Delete", Role::Button, Rect::new(360.0, 53.0, 30.0, 30.0)),
            zone(
                "Home server",
                Role::ListItem,
                Rect::new(0.0, 104.0, 400.0, 56.0),
            ),
            zone("Delete ", Role::Button, Rect::new(360.0, 117.0, 30.0, 30.0)),
            zone("", Role::Backdrop, Rect::new(0.0, 0.0, 800.0, 600.0)),
            zone(
                "Duplicate",
                Role::MenuItem,
                Rect::new(300.0, 60.0, 90.0, 28.0),
            ),
        ];
        let update = tree(&zones, Some(Id::new("Search")), 2.0, "Annulus");
        assert_eq!(faults(&update), Vec::<String>::new());
        let (_, laptop) = by_label(&update, "Work laptop");
        let (delete, _) = by_label(&update, "Delete");
        assert_eq!(laptop.children(), [delete]);
        let (_, window) = by_label(&update, "Annulus");
        let (search, _) = by_label(&update, "Search");
        let (menu, _) = by_label(&update, "Duplicate");
        assert_eq!(
            window.children().len(),
            4,
            "search, two rows, the menu item"
        );
        assert!(window.children().contains(&menu));
        assert_eq!(update.focus, search);
        // Bounds are in device pixels.
        assert_eq!(laptop.bounds().map(|rect| rect.x1), Some(800.0));
    }

    #[test]
    fn a_modal_dialog_hides_the_window_behind_it_from_the_tree() {
        let zones = [
            zone("Hosts", Role::Button, Rect::new(0.0, 0.0, 80.0, 32.0)),
            zone("", Role::Backdrop, Rect::new(0.0, 0.0, 800.0, 600.0)),
            zone(
                "Command palette",
                Role::Dialog,
                Rect::new(200.0, 60.0, 400.0, 300.0),
            ),
            zone(
                "Search commands",
                Role::TextInput,
                Rect::new(220.0, 80.0, 360.0, 40.0),
            ),
        ];
        let update = tree(&zones, Some(Id::new("Search commands")), 1.0, "Annulus");
        assert!(
            update
                .nodes
                .iter()
                .all(|(_, node)| node.label() != Some("Hosts"))
        );
        let (dialog, node) = by_label(&update, "Command palette");
        let (search, _) = by_label(&update, "Search commands");
        assert_eq!(node.role(), AxRole::Dialog);
        assert_eq!(node.children(), [search]);
        assert_eq!(update.focus, search);
        assert_ne!(dialog, search);
    }

    #[test]
    fn a_secret_field_is_a_password_input_without_its_text() {
        let mut password = zone(
            "Master password",
            Role::TextInput,
            Rect::new(0.0, 0.0, 200.0, 32.0),
        );
        password.value = Some("hunter2".into());
        password.secret = true;
        let mut host = zone(
            "Address",
            Role::TextInput,
            Rect::new(0.0, 40.0, 200.0, 32.0),
        );
        host.value = Some("10.0.4.12".into());
        let mut remember = zone(
            "Remember",
            Role::CheckBox,
            Rect::new(0.0, 80.0, 200.0, 32.0),
        );
        remember.toggled = Some(true);
        let update = tree(&[password, host, remember], None, 1.0, "Annulus");
        let (_, password) = by_label(&update, "Master password");
        assert_eq!(password.role(), AxRole::PasswordInput);
        assert_eq!(password.value(), None);
        assert!(
            update
                .nodes
                .iter()
                .all(|(_, node)| node.value() != Some("hunter2"))
        );
        assert_eq!(by_label(&update, "Address").1.value(), Some("10.0.4.12"));
        assert_eq!(
            by_label(&update, "Remember").1.toggled(),
            Some(Toggled::True)
        );
        assert_eq!(update.focus, ROOT, "nothing focused: the window");
    }

    #[test]
    fn requests_map_back_to_zones_and_unknown_actions_are_ignored() {
        let id = Id::new("Connect");
        let request = |action| ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: NodeId(id.0),
            data: None,
        };
        assert_eq!(
            Request::from_action(&request(Action::Click)),
            Some(Request::Click(id))
        );
        assert_eq!(
            Request::from_action(&request(Action::Focus)),
            Some(Request::Focus(id))
        );
        assert_eq!(Request::from_action(&request(Action::ScrollDown)), None);
    }

    #[test]
    fn the_lint_finds_a_second_parent_and_a_lost_focus() {
        let mut update = tree(
            &[zone("A", Role::Button, Rect::new(0.0, 0.0, 10.0, 10.0))],
            None,
            1.0,
            "Annulus",
        );
        let a = update.nodes[1].0;
        update.nodes[0].1.push_child(a);
        update.focus = NodeId(42);
        assert_eq!(faults(&update).len(), 2, "{:?}", faults(&update));
    }
}
