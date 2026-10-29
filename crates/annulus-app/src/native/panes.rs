//! Split panes in a terminal tab (UI-SPEC §3): a tree of side-by-side and stacked splits,
//! dividers that drag, keyboard focus between panes, and broadcast input that types into
//! every pane at once.

use annulus_platform::window::Waker;
use annulus_term::Palette;
use annulus_ui::gfx::{Align, Font, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Role, Ui};
use annulus_ui::widgets::SMALL;

use super::live::LiveTerminal;
use crate::terminal_view::TabStatus;

/// Width of a divider's drag handle.
const HANDLE: f32 = 6.0;

/// How a tab is split.
enum Split {
    Leaf(u64),
    Node {
        /// Side by side (a vertical divider) rather than stacked.
        across: bool,
        /// The first part's share, 0.15 to 0.85.
        ratio: f32,
        first: Box<Split>,
        second: Box<Split>,
    },
}

impl Split {
    fn leaves(&self, out: &mut Vec<u64>) {
        match self {
            Split::Leaf(id) => out.push(*id),
            Split::Node { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }

    /// Replaces leaf `target` with a split of it and `new`.
    fn split(&mut self, target: u64, new: u64, across: bool) -> bool {
        match self {
            Split::Leaf(id) if *id == target => {
                *self = Split::Node {
                    across,
                    ratio: 0.5,
                    first: Box::new(Split::Leaf(target)),
                    second: Box::new(Split::Leaf(new)),
                };
                true
            }
            Split::Leaf(_) => false,
            Split::Node { first, second, .. } => {
                first.split(target, new, across) || second.split(target, new, across)
            }
        }
    }

    /// Removes leaf `target`; its sibling takes its parent's place.
    fn remove(&mut self, target: u64) -> bool {
        let Split::Node { first, second, .. } = self else {
            return false;
        };
        let keep = if matches!(**first, Split::Leaf(id) if id == target) {
            Some(std::mem::replace(&mut **second, Split::Leaf(0)))
        } else if matches!(**second, Split::Leaf(id) if id == target) {
            Some(std::mem::replace(&mut **first, Split::Leaf(0)))
        } else {
            None
        };
        if let Some(keep) = keep {
            *self = keep;
            return true;
        }
        first.remove(target) || second.remove(target)
    }

    /// Sets the ratio of the split at `path` (the root is 0, children `2p+1` and `2p+2`).
    fn set_ratio(&mut self, path: u64, here: u64, value: f32) {
        if let Split::Node {
            ratio,
            first,
            second,
            ..
        } = self
        {
            if path == here {
                *ratio = value;
            } else {
                first.set_ratio(path, here * 2 + 1, value);
                second.set_ratio(path, here * 2 + 2, value);
            }
        }
    }

    /// The rectangle of every leaf and every divider in `rect`.
    fn layout(&self, rect: Rect, path: u64, out: &mut Layout) {
        match self {
            Split::Leaf(id) => out.panes.push((*id, rect)),
            Split::Node {
                across,
                ratio,
                first,
                second,
            } => {
                let (a, handle, b) = if *across {
                    let w = (rect.w - HANDLE) * ratio;
                    (
                        Rect::new(rect.x, rect.y, w, rect.h),
                        Rect::new(rect.x + w, rect.y, HANDLE, rect.h),
                        Rect::new(rect.x + w + HANDLE, rect.y, rect.w - w - HANDLE, rect.h),
                    )
                } else {
                    let h = (rect.h - HANDLE) * ratio;
                    (
                        Rect::new(rect.x, rect.y, rect.w, h),
                        Rect::new(rect.x, rect.y + h, rect.w, HANDLE),
                        Rect::new(rect.x, rect.y + h + HANDLE, rect.w, rect.h - h - HANDLE),
                    )
                };
                out.dividers.push((path, handle, rect, *across));
                first.layout(a, path * 2 + 1, out);
                second.layout(b, path * 2 + 2, out);
            }
        }
    }
}

/// Where the panes and dividers of one frame go.
struct Layout {
    panes: Vec<(u64, Rect)>,
    /// Divider path, handle, the split's whole rectangle and its direction.
    dividers: Vec<(u64, Rect, Rect, bool)>,
}

/// The panes of one terminal tab.
pub struct Panes {
    tree: Split,
    terminals: Vec<(u64, LiveTerminal)>,
    /// The pane with the keyboard.
    pub focused: u64,
    next: u64,
    /// Typing goes to every pane (Ctrl+Shift+B).
    pub broadcast: bool,
    /// Where each pane was drawn last, for moving focus with the keyboard.
    rects: Vec<(u64, Rect)>,
}

impl Panes {
    /// A tab with one pane.
    #[must_use]
    pub fn new(terminal: LiveTerminal) -> Self {
        Self {
            tree: Split::Leaf(1),
            terminals: vec![(1, terminal)],
            focused: 1,
            next: 2,
            broadcast: false,
            rects: Vec::new(),
        }
    }

    /// How many panes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.terminals.len()
    }

    fn focused_pane(&self) -> Option<&LiveTerminal> {
        self.terminals
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, pane)| pane)
    }

    /// The title of the focused pane's program, if it set one.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.focused_pane().and_then(|pane| pane.title.as_deref())
    }

    /// The tab's dot: the most urgent of its panes.
    #[must_use]
    pub fn status(&self) -> Option<TabStatus> {
        let all: Vec<TabStatus> = self
            .terminals
            .iter()
            .filter_map(|(_, pane)| pane.status())
            .collect();
        [TabStatus::Error, TabStatus::Input, TabStatus::Unread]
            .into_iter()
            .find(|wanted| all.contains(wanted))
    }

    /// Splits the focused pane, side by side when `across`, running the same program.
    pub fn split(&mut self, across: bool) {
        let Some(program) = self.focused_pane().map(LiveTerminal::program) else {
            return;
        };
        let id = self.next;
        self.next += 1;
        if self.tree.split(self.focused, id, across) {
            self.terminals.push((id, LiveTerminal::new(program)));
            self.focused = id;
        }
    }

    /// Every pane's title and transcript, for logs when the tab closes.
    #[must_use]
    pub fn transcripts(&self) -> Vec<(String, String)> {
        self.terminals
            .iter()
            .filter_map(|(_, pane)| {
                let text = pane.transcript()?;
                Some((pane.title.clone().unwrap_or_default(), text))
            })
            .collect()
    }

    /// Takes in every pane's news while the tab is not shown, and reconnects dropped ones
    /// when their time comes; returns the seconds until the soonest next try.
    pub fn poll(&mut self, now: f64, waker: Option<&Waker>) -> Option<f64> {
        self.terminals
            .iter_mut()
            .filter_map(|(_, pane)| pane.poll(now, waker))
            .min_by(f64::total_cmp)
    }

    /// Types `bytes` into the focused pane (a snippet from the side panel).
    pub fn write_focused(&self, bytes: &[u8]) {
        if let Some(pane) = self.focused_pane() {
            pane.write(bytes);
        }
    }

    /// Closes the focused pane; `false` when it is the last (the tab closes instead). The
    /// closed pane's transcript goes to `closed`.
    pub fn close_focused(&mut self, closed: &mut Vec<(String, String)>) -> bool {
        if self.terminals.len() < 2 || !self.tree.remove(self.focused) {
            return false;
        }
        if let Some(pane) = self.focused_pane()
            && let Some(text) = pane.transcript()
        {
            closed.push((pane.title.clone().unwrap_or_default(), text));
        }
        self.terminals.retain(|(id, _)| *id != self.focused);
        let mut order = Vec::new();
        self.tree.leaves(&mut order);
        self.focused = order.first().copied().unwrap_or(0);
        true
    }

    /// Moves the keyboard to the pane in direction (`dx`, `dy`) from the focused one.
    pub fn move_focus(&mut self, dx: f32, dy: f32) {
        let rects = &self.rects;
        let Some((_, from)) = rects.iter().find(|(id, _)| *id == self.focused) else {
            return;
        };
        let center = from.center();
        let best = rects
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter(|(_, rect)| {
                let c = rect.center();
                (c.x - center.x) * dx + (c.y - center.y) * dy > 1.0
            })
            .min_by(|(_, a), (_, b)| {
                let d =
                    |r: &Rect| (r.center().x - center.x).abs() + (r.center().y - center.y).abs();
                d(a).total_cmp(&d(b))
            });
        if let Some((id, _)) = best {
            self.focused = *id;
        }
    }

    /// Draws every pane into `rect`; `pane_id` names a pane's zone.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        style: (Font, &Palette),
        waker: Option<&Waker>,
        pane_id: impl Fn(u64) -> Id,
    ) {
        let t = ui.theme;
        let mut area = rect;
        if self.broadcast && self.terminals.len() > 1 {
            let banner = area.take_top(30.0);
            ui.gfx.fill_rect(banner, 0.0, t.warn.alpha(0.18));
            let mut inner = banner.inset(annulus_config::tokens::space::L, 0.0);
            let icon = inner.take_left(16.0).centered(14.0, 14.0);
            ui.gfx.icon(icons::WARNING, icon, t.warn);
            inner.take_left(8.0);
            let words = format!(
                "Typing into {} panes \u{b7} Ctrl+Shift+B to stop",
                self.terminals.len()
            );
            ui.gfx.text(inner, &words, SMALL, t.text, Align::Start);
        }
        let mut layout = Layout {
            panes: Vec::new(),
            dividers: Vec::new(),
        };
        self.tree.layout(area, 0, &mut layout);
        // A divider drags its split's ratio.
        let mut changes = Vec::new();
        for (path, handle, whole, across) in &layout.dividers {
            let id = Id::new(("divider", *path, std::ptr::from_ref(self) as usize));
            let response = ui.zone(id, *handle, Role::Splitter, "Resize panes");
            let hot = response.hovered || ui.dragging(id);
            ui.gfx.fill_rect(
                handle.centered(
                    if *across { 1.0 } else { handle.w },
                    if *across { handle.h } else { 1.0 },
                ),
                0.0,
                if hot { t.accent } else { t.line },
            );
            if hot {
                ui.set_cursor(if *across {
                    Cursor::ResizeHorizontal
                } else {
                    Cursor::ResizeVertical
                });
            }
            if ui.dragging(id)
                && let Some(pointer) = ui.pointer()
            {
                let share = if *across {
                    (pointer.x - whole.x) / whole.w
                } else {
                    (pointer.y - whole.y) / whole.h
                };
                changes.push((*path, share.clamp(0.15, 0.85)));
            }
        }
        for (path, value) in changes {
            self.tree.set_ratio(path, 0, value);
        }
        let many = self.terminals.len() > 1;
        for (id, area) in &layout.panes {
            let Some((_, pane)) = self.terminals.iter_mut().find(|(known, _)| known == id) else {
                continue;
            };
            let zone = pane_id(*id);
            pane.draw(ui, *area, style, waker, zone);
            if ui.state.focus() == Some(zone) {
                self.focused = *id;
            }
            if many {
                let outline = if self.broadcast {
                    Some(t.warn)
                } else if self.focused == *id {
                    Some(t.accent.alpha(0.7))
                } else {
                    None
                };
                if let Some(color) = outline {
                    ui.gfx.stroke_rect(area.inset(1.0, 1.0), 6.0, 1.0, color);
                }
            }
        }
        if self.broadcast {
            let sent: Vec<Vec<u8>> = self
                .terminals
                .iter_mut()
                .find(|(id, _)| *id == self.focused)
                .map(|(_, pane)| std::mem::take(&mut pane.sent))
                .unwrap_or_default();
            for (id, pane) in &self.terminals {
                if *id != self.focused {
                    for bytes in &sent {
                        pane.write(bytes);
                    }
                }
            }
        }
        self.rects = layout.panes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(split: &Split) -> Vec<u64> {
        let mut out = Vec::new();
        split.leaves(&mut out);
        out
    }

    #[test]
    fn splitting_and_closing_keep_the_tree_whole() {
        let mut tree = Split::Leaf(1);
        assert!(tree.split(1, 2, true));
        assert!(tree.split(2, 3, false));
        assert_eq!(leaves(&tree), [1, 2, 3]);
        assert!(!tree.split(9, 4, true), "an unknown pane is not split");
        // Closing the middle pane hands its place to its sibling.
        assert!(tree.remove(2));
        assert_eq!(leaves(&tree), [1, 3]);
        assert!(tree.remove(1));
        assert_eq!(leaves(&tree), [3]);
        assert!(!tree.remove(3), "the last pane stays");
    }

    #[test]
    fn a_divider_moves_its_own_split_only() {
        let mut tree = Split::Leaf(1);
        tree.split(1, 2, true);
        tree.split(2, 3, false);
        tree.set_ratio(2, 0, 0.25);
        let mut layout = Layout {
            panes: Vec::new(),
            dividers: Vec::new(),
        };
        tree.layout(Rect::new(0.0, 0.0, 406.0, 206.0), 0, &mut layout);
        // The root still splits in half; the right half is split a quarter of the way down.
        assert_eq!(layout.panes[0], (1, Rect::new(0.0, 0.0, 200.0, 206.0)));
        assert_eq!(layout.panes[1], (2, Rect::new(206.0, 0.0, 200.0, 50.0)));
        assert_eq!(layout.dividers.len(), 2);
    }

    #[test]
    fn focus_moves_to_the_nearest_pane_in_that_direction() {
        let mut panes = Panes {
            tree: Split::Leaf(1),
            terminals: Vec::new(),
            focused: 1,
            next: 4,
            broadcast: false,
            rects: vec![
                (1, Rect::new(0.0, 0.0, 100.0, 200.0)),
                (2, Rect::new(106.0, 0.0, 100.0, 97.0)),
                (3, Rect::new(106.0, 103.0, 100.0, 97.0)),
            ],
        };
        panes.move_focus(1.0, 0.0);
        assert!(matches!(panes.focused, 2 | 3));
        panes.focused = 2;
        panes.move_focus(0.0, 1.0);
        assert_eq!(panes.focused, 3);
        panes.move_focus(-1.0, 0.0);
        assert_eq!(panes.focused, 1);
        panes.move_focus(-1.0, 0.0);
        assert_eq!(panes.focused, 1, "nothing further left");
    }
}
