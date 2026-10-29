//! Split panes in a terminal tab (F-WS-001, F-WS-005): a row or a column of terminals, each
//! with its own session. Ctrl+Shift+D splits right, Ctrl+Shift+E splits down, Ctrl+Shift+W
//! closes the focused pane, and Alt+arrows move the focus.

use eframe::egui::{self, Rect, Ui, UiBuilder, Vec2};
use annulus_session::Program;

use crate::terminal_view::{TabStatus, TerminalTab};

/// The amber of broadcast mode: the banner and every receiving pane's outline.
const BROADCAST: egui::Color32 = egui::Color32::from_rgb(0xE0, 0xA4, 0x3A);

/// Most panes in one tab.
pub const MAX_PANES: usize = 16;

/// How panes are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Side by side.
    Row,
    /// One above the other.
    Column,
}

/// The panes of one terminal tab.
pub struct Panes {
    list: Vec<TerminalTab>,
    layout: Layout,
    focused: usize,
    /// Typing in the focused pane goes to every pane (F-WS-003).
    broadcast: bool,
    /// One large pane and a list of the others instead of the split (F-WS-002).
    focus_mode: bool,
}

/// Width of the session list beside the large pane in focus mode (UI-SPEC §3).
const FOCUS_LIST: f32 = 220.0;

/// Where each pane goes: in split mode every pane gets a part of `area`; in focus mode only
/// `focused` does, beside a list strip on the right (the second value).
fn arrange(
    area: Rect,
    count: usize,
    focused: usize,
    focus_mode: bool,
    layout: Layout,
) -> (Vec<Option<Rect>>, Option<Rect>) {
    if !focus_mode || count < 2 {
        let rects = split_rect(area, count, layout, 2.0);
        return (rects.into_iter().map(Some).collect(), None);
    }
    let list = Rect::from_min_max(egui::pos2(area.max.x - FOCUS_LIST, area.min.y), area.max);
    let large = Rect::from_min_max(area.min, egui::pos2(list.min.x - 2.0, area.max.y));
    let rects = (0..count)
        .map(|index| (index == focused).then_some(large))
        .collect();
    (rects, Some(list))
}

impl Panes {
    /// A tab with one pane running `program`.
    #[must_use]
    pub fn single(title: impl Into<String>, program: Program) -> Self {
        Self {
            list: vec![TerminalTab::new(title, program)],
            layout: Layout::Row,
            focused: 0,
            broadcast: false,
            focus_mode: false,
        }
    }

    /// Switches between split and focus mode (Ctrl+Shift+M).
    pub fn toggle_focus_mode(&mut self) {
        self.focus_mode = !self.focus_mode;
    }

    /// The tab title: the focused pane's.
    #[must_use]
    pub fn title(&self) -> &str {
        self.list
            .get(self.focused)
            .map_or("Terminal", |pane| pane.title.as_str())
    }

    /// The most urgent status dot of any pane.
    #[must_use]
    pub fn status(&self) -> Option<crate::terminal_view::TabStatus> {
        self.list.iter().filter_map(TerminalTab::status).max()
    }

    /// Takes in every pane's news while the tab is not shown.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for pane in &mut self.list {
            pane.poll(ctx);
        }
    }

    /// Focus mode's list of the tab's sessions; a click brings one into the large pane.
    fn session_list(&mut self, ui: &mut Ui, list: Rect) {
        ui.scope_builder(
            UiBuilder::new().id_salt("focus-list").max_rect(list),
            |ui| {
                ui.label(egui::RichText::new("Sessions").strong());
                for (index, pane) in self.list.iter().enumerate() {
                    let label = match pane.status() {
                        Some(status) => format!("{} \u{2022} {}", pane.title, status.describe()),
                        None => pane.title.clone(),
                    };
                    if ui.selectable_label(index == self.focused, label).clicked() {
                        self.focused = index;
                    }
                }
                ui.label(
                    egui::RichText::new("Ctrl+Shift+M: back to split")
                        .size(annulus_config::tokens::text::SECONDARY),
                );
            },
        );
    }

    /// The pane that has the keyboard.
    pub fn focused_mut(&mut self) -> Option<&mut TerminalTab> {
        self.list.get_mut(self.focused)
    }

    /// Every pane.
    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut TerminalTab> {
        self.list.iter_mut()
    }

    /// Adds a pane after the focused one, running `program`, and focuses it. The first
    /// split decides the layout; later splits add to it. False when the tab is full.
    pub fn split(&mut self, layout: Layout, title: impl Into<String>, program: Program) -> bool {
        if self.list.len() >= MAX_PANES {
            return false;
        }
        if self.list.len() == 1 {
            self.layout = layout;
        }
        let at = (self.focused + 1).min(self.list.len());
        self.list.insert(at, TerminalTab::new(title, program));
        self.focused = at;
        true
    }

    /// Closes the focused pane; false when it was the last one (close the tab then).
    pub fn close_focused(&mut self) -> bool {
        if self.list.len() <= 1 {
            return false;
        }
        self.list.remove(self.focused);
        self.focused = self.focused.min(self.list.len() - 1);
        self.list[self.focused].take_focus();
        true
    }

    /// Moves the focus `step` panes along (negative: back), wrapping around.
    pub fn move_focus(&mut self, step: isize) {
        let count = isize::try_from(self.list.len()).unwrap_or(1);
        let current = isize::try_from(self.focused).unwrap_or(0);
        self.focused = usize::try_from((current + step).rem_euclid(count)).unwrap_or(0);
        self.list[self.focused].take_focus();
    }

    /// Turns broadcast input on or off (Ctrl+Shift+B); it needs two panes or more.
    pub fn toggle_broadcast(&mut self) {
        self.broadcast = !self.broadcast && self.list.len() > 1;
    }

    /// Whether typing goes to every pane.
    #[must_use]
    pub fn broadcasting(&self) -> bool {
        self.broadcast && self.list.len() > 1
    }

    /// A context-menu choice from any pane, with the pane focused (taken once).
    pub fn take_request(&mut self) -> Option<crate::terminal_view::PaneRequest> {
        for (index, pane) in self.list.iter_mut().enumerate() {
            if let Some(request) = pane.take_request() {
                self.focused = index;
                return Some(request);
            }
        }
        None
    }

    /// How many panes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// The program of the focused pane, for a split that opens the same thing again.
    #[must_use]
    pub fn focused_program(&self) -> Option<(String, Program)> {
        self.list
            .get(self.focused)
            .map(|pane| (pane.title.clone(), pane.program().clone()))
    }

    /// Draws the panes in equal parts of the area, with a thin line between them.
    pub fn ui(&mut self, ui: &mut Ui, font_size: f32) {
        if self.broadcasting() {
            ui.colored_label(
                BROADCAST,
                format!(
                    "Typing into {} panes \u{b7} Ctrl+Shift+B to stop",
                    self.list.len()
                ),
            );
        }
        let area = ui.available_rect_before_wrap();
        let (rects, list) = arrange(
            area,
            self.list.len(),
            self.focused,
            self.focus_mode,
            self.layout,
        );
        let focused = self.focused;
        let several = self.list.len() > 1;
        let broadcasting = self.broadcasting();
        for (index, (pane, rect)) in self.list.iter_mut().zip(rects).enumerate() {
            // A pane hidden by focus mode still takes in its session's news.
            let Some(rect) = rect else {
                pane.poll(ui.ctx());
                continue;
            };
            if broadcasting {
                ui.painter().rect_stroke(
                    rect.expand(1.0),
                    0.0,
                    egui::Stroke::new(2.0, BROADCAST),
                    egui::StrokeKind::Outside,
                );
            } else if several && index == focused && list.is_none() {
                let accent = ui.visuals().selection.stroke.color;
                ui.painter().rect_stroke(
                    rect.expand(1.0),
                    0.0,
                    egui::Stroke::new(1.0, accent),
                    egui::StrokeKind::Outside,
                );
            }
            let response = ui.scope_builder(
                UiBuilder::new().id_salt(("pane", index)).max_rect(rect),
                |ui| pane.ui(ui, font_size),
            );
            if response.response.contains_pointer() && ui.input(|input| input.pointer.any_pressed())
            {
                self.focused = index;
            }
            if several && let Some(status) = pane.status() {
                pane_dot(ui, rect, index, status);
            }
        }
        if let Some(list) = list {
            self.session_list(ui, list);
        }
        let typed = self
            .list
            .get_mut(focused)
            .map(TerminalTab::take_sent)
            .unwrap_or_default();
        if self.broadcasting() {
            for (index, pane) in self.list.iter().enumerate() {
                if index != focused {
                    for bytes in &typed {
                        pane.write(bytes);
                    }
                }
            }
        }
        if self.list.len() > 1 && !self.focus_mode {
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            for rect in split_rect(area, self.list.len(), self.layout, 2.0)
                .iter()
                .skip(1)
            {
                let (from, to) = match self.layout {
                    Layout::Row => (rect.left_top() - Vec2::X, rect.left_bottom() - Vec2::X),
                    Layout::Column => (rect.left_top() - Vec2::Y, rect.right_top() - Vec2::Y),
                };
                ui.painter().line_segment([from, to], stroke);
            }
        }
        ui.allocate_rect(area, egui::Sense::hover());
    }
}

/// The colour of a pane's dot: accent, amber or red, from the theme.
fn dot_color(visuals: &egui::Visuals, status: TabStatus) -> egui::Color32 {
    match status {
        TabStatus::Unread => visuals.selection.stroke.color,
        TabStatus::Input => visuals.warn_fg_color,
        TabStatus::Error => visuals.error_fg_color,
    }
}

/// A pane's status dot in its top-right corner (F-APP-002), with a tooltip and an
/// accessible name, so a split tab shows which pane needs attention.
fn pane_dot(ui: &Ui, rect: Rect, index: usize, status: TabStatus) {
    let color = dot_color(ui.visuals(), status);
    let center = rect.right_top() + egui::vec2(-10.0, 10.0);
    ui.painter().circle_filled(center, 4.0, color);
    let response = ui.interact(
        egui::Rect::from_center_size(center, egui::vec2(12.0, 12.0)),
        ui.scope_id().with(("pane-dot", index)),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, status.describe()));
    response.on_hover_text(status.describe());
}

/// `count` equal parts of `area` along `layout`, with `gap` points between them.
#[must_use]
pub fn split_rect(area: Rect, count: usize, layout: Layout, gap: f32) -> Vec<Rect> {
    let count = count.max(1);
    #[allow(clippy::cast_precision_loss)] // At most 16 panes.
    let n = count as f32;
    let length = match layout {
        Layout::Row => area.width(),
        Layout::Column => area.height(),
    };
    let size = ((length - gap * (n - 1.0)) / n).max(0.0);
    (0..count)
        .map(|index| {
            #[allow(clippy::cast_precision_loss)] // At most 16 panes.
            let start = (size + gap) * index as f32;
            match layout {
                Layout::Row => Rect::from_min_size(
                    area.min + Vec2::new(start, 0.0),
                    Vec2::new(size, area.height()),
                ),
                Layout::Column => Rect::from_min_size(
                    area.min + Vec2::new(0.0, start),
                    Vec2::new(area.width(), size),
                ),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes() -> Panes {
        Panes::single("one", Program::DefaultShell)
    }

    #[test]
    fn focus_mode_gives_the_focused_pane_the_room_beside_a_list() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
        let (split, list) = arrange(area, 3, 1, false, Layout::Row);
        assert!(list.is_none());
        assert!(split.iter().all(Option::is_some));
        let (focus, list) = arrange(area, 3, 1, true, Layout::Row);
        assert_eq!(list.map(|rect| rect.width()), Some(FOCUS_LIST));
        assert_eq!(focus[0], None);
        assert_eq!(focus[2], None);
        assert_eq!(
            focus[1].map(|rect| rect.width()),
            Some(1000.0 - FOCUS_LIST - 2.0)
        );
        let (single, list) = arrange(area, 1, 0, true, Layout::Row);
        assert_eq!(
            (single, list),
            (vec![Some(area)], None),
            "one pane: no list"
        );
    }

    #[test]
    fn pane_dots_take_the_themes_accent_warning_and_error_colours() {
        let visuals = egui::Visuals::dark();
        assert_eq!(
            dot_color(&visuals, TabStatus::Unread),
            visuals.selection.stroke.color
        );
        assert_eq!(dot_color(&visuals, TabStatus::Input), visuals.warn_fg_color);
        assert_eq!(
            dot_color(&visuals, TabStatus::Error),
            visuals.error_fg_color
        );
    }

    #[test]
    fn splitting_focuses_the_new_pane_and_closing_keeps_one() {
        let mut tab = panes();
        assert!(tab.split(Layout::Column, "two", Program::DefaultShell));
        assert_eq!(
            (tab.len(), tab.title(), tab.layout),
            (2, "two", Layout::Column)
        );
        // A later split keeps the first layout.
        tab.split(Layout::Row, "three", Program::DefaultShell);
        assert_eq!(tab.layout, Layout::Column);
        assert!(tab.close_focused());
        assert_eq!((tab.len(), tab.title()), (2, "two"));
        tab.close_focused();
        assert!(!tab.close_focused(), "the last pane closes with the tab");
        assert_eq!(tab.len(), 1);
    }

    #[test]
    fn focus_moves_round_and_a_tab_holds_at_most_sixteen() {
        let mut tab = panes();
        for index in 1..MAX_PANES {
            assert!(tab.split(Layout::Row, format!("p{index}"), Program::DefaultShell));
        }
        assert!(!tab.split(Layout::Row, "too many", Program::DefaultShell));
        tab.move_focus(1);
        assert_eq!(tab.title(), "one", "wraps from the last to the first");
        tab.move_focus(-1);
        assert_eq!(tab.title(), "p15");
    }

    #[test]
    fn broadcast_needs_two_panes_and_ends_when_one_is_left() {
        let mut tab = panes();
        tab.toggle_broadcast();
        assert!(!tab.broadcasting(), "nothing to broadcast to");
        tab.split(Layout::Row, "two", Program::DefaultShell);
        tab.toggle_broadcast();
        assert!(tab.broadcasting());
        tab.close_focused();
        assert!(!tab.broadcasting());
    }

    #[test]
    fn parts_fill_the_area_with_gaps() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(202.0, 100.0));
        let parts = split_rect(area, 2, Layout::Row, 2.0);
        assert_eq!(
            parts[0],
            Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(100.0, 100.0))
        );
        assert_eq!(parts[1].min.x, 102.0);
        assert_eq!(parts[1].max.x, 202.0);
        let rows = split_rect(area, 4, Layout::Column, 0.0);
        assert_eq!(rows[3].max.y, 100.0);
    }
}
