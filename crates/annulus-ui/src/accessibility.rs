//! The accessibility tree: the widget tree as screen readers see it, as an AccessKit update.
//! The per-OS adapters (UIA, `NSAccessibility`, AT-SPI) in `annulus-platform` pass
//! it on unchanged.

use accesskit::{
    Action as AxAction, Node as AxNode, NodeId as AxId, Rect as AxRect, Role as AxRole, Toggled,
    TreeId, TreeInfo, TreeUpdate,
};

use crate::tree::{NodeId, Tree};
use crate::widget::Role;

impl Tree {
    /// The whole accessibility tree, with the focused node (or the root) as focus. Nodes
    /// appear in paint order, so screen readers walk them in visual order.
    #[must_use]
    pub fn accessibility_update(&self) -> TreeUpdate {
        let mut nodes = Vec::new();
        let mut pending = vec![self.root()];
        while let Some(id) = pending.pop() {
            let (overlays, flow) = self.split_children(id);
            let children: Vec<NodeId> = flow.into_iter().chain(overlays).collect();
            if let Some(node) = self.accessibility_node(id, &children) {
                nodes.push((ax_id(id), node));
            }
            pending.extend(children.iter().rev());
        }
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo {
                toolkit_name: Some("annulus-ui".into()),
                ..TreeInfo::new(ax_id(self.root()))
            }),
            tree_id: TreeId::ROOT,
            focus: ax_id(self.focused().unwrap_or(self.root())),
        }
    }

    fn accessibility_node(&self, id: NodeId, children: &[NodeId]) -> Option<AxNode> {
        let widget = self.widget(id)?;
        let role = if id == self.root() {
            AxRole::Window
        } else {
            ax_role(widget.role())
        };
        let mut node = AxNode::new(role);
        if let Some(label) = widget.label().filter(|label| !label.is_empty()) {
            node.set_label(label);
        }
        if let Some(value) = widget.value() {
            node.set_value(value);
        }
        match (widget.role(), widget.checked()) {
            (Role::RadioButton, Some(checked)) => node.set_selected(checked),
            (_, Some(checked)) => node.set_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            }),
            (_, None) => {}
        }
        if widget.modal() {
            node.set_modal();
        }
        if widget.focusable() {
            node.add_action(AxAction::Focus);
            node.add_action(AxAction::Click);
        }
        if let Some(bounds) = self.bounds(id) {
            node.set_bounds(AxRect {
                x0: f64::from(bounds.origin.x),
                y0: f64::from(bounds.origin.y),
                x1: f64::from(bounds.max_x()),
                y1: f64::from(bounds.max_y()),
            });
        }
        node.set_children(
            children
                .iter()
                .map(|child| ax_id(*child))
                .collect::<Vec<_>>(),
        );
        Some(node)
    }
}

fn ax_id(id: NodeId) -> AxId {
    AxId(id.to_raw())
}

fn ax_role(role: Role) -> AxRole {
    match role {
        Role::Group => AxRole::GenericContainer,
        Role::Label => AxRole::Label,
        Role::Button => AxRole::Button,
        Role::CheckBox => AxRole::CheckBox,
        Role::Switch => AxRole::Switch,
        Role::RadioButton => AxRole::RadioButton,
        Role::TextInput => AxRole::TextInput,
        Role::PasswordInput => AxRole::PasswordInput,
        Role::SearchInput => AxRole::SearchInput,
        Role::ComboBox => AxRole::ComboBox,
        Role::RadioGroup => AxRole::RadioGroup,
        Role::Tooltip => AxRole::Tooltip,
        Role::Dialog => AxRole::Dialog,
        Role::Alert => AxRole::Alert,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::geometry::Size;
    use crate::input::Event;
    use crate::style::{Axis, Style};
    use crate::test_support::FixedAdvance;
    use crate::widgets::{
        Button, ButtonKind, Checkbox, FieldStyle, Group, Label, Radio, Select, TextField,
    };

    /// One line per node: depth, role, name, value and states, in tree order.
    fn outline(update: &TreeUpdate) -> String {
        let nodes: std::collections::HashMap<_, _> =
            update.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let mut out = String::new();
        let mut pending = vec![(update.tree.as_ref().unwrap().root, 0)];
        while let Some((id, depth)) = pending.pop() {
            let node = nodes[&id];
            let _ = write!(out, "{}{:?}", "  ".repeat(depth), node.role());
            if let Some(label) = node.label() {
                let _ = write!(out, " {label:?}");
            }
            if let Some(value) = node.value() {
                let _ = write!(out, " value={value:?}");
            }
            if let Some(toggled) = node.toggled() {
                let _ = write!(out, " toggled={toggled:?}");
            }
            if let Some(selected) = node.is_selected() {
                let _ = write!(out, " selected={selected}");
            }
            if update.focus == id {
                out.push_str(" focused");
            }
            out.push('\n');
            pending.extend(
                node.children()
                    .iter()
                    .rev()
                    .map(|child| (*child, depth + 1)),
            );
        }
        out
    }

    #[test]
    fn form_snapshot_names_every_control_with_its_state() {
        let mut tree = Tree::new(Box::new(Label::new("")));
        let root = tree.root();
        let row = tree.append_child(root, Box::new(Group)).unwrap();
        tree.set_style(
            row,
            Style {
                axis: Axis::Row,
                ..Style::default()
            },
        )
        .unwrap();
        let host = tree
            .append_child(row, Box::new(TextField::new(FieldStyle::Text, "Hostname")))
            .unwrap();
        let password = tree
            .append_child(
                row,
                Box::new(TextField::new(FieldStyle::Password, "Password")),
            )
            .unwrap();
        tree.append_child(root, Box::new(Checkbox::new("Agent forwarding", true)))
            .unwrap();
        tree.append_child(root, Box::new(Radio::new("Key", 1, false)))
            .unwrap();
        tree.append_child(
            root,
            Box::new(Select::new(
                "Key type",
                vec!["Ed25519".into(), "RSA".into()],
                1,
            )),
        )
        .unwrap();
        tree.append_child(root, Box::new(Button::new("Save", ButtonKind::Primary)))
            .unwrap();
        tree.layout(Size::new(600.0, 400.0), &mut FixedAdvance);
        tree.set_focus(Some(host), false);
        tree.dispatch(&Event::Text("prod-db".into()));
        tree.set_focus(Some(password), false);
        tree.dispatch(&Event::Text("hunter2".into()));

        let expected = "\
Window
  GenericContainer
    TextInput \"Hostname\" value=\"prod-db\"
    PasswordInput \"Password\" focused
  CheckBox \"Agent forwarding\" toggled=True
  RadioButton \"Key\" selected=false
  ComboBox \"Key type\" value=\"RSA\"
  Button \"Save\"
";
        assert_eq!(outline(&tree.accessibility_update()), expected);
    }

    #[test]
    fn interactive_nodes_offer_focus_and_click_and_have_bounds() {
        let mut tree = Tree::new(Box::new(Label::new("")));
        let button = tree
            .append_child(
                tree.root(),
                Box::new(Button::new("Connect", ButtonKind::Primary)),
            )
            .unwrap();
        tree.layout(Size::new(300.0, 100.0), &mut FixedAdvance);
        let update = tree.accessibility_update();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ax_id(button))
            .unwrap();
        assert!(node.supports_action(AxAction::Focus) && node.supports_action(AxAction::Click));
        let bounds = node.bounds().unwrap();
        assert!(bounds.x1 > bounds.x0 && bounds.y1 > bounds.y0);
    }
}
