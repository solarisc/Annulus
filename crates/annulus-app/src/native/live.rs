//! A live terminal in the new UI: a local shell or an SSH session from
//! `annulus-session`, drawn by our renderers. Keys go through the xterm encoder, pastes use
//! bracketed paste when the program asks for it (and a paste of several lines is confirmed
//! first when it does not), SSH questions (a new host key, a password) are answered inline
//! over the terminal, and a dropped SSH session reconnects by itself with a growing pause,
//! keeping its earlier output (F-SSH-009).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::{Program, Session, SessionEvent};
use annulus_term::alacritty_terminal::term::TermMode;
use annulus_term::find::Find;
use annulus_term::keys::{self, Mods};
use annulus_term::select::SelectionKind;
use annulus_term::{Palette, Size};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Font, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Role, Typed, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, button_width,
};

use super::ask::{self, Ask};
use super::terminal;
use crate::terminal_view::{
    Reconnect, TabStatus, carried, diagnostic, needs_confirm, next_step, one_line, paste_bytes,
};

/// One live session and what the tab shows about it.
pub struct LiveTerminal {
    program: Program,
    session: Option<Session>,
    grid: Option<(usize, usize)>,
    /// The title the program set, if any.
    pub title: Option<String>,
    failed: Option<String>,
    exited: bool,
    ask: Option<Ask>,
    unread: Arc<AtomicBool>,
    /// A drag selection in progress, from this cell.
    selecting: Option<(usize, usize)>,
    /// Wheel movement not yet a whole line.
    wheel: f32,
    find: Option<FindBar>,
    /// What the keyboard sent this frame, for broadcast to other panes.
    pub sent: Vec<Vec<u8>>,
    /// Automatic reconnects of a dropped SSH session.
    reconnect: Reconnect,
    /// Earlier output to show again once the next session starts.
    carry: Option<String>,
    /// Cell size in device pixels, for a reconnect while the tab is not drawn.
    cell: Option<(u16, u16)>,
    /// The current session has drawn something (cleared when it starts again).
    drawn: Arc<AtomicBool>,
    /// A paste of several lines waiting for a yes, since each line would run (F-TERM-006).
    held_paste: Option<String>,
    /// The SSH server's identification line, once its shell opened (F-HOST-009).
    pub server: Option<String>,
    /// How far connecting got: 0 reaching the server, 1 its key checked, 2 logged in, 3 the
    /// shell open (F-APP-007).
    stage: u8,
    /// What happened while connecting, for Show log.
    log: Vec<String>,
    /// The connecting card shows its log.
    show_log: bool,
    /// The saved host this connects to: its name and its server's OS, for the connecting card.
    pub identity: Option<(String, Option<&'static super::os::Os>)>,
    /// Where an agent typed into this terminal, marked beside its rows (F-AUTO-004).
    pub marks: AgentMarks,
    /// Where on the scroll bar's thumb a drag took it, while one goes on.
    bar_grab: Option<f32>,
    /// Held connecting, never started (the scenes).
    #[cfg(test)]
    held: bool,
}

/// Where an agent typed into a terminal (F-AUTO-004): the row each of its commands went in
/// on (counted from the oldest row of the scrollback), marked in the margin and on the
/// scroll bar, and the rows of the one picked in its panel, tinted.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AgentMarks {
    pub lines: Vec<usize>,
    pub picked: Option<std::ops::Range<usize>>,
}

/// A grid too small for any real window (the smallest Annulus allows has about 70 x 20): what
/// a minimized window draws into. A terminal never starts or resizes into one, which would
/// squeeze its output into a column or two for good.
fn usable((columns, rows): (usize, usize)) -> bool {
    columns >= 20 && rows >= 5
}

/// The grid a terminal starts in: `grid`, or a usual 120 x 30 while the window has no room
/// (minimized), until it is drawn at its real size.
fn start_grid(grid: (usize, usize)) -> (usize, usize) {
    if usable(grid) { grid } else { (120, 30) }
}

/// The thumb of a terminal's scroll bar on a track `h` tall: its height, the room it moves
/// in, and where in that room it is (0 at the oldest row, 1 at the newest screen).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bar {
    thumb_h: f32,
    room: f32,
    at: f32,
}

impl Bar {
    /// The thumb for `rows` of which `screen_rows` show; `None` while all of them fit.
    fn new(rows: annulus_session::Rows, screen_rows: usize, h: f32) -> Option<Self> {
        let most_top = rows.total.saturating_sub(screen_rows);
        if most_top == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)] // Rows of scrollback, at most a few thousand.
        let (total, screen, most) = (rows.total as f32, screen_rows as f32, most_top as f32);
        let thumb_h = (h * screen / total).clamp(24.0_f32.min(h), h);
        #[allow(clippy::cast_precision_loss)] // As above.
        let at = rows.top.min(most_top) as f32 / most;
        Some(Self {
            thumb_h,
            room: h - thumb_h,
            at,
        })
    }

    /// The row to show at the top with the thumb's top `top` down the track.
    fn row_at(self, top: f32, rows: annulus_session::Rows, screen_rows: usize) -> Option<usize> {
        if self.room <= 0.0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)] // As above.
        let most = rows.total.saturating_sub(screen_rows) as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // A row.
        let row = ((top / self.room).clamp(0.0, 1.0) * most).round() as usize;
        Some(row)
    }
}

/// The scroll bar down the right of a terminal (F-TERM-017): a thumb as tall as the screen
/// is of all `rows`, at the view, with a tick for each row in `marks`. Dragging the thumb
/// scrolls; a press on the track takes the thumb by its middle there. `grab` keeps where the
/// thumb was taken for the drag. Returns the row to show at the top while dragging. Nothing
/// shows while all of it fits on the screen.
fn scrollback_bar(
    ui: &mut Ui<'_>,
    id: Id,
    inner: Rect,
    (rows, screen_rows): (annulus_session::Rows, usize),
    marks: &[usize],
    grab: &mut Option<f32>,
) -> Option<usize> {
    let track = Rect::new(inner.r() + 5.0, inner.y + 2.0, 5.0, inner.h - 4.0);
    let Some(bar) = Bar::new(rows, screen_rows, track.h) else {
        *grab = None;
        return None;
    };
    let t = ui.theme;
    let Bar { thumb_h, room, at } = bar;
    #[allow(clippy::cast_precision_loss)] // Rows of scrollback, at most a few thousand.
    let total = rows.total as f32;
    // Wider than it is drawn, so it is easy to hit.
    let response = ui.zone(
        id,
        Rect::new(track.x - 4.0, inner.y, track.w + 8.0, inner.h),
        Role::ScrollView,
        "Scrollback",
    );
    let held = ui.dragging(id);
    let mut moved = None;
    if let (true, Some(pointer)) = (held, ui.pointer()) {
        let screen_track = ui.gfx.to_screen(track);
        let scale = screen_track.h / track.h;
        let taken = *grab.get_or_insert_with(|| {
            let thumb_y = screen_track.y + room * at * scale;
            let pressed = ui.pressed_at().map_or(pointer.y, |point| point.y);
            if (thumb_y..=thumb_y + thumb_h * scale).contains(&pressed) {
                (pressed - thumb_y) / scale
            } else {
                thumb_h / 2.0
            }
        });
        moved = bar.row_at(
            (pointer.y - screen_track.y) / scale - taken,
            rows,
            screen_rows,
        );
    } else {
        *grab = None;
    }
    let hot = response.hovered || held;
    let thumb = Rect::new(track.x, track.y + room * at, track.w, thumb_h);
    ui.gfx
        .fill_rect(thumb, 2.5, t.text2.alpha(if hot { 0.65 } else { 0.3 }));
    for &row in marks {
        #[allow(clippy::cast_precision_loss)] // As above.
        let y = track.y + track.h * (row as f32 / total);
        ui.gfx.fill_rect(
            Rect::new(track.x - 1.0, y, track.w + 2.0, 2.0),
            1.0,
            t.accent,
        );
    }
    moved
}

/// The step of Server, Key, Login, Shell under way (4 once all passed) at connecting `stage`:
/// a question waits on its own step.
fn active_step(stage: u8, ask: Option<&Ask>) -> usize {
    match ask {
        Some(Ask::HostKey(_)) => 1,
        Some(_) => 2,
        None => match stage {
            0 => 0,
            1 => 2,
            2 => 3,
            _ => 4,
        },
    }
}

/// The find bar of a terminal (Ctrl+Shift+F).
#[derive(Default)]
struct FindBar {
    query: String,
    search: Option<Find>,
    case: bool,
    focus: bool,
}

impl LiveTerminal {
    /// A terminal that starts `program` when first drawn.
    #[must_use]
    pub fn new(program: Program) -> Self {
        Self {
            program,
            session: None,
            grid: None,
            title: None,
            failed: None,
            exited: false,
            ask: None,
            unread: Arc::new(AtomicBool::new(false)),
            selecting: None,
            wheel: 0.0,
            find: None,
            sent: Vec::new(),
            reconnect: Reconnect::default(),
            carry: None,
            cell: None,
            drawn: Arc::new(AtomicBool::new(false)),
            held_paste: None,
            server: None,
            stage: 0,
            log: Vec::new(),
            show_log: false,
            identity: None,
            marks: AgentMarks::default(),
            bar_grab: None,
            #[cfg(test)]
            held: false,
        }
    }

    /// An SSH session that has not drawn anything yet: the tab says it is connecting.
    fn connecting(&self) -> bool {
        #[cfg(test)]
        if self.held {
            return true;
        }
        matches!(self.program, Program::Ssh { .. })
            && self.session.is_some()
            && self.failed.is_none()
            && !self.exited
            && !self.drawn.load(Ordering::Relaxed)
    }

    /// An SSH terminal held at connecting step `stage`, never dialing out, for the scenes.
    #[cfg(test)]
    pub fn held_at(program: Program, stage: u8, log: &[&str]) -> Self {
        Self {
            held: true,
            stage,
            log: log.iter().map(|line| (*line).to_owned()).collect(),
            show_log: true,
            ..Self::new(program)
        }
    }

    /// Takes in the session's news while the tab is not shown, so its dot stays current, and
    /// reconnects a dropped session when its time comes, at the size it last had. Returns
    /// the seconds until the next try, while one waits.
    pub fn poll(&mut self, now: f64, waker: Option<&Waker>) -> Option<f64> {
        self.drain(now);
        let at = self.reconnect.at?;
        if now < at {
            return Some(at - now);
        }
        self.restart();
        if let (Some(grid), Some(cell)) = (self.grid, self.cell) {
            self.spawn(grid, cell, waker, now);
        }
        None
    }

    /// Starts the program again: an SSH reconnect keeps the earlier output above a line.
    fn restart(&mut self) {
        if matches!(self.program, Program::Ssh { .. })
            && let Some(text) = self.transcript().filter(|text| !text.trim().is_empty())
        {
            self.carry = Some(carried(&text));
        }
        self.reconnect.at = None;
        self.session = None;
        self.failed = None;
        self.exited = false;
        self.ask = None;
        self.held_paste = None;
    }

    /// The connection dropped at `now`, or a reconnect failed: tries again after a pause, or
    /// gives up with the error.
    fn dropped(&mut self, now: f64, failure: Option<String>) {
        match self.reconnect.dropped(now, failure) {
            Some(error) => self.failed = Some(error),
            None => self.exited = true,
        }
    }

    /// The name the failure details give the tab.
    fn name(&self) -> String {
        self.title.clone().unwrap_or_else(|| match &self.program {
            Program::Ssh { target, .. } => format!("{}@{}", target.user, target.host),
            _ => "Local terminal".to_owned(),
        })
    }

    /// Drops the program's secrets, as when the vault locks; reconnecting needs them back
    /// through [`LiveTerminal::set_program`].
    pub fn forget_secrets(&mut self) {
        self.program.forget_secrets();
    }

    /// Runs `program` from the next start on (the host's login again, after an unlock).
    pub fn set_program(&mut self, program: Program) {
        self.program = program;
    }

    /// The program this terminal runs (a split pane runs it again).
    #[must_use]
    pub fn program(&self) -> Program {
        self.program.clone()
    }

    /// What the session showed, for its log; `None` if it never started.
    #[must_use]
    pub fn transcript(&self) -> Option<String> {
        self.session.as_ref().map(Session::transcript)
    }

    /// Why its program could not start or its connection ended, as the tab shows it.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// Whether its program has started (typing reaches it).
    #[must_use]
    pub fn started(&self) -> bool {
        self.session.is_some()
    }

    /// Whether its program has ended (a shell that exited): typing reaches nothing.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.exited
    }

    /// Goes up as the program's output comes in ([`Session::output`]); 0 before it starts.
    #[must_use]
    pub fn output(&self) -> u64 {
        self.session.as_ref().map_or(0, Session::output)
    }

    /// Scrolls the view so `row` (counted from the oldest row of the scrollback) is at the top.
    pub fn scroll_to_row(&self, row: usize) {
        if let Some(session) = &self.session {
            session.scroll_to_row(row);
        }
    }

    /// The row the cursor is on, where typing goes in; `None` before the program starts.
    #[must_use]
    pub fn cursor_row(&self) -> Option<usize> {
        self.session.as_ref().map(|session| session.rows().cursor)
    }

    /// Over the drawn screen: the agent's marks, and the scroll bar (not in a full-screen
    /// program such as vim or less, which has no scrollback of its own).
    fn draw_over(
        &mut self,
        ui: &mut Ui<'_>,
        inner: Rect,
        id: Id,
        (screen_rows, cell_h): (usize, f32),
    ) {
        let Some(session) = &self.session else {
            return;
        };
        let rows = session.rows();
        self.draw_marks(ui, inner, rows.top, (screen_rows, cell_h));
        if !session.mode().contains(TermMode::ALT_SCREEN)
            && let Some(top) = scrollback_bar(
                ui,
                id.with("scrollbar"),
                inner,
                (rows, screen_rows),
                &self.marks.lines,
                &mut self.bar_grab,
            )
        {
            session.scroll_to_row(top);
        }
    }

    /// Marks the agent's rows on screen, whose top shows row `top`: a bar in the margin beside
    /// each command's row, and a tint over the picked command's rows.
    fn draw_marks(&self, ui: &mut Ui<'_>, inner: Rect, top: usize, (rows, cell_h): (usize, f32)) {
        if self.marks.lines.is_empty() {
            return;
        }
        let t = ui.theme;
        let shown = top..top + rows;
        #[allow(clippy::cast_precision_loss)] // A screen row.
        let row_y = |line: usize| inner.y + (line - top) as f32 * cell_h;
        if let Some(picked) = &self.marks.picked {
            let from = picked.start.max(shown.start);
            let to = picked.end.min(shown.end);
            if from < to {
                let y = row_y(from);
                #[allow(clippy::cast_precision_loss)] // Screen rows.
                let h = (to - from) as f32 * cell_h;
                ui.gfx.fill_rect(
                    Rect::new(inner.x - space::S, y, inner.w + space::S, h),
                    2.0,
                    t.accent.alpha(0.10),
                );
            }
        }
        for &line in self.marks.lines.iter().filter(|line| shown.contains(*line)) {
            ui.gfx.fill_rect(
                Rect::new(
                    inner.x - space::S - 1.0,
                    row_y(line) + 2.0,
                    3.0,
                    cell_h - 4.0,
                ),
                1.5,
                t.accent,
            );
        }
    }

    /// Sends bytes typed in another pane (broadcast input).
    pub fn write(&self, bytes: &[u8]) {
        if let Some(session) = &self.session {
            session.scroll_to_bottom();
            session.write(bytes.to_vec());
        }
    }

    /// The wheel scrolls back through history; in a full-screen program (vim, less) it
    /// sends arrow keys instead, as xterm does.
    fn scroll(&mut self, ui: &mut Ui<'_>, session: &Session, inner: Rect, cell_h: f32) {
        let wheel = ui.take_wheel(inner);
        if wheel == 0.0 {
            return;
        }
        self.wheel += wheel / cell_h;
        let lines = self.wheel.trunc();
        if lines == 0.0 {
            return;
        }
        self.wheel -= lines;
        #[allow(clippy::cast_possible_truncation)] // A few lines per frame.
        let lines = lines as i32;
        let mode = session.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key = if lines > 0 {
                keys::Key::Up
            } else {
                keys::Key::Down
            };
            if let Some(bytes) = keys::encode(key, Mods::default(), mode) {
                for _ in 0..lines.unsigned_abs() {
                    session.write(bytes.clone());
                }
            }
        } else {
            session.scroll(lines);
        }
    }

    /// Drag selects, a double click selects a word, Ctrl+click opens a link.
    fn mouse(
        &mut self,
        ui: &mut Ui<'_>,
        session: &Session,
        response: &annulus_ui::kit::Response,
        id: Id,
        cell_at: impl Fn(annulus_ui::gfx::Point) -> (usize, usize),
    ) {
        let Some(pointer) = ui.pointer() else { return };
        let (row, column) = cell_at(pointer);
        let ctrl = ui.modifiers().ctrl;
        let link = (ctrl && response.hovered)
            .then(|| session.link_at(row, column))
            .flatten();
        if let Some(link) = link {
            ui.set_cursor(annulus_ui::kit::Cursor::Pointer);
            // Where it really goes, which a link's text can hide.
            let near = annulus_ui::gfx::Rect::new(pointer.x - 8.0, pointer.y - 8.0, 16.0, 16.0);
            ui.tooltip(id, near, &link_hint(&link.url));
        } else if response.hovered {
            ui.set_cursor(annulus_ui::kit::Cursor::Text);
        }
        if response.clicked && ctrl {
            if let Some(link) = session.link_at(row, column)
                && annulus_term::links::allowed(&link.url)
            {
                annulus_platform::window::open_url(&link.url);
            }
            return;
        }
        if response.double_clicked {
            session.select_start(row, column, SelectionKind::Words);
            session.select_to(row, column);
            self.selecting = None;
            return;
        }
        if ui.dragging(id) {
            match self.selecting {
                None => {
                    session.select_start(row, column, SelectionKind::Characters);
                    self.selecting = Some((row, column));
                }
                Some(_) => session.select_to(row, column),
            }
        } else if let Some(start) = self.selecting.take()
            && start == (row, column)
        {
            // A click without a drag clears the selection.
            session.clear_selection();
        }
    }

    /// Keys for the program, and the terminal's own shortcuts.
    fn keyboard(&mut self, ui: &mut Ui<'_>) {
        let Some(session) = &self.session else { return };
        let mode = session.mode();
        self.sent.clear();
        for typed in ui.take_typed() {
            match typed {
                Typed::Key(key, modifiers) if super::shortcuts::COPY.is(key, modifiers) => {
                    if let Some(text) = session.selection_text() {
                        ui.copy(text);
                    }
                }
                Typed::Key(key, modifiers) if super::shortcuts::FIND.is(key, modifiers) => {
                    self.find.get_or_insert_with(FindBar::default).focus = true;
                }
                Typed::Key(key, modifiers) if super::shortcuts::CLEAR.is(key, modifiers) => {
                    session.clear_scrollback();
                }
                // Each line would run as it arrives: ask first, and send nothing typed after it
                // until the question is answered.
                Typed::Paste(text) if needs_confirm(&text, mode) => {
                    self.held_paste = Some(text);
                    break;
                }
                typed => {
                    if let Some(bytes) = encode(&typed, mode) {
                        session.scroll_to_bottom();
                        session.clear_selection();
                        self.sent.push(bytes.clone());
                        session.write(bytes);
                    }
                }
            }
        }
    }

    fn find_bar(&mut self, ui: &mut Ui<'_>, bar: Rect, id: Id) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let Some(session) = &self.session else { return };
        let t = ui.theme;
        let panel = bar.inset(0.0, 4.0);
        ui.gfx.fill_rect(panel, 10.0, t.raised);
        ui.gfx.stroke_rect(panel, 10.0, 1.0, t.line);
        let mut row = panel.inset(space::S, 4.0);
        let close = row.take_right(28.0);
        row.take_right(4.0);
        let next = row.take_right(28.0);
        row.take_right(4.0);
        let previous = row.take_right(28.0);
        row.take_right(space::S);
        let case = row.take_right(button_width(&mut ui.gfx, "Aa", None));
        row.take_right(space::S);
        let field_id = id.with("find");
        if find.focus {
            ui.state.set_focus(Some(field_id), false);
            find.focus = false;
        }
        let field = ui.text_field(
            field_id,
            row,
            &mut find.query,
            FieldStyle {
                placeholder: "Find in this terminal",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        if ui
            .button(
                id.with("find.case"),
                case,
                "Aa",
                None,
                if find.case {
                    Kind::Secondary
                } else {
                    Kind::Ghost
                },
            )
            .clicked
        {
            find.case = !find.case;
            find.search = None;
        }
        if field.changed || (find.search.is_none() && !find.query.is_empty()) {
            find.search = Find::new(&find.query, false, find.case).ok();
        }
        let back = ui
            .icon_button(
                id.with("find.previous"),
                previous,
                icons::CHEVRON_UP,
                "Previous match",
                false,
            )
            .clicked
            || ui.key(Key::Enter, Modifiers::SHIFT);
        let forward = ui
            .icon_button(
                id.with("find.next"),
                next,
                icons::CHEVRON_DOWN,
                "Next match",
                false,
            )
            .clicked
            || field.submitted;
        if let Some(search) = &mut find.search
            && (back || forward)
        {
            session.find_step(search, back);
        }
        let closed = ui
            .icon_button(
                id.with("find.close"),
                close,
                icons::CLOSE,
                "Close find",
                false,
            )
            .clicked
            || (field.response.focused && ui.key(Key::Escape, Modifiers::NONE));
        if closed {
            self.find = None;
        }
    }

    /// The tab's status dot.
    #[must_use]
    pub fn status(&self) -> Option<TabStatus> {
        if self.failed.is_some() || self.reconnect.at.is_some() {
            Some(TabStatus::Error)
        } else if self.ask.is_some() {
            Some(TabStatus::Input)
        } else if self.unread.load(Ordering::Relaxed) {
            Some(TabStatus::Unread)
        } else {
            None
        }
    }

    fn start(
        &mut self,
        ui: &mut Ui<'_>,
        font: Font,
        (columns, rows): (usize, usize),
        waker: Option<&Waker>,
    ) {
        let (cell_w, cell_h) = terminal::cell_size(ui, font);
        let scale = ui.gfx.scale();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small pixels.
        let cell = ((cell_w * scale) as u16, (cell_h * scale) as u16);
        self.spawn((columns, rows), cell, waker, ui.time());
    }

    /// Starts the session in a grid of `columns` x `rows` cells `cell` device pixels large,
    /// showing any output carried over from before a reconnect.
    fn spawn(
        &mut self,
        (columns, rows): (usize, usize),
        cell: (u16, u16),
        waker: Option<&Waker>,
        now: f64,
    ) {
        let unread = self.unread.clone();
        self.drawn = Arc::new(AtomicBool::new(false));
        let drawn = self.drawn.clone();
        let waker = waker.cloned();
        let wake = move || {
            unread.store(true, Ordering::Relaxed);
            drawn.store(true, Ordering::Relaxed);
            if let Some(waker) = &waker {
                waker();
            }
        };
        self.stage = 0;
        self.log = vec![match &self.program {
            Program::Ssh { target, .. } => {
                format!(
                    "Reaching {}:{} as {}",
                    target.host, target.port, target.user
                )
            }
            _ => "Starting the shell".to_owned(),
        }];
        match Session::spawn(
            &self.program,
            Size::new(columns, rows),
            cell.0,
            cell.1,
            wake,
        ) {
            Ok(session) => {
                if let Some(text) = self.carry.take() {
                    session.show(&text);
                }
                self.session = Some(session);
                self.reconnect.started = now;
            }
            Err(error) => self.failed = Some(format!("Could not start: {error}")),
        }
        self.grid = Some((columns, rows));
        self.cell = Some(cell);
    }

    fn drain(&mut self, now: f64) {
        let Some(session) = &self.session else { return };
        let events = session.take_events();
        let reconnects = matches!(
            self.program,
            Program::Ssh {
                reconnect: true,
                ..
            }
        );
        for event in events {
            match event {
                SessionEvent::Title(title) => {
                    if let Some(title) = program_title(&title) {
                        self.title = Some(title);
                    }
                }
                SessionEvent::Bell => {}
                SessionEvent::Exited => {
                    self.exited = true;
                    self.reconnect.stop();
                }
                SessionEvent::Lost if reconnects => self.dropped(now, None),
                SessionEvent::Lost => self.failed = Some("The connection dropped.".into()),
                SessionEvent::HostKey(_) | SessionEvent::Password { .. } => {
                    self.log.push(match &event {
                        SessionEvent::HostKey(question) => format!(
                            "{} is new: asking whether to trust its {} key",
                            question.host, question.algorithm
                        ),
                        _ => "Asking for the password".to_owned(),
                    });
                    self.ask = Ask::from_event(event);
                }
                SessionEvent::Progress { host, step } => {
                    let (stage, line) = match step {
                        annulus_session::Step::KeyChecked => {
                            (1, format!("{host}'s key is trusted"))
                        }
                        annulus_session::Step::LoggedIn => (2, format!("Logged in to {host}")),
                    };
                    self.stage = self.stage.max(stage);
                    self.log.push(line);
                }
                // A reconnect that fails waits and tries again.
                SessionEvent::Failed(message) if self.reconnect.tries > 0 => {
                    self.dropped(now, Some(message));
                }
                SessionEvent::Failed(message) => self.failed = Some(message),
                SessionEvent::Server(id) => {
                    self.stage = 3;
                    self.log.push(format!("Shell open on {}", id.trim()));
                    self.server = Some(id);
                }
            }
        }
    }

    /// Draws the terminal into `rect` and feeds it the keyboard while focused.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        (font, palette): (Font, &Palette),
        waker: Option<&Waker>,
        id: Id,
    ) {
        let inner = rect.inset(space::L, space::M);
        let grid = terminal::grid_size(ui, inner, font);
        #[cfg(test)]
        let held = self.held;
        #[cfg(not(test))]
        let held = false;
        if self.session.is_none() && self.failed.is_none() && !self.exited && !held {
            self.start(ui, font, start_grid(grid), waker);
            ui.state.set_focus(Some(id), false);
        } else if usable(grid)
            && self.grid != Some(grid)
            && let Some(session) = &self.session
        {
            let (cell_w, cell_h) = terminal::cell_size(ui, font);
            let scale = ui.gfx.scale();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small pixels.
            session.resize(
                Size::new(grid.0, grid.1),
                (cell_w * scale) as u16,
                (cell_h * scale) as u16,
            );
            self.grid = Some(grid);
        }
        if let Some(session) = &mut self.session {
            session.set_palette(palette);
        }
        if let Some(wait) = self.poll(ui.time(), waker) {
            ui.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        // The find bar takes the bottom of the tab while open.
        let mut inner = inner;
        let bar = self.find.is_some().then(|| inner.take_bottom(44.0));
        let response = ui.zone(id, inner, Role::Terminal, "Terminal");
        let (cell_w, cell_h) = terminal::cell_size(ui, font);
        let cell_at = |point: annulus_ui::gfx::Point| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let column = ((point.x - inner.x) / cell_w).max(0.0) as usize;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let row = ((point.y - inner.y) / cell_h).max(0.0) as usize;
            (
                row.min(grid.1.saturating_sub(1)),
                column.min(grid.0.saturating_sub(1)),
            )
        };
        if let Some(session) = self.session.take() {
            self.scroll(ui, &session, inner, cell_h);
            self.mouse(ui, &session, &response, id, cell_at);
            self.session = Some(session);
        }
        if response.focused && self.exited && self.failed.is_none() {
            // Enter starts an ended session again; Escape stops a waiting reconnect.
            if ui.key(Key::Enter, Modifiers::NONE) {
                self.restart();
            } else if ui.key(Key::Escape, Modifiers::NONE) {
                self.reconnect.stop();
            }
        }
        if response.focused && self.ask.is_none() && self.held_paste.is_none() {
            self.unread.store(false, Ordering::Relaxed);
            self.keyboard(ui);
        }
        if let Some(session) = &self.session {
            let screen = session.screen();
            terminal::draw(ui, inner, &screen, font, response.focused);
            if let Some(find) = &mut self.find
                && let Some(search) = &mut find.search
            {
                let t = ui.theme;
                ui.push_clip(inner);
                for mark in session.find_visible(search) {
                    #[allow(clippy::cast_precision_loss)] // Screen cells.
                    let cell = Rect::new(
                        inner.x + mark.start as f32 * cell_w,
                        inner.y + mark.row as f32 * cell_h,
                        (mark.end + 1 - mark.start) as f32 * cell_w,
                        cell_h,
                    );
                    let color = if mark.current { t.warn } else { t.accent };
                    ui.gfx.fill_rect(cell, 2.0, color.alpha(0.35));
                }
                ui.pop_clip();
            }
        }
        self.draw_over(ui, inner, id, (grid.1, cell_h));
        if self.connecting() {
            self.connect_card(ui, rect, id);
        }
        if let Some(bar) = bar {
            self.find_bar(ui, bar, id);
        }
        self.ask_card(ui, rect, id);
        self.paste_card(ui, rect, id);
        self.ended_banner(ui, rect, id);
    }

    /// Where an SSH session is while it connects (F-APP-007): the host, its four steps on a
    /// track that fills as each passes (the one under way spins, one waiting for an answer
    /// says so), and what happened so far behind Show log.
    #[allow(clippy::too_many_lines)] // The card top to bottom: head, track, log, hint.
    fn connect_card(&mut self, ui: &mut Ui<'_>, rect: Rect, terminal: Id) {
        const STEPS: [&str; 4] = ["Server", "Key", "Login", "Shell"];
        let t = ui.theme;
        let id = terminal.with("connect");
        let (mut who, route) = match &self.program {
            Program::Ssh { target, .. } => (
                target.host.clone(),
                format!("SSH {}@{}:{}", target.user, target.host, target.port),
            ),
            _ => (self.name(), String::new()),
        };
        let mut mark = None;
        if let Some((label, os)) = &self.identity {
            label.clone_into(&mut who);
            mark = *os;
        }
        let active = active_step(self.stage, self.ask.as_ref());
        let log_h = if self.show_log { 132.0 } else { 0.0 };
        let width = (rect.w - 2.0 * space::XL).clamp(0.0, 520.0);
        let height = 196.0 + log_h;
        let top = rect.y + (rect.h * 0.16).max(space::XL);
        let card = Rect::new(rect.center().x - width / 2.0, top, width, height);
        let shown = ui.entrance(id.with("enter"), Motion::SLOW);
        ui.push_layer(Layer::appear(shown, card.center(), 0.96, 10.0));
        ui.gfx.shadow(card, 16.0, 30.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 16.0, t.raised);
        ui.gfx.stroke_rect(card, 16.0, 1.0, t.line);
        let mut inner = card.inset(space::XL, space::XL);
        // The host, and Show log.
        let mut head = inner.take_top(44.0);
        let tile = head.take_left(44.0).centered(40.0, 40.0);
        super::os::tile(ui, tile, &who, mark);
        head.take_left(space::M);
        let label = if self.show_log {
            "Hide log"
        } else {
            "Show log"
        };
        let toggle = head.take_right(button_width(&mut ui.gfx, label, None));
        let name = Rect::new(head.x, head.y + 2.0, head.w - space::S, 22.0);
        ui.gfx.text(name, &who, HEADING, t.text, Align::Start);
        let detail = Rect::new(head.x, head.y + 24.0, head.w - space::S, 18.0);
        ui.gfx.text(detail, &route, SMALL, t.text2, Align::Start);
        if ui
            .button(
                id.with("log"),
                toggle.centered(toggle.w, CONTROL),
                label,
                None,
                Kind::Secondary,
            )
            .clicked
        {
            self.show_log = !self.show_log;
        }
        inner.take_top(space::XL);
        // The track: a node per step, the line between them filling as steps pass.
        let track = inner.take_top(28.0);
        let span = track.w - 28.0;
        #[allow(clippy::cast_precision_loss)] // Four steps.
        let node_x = |step: f32| track.x + 14.0 + span * step / (STEPS.len() - 1) as f32;
        #[allow(clippy::cast_precision_loss)] // As above.
        let reached = ui.animate(id.with("track"), active.min(3) as f32, Motion::SLOW);
        let y = track.center().y;
        ui.gfx.fill_rect(
            Rect::new(node_x(0.0), y - 1.5, span, 3.0),
            1.5,
            t.line_strong,
        );
        ui.gfx.fill_rect(
            Rect::new(node_x(0.0), y - 1.5, node_x(reached) - node_x(0.0), 3.0),
            1.5,
            t.accent,
        );
        let labels = inner.take_top(22.0);
        for (index, step) in STEPS.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let center = Point::new(node_x(index as f32), y);
            let done = index < active;
            if done {
                ui.gfx.fill_circle(center, 11.0, t.accent);
                ui.gfx.icon(
                    icons::CHECK,
                    Rect::new(center.x - 7.0, center.y - 7.0, 14.0, 14.0),
                    t.on_accent,
                );
            } else if index == active {
                ui.gfx.fill_circle(center, 11.0, t.raised);
                ui.gfx.stroke_circle(center, 10.0, 2.0, t.accent.alpha(0.3));
                // A quarter turn of the ring, going round while this step is under way.
                #[allow(clippy::cast_possible_truncation)] // An angle for drawing.
                let turn = (ui.time() * 300.0 % 360.0) as f32;
                let arc: Vec<Point> = annulus_ui::icons::Segment::arc_points(
                    center.x,
                    center.y,
                    10.0,
                    turn,
                    turn + 100.0,
                )
                .into_iter()
                .map(|(x, y)| Point::new(x, y))
                .collect();
                ui.gfx.polyline(&arc, false, 2.5, t.accent);
                ui.request_repaint_after(std::time::Duration::from_millis(16));
            } else {
                ui.gfx.fill_circle(center, 11.0, t.raised);
                ui.gfx.stroke_circle(center, 10.0, 2.0, t.line_strong);
            }
            let cell = Rect::new(center.x - 40.0, labels.y + 4.0, 80.0, 18.0);
            let ink = if index == active { t.text } else { t.text2 };
            ui.gfx.text(cell, step, SMALL, ink, Align::Center);
        }
        inner.take_top(space::M);
        let waiting = match &self.ask {
            Some(Ask::HostKey(_)) => "Waiting for you to check the server's key",
            Some(_) => "Waiting for the password",
            None => match active {
                0 => "Reaching the server\u{2026}",
                1 | 2 => "Logging in\u{2026}",
                _ => "Opening the shell\u{2026}",
            },
        };
        ui.gfx
            .text(inner.take_top(20.0), waiting, BODY, t.text2, Align::Center);
        if self.show_log {
            inner.take_top(space::M);
            let box_rect = inner.take_top(log_h - space::M);
            ui.gfx.fill_rect(box_rect, 10.0, t.field);
            let mut lines = box_rect.inset(space::M, space::S);
            for line in self.log.iter().rev().take(6).rev() {
                ui.gfx.text(
                    lines.take_top(18.0),
                    line,
                    Font::mono(11.5),
                    t.text2,
                    Align::Start,
                );
            }
        }
        ui.pop_layer();
    }

    /// Asks before pasting several lines into a program that did not ask for bracketed
    /// paste: each line would run as soon as it arrives.
    fn paste_card(&mut self, ui: &mut Ui<'_>, rect: Rect, terminal: Id) {
        let Some(text) = &self.held_paste else { return };
        let t = ui.theme;
        let lines = text.lines().count().max(1);
        ui.gfx.fill_rect(rect, 0.0, t.scrim);
        ui.zone(
            terminal.with("live.paste.backdrop"),
            rect,
            Role::Backdrop,
            "",
        );
        let card = rect.centered(rect.w.min(480.0), 186.0);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        ui.strict_text(
            column.take_top(24.0),
            "Paste several lines?",
            HEADING,
            t.text,
            Align::Start,
        );
        column.take_top(space::XS);
        ui.gfx.text(
            column.take_top(20.0),
            &format!(
                "The text has {lines} lines. Each one runs as a command as soon as it is pasted."
            ),
            SMALL,
            t.text2,
            Align::Start,
        );
        column.take_top(space::L);
        let mut row = column.take_top(CONTROL);
        let paste = row.take_right(button_width(&mut ui.gfx, "Paste", None).max(80.0));
        row.take_right(space::S);
        let one = row.take_right(button_width(&mut ui.gfx, "Paste as one line", None));
        row.take_right(space::S);
        let cancel = row.take_right(button_width(&mut ui.gfx, "Cancel", None));
        let mut choice = None;
        // Drawn left to right, so Tab follows the eye.
        if ui
            .button(
                terminal.with("live.paste.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Ghost,
            )
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE)
        {
            choice = Some(None);
        }
        if ui
            .button(
                terminal.with("live.paste.one"),
                one,
                "Paste as one line",
                None,
                Kind::Secondary,
            )
            .clicked
        {
            choice = Some(Some(one_line(text)));
        }
        if ui
            .button(
                terminal.with("live.paste.go"),
                paste,
                "Paste",
                None,
                Kind::Primary,
            )
            .clicked
        {
            choice = Some(Some(text.clone()));
        }
        let Some(choice) = choice else { return };
        self.held_paste = None;
        // The keyboard goes back to the terminal.
        ui.state.set_focus(Some(terminal), false);
        if let (Some(text), Some(session)) = (choice, &self.session) {
            let bytes = paste_bytes(&text, session.mode());
            session.scroll_to_bottom();
            self.sent.push(bytes.clone());
            session.write(bytes);
        }
    }

    fn ask_card(&mut self, ui: &mut Ui<'_>, rect: Rect, id: Id) {
        let (Some(ask), Some(session)) = (self.ask.as_mut(), &self.session) else {
            return;
        };
        if ask::card(ui, id, rect, ask, session.answers()) {
            self.ask = None;
        }
    }

    /// The bar over a session that ended, dropped or failed: why, what to try, and a way to
    /// start again.
    #[allow(clippy::too_many_lines)] // Three states of one bar, laid out once.
    fn ended_banner(&mut self, ui: &mut Ui<'_>, rect: Rect, id: Id) {
        if self.failed.is_none() && !self.exited {
            return;
        }
        let t = ui.theme;
        let now = ui.time();
        let error = self.failed.is_some();
        let waiting = self.reconnect.at;
        let message = match (&self.failed, waiting) {
            (Some(failed), _) => failed.clone(),
            (None, Some(at)) => format!(
                "Connection lost \u{2014} reconnecting in {:.0} s",
                (at - now).max(0.0).ceil()
            ),
            (None, None) => "The session ended. Press Enter to start it again.".into(),
        };
        let step = self.failed.as_deref().and_then(next_step);
        let height = if step.is_some() { 64.0 } else { 44.0 };
        let bar = Rect::new(
            rect.x + space::L,
            rect.b() - height - 12.0,
            rect.w - 2.0 * space::L,
            height,
        );
        ui.gfx.shadow(bar, 10.0, 14.0, 4.0, t.shadow);
        ui.gfx.fill_rect(bar, 10.0, t.raised);
        let edge = if error { t.error } else { t.line_strong };
        ui.gfx.stroke_rect(bar, 10.0, 1.0, edge);
        let mut inner = bar.inset(space::M, 0.0);
        let icon = inner.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(
            if error { icons::WARNING } else { icons::INFO },
            icon,
            if error { t.error } else { t.text2 },
        );
        inner.take_left(space::S);
        let ssh = matches!(self.program, Program::Ssh { .. });
        let again_label = match (waiting, ssh) {
            (Some(_), _) => "Reconnect now",
            (None, true) => "Reconnect",
            (None, false) => "Start again",
        };
        let again = inner.take_right(button_width(&mut ui.gfx, again_label, Some(icons::SYNC)));
        let again = again.centered(again.w, 30.0);
        inner.take_right(space::S);
        let second = if waiting.is_some() {
            Some("Stop")
        } else if error {
            Some("Copy details")
        } else {
            None
        };
        let second_rect = second.map(|label| {
            let slot = inner.take_right(button_width(&mut ui.gfx, label, None));
            inner.take_right(space::S);
            slot.centered(slot.w, 30.0)
        });
        match step {
            Some(step) => {
                let top = Rect::new(inner.x, inner.center().y - 19.0, inner.w, 20.0);
                let bottom = Rect::new(inner.x, inner.center().y + 1.0, inner.w, 18.0);
                ui.gfx
                    .text(top, &message, BODY_STRONG, t.text, Align::Start);
                ui.gfx.text(bottom, step, SMALL, t.text2, Align::Start);
            }
            None => {
                ui.gfx.text(
                    inner,
                    &message,
                    if error { BODY_STRONG } else { BODY },
                    t.text,
                    Align::Start,
                );
            }
        }
        if waiting.is_some() {
            ui.request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Drawn left to right, so Tab follows the eye.
        if let (Some(label), Some(slot)) = (second, second_rect)
            && ui
                .button(id.with("live.second"), slot, label, None, Kind::Ghost)
                .clicked
        {
            if waiting.is_some() {
                self.reconnect.stop();
            } else if let Some(failed) = &self.failed {
                // For a bug report: the version, the tab and the error; no secrets.
                ui.copy(diagnostic(&self.name(), failed));
            }
        }
        if ui
            .button(
                id.with("live.again"),
                again,
                again_label,
                Some(icons::SYNC),
                Kind::Secondary,
            )
            .clicked
        {
            self.reconnect.stop();
            self.restart();
        }
    }
}

/// The bytes one typed input sends to a program in `mode`, or `None` for input it ignores.
fn encode(typed: &Typed, mode: TermMode) -> Option<Vec<u8>> {
    match typed {
        Typed::Text(text) => Some(text.as_bytes().to_vec()),
        // Wrapped when the program asked for it, and never able to end the bracket early.
        Typed::Paste(text) => Some(paste_bytes(text, mode)),
        Typed::Key(key, modifiers) => {
            let held = Mods {
                shift: modifiers.shift,
                alt: modifiers.alt,
                ctrl: modifiers.ctrl,
            };
            let key = match key {
                Key::Enter => keys::Key::Enter,
                Key::Backspace => keys::Key::Backspace,
                Key::Tab => keys::Key::Tab,
                Key::Escape => keys::Key::Escape,
                Key::Up => keys::Key::Up,
                Key::Down => keys::Key::Down,
                Key::Left => keys::Key::Left,
                Key::Right => keys::Key::Right,
                Key::Home => keys::Key::Home,
                Key::End => keys::Key::End,
                Key::PageUp => keys::Key::PageUp,
                Key::PageDown => keys::Key::PageDown,
                Key::Delete => keys::Key::Delete,
                Key::F(n) => keys::Key::F(*n),
                // Ctrl+letter is the shell's (UI-SPEC §7); Ctrl+Shift+letter is the app's.
                Key::Char(c) if modifiers.ctrl && !modifiers.shift => keys::Key::Ctrl(*c),
                // Alt+letter sends ESC and the letter, as xterm's meta does.
                Key::Char(c) if modifiers.alt && !modifiers.ctrl => {
                    return Some(format!("\x1b{c}").into_bytes());
                }
                // Plain characters and Space arrive as text.
                Key::Char(_) | Key::Space => return None,
            };
            keys::encode(key, held, mode)
        }
    }
}

/// What a link's tooltip says: its real target, and whether a click opens it.
fn link_hint(url: &str) -> String {
    const LONGEST: usize = 80;
    let shown: String = if url.chars().count() > LONGEST {
        url.chars()
            .take(LONGEST - 1)
            .chain(std::iter::once('\u{2026}'))
            .collect()
    } else {
        url.to_owned()
    };
    if annulus_term::links::allowed(url) {
        format!("Ctrl+click opens {shown}")
    } else {
        format!("Not opened: {shown}")
    }
}

/// The title a program set, as the tab shows it; `None` when it only names the program's
/// file, as Windows' console does for a shell it starts (`C:\...\pwsh.exe`): the tab keeps
/// its own name then. A path heading a longer title is cut to its file name
/// (`cmd.exe - ping host`).
fn program_title(title: &str) -> Option<String> {
    let title = title.trim();
    let bare = title.strip_prefix("Administrator: ").unwrap_or(title);
    let path_end = bare
        .to_ascii_lowercase()
        .find(".exe")
        .map(|at| at + ".exe".len())
        .filter(|&end| bare[..end].contains(['\\', '/']));
    let Some(end) = path_end else {
        return (!title.is_empty()).then(|| title.to_owned());
    };
    let rest = &bare[end..];
    if rest.trim().is_empty() {
        return None;
    }
    let path = &bare[..end];
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    Some(format!("{name}{rest}"))
}

#[cfg(test)]
mod tests {
    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_ui::gfx::Rect;
    use annulus_ui::kit::{Event, Theme, UiState};

    use super::*;

    #[test]
    fn a_console_title_that_only_names_the_shells_file_leaves_the_tab_its_name() {
        assert_eq!(
            program_title(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            None
        );
        assert_eq!(
            program_title(r"Administrator: C:\WINDOWS\system32\cmd.exe"),
            None
        );
        assert_eq!(
            program_title(r"C:\WINDOWS\system32\cmd.exe - ping -n 4 host").as_deref(),
            Some("cmd.exe - ping -n 4 host"),
            "a path heading a longer title is cut to the file's name"
        );
        assert_eq!(
            program_title("deploy@web-1: ~/app").as_deref(),
            Some("deploy@web-1: ~/app")
        );
        assert_eq!(
            program_title("notes.exe.txt - vim").as_deref(),
            Some("notes.exe.txt - vim"),
            "a file name with no folder is no program path"
        );
    }

    #[test]
    fn a_minimized_window_neither_starts_nor_squeezes_a_terminal_into_a_column() {
        // Minimized: next to no room. The terminal starts at a usual size instead.
        assert!(!usable((1, 1)));
        assert_eq!(start_grid((2, 40)), (120, 30));
        // The smallest real window, and a usual one, are taken as they are.
        assert!(usable((70, 20)));
        assert_eq!(start_grid((70, 20)), (70, 20));
        assert_eq!(start_grid((160, 45)), (160, 45));
    }

    #[test]
    fn the_scroll_bar_thumb_is_the_screens_share_and_a_drag_lands_on_the_matching_row() {
        let rows = |top, total| annulus_session::Rows {
            top,
            cursor: 0,
            total,
        };
        assert_eq!(Bar::new(rows(0, 30), 30, 300.0), None, "it all fits");

        // 40 of 400 rows show: a tenth of the track, at the bottom while following output.
        let bar = Bar::new(rows(360, 400), 40, 300.0).unwrap();
        assert_eq!((bar.thumb_h, bar.room, bar.at), (30.0, 270.0, 1.0));
        let oldest = Bar::new(rows(0, 400), 40, 300.0).unwrap();
        assert_eq!(oldest.at, 0.0);

        // Dragged to the top, the middle and past the end of the track.
        assert_eq!(bar.row_at(0.0, rows(360, 400), 40), Some(0));
        assert_eq!(bar.row_at(135.0, rows(360, 400), 40), Some(180));
        assert_eq!(bar.row_at(999.0, rows(360, 400), 40), Some(360));

        // A huge scrollback keeps the thumb big enough to take.
        let long = Bar::new(rows(0, 10_040), 40, 300.0).unwrap();
        assert_eq!(long.thumb_h, 24.0);
    }

    #[test]
    fn a_link_names_its_real_target_and_whether_a_click_opens_it() {
        assert_eq!(
            link_hint("https://example.org/x"),
            "Ctrl+click opens https://example.org/x"
        );
        assert_eq!(
            link_hint("file:///etc/passwd"),
            "Not opened: file:///etc/passwd"
        );
        let long = format!("https://example.org/{}", "a".repeat(200));
        let hint = link_hint(&long);
        assert!(hint.starts_with("Ctrl+click opens https://example.org/aaa"));
        assert!(
            hint.ends_with('\u{2026}') && hint.chars().count() < 100,
            "{hint}"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_real_shell_runs_in_the_new_terminal_and_gets_what_is_typed() {
        let program = if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: Vec::new(),
            }
        };
        let mut live = LiveTerminal::new(program);
        let mut state = UiState::default();
        let mut text = Some(TextEngine::new());
        let id = Id::new("live-test");
        let started = std::time::Instant::now();
        let mut sent = false;
        let mut seen = String::new();
        while started.elapsed() < std::time::Duration::from_secs(20) {
            let mut canvas = CpuCanvas::new(800, 400, 1.0, text.take().unwrap()).unwrap();
            let events = if live.session.is_some() && !sent {
                sent = true;
                vec![
                    Event::Text("echo got:typed".into()),
                    Event::Key(Key::Enter, Modifiers::NONE),
                ]
            } else {
                Vec::new()
            };
            state.begin_frame(&events, started.elapsed().as_secs_f64());
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                &mut state,
                Rect::new(0.0, 0.0, 800.0, 400.0),
            );
            let palette = Palette::annulus_dark();
            live.draw(
                &mut ui,
                Rect::new(0.0, 0.0, 800.0, 400.0),
                (Font::mono(13.0), &palette),
                None,
                id,
            );
            state.end_frame();
            text = Some(canvas.into_text_engine());
            if let Some(session) = &live.session {
                seen = session
                    .screen()
                    .rows
                    .iter()
                    .map(|row| row.iter().map(|cell| cell.ch).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n");
                // The answer, not the echoed command; typed before the shell drew its prompt,
                // it lands after the prompt on that line ("# got:typed").
                if seen
                    .lines()
                    .any(|line| line.trim_end().ends_with("got:typed") && !line.contains("echo"))
                {
                    assert_eq!(live.failed, None);
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        unreachable!(
            "no output from the shell; screen:\n{seen}\nfailed: {:?}",
            live.failed
        );
    }

    fn shell() -> Program {
        if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: Vec::new(),
            }
        }
    }

    /// Draws `live` once with `events` at `time`; returns the frame's zones.
    #[allow(clippy::unwrap_used)]
    fn frame(
        live: &mut LiveTerminal,
        state: &mut UiState,
        events: &[Event],
        time: f64,
    ) -> Vec<annulus_ui::kit::Zone> {
        let mut canvas = CpuCanvas::new(800, 400, 1.0, TextEngine::new()).unwrap();
        state.begin_frame(events, time);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            state,
            Rect::new(0.0, 0.0, 800.0, 400.0),
        );
        let palette = Palette::annulus_dark();
        live.draw(
            &mut ui,
            Rect::new(0.0, 0.0, 800.0, 400.0),
            (Font::mono(13.0), &palette),
            None,
            Id::new("live-test"),
        );
        state.end_frame().zones
    }

    fn click(zones: &[annulus_ui::kit::Zone], name: &str) -> Vec<Event> {
        let zone = zones
            .iter()
            .rev()
            .find(|zone| zone.label == name)
            .unwrap_or_else(|| panic!("no zone {name:?}"));
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(annulus_ui::kit::Button::Primary),
            Event::PointerUp(annulus_ui::kit::Button::Primary),
        ]
    }

    #[test]
    fn the_dot_shows_the_most_urgent_state() {
        let mut live = LiveTerminal::new(Program::system_ssh("web"));
        assert_eq!(live.status(), None);
        live.unread.store(true, Ordering::Relaxed);
        assert_eq!(live.status(), Some(TabStatus::Unread));
        live.ask = Ask::from_event(SessionEvent::Password {
            user: "me".into(),
            host: "web".into(),
        });
        assert_eq!(live.status(), Some(TabStatus::Input));
        live.reconnect.at = Some(1.0);
        assert_eq!(live.status(), Some(TabStatus::Error));
        assert!(TabStatus::Error > TabStatus::Input && TabStatus::Input > TabStatus::Unread);
    }

    #[test]
    fn a_background_tab_reconnects_when_its_time_comes() {
        let mut live = LiveTerminal::new(Program::Command {
            program: "annulus-test-no-such-program".into(),
            args: Vec::new(),
        });
        // It ran before, at this size.
        (live.grid, live.cell) = (Some((80, 24)), Some((8, 16)));
        live.reconnect.started = -100.0;
        live.dropped(0.0, None);
        assert_eq!(
            live.poll(0.5, None),
            Some(0.5),
            "the first try waits a second"
        );
        assert_eq!(live.poll(5.0, None), None);
        assert_eq!(live.reconnect.at, None);
        assert!(
            live.session.is_some() || live.failed.is_some(),
            "it tried again without being drawn"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn an_ssh_tab_says_it_is_connecting_until_it_draws() {
        let program = Program::Ssh {
            target: annulus_session::Target {
                host: "127.0.0.1".into(),
                port: 1,
                user: "nobody".into(),
            },
            files: annulus_session::SshFiles {
                known_hosts: std::env::temp_dir().join("annulus-native-connecting-known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: false,
            env: Vec::new(),
            startup: None,
            term: String::new(),
        };
        let mut live = LiveTerminal::new(program);
        assert!(!live.connecting(), "nothing started yet");
        live.spawn((80, 24), (8, 16), None, 0.0);
        assert!(live.session.is_some());
        assert!(live.connecting());
        assert_eq!(live.name(), "nobody@127.0.0.1");
        live.drawn.store(true, Ordering::Relaxed);
        assert!(!live.connecting(), "output arrived");
        live.drawn.store(false, Ordering::Relaxed);
        live.failed = Some("refused".into());
        assert!(!live.connecting(), "a failure shows instead");
    }

    #[test]
    fn the_connecting_card_follows_the_steps_and_waits_on_a_question() {
        let key = Ask::from_event(SessionEvent::HostKey(annulus_session::HostKeyQuestion {
            host: "web-1".into(),
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        }))
        .unwrap();
        let password = Ask::from_event(SessionEvent::Password {
            user: "deploy".into(),
            host: "web-1".into(),
        })
        .unwrap();
        // Reaching the server, then past Server and Key, then past Login, then all done.
        let steps: Vec<usize> = (0..4).map(|stage| active_step(stage, None)).collect();
        assert_eq!(steps, [0, 2, 3, 4]);
        // A question holds its own step, whatever came before.
        assert_eq!(active_step(0, Some(&key)), 1);
        assert_eq!(active_step(1, Some(&password)), 2);
    }

    #[test]
    fn a_reconnect_keeps_the_earlier_output_above_a_line() {
        let mut live = LiveTerminal::new(Program::system_ssh("web"));
        live.restart();
        assert_eq!(live.carry, None, "nothing was shown yet");
        live.reconnect.at = Some(3.0);
        live.restart();
        assert_eq!(live.reconnect.at, None, "a try clears the timer");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_paste_of_several_lines_waits_for_a_yes() {
        let mut live = LiveTerminal::new(shell());
        let mut state = UiState::default();
        let started = std::time::Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        frame(&mut live, &mut state, &[], clock());
        let mode = live.session.as_ref().unwrap().mode();
        if mode.contains(TermMode::BRACKETED_PASTE) {
            // This shell brackets pastes: nothing would run, so nothing is asked.
            return;
        }
        // An Enter typed in the same frame waits with the paste.
        let paste = [
            Event::Paste("echo one\necho two".into()),
            Event::Key(Key::Enter, Modifiers::NONE),
        ];
        let mut zones = frame(&mut live, &mut state, &paste, clock());
        zones = frame(&mut live, &mut state, &[], clock())
            .into_iter()
            .chain(zones)
            .collect();
        assert!(live.held_paste.is_some(), "the paste waits");
        assert!(live.sent.is_empty(), "nothing reached the shell yet");
        frame(&mut live, &mut state, &click(&zones, "Cancel"), clock());
        assert_eq!(live.held_paste, None);
        assert!(live.sent.is_empty(), "Cancel sends nothing");
        frame(&mut live, &mut state, &paste, clock());
        let zones = frame(&mut live, &mut state, &[], clock());
        frame(
            &mut live,
            &mut state,
            &click(&zones, "Paste as one line"),
            clock(),
        );
        assert_eq!(live.held_paste, None);
        assert_eq!(live.sent, [b"echo one echo two".to_vec()]);
    }

    #[test]
    fn two_terminals_with_the_same_cards_open_share_no_zone_id() {
        let mut state = UiState::default();
        let mut canvas = CpuCanvas::new(1000, 400, 1.0, TextEngine::new()).unwrap();
        state.begin_frame(&[], 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 1000.0, 400.0),
        );
        let palette = Palette::annulus_dark();
        for (index, x) in [0.0, 500.0].into_iter().enumerate() {
            let mut live = LiveTerminal::new(Program::system_ssh("web"));
            // Ended with a failure, the find bar open, a paste waiting: every card at once.
            live.failed = Some("could not reach web:22: refused".into());
            live.find = Some(FindBar::default());
            live.held_paste = Some("a\nb".into());
            live.draw(
                &mut ui,
                Rect::new(x, 0.0, 500.0, 400.0),
                (Font::mono(13.0), &palette),
                None,
                Id::new(("pane", index)),
            );
        }
        let zones = state.end_frame().zones;
        let mut ids = std::collections::HashSet::new();
        for zone in &zones {
            assert!(
                ids.insert(zone.id),
                "{:?} is in both panes under one id",
                zone.label
            );
        }
    }

    #[test]
    fn keys_text_and_pastes_become_the_bytes_a_program_expects() {
        let plain = TermMode::empty();
        assert_eq!(
            encode(&Typed::Text("ls".into()), plain),
            Some(b"ls".to_vec())
        );
        assert_eq!(
            encode(&Typed::Key(Key::Char('c'), Modifiers::CTRL), plain),
            Some(vec![0x03])
        );
        assert_eq!(
            encode(&Typed::Key(Key::Enter, Modifiers::NONE), plain),
            Some(b"\r".to_vec())
        );
        // Ctrl+Shift+letter is an app shortcut, never sent; plain letters come as text.
        assert_eq!(
            encode(&Typed::Key(Key::Char('t'), Modifiers::CTRL_SHIFT), plain),
            None
        );
        assert_eq!(
            encode(&Typed::Key(Key::Char('a'), Modifiers::NONE), plain),
            None
        );
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            encode(&Typed::Key(Key::Char('b'), alt), plain),
            Some(b"\x1bb".to_vec())
        );
        // A paste's line breaks become Enter, wrapped when the program asked for it.
        assert_eq!(
            encode(&Typed::Paste("a\nb".into()), plain),
            Some(b"a\rb".to_vec())
        );
        assert_eq!(
            encode(&Typed::Paste("a".into()), TermMode::BRACKETED_PASTE),
            Some(b"\x1b[200~a\x1b[201~".to_vec())
        );
        // A pasted end marker cannot close the bracket early and run what follows.
        assert_eq!(
            encode(
                &Typed::Paste("x\x1b[201~; reboot".into()),
                TermMode::BRACKETED_PASTE
            ),
            Some(b"\x1b[200~x; reboot\x1b[201~".to_vec())
        );
    }
}
