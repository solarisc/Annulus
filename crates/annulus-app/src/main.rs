//! Annulus desktop binary: app state, view models, command registry, window, tab and pane management.
//!
//! Responsibilities and allowed dependencies: `docs/DESIGN.md` §4.
//!
//! For now this is the hello window: one window showing a few widgets, enough to
//! check input, focus, text and theming on each OS and to measure startup and memory.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use annulus_platform::cpu::{TEST_FONT, TEST_FONT_FAMILY, TextEngine};
use annulus_platform::window::{self, WindowApp};
use annulus_ui::tokens::{self, Appearance, Palette, text};
use annulus_ui::widgets::{Button, ButtonKind, FieldStyle, Group, Label, Switch, TextField};
use annulus_ui::{Action, Axis, Edges, NodeId, Style, Tree};

/// The hello window's state: which node is the theme switch.
struct Hello {
    theme_switch: NodeId,
    /// Set once the user flips the switch; until then the window follows the OS.
    chosen: Option<Appearance>,
}

impl WindowApp for Hello {
    fn title(&self) -> String {
        "Annulus".to_owned()
    }

    fn on_action(&mut self, tree: &mut Tree, node: NodeId, action: Action) {
        if node == self.theme_switch
            && let Action::Toggle(dark) = action
        {
            let appearance = if dark {
                Appearance::Dark
            } else {
                Appearance::Light
            };
            self.chosen = Some(appearance);
            tree.set_palette(palette(appearance));
        }
    }

    fn on_appearance(&mut self, tree: &mut Tree, appearance: Appearance) {
        if self.chosen.is_none() {
            tree.set_palette(palette(appearance));
        }
    }
}

fn palette(appearance: Appearance) -> Palette {
    Palette::for_appearance(appearance)
}

fn hello_tree() -> (Tree, NodeId) {
    let mut tree = Tree::new(Box::new(Group));
    let root = tree.root();
    let _ = tree.set_style(
        root,
        Style {
            padding: Edges::all(tokens::space::XL),
            gap: tokens::space::L,
            align: annulus_ui::Align::Start,
            ..Style::default()
        },
    );
    let mut add = |parent: NodeId, widget: Box<dyn annulus_ui::Widget>| {
        tree.append_child(parent, widget).ok()
    };
    add(root, Box::new(Label::heading("Annulus", text::TITLE)));
    add(
        root,
        Box::new(Label::secondary(
            "Hello window: input, focus, text and theming on this OS.",
        )),
    );
    let row = add(root, Box::new(Group));
    let theme_switch = add(root, Box::new(Switch::new("Dark theme", true)));
    if let Some(row) = row {
        add(
            row,
            Box::new(TextField::new(FieldStyle::Text, "user@host:22")),
        );
        add(row, Box::new(Button::new("Connect", ButtonKind::Primary)));
        add(row, Box::new(Button::new("Edit", ButtonKind::Secondary)));
        let _ = tree.set_style(
            row,
            Style {
                axis: Axis::Row,
                gap: tokens::space::S,
                align: annulus_ui::Align::Center,
                ..Style::default()
            },
        );
    }
    let theme_switch = theme_switch.unwrap_or(root);
    (tree, theme_switch)
}

fn main() -> ExitCode {
    let (tree, theme_switch) = hello_tree();
    // TODO: use the system UI font through fontique instead of the bundled one.
    let text = TextEngine::with_fonts(&[TEST_FONT], TEST_FONT_FAMILY);
    match window::run(
        tree,
        text,
        Hello {
            theme_switch,
            chosen: None,
        },
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
