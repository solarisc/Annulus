//! Split panes in a terminal tab (UI-SPEC §3): a tree of side-by-side and stacked splits,
//! dividers that drag, keyboard focus between panes, focus mode (one large pane beside a list
//! of them all), and broadcast input that types into every pane but those left out.

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::Program;
use annulus_term::Palette;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Font, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Role, Theme, Ui};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, SMALL};

use super::live::LiveTerminal;
use crate::terminal_view::TabStatus;

/// Width of a divider's drag handle, and the gap between focus mode's large pane and its list.
const HANDLE: f32 = 6.0;
/// Most panes in one tab (F-WS-005).
const MAX_PANES: usize = 16;
/// Width of the list beside the large pane in focus mode (UI-SPEC §3).
const FOCUS_LIST: f32 = 220.0;
/// Height of the bar above the panes of a split tab: broadcast and focus mode.
const BAR_H: f32 = 44.0;
/// Height of one row of focus mode's list.
const ROW_H: f32 = 44.0;

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
    /// `ids` split in halves, side by side first and then stacked, alternating, so every
    /// pane gets about the same room.
    fn balanced(ids: &[u64], across: bool) -> Split {
        match ids {
            [] => Split::Leaf(0),
            [id] => Split::Leaf(*id),
            _ => {
                let (first, second) = ids.split_at(ids.len().div_ceil(2));
                let share = |n: usize| f32::from(u16::try_from(n).unwrap_or(u16::MAX));
                Split::Node {
                    across,
                    ratio: share(first.len()) / share(ids.len()),
                    first: Box::new(Self::balanced(first, !across)),
                    second: Box::new(Self::balanced(second, !across)),
                }
            }
        }
    }

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

    /// The rectangle of every leaf and every divider in `rect`. However often one pane is
    /// split, no part is smaller than nothing or leaves `rect`.
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
                    let gap = HANDLE.min(rect.w.max(0.0));
                    let w = (rect.w - gap) * ratio;
                    (
                        Rect::new(rect.x, rect.y, w, rect.h),
                        Rect::new(rect.x + w, rect.y, gap, rect.h),
                        Rect::new(rect.x + w + gap, rect.y, rect.w - w - gap, rect.h),
                    )
                } else {
                    let gap = HANDLE.min(rect.h.max(0.0));
                    let h = (rect.h - gap) * ratio;
                    (
                        Rect::new(rect.x, rect.y, rect.w, h),
                        Rect::new(rect.x, rect.y + h, rect.w, gap),
                        Rect::new(rect.x, rect.y + h + gap, rect.w, rect.h - h - gap),
                    )
                };
                out.dividers.push((path, handle, rect, *across));
                first.layout(a, path * 2 + 1, out);
                second.layout(b, path * 2 + 2, out);
            }
        }
    }
}

/// Where the panes, the dividers and focus mode's list go in one frame.
struct Layout {
    panes: Vec<(u64, Rect)>,
    /// Divider path, handle, the split's whole rectangle and its direction.
    dividers: Vec<(u64, Rect, Rect, bool)>,
    /// Focus mode's list of the tab's panes, right of the large one.
    list: Option<Rect>,
}

/// Recorded output with a made-up status: the panes of the scene catalogue.
#[cfg(test)]
pub struct Replay {
    /// The pane's title.
    pub title: String,
    /// What it shows.
    pub emulator: annulus_term::Emulator,
    /// Its dot.
    pub status: Option<TabStatus>,
}

#[cfg(test)]
impl Replay {
    fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect, font: Font, id: Id) {
        let inner = rect.inset(space::L, space::M);
        let response = ui.zone(id, inner, Role::Terminal, "Terminal");
        let (columns, rows) = super::terminal::grid_size(ui, inner, font);
        self.emulator.resize(columns, rows);
        let screen = self.emulator.screen();
        super::terminal::draw(ui, inner, &screen, font, response.focused);
    }
}

/// One pane of a tab.
struct Pane {
    id: u64,
    terminal: LiveTerminal,
    /// What the pane runs, its name until the program sets a title.
    runs: String,
    /// Typing broadcast from another pane does not reach this one (F-WS-003).
    skip_broadcast: bool,
    /// Shown instead of the terminal, which then never starts.
    #[cfg(test)]
    replay: Option<Replay>,
}

impl Pane {
    fn new(id: u64, terminal: LiveTerminal) -> Self {
        let runs = program_name(&terminal.program());
        Self {
            id,
            terminal,
            runs,
            skip_broadcast: false,
            #[cfg(test)]
            replay: None,
        }
    }

    fn title(&self) -> Option<&str> {
        #[cfg(test)]
        if let Some(replay) = &self.replay {
            return Some(&replay.title);
        }
        self.terminal.title.as_deref()
    }

    /// Its name in focus mode's list and on its controls.
    fn name(&self) -> &str {
        self.title().unwrap_or(&self.runs)
    }

    fn status(&self) -> Option<TabStatus> {
        #[cfg(test)]
        if let Some(replay) = &self.replay {
            return replay.status;
        }
        self.terminal.status()
    }

    fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        style: (Font, &Palette),
        waker: Option<&Waker>,
        id: Id,
    ) {
        #[cfg(test)]
        if let Some(replay) = &mut self.replay {
            replay.draw(ui, rect, style.0, id);
            return;
        }
        self.terminal.draw(ui, rect, style, waker, id);
    }
}

/// A name for what `program` runs: the host it logs in to, or the shell's file name.
fn program_name(program: &Program) -> String {
    match program {
        Program::DefaultShell => "Shell".to_owned(),
        Program::Command { program, .. } => std::path::Path::new(program).file_stem().map_or_else(
            || program.clone(),
            |stem| stem.to_string_lossy().into_owned(),
        ),
        Program::Ssh { target, .. } => format!("{}@{}", target.user, target.host),
    }
}

/// The colour of a pane's dot (F-APP-002): accent for news, amber for a question, red for a
/// failure, from the theme.
fn dot_color(theme: &Theme, status: TabStatus) -> Color {
    match status {
        TabStatus::Unread => theme.accent,
        TabStatus::Input => theme.warn,
        TabStatus::Error => theme.error,
    }
}

/// The panes of one terminal tab.
pub struct Panes {
    tree: Split,
    all: Vec<Pane>,
    /// The pane with the keyboard.
    pub focused: u64,
    next: u64,
    /// Typing goes to every pane not left out (Ctrl+Shift+B), while there are two or more.
    broadcast: bool,
    /// One large pane beside a list of them all, instead of the split (Ctrl+Shift+M).
    focus_mode: bool,
    /// The keyboard goes back to the focused pane next frame (after a click on the bar, the
    /// list or a pane's broadcast toggle, or a closed pane).
    refocus: bool,
    /// Where each pane was drawn last, for moving focus with the keyboard.
    rects: Vec<(u64, Rect)>,
}

impl Panes {
    /// A tab with one pane.
    #[must_use]
    pub fn new(terminal: LiveTerminal) -> Self {
        Self {
            tree: Split::Leaf(1),
            all: vec![Pane::new(1, terminal)],
            focused: 1,
            next: 2,
            broadcast: false,
            focus_mode: false,
            refocus: false,
            rects: Vec::new(),
        }
    }

    /// A tab of one pane per program, titled, in a balanced split (a snippet run on several
    /// hosts, F-SNIP-004); at most [`MAX_PANES`]. `None` for no program.
    #[must_use]
    pub fn several(programs: Vec<(String, Program)>) -> Option<Self> {
        let mut all = Vec::new();
        for (id, (title, program)) in (1..).zip(programs.into_iter().take(MAX_PANES)) {
            let mut terminal = LiveTerminal::new(program);
            terminal.title = Some(title);
            all.push(Pane::new(id, terminal));
        }
        let ids: Vec<u64> = all.iter().map(|pane| pane.id).collect();
        let next = ids.last().copied()? + 1;
        Some(Self {
            tree: Split::balanced(&ids, true),
            all,
            focused: 1,
            next,
            broadcast: false,
            focus_mode: false,
            refocus: false,
            rects: Vec::new(),
        })
    }

    /// A tab with one pane showing `replay`.
    #[cfg(test)]
    #[must_use]
    pub fn replay(replay: Replay) -> Self {
        let mut tab = Self::new(LiveTerminal::new(Program::DefaultShell));
        tab.all[0].replay = Some(replay);
        tab
    }

    /// Splits the focused pane, side by side when `across`, with a pane showing `replay`.
    #[cfg(test)]
    pub fn split_replay(&mut self, across: bool, replay: Replay) {
        if self.split(across)
            && let Some(pane) = self.all.last_mut()
        {
            pane.replay = Some(replay);
        }
    }

    /// Drops every pane's secrets, as when the vault locks.
    pub fn forget_secrets(&mut self) {
        for pane in &mut self.all {
            pane.terminal.forget_secrets();
        }
    }

    /// Gives every pane `program` from its next start on (a host tab after an unlock).
    pub fn set_program(&mut self, program: &Program) {
        for pane in &mut self.all {
            pane.terminal.set_program(program.clone());
        }
    }

    /// What each pane runs (the tests look at the secrets it carries).
    #[cfg(test)]
    #[must_use]
    pub fn programs(&self) -> Vec<Program> {
        self.all
            .iter()
            .map(|pane| pane.terminal.program())
            .collect()
    }

    /// How many panes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all.len()
    }

    fn pane(&self, id: u64) -> Option<&Pane> {
        self.all.iter().find(|pane| pane.id == id)
    }

    /// The panes in reading order: left to right, top to bottom.
    fn order(&self) -> Vec<u64> {
        let mut order = Vec::new();
        self.tree.leaves(&mut order);
        order
    }

    /// The title of the focused pane's program, if it set one.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.pane(self.focused).and_then(Pane::title)
    }

    /// The tab's dot: the most urgent of its panes.
    #[must_use]
    pub fn status(&self) -> Option<TabStatus> {
        self.all.iter().filter_map(Pane::status).max()
    }

    /// The identification line of the first SSH server a pane reached (F-HOST-009).
    #[must_use]
    pub fn server(&self) -> Option<&str> {
        self.all
            .iter()
            .find_map(|pane| pane.terminal.server.as_deref())
    }

    /// Splits the focused pane, side by side when `across`, running the same program, and
    /// focuses the new pane. False when the tab already holds [`MAX_PANES`].
    pub fn split(&mut self, across: bool) -> bool {
        if self.all.len() >= MAX_PANES {
            return false;
        }
        let Some(program) = self.pane(self.focused).map(|pane| pane.terminal.program()) else {
            return false;
        };
        let id = self.next;
        if !self.tree.split(self.focused, id, across) {
            return false;
        }
        self.next += 1;
        self.all.push(Pane::new(id, LiveTerminal::new(program)));
        self.focused = id;
        true
    }

    /// Every pane's title and transcript, for logs when the tab closes.
    #[must_use]
    pub fn transcripts(&self) -> Vec<(String, String)> {
        self.all
            .iter()
            .filter_map(|pane| {
                let text = pane.terminal.transcript()?;
                Some((pane.terminal.title.clone().unwrap_or_default(), text))
            })
            .collect()
    }

    /// Takes in every pane's news while the tab is not shown, and reconnects dropped ones
    /// when their time comes; returns the seconds until the soonest next try.
    pub fn poll(&mut self, now: f64, waker: Option<&Waker>) -> Option<f64> {
        self.all
            .iter_mut()
            .filter_map(|pane| pane.terminal.poll(now, waker))
            .min_by(f64::total_cmp)
    }

    /// The panes a snippet goes to: the focused one, and while broadcasting every pane
    /// broadcast reaches (F-SNIP-005).
    #[must_use]
    pub fn snippet_targets(&self) -> Vec<u64> {
        let mut targets = vec![self.focused];
        if self.broadcasting() {
            targets.extend(self.receivers());
        }
        targets
    }

    /// Types `bytes` (a snippet from the side panel) into [`Panes::snippet_targets`].
    pub fn write_snippet(&self, bytes: &[u8]) {
        for id in self.snippet_targets() {
            if let Some(pane) = self.pane(id) {
                pane.terminal.write(bytes);
            }
        }
    }

    /// Types `bytes` into the focused pane only (an agent's command, ADR-0020); `false` while
    /// its program has not started yet.
    pub fn type_focused(&self, bytes: &[u8]) -> bool {
        let Some(pane) = self.pane(self.focused) else {
            return false;
        };
        if !pane.terminal.started() {
            return false;
        }
        pane.terminal.write(bytes);
        true
    }

    /// The focused pane's text, scrollback included; `None` before its program starts.
    #[must_use]
    pub fn focused_transcript(&self) -> Option<String> {
        self.pane(self.focused)?.terminal.transcript()
    }

    /// Why the focused pane's program could not start or its connection ended, if it did.
    #[must_use]
    pub fn focused_failure(&self) -> Option<String> {
        self.pane(self.focused)?
            .terminal
            .failure()
            .map(str::to_owned)
    }

    /// Marks where an agent typed in the focused pane, the one it types into (F-AUTO-004).
    pub fn mark_focused(&mut self, marks: &super::live::AgentMarks) {
        let focused = self.focused;
        if let Some(pane) = self.all.iter_mut().find(|pane| pane.id == focused)
            && pane.terminal.marks != *marks
        {
            marks.clone_into(&mut pane.terminal.marks);
        }
    }

    /// Scrolls the focused pane so `row` (from the oldest row of its scrollback) is at the top.
    pub fn scroll_focused_to(&self, row: usize) {
        if let Some(pane) = self.pane(self.focused) {
            pane.terminal.scroll_to_row(row);
        }
    }

    /// The row the focused pane's cursor is on; `None` before its program starts.
    #[must_use]
    pub fn focused_cursor_row(&self) -> Option<usize> {
        self.pane(self.focused)?.terminal.cursor_row()
    }

    /// Closes the focused pane; `false` when it is the last (the tab closes instead). The
    /// pane before it takes the keyboard, and the closed pane's transcript goes to `closed`.
    /// Broadcast and focus mode end when one pane is left.
    pub fn close_focused(&mut self, closed: &mut Vec<(String, String)>) -> bool {
        let order = self.order();
        if self.all.len() < 2 || !self.tree.remove(self.focused) {
            return false;
        }
        if let Some(pane) = self.pane(self.focused)
            && let Some(text) = pane.terminal.transcript()
        {
            closed.push((pane.terminal.title.clone().unwrap_or_default(), text));
        }
        let at = order.iter().position(|id| *id == self.focused).unwrap_or(0);
        self.all.retain(|pane| pane.id != self.focused);
        let left = self.order();
        self.focused = left.get(at.saturating_sub(1)).copied().unwrap_or_default();
        if self.all.len() < 2 {
            self.broadcast = false;
            self.focus_mode = false;
        }
        self.refocus = true;
        true
    }

    /// Turns broadcast input on or off (Ctrl+Shift+B); it needs two panes or more.
    pub fn toggle_broadcast(&mut self) {
        self.broadcast = !self.broadcast && self.all.len() > 1;
    }

    /// Leaves the focused pane out of broadcast, or takes it back in (Ctrl+Alt+Shift+B): the
    /// keyboard's way to its switch, which a focused terminal otherwise keeps out of Tab's
    /// reach.
    pub fn toggle_left_out(&mut self) {
        let focused = self.focused;
        if let Some(pane) = self.all.iter_mut().find(|pane| pane.id == focused) {
            pane.skip_broadcast = !pane.skip_broadcast;
        }
    }

    /// Whether typing in the focused pane goes to the others.
    #[must_use]
    pub fn broadcasting(&self) -> bool {
        self.broadcast && self.all.len() > 1
    }

    /// The panes typing in the focused one reaches while broadcasting: all others but those
    /// left out, in reading order.
    #[must_use]
    pub fn receivers(&self) -> Vec<u64> {
        self.order()
            .into_iter()
            .filter(|id| {
                *id != self.focused && self.pane(*id).is_some_and(|pane| !pane.skip_broadcast)
            })
            .collect()
    }

    /// Takes what the focused pane's keyboard sent this frame, paired with every pane it
    /// reaches: none unless broadcasting.
    fn broadcast_out(&mut self) -> Vec<(u64, Vec<u8>)> {
        let focused = self.focused;
        let sent = self
            .all
            .iter_mut()
            .find(|pane| pane.id == focused)
            .map(|pane| std::mem::take(&mut pane.terminal.sent))
            .unwrap_or_default();
        if !self.broadcasting() {
            return Vec::new();
        }
        self.receivers()
            .into_iter()
            .flat_map(|id| sent.iter().map(move |bytes| (id, bytes.clone())))
            .collect()
    }

    /// Switches between the split and focus mode (Ctrl+Shift+M); it needs two panes or more.
    pub fn toggle_focus_mode(&mut self) {
        self.focus_mode = !self.focus_mode && self.all.len() > 1;
        self.refocus = true;
    }

    /// Whether one large pane shows beside the list.
    #[must_use]
    pub fn in_focus_mode(&self) -> bool {
        self.focus_mode && self.all.len() > 1
    }

    /// Moves the keyboard to the pane in direction (`dx`, `dy`) from the focused one. In focus
    /// mode no pane lies beside another: right and down go to the next pane in the list, left
    /// and up to the one before, round from the last to the first.
    pub fn move_focus(&mut self, dx: f32, dy: f32) {
        if self.in_focus_mode() {
            let order = self.order();
            let Some(at) = order.iter().position(|id| *id == self.focused) else {
                return;
            };
            let step = if dx + dy > 0.0 { 1 } else { order.len() - 1 };
            self.focused = order[(at + step) % order.len()];
            return;
        }
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

    /// Where the panes go in `area`: the split tree, or in focus mode the focused pane with
    /// the list on its right.
    fn arrange(&self, area: Rect) -> Layout {
        let mut layout = Layout {
            panes: Vec::new(),
            dividers: Vec::new(),
            list: None,
        };
        if self.in_focus_mode() {
            let mut large = area;
            let list = large.take_right(FOCUS_LIST);
            large.take_right(HANDLE);
            layout.panes.push((self.focused, large));
            layout.list = Some(list);
        } else {
            self.tree.layout(area, 0, &mut layout);
        }
        layout
    }

    /// Draws every pane into `rect`; `pane_id` names a pane's zone. Pane 0 never exists, so
    /// its id is the base of the tab's own controls.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        style: (Font, &Palette),
        waker: Option<&Waker>,
        pane_id: impl Fn(u64) -> Id,
    ) {
        let mut area = rect;
        if self.all.len() > 1 {
            let bar = area.take_top(BAR_H);
            self.bar(ui, bar, pane_id(0));
        }
        let mut layout = self.arrange(area);
        // A row of focus mode's list brings its pane in front before anything is drawn.
        if layout.list.is_some()
            && let Some(id) = self
                .order()
                .into_iter()
                .find(|id| ui.state.was_clicked(pane_id(*id).with("row")))
        {
            self.focused = id;
            self.refocus = true;
            layout = self.arrange(area);
        }
        if let Some((id, _)) = layout
            .panes
            .iter()
            .find(|(id, _)| ui.state.focus() == Some(pane_id(*id)))
        {
            self.focused = *id;
        }
        if std::mem::take(&mut self.refocus) {
            ui.state.set_focus(Some(pane_id(self.focused)), false);
        }
        self.dividers(ui, &layout);
        self.draw_panes(ui, &layout, style, waker, &pane_id);
        // The panes focus mode hides still take in their news and reconnect.
        let now = ui.time();
        for pane in &mut self.all {
            if !layout.panes.iter().any(|(id, _)| *id == pane.id)
                && let Some(wait) = pane.terminal.poll(now, waker)
            {
                ui.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            }
        }
        if let Some(list) = layout.list {
            self.list(ui, list, &pane_id);
        }
        for (id, bytes) in self.broadcast_out() {
            if let Some(pane) = self.pane(id) {
                pane.terminal.write(&bytes);
            }
        }
        self.rects = layout.panes;
    }

    /// The bar above a split tab: what broadcast is doing, and the switches for broadcast
    /// and focus mode, so both are a click away.
    fn bar(&mut self, ui: &mut Ui<'_>, bar: Rect, base: Id) {
        let t = ui.theme;
        let broadcasting = self.broadcasting();
        let strip = Rect::new(
            bar.x + space::S,
            bar.y + space::S,
            bar.w - 2.0 * space::S,
            bar.h - space::S - space::XS,
        );
        let fill = if broadcasting {
            t.surface.mix(t.warn, 0.18)
        } else {
            t.surface
        };
        ui.gfx.fill_rect(strip, 8.0, fill);
        ui.gfx.stroke_rect(
            strip,
            8.0,
            1.0,
            if broadcasting {
                t.warn.alpha(0.6)
            } else {
                t.line
            },
        );
        let mut inner = strip.inset(space::S, 0.0);
        let focus_w = toggle_width(ui, "Focus mode");
        let focus = inner.take_right(focus_w).centered(focus_w, 24.0);
        inner.take_right(space::XS);
        let cast_w = toggle_width(ui, "Broadcast");
        let cast = inner.take_right(cast_w).centered(cast_w, 24.0);
        inner.take_right(space::M);
        inner.take_left(space::XS);
        let icon = inner.take_left(16.0).centered(16.0, 16.0);
        inner.take_left(space::S);
        if broadcasting {
            ui.gfx.icon(icons::WARNING, icon, t.warn);
            let words = format!(
                "Typing into {} panes \u{b7} {} to stop",
                self.receivers().len() + 1,
                super::shortcuts::BROADCAST.keys()
            );
            ui.gfx.text(inner, &words, SMALL, t.text, Align::Start);
        } else {
            ui.gfx.icon(icons::SPLIT, icon, t.text2);
            let words = format!("{} panes", self.all.len());
            ui.gfx.text(inner, &words, SMALL, t.text2, Align::Start);
        }
        if toggle(
            ui,
            base.with("broadcast"),
            cast,
            "Broadcast",
            broadcasting,
            t.warn,
        ) {
            self.toggle_broadcast();
            self.refocus = true;
        }
        if toggle(
            ui,
            base.with("focus-mode"),
            focus,
            "Focus mode",
            self.in_focus_mode(),
            t.accent,
        ) {
            self.toggle_focus_mode();
        }
    }

    /// A divider drags its split's ratio.
    fn dividers(&mut self, ui: &mut Ui<'_>, layout: &Layout) {
        let t = ui.theme;
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
    }

    /// Each pane with its outline, its dot, and while broadcasting its toggle.
    fn draw_panes(
        &mut self,
        ui: &mut Ui<'_>,
        layout: &Layout,
        style: (Font, &Palette),
        waker: Option<&Waker>,
        pane_id: &dyn Fn(u64) -> Id,
    ) {
        let t = ui.theme;
        let several = self.all.len() > 1;
        let broadcasting = self.broadcasting();
        let mut refocus = false;
        for (id, area) in &layout.panes {
            let zone = pane_id(*id);
            let Some(pane) = self.all.iter_mut().find(|pane| pane.id == *id) else {
                continue;
            };
            pane.draw(ui, *area, style, waker, zone);
            if ui.state.focus() == Some(zone) {
                self.focused = *id;
            }
            let focused = self.focused == *id;
            if broadcasting && !focused && broadcast_toggle(ui, *area, zone.with("broadcast"), pane)
            {
                pane.skip_broadcast = !pane.skip_broadcast;
                refocus = true;
            }
            if !several {
                continue;
            }
            let outline = if broadcasting && (focused || !pane.skip_broadcast) {
                Some(t.warn)
            } else if focused && layout.list.is_none() {
                Some(t.accent.alpha(0.7))
            } else {
                None
            };
            if let Some(color) = outline {
                ui.gfx.stroke_rect(area.inset(1.0, 1.0), 6.0, 1.0, color);
            }
            if let Some(status) = pane.status() {
                ui.gfx.fill_circle(
                    Point::new(area.r() - 10.0, area.y + 10.0),
                    3.5,
                    dot_color(&t, status),
                );
            }
        }
        // The toggle took the keyboard with the click: it goes back to the typing pane.
        if refocus {
            ui.state.set_focus(Some(pane_id(self.focused)), false);
        }
    }

    /// Focus mode's list of the tab's panes, each with its dot; a click brings one in front
    /// (see [`Panes::draw`]).
    fn list(&self, ui: &mut Ui<'_>, list: Rect, pane_id: &dyn Fn(u64) -> Id) {
        let t = ui.theme;
        ui.gfx.fill_rect(list, 0.0, t.bg);
        ui.gfx
            .fill_rect(Rect::new(list.x, list.y, 1.0, list.h), 0.0, t.line);
        let mut inner = list.inset(space::S, space::S);
        let heading = inner.take_top(24.0);
        ui.section_label(heading.inset(space::XS, 0.0), "Sessions");
        let hint = inner.take_bottom(24.0);
        let back = format!("{}: back to split", super::shortcuts::FOCUS_MODE.keys());
        ui.gfx.text(
            hint.inset(space::XS, 0.0),
            &back,
            SMALL,
            t.text3,
            Align::Start,
        );
        let order = self.order();
        #[allow(clippy::cast_precision_loss)] // At most 16 panes.
        let height = order.len() as f32 * ROW_H;
        let offset = ui.scroll_view(pane_id(0).with("list"), inner, height);
        ui.push_clip(inner);
        for (index, id) in order.iter().enumerate() {
            let Some(pane) = self.pane(*id) else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)] // At most 16 panes.
            let y = inner.y - offset + index as f32 * ROW_H;
            if y + ROW_H < inner.y || y > inner.b() {
                continue;
            }
            let row = Rect::new(inner.x, y, inner.w, ROW_H - space::XS);
            let status = pane.status();
            let label = match status {
                Some(status) => format!("{}, {}", pane.name(), status.describe()),
                None => pane.name().to_owned(),
            };
            let row_id = pane_id(*id).with("row");
            let response = ui.zone(row_id, row, Role::ListItem, &label);
            let selected = *id == self.focused;
            let hover = ui.ease(
                row_id.with("hover"),
                if response.hovered { 1.0 } else { 0.0 },
            );
            let fill = if selected {
                t.selected
            } else {
                t.hover.alpha(hover)
            };
            ui.gfx.fill_rect(row, 6.0, fill);
            if response.focus_ring {
                ui.focus_ring(row, 6.0);
            }
            let mut text = row.inset(space::S, 0.0);
            let dot = text.take_left(8.0);
            let color = status.map_or(t.ok.alpha(0.85), |status| dot_color(&t, status));
            ui.gfx.fill_circle(dot.center(), 3.5, color);
            text.take_left(space::S);
            let font = if selected { BODY_MEDIUM } else { BODY };
            // The status, when there is one, on a second line under the name.
            if let Some(status) = status {
                let name = Rect::new(text.x, row.y + 3.0, text.w, 18.0);
                ui.gfx.text(name, pane.name(), font, t.text, Align::Start);
                let words = Rect::new(text.x, name.b(), text.w, 16.0);
                ui.gfx
                    .text(words, status.describe(), SMALL, t.text2, Align::Start);
            } else {
                ui.gfx.text(text, pane.name(), font, t.text, Align::Start);
            }
            if response.hovered {
                ui.set_cursor(Cursor::Pointer);
            }
        }
        ui.pop_clip();
    }
}

/// The width of a [`toggle`] for `label`.
fn toggle_width(ui: &mut Ui<'_>, label: &str) -> f32 {
    (ui.gfx.measure(label, BODY_MEDIUM).width + 2.0 * space::M).ceil()
}

/// A button that stays lit in `color` while `on`, a check box to assistive technology.
/// True when clicked.
fn toggle(ui: &mut Ui<'_>, id: Id, rect: Rect, label: &str, on: bool, color: Color) -> bool {
    let response = ui.zone(id, rect, Role::CheckBox, label);
    ui.set_toggled(id, on);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let fill = if on {
        color.alpha(0.18 + 0.08 * hover)
    } else {
        t.hover.alpha(hover)
    };
    ui.gfx.fill_rect(rect, 6.0, fill);
    if on {
        ui.gfx.stroke_rect(rect, 6.0, 1.0, color.alpha(0.6));
    }
    if response.focus_ring {
        ui.focus_ring(rect, 6.0);
    }
    let ink = if on {
        color.mix(t.text, 0.35)
    } else {
        t.text2.mix(t.text, hover)
    };
    ui.strict_text(rect, label, BODY_MEDIUM, ink, Align::Center);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.clicked
}

/// The switch in a pane's top-right corner while broadcasting: whether typing in the focused
/// pane reaches it. It sits inside the terminal, never across its edge. True when clicked.
fn broadcast_toggle(ui: &mut Ui<'_>, area: Rect, id: Id, pane: &Pane) -> bool {
    let inner = area.inset(space::L, space::M);
    let on = !pane.skip_broadcast;
    let (icon, words) = if on {
        (icons::CHECK, "In broadcast")
    } else {
        (icons::CLOSE, "Left out")
    };
    let width = (ui.gfx.measure(words, SMALL).width + 14.0 + space::XS + 2.0 * space::S).ceil();
    let chip = Rect::new(
        inner.r() - width - space::XS,
        inner.y + space::XS,
        width,
        24.0,
    );
    // Too small a pane: no room inside the terminal.
    if chip.x < inner.x + space::XS || chip.b() > inner.b() - space::XS {
        return false;
    }
    let name = format!("Broadcast to {}", pane.name());
    let response = ui.zone(id, chip, Role::CheckBox, &name);
    ui.set_toggled(id, on);
    let t = ui.theme;
    let color = if on { t.warn } else { t.text2 };
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    ui.gfx.fill_rect(chip, 12.0, t.raised);
    ui.gfx
        .fill_rect(chip, 12.0, color.alpha(0.16 + 0.1 * hover));
    ui.gfx.stroke_rect(chip, 12.0, 1.0, color.alpha(0.6));
    if response.focus_ring {
        ui.focus_ring(chip, 12.0);
    }
    let mut row = chip.inset(space::S, 0.0);
    let glyph = row.take_left(14.0).centered(14.0, 14.0);
    ui.gfx.icon(icon, glyph, color);
    row.take_left(space::XS);
    ui.gfx
        .text(row, words, SMALL, color.mix(t.text, 0.35), Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.clicked
}

#[cfg(test)]
mod tests {
    use annulus_config::tokens::Appearance;

    use super::*;

    fn leaves(split: &Split) -> Vec<u64> {
        let mut out = Vec::new();
        split.leaves(&mut out);
        out
    }

    fn layout(split: &Split, rect: Rect) -> Layout {
        let mut out = Layout {
            panes: Vec::new(),
            dividers: Vec::new(),
            list: None,
        };
        split.layout(rect, 0, &mut out);
        out
    }

    /// A tab of one pane that never starts: nothing is drawn.
    fn panes() -> Panes {
        Panes::new(LiveTerminal::new(Program::DefaultShell))
    }

    #[test]
    fn several_hosts_share_one_tab_with_about_the_same_room_each() {
        for count in [1_u64, 2, 3, 5, 9, 16] {
            let programs = (1..=count)
                .map(|n| (format!("host-{n}"), Program::DefaultShell))
                .collect();
            let tab = Panes::several(programs).unwrap();
            assert_eq!(tab.len(), usize::try_from(count).unwrap());
            assert_eq!(leaves(&tab.tree), (1..=count).collect::<Vec<_>>());
            let area = Rect::new(0.0, 0.0, 1600.0, 900.0);
            let placed = layout(&tab.tree, area).panes;
            let fair = 1600.0 * 900.0 / f32::from(u16::try_from(count).unwrap());
            for (id, rect) in &placed {
                assert!(
                    rect.w * rect.h > fair * 0.6,
                    "{count} panes: pane {id} gets {}x{}",
                    rect.w,
                    rect.h
                );
            }
            assert_eq!(
                tab.all[0].terminal.title.as_deref(),
                Some("host-1"),
                "each pane is named for its host"
            );
        }
        assert!(Panes::several(Vec::new()).is_none());
        let many = (0..20)
            .map(|n| (format!("host-{n}"), Program::DefaultShell))
            .collect();
        assert_eq!(Panes::several(many).unwrap().len(), MAX_PANES);
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
        let layout = layout(&tree, Rect::new(0.0, 0.0, 406.0, 206.0));
        // The root still splits in half; the right half is split a quarter of the way down.
        assert_eq!(layout.panes[0], (1, Rect::new(0.0, 0.0, 200.0, 206.0)));
        assert_eq!(layout.panes[1], (2, Rect::new(206.0, 0.0, 200.0, 50.0)));
        assert_eq!(layout.dividers.len(), 2);
    }

    #[test]
    fn focus_moves_to_the_nearest_pane_in_that_direction() {
        let mut panes = Panes {
            tree: Split::Leaf(1),
            all: Vec::new(),
            focused: 1,
            next: 4,
            broadcast: false,
            focus_mode: false,
            refocus: false,
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

    #[test]
    fn splitting_focuses_the_new_pane_and_closing_keeps_one() {
        let mut tab = panes();
        assert!(tab.split(false));
        assert_eq!((tab.len(), tab.focused), (2, 2));
        // A later split divides the focused pane only: the top pane keeps the full width.
        assert!(tab.split(true));
        assert_eq!((tab.order(), tab.focused), (vec![1, 2, 3], 3));
        let parts = tab.arrange(Rect::new(0.0, 0.0, 406.0, 206.0)).panes;
        assert_eq!(
            parts,
            [
                (1, Rect::new(0.0, 0.0, 406.0, 100.0)),
                (2, Rect::new(0.0, 106.0, 200.0, 100.0)),
                (3, Rect::new(206.0, 106.0, 200.0, 100.0)),
            ]
        );
        let mut closed = Vec::new();
        assert!(tab.close_focused(&mut closed));
        assert_eq!(
            (tab.len(), tab.focused),
            (2, 2),
            "the pane before it takes the keyboard"
        );
        tab.focused = 1;
        assert!(tab.close_focused(&mut closed));
        assert_eq!(tab.focused, 2, "the first pane hands over to the next");
        assert!(
            !tab.close_focused(&mut closed),
            "the last pane closes with the tab"
        );
        assert_eq!(tab.len(), 1);
        assert!(closed.is_empty(), "panes that never started leave no log");
    }

    #[test]
    fn parts_fill_the_area_with_gaps() {
        let mut tree = Split::Leaf(1);
        tree.split(1, 2, true);
        let area = Rect::new(0.0, 0.0, 206.0, 100.0);
        let parts = layout(&tree, area).panes;
        assert_eq!(parts[0], (1, Rect::new(0.0, 0.0, 100.0, 100.0)));
        assert_eq!(parts[1].1.x, parts[0].1.r() + HANDLE);
        assert_eq!(parts[1].1.r(), area.r());
        // Four stacked panes: each starts one handle below the last and the last ends at the
        // bottom.
        let mut rows = Split::Leaf(1);
        for id in 1..4 {
            rows.split(id, id + 1, false);
        }
        let area = Rect::new(0.0, 0.0, 100.0, 400.0);
        let parts = layout(&rows, area).panes;
        assert_eq!(parts.len(), 4);
        for pair in parts.windows(2) {
            assert_eq!(pair[1].1.y, pair[0].1.b() + HANDLE);
        }
        assert_eq!(parts[3].1.b(), area.b());
        // Splitting one pane until the tab is full never makes a part smaller than nothing.
        let mut narrow = Split::Leaf(1);
        for id in 1..16 {
            narrow.split(id, id + 1, true);
        }
        let parts = layout(&narrow, Rect::new(0.0, 0.0, 300.0, 100.0)).panes;
        assert_eq!(parts.len(), 16);
        assert!(
            parts
                .iter()
                .all(|(_, rect)| rect.w >= 0.0 && rect.r() <= 300.0 + 1e-3),
            "{parts:?}"
        );
    }

    #[test]
    fn focus_mode_gives_the_focused_pane_the_room_beside_a_list() {
        let mut tab = panes();
        tab.split(true);
        tab.split(false);
        tab.focused = 2;
        let area = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let split = tab.arrange(area);
        assert_eq!((split.panes.len(), split.list), (3, None));
        tab.toggle_focus_mode();
        let focus = tab.arrange(area);
        assert_eq!(
            focus.list,
            Some(Rect::new(1000.0 - FOCUS_LIST, 0.0, FOCUS_LIST, 600.0))
        );
        assert_eq!(
            focus.panes,
            [(2, Rect::new(0.0, 0.0, 1000.0 - FOCUS_LIST - HANDLE, 600.0))],
            "only the focused pane is shown, with the room the list leaves"
        );
        assert_eq!(focus.dividers.len(), 0);
        tab.toggle_focus_mode();
        assert_eq!(tab.arrange(area).panes.len(), 3, "back to the split");
        // One pane left: no list, and focus mode is over.
        tab.toggle_focus_mode();
        let mut closed = Vec::new();
        tab.close_focused(&mut closed);
        tab.close_focused(&mut closed);
        assert_eq!(tab.len(), 1);
        let single = tab.arrange(area);
        assert_eq!(
            (single.panes, single.list),
            (vec![(tab.focused, area)], None)
        );
        assert!(!tab.in_focus_mode());
        tab.toggle_focus_mode();
        assert!(!tab.in_focus_mode(), "one pane has no list");
    }

    #[test]
    fn broadcast_needs_two_panes_and_ends_when_one_is_left() {
        let mut tab = panes();
        tab.toggle_broadcast();
        assert!(!tab.broadcasting(), "nothing to broadcast to");
        tab.split(true);
        tab.toggle_broadcast();
        assert!(tab.broadcasting());
        tab.close_focused(&mut Vec::new());
        assert!(!tab.broadcasting());
        tab.split(true);
        assert!(!tab.broadcasting(), "a new split does not start it again");
    }

    #[test]
    fn a_pane_left_out_of_broadcast_gets_no_typing() {
        let mut tab = panes();
        tab.split(true);
        tab.split(true);
        tab.focused = 1;
        assert_eq!(tab.receivers(), [2, 3]);
        tab.all[2].skip_broadcast = true;
        assert_eq!(tab.receivers(), [2]);
        assert_eq!(
            tab.snippet_targets(),
            [1],
            "not broadcasting: the focused pane only"
        );
        let nothing: Vec<(u64, Vec<u8>)> = Vec::new();
        tab.all[0].terminal.sent = vec![b"uptime\r".to_vec()];
        assert_eq!(
            tab.broadcast_out(),
            nothing,
            "not broadcasting: typing stays"
        );
        tab.toggle_broadcast();
        assert_eq!(tab.snippet_targets(), [1, 2]);
        tab.all[0].terminal.sent = vec![b"uptime\r".to_vec()];
        assert_eq!(tab.broadcast_out(), [(2, b"uptime\r".to_vec())]);
        assert_eq!(tab.broadcast_out(), nothing, "what was typed goes out once");
        // Typing in the pane left out still reaches the others.
        tab.focused = 3;
        assert_eq!(tab.receivers(), [1, 2]);
    }

    #[test]
    fn focus_moves_round_and_a_tab_holds_at_most_sixteen() {
        let mut tab = panes();
        for _ in 1..MAX_PANES {
            assert!(tab.split(true));
        }
        assert!(!tab.split(false), "a seventeenth pane is refused");
        assert_eq!(tab.len(), MAX_PANES);
        let order = tab.order();
        assert_eq!(tab.focused, order[15], "the newest pane has the keyboard");
        tab.toggle_focus_mode();
        tab.move_focus(1.0, 0.0);
        assert_eq!(tab.focused, order[0], "wraps from the last to the first");
        tab.move_focus(0.0, -1.0);
        assert_eq!(tab.focused, order[15], "and back");
        tab.move_focus(0.0, -1.0);
        assert_eq!(tab.focused, order[14]);
    }

    #[test]
    fn pane_dots_take_the_themes_accent_warning_and_error_colours() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::new(appearance);
            assert_eq!(dot_color(&theme, TabStatus::Unread), theme.accent);
            assert_eq!(dot_color(&theme, TabStatus::Input), theme.warn);
            assert_eq!(dot_color(&theme, TabStatus::Error), theme.error);
        }
    }

    #[test]
    fn a_pane_is_named_after_what_it_runs_until_its_program_sets_a_title() {
        let shell = Program::Command {
            program: "/usr/bin/fish".into(),
            args: Vec::new(),
        };
        assert_eq!(program_name(&shell), "fish");
        assert_eq!(program_name(&Program::DefaultShell), "Shell");
        let mut tab = Panes::new(LiveTerminal::new(shell));
        assert_eq!(tab.pane(1).map(Pane::name), Some("fish"));
        tab.all[0].terminal.title = Some("~/src".into());
        assert_eq!(tab.pane(1).map(Pane::name), Some("~/src"));
    }
}
