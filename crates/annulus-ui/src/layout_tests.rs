//! Table-driven layout tests: a container style, its children's styles and content sizes, a
//! viewport, and the rectangles each child must get.

// Layout results are exact sums of exact inputs, so tests compare floats exactly. The case
// table is one long list by design.
#![allow(clippy::float_cmp, clippy::too_many_lines)]

use super::*;
use crate::display_list::{DisplayList, Op};
use crate::style::Edges;
use crate::test_support::FixedAdvance;
use crate::widget::{PaintCx, TextMeasure, Widget};

/// A widget whose content has a fixed size, like a label of known text.
struct Leaf(Size);

impl Widget for Leaf {
    fn name(&self) -> &'static str {
        "leaf"
    }

    fn content_size(&self, _available: Size, _text: &mut dyn TextMeasure) -> Size {
        self.0
    }

    fn paint(&self, cx: &PaintCx<'_>, list: &mut DisplayList) {
        list.push(Op::Rect {
            rect: cx.bounds,
            color: crate::geometry::Color::rgb(0, 0, 0),
        });
    }
}

struct Case {
    name: &'static str,
    parent: Style,
    children: Vec<(Style, Size)>,
    viewport: Size,
    expected: Vec<Rect>,
}

fn build(parent: Style, children: &[(Style, Size)]) -> (Tree, Vec<NodeId>) {
    let mut tree = Tree::new(Box::new(Leaf(Size::default())));
    tree.set_style(tree.root(), parent).unwrap();
    let ids = children
        .iter()
        .map(|(style, content)| {
            let id = tree
                .append_child(tree.root(), Box::new(Leaf(*content)))
                .unwrap();
            tree.set_style(id, *style).unwrap();
            id
        })
        .collect();
    (tree, ids)
}

fn fixed(width: f32, height: f32) -> Style {
    Style {
        width: Length::Fixed(width),
        height: Length::Fixed(height),
        ..Style::default()
    }
}

fn row() -> Style {
    Style {
        axis: Axis::Row,
        ..Style::default()
    }
}

const NO_CONTENT: Size = Size::new(0.0, 0.0);

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "column stacks fixed children with gap and padding, stretched across",
            parent: Style {
                gap: 4.0,
                padding: Edges::all(10.0),
                ..Style::default()
            },
            children: vec![
                (
                    Style {
                        height: Length::Fixed(20.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
                (
                    Style {
                        height: Length::Fixed(30.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
            ],
            viewport: Size::new(200.0, 300.0),
            expected: vec![
                Rect::new(10.0, 10.0, 180.0, 20.0),
                Rect::new(10.0, 34.0, 180.0, 30.0),
            ],
        },
        Case {
            name: "row fill children split the leftover by weight",
            parent: row(),
            children: vec![
                (
                    Style {
                        width: Length::Fixed(40.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
                (
                    Style {
                        width: Length::Fill(1.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
                (
                    Style {
                        width: Length::Fill(2.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
            ],
            viewport: Size::new(340.0, 50.0),
            expected: vec![
                Rect::new(0.0, 0.0, 40.0, 50.0),
                Rect::new(40.0, 0.0, 100.0, 50.0),
                Rect::new(140.0, 0.0, 200.0, 50.0),
            ],
        },
        Case {
            name: "fill child stops at its max",
            parent: row(),
            children: vec![(
                Style {
                    width: Length::Fill(1.0),
                    max: Size::new(80.0, f32::INFINITY),
                    ..Style::default()
                },
                NO_CONTENT,
            )],
            viewport: Size::new(300.0, 10.0),
            expected: vec![Rect::new(0.0, 0.0, 80.0, 10.0)],
        },
        Case {
            name: "fit child takes its content size plus padding",
            parent: Style {
                align: Align::Start,
                ..row()
            },
            children: vec![(
                Style {
                    padding: Edges::all(2.0),
                    ..Style::default()
                },
                Size::new(30.0, 12.0),
            )],
            viewport: Size::new(300.0, 100.0),
            expected: vec![Rect::new(0.0, 0.0, 34.0, 16.0)],
        },
        Case {
            name: "justify center and align center",
            parent: Style {
                justify: Justify::Center,
                align: Align::Center,
                ..row()
            },
            children: vec![(fixed(20.0, 10.0), NO_CONTENT)],
            viewport: Size::new(100.0, 50.0),
            expected: vec![Rect::new(40.0, 20.0, 20.0, 10.0)],
        },
        Case {
            name: "justify end and align end",
            parent: Style {
                justify: Justify::End,
                align: Align::End,
                ..row()
            },
            children: vec![(fixed(20.0, 10.0), NO_CONTENT)],
            viewport: Size::new(100.0, 50.0),
            expected: vec![Rect::new(80.0, 40.0, 20.0, 10.0)],
        },
        Case {
            name: "space between puts the ends on the edges",
            parent: Style {
                justify: Justify::SpaceBetween,
                align: Align::Start,
                ..row()
            },
            children: vec![
                (fixed(10.0, 10.0), NO_CONTENT),
                (fixed(10.0, 10.0), NO_CONTENT),
                (fixed(10.0, 10.0), NO_CONTENT),
            ],
            viewport: Size::new(110.0, 10.0),
            expected: vec![
                Rect::new(0.0, 0.0, 10.0, 10.0),
                Rect::new(50.0, 0.0, 10.0, 10.0),
                Rect::new(100.0, 0.0, 10.0, 10.0),
            ],
        },
        Case {
            name: "min wins over a smaller max",
            parent: Style {
                align: Align::Start,
                ..row()
            },
            children: vec![(
                Style {
                    min: Size::new(50.0, 0.0),
                    max: Size::new(20.0, f32::INFINITY),
                    ..Style::default()
                },
                Size::new(30.0, 5.0),
            )],
            viewport: Size::new(100.0, 10.0),
            expected: vec![Rect::new(0.0, 0.0, 50.0, 5.0)],
        },
        Case {
            name: "wrapping row starts a new line when the next card does not fit",
            parent: Style {
                wrap: true,
                gap: 10.0,
                padding: Edges::all(5.0),
                ..row()
            },
            children: vec![(fixed(100.0, 40.0), NO_CONTENT); 3],
            viewport: Size::new(225.0, 200.0),
            expected: vec![
                Rect::new(5.0, 5.0, 100.0, 40.0),
                Rect::new(115.0, 5.0, 100.0, 40.0),
                Rect::new(5.0, 55.0, 100.0, 40.0),
            ],
        },
        Case {
            name: "overlay sits at its offset and does not move flow siblings",
            parent: Style::default(),
            children: vec![
                (
                    Style {
                        position: Position::Overlay { x: 30.0, y: 40.0 },
                        ..fixed(50.0, 60.0)
                    },
                    NO_CONTENT,
                ),
                (
                    Style {
                        height: Length::Fixed(20.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
            ],
            viewport: Size::new(200.0, 200.0),
            expected: vec![
                Rect::new(30.0, 40.0, 50.0, 60.0),
                Rect::new(0.0, 0.0, 200.0, 20.0),
            ],
        },
        Case {
            name: "hostile styles never produce negative or NaN sizes",
            parent: Style {
                gap: -5.0,
                padding: Edges::all(f32::NAN),
                ..row()
            },
            children: vec![
                (
                    Style {
                        width: Length::Fill(f32::NAN),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
                (
                    Style {
                        width: Length::Fixed(-10.0),
                        ..Style::default()
                    },
                    NO_CONTENT,
                ),
            ],
            viewport: Size::new(100.0, 10.0),
            expected: vec![],
        },
    ]
}

#[test]
fn layout_cases() {
    for case in cases() {
        let (mut tree, ids) = build(case.parent, &case.children);
        tree.layout(case.viewport, &mut FixedAdvance);
        let got: Vec<Rect> = ids.iter().map(|id| tree.bounds(*id).unwrap()).collect();
        if case.expected.is_empty() {
            for rect in &got {
                let values = [
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                ];
                assert!(
                    values.iter().all(|v| !v.is_nan())
                        && rect.size.width >= 0.0
                        && rect.size.height >= 0.0,
                    "{}: {rect:?}",
                    case.name
                );
            }
        } else {
            assert_eq!(got, case.expected, "{}", case.name);
        }
    }
}

#[test]
fn scrolling_moves_children_and_stops_at_the_content_end() {
    let parent = Style {
        scroll: true,
        ..Style::default()
    };
    let children = vec![
        (
            Style {
                height: Length::Fixed(100.0),
                ..Style::default()
            },
            NO_CONTENT
        );
        5
    ];
    let (mut tree, ids) = build(parent, &children);
    tree.layout(Size::new(50.0, 200.0), &mut FixedAdvance);
    let root = tree.root();

    assert_eq!(tree.scroll_by(root, 1000.0).unwrap(), 300.0);
    tree.layout(Size::new(50.0, 200.0), &mut FixedAdvance);
    assert_eq!(
        tree.bounds(ids[3]).unwrap(),
        Rect::new(0.0, 0.0, 50.0, 100.0)
    );

    assert_eq!(tree.scroll_by(root, -1000.0).unwrap(), 0.0);
    assert_eq!(tree.scroll_by(root, f32::NAN).unwrap(), 0.0);
}

#[test]
fn scroll_container_clips_its_children_but_not_overlays() {
    let parent = Style {
        scroll: true,
        ..Style::default()
    };
    let children = vec![
        (
            Style {
                position: Position::Overlay { x: 0.0, y: 0.0 },
                ..fixed(5.0, 5.0)
            },
            NO_CONTENT,
        ),
        (
            Style {
                height: Length::Fixed(10.0),
                ..Style::default()
            },
            NO_CONTENT,
        ),
    ];
    let (mut tree, _) = build(parent, &children);
    tree.layout(Size::new(50.0, 50.0), &mut FixedAdvance);
    let ops = tree.paint();
    let kinds: Vec<&str> = ops
        .ops()
        .iter()
        .map(|op| match op {
            Op::PushClip(_) => "clip",
            Op::PopClip => "unclip",
            _ => "draw",
        })
        .collect();
    // Root, then the flow child inside the clip, then the overlay above and outside it.
    assert_eq!(kinds, ["draw", "clip", "draw", "unclip", "draw"]);
}

#[test]
fn unchanged_tree_and_viewport_relayout_nothing() {
    let (mut tree, _) = build(row(), &[(fixed(10.0, 10.0), NO_CONTENT)]);
    tree.layout(Size::new(100.0, 100.0), &mut FixedAdvance);
    tree.paint();
    tree.layout(Size::new(100.0, 100.0), &mut FixedAdvance);
    assert!(tree.damage().is_empty());
    tree.layout(Size::new(120.0, 100.0), &mut FixedAdvance);
    assert!(!tree.damage().is_empty());
}

#[test]
fn nested_fit_container_wraps_its_children() {
    let mut tree = Tree::new(Box::new(Leaf(Size::default())));
    tree.set_style(
        tree.root(),
        Style {
            align: Align::Start,
            ..Style::default()
        },
    )
    .unwrap();
    let card = tree
        .append_child(tree.root(), Box::new(Leaf(Size::default())))
        .unwrap();
    tree.set_style(
        card,
        Style {
            padding: Edges::all(8.0),
            gap: 4.0,
            ..row()
        },
    )
    .unwrap();
    for width in [20.0, 30.0] {
        let child = tree
            .append_child(card, Box::new(Leaf(Size::new(width, 10.0))))
            .unwrap();
        tree.set_style(child, Style::default()).unwrap();
    }
    tree.layout(Size::new(500.0, 500.0), &mut FixedAdvance);
    assert_eq!(tree.bounds(card).unwrap(), Rect::new(0.0, 0.0, 70.0, 26.0));
}
