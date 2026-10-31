//! The app's side of the control door (ADR-0020, F-AUTO): it is open while Settings →
//! Security → Automation is on and the vault is unlocked, and [`handle`] answers what comes
//! through it. Answers carry what a person sees in the window, never a secret.
//!
//! An agent opens tabs of its own, marked as its, and types only into those; the owner's
//! tabs are read-only to it. What it typed is listed beside its tab ([`AgentLog`]).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use annulus_config::tokens::space;
use annulus_control::door::{Door, Pending};
use annulus_control::protocol::{HostInfo, Request, Response, Status, TabInfo};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Role, Ui};
use annulus_ui::widgets::{BODY_MEDIUM, CONTROL, Kind, SMALL, Tone, button_width};
use annulus_vault::groups::Group;
use annulus_vault::hosts::Host;
use annulus_vault::vault::Vault;

use super::live::AgentMarks;
use super::{NativeApp, Tab, TabKind, TermView};
use crate::terminal_view::TabStatus;

/// What an agent typed into its tab, newest last, and how its panel stands (F-AUTO-004). In
/// memory only: it can hold secrets.
pub struct AgentLog {
    /// When the agent opened the tab (seconds since the Unix epoch).
    pub since: u64,
    pub commands: Vec<AgentCommand>,
    /// The command picked in the panel: the terminal shows it, its lines tinted.
    pub picked: Option<usize>,
    /// The panel is folded to a narrow rail.
    pub folded: bool,
    /// How many commands the list showed last frame: a new one scrolls it to the bottom.
    listed: usize,
}

impl AgentLog {
    /// The log of a tab an agent opened at `since`.
    #[must_use]
    pub fn new(since: u64) -> Self {
        Self {
            since,
            commands: Vec::new(),
            picked: None,
            folded: false,
            listed: 0,
        }
    }

    /// Where the agent typed, for the terminal's margin: each command's line, and the lines
    /// from the picked command's to the next one's.
    #[must_use]
    pub fn marks(&self) -> AgentMarks {
        let picked = self.picked.and_then(|index| {
            let from = self.commands.get(index)?.line;
            let to = self
                .commands
                .get(index + 1)
                .map_or(usize::MAX, |next| next.line);
            Some(from..to)
        });
        AgentMarks {
            lines: self.commands.iter().map(|command| command.line).collect(),
            picked,
        }
    }
}

/// One command an agent typed.
pub struct AgentCommand {
    pub text: String,
    /// When it was typed (seconds since the Unix epoch).
    pub at: u64,
    /// The transcript line it was typed on.
    pub line: usize,
    pub state: Ran,
}

/// How far a command got.
pub enum Ran {
    /// Typed at this instant; its output has not settled yet.
    Running(Instant),
    /// Its output settled after `took`, `lines` long.
    Done { took: Duration, lines: usize },
    /// Its wait ran out after `took` with output still coming, `lines` long so far: it may
    /// still be running.
    TimedOut { took: Duration, lines: usize },
    /// The agent pressed Ctrl+C on it after `took`.
    Interrupted { took: Duration },
    /// Stop (or the vault locking) cut the agent off after `took`, before it settled.
    Cut { took: Duration },
}

impl Ran {
    /// Typed and not known to have ended: running, or out of its wait and maybe still going.
    #[must_use]
    pub fn going(&self) -> bool {
        matches!(self, Self::Running(_) | Self::TimedOut { .. })
    }
}

/// An agent's command on its way: waiting for its terminal to start, then for its output to
/// settle.
pub struct Running {
    call: Pending,
    tab: u64,
    command: String,
    /// How many lines the terminal held before it was typed; `None` until it is.
    before: Option<usize>,
    asked: Instant,
    /// The longest to wait for output.
    wait: Duration,
    /// The length of the text last seen, and when it last changed.
    seen: (usize, Instant),
    /// The length of the text when the command was typed: quiet counts only after it grew.
    typed_at: usize,
    /// Its place in its tab's [`AgentLog`], once typed.
    entry: usize,
}

/// Output quiet this long counts as finished.
const QUIET: Duration = Duration::from_millis(1200);
/// How long the terminal's prompt must sit unchanged before a command is typed at it.
const PROMPT_QUIET: Duration = Duration::from_millis(300);
/// How long to wait for a prompt from a shell that prints none before typing anyway.
const SILENT_PROMPT: Duration = Duration::from_secs(10);
/// How long a new terminal may take to start before a command for it is given up.
const START_WITHIN: Duration = Duration::from_secs(30);
/// What `run` waits at most when the agent names nothing, and the most it may name.
const WAIT_DEFAULT: u64 = 30;
const WAIT_MAX: u64 = 300;

impl NativeApp {
    /// Opens the door when it should be open and closes it when it should not, answers every
    /// request that came through it, and moves agents' commands along. Returns whether a
    /// command is still waiting (the window should look again soon).
    pub(super) fn control(&mut self) -> bool {
        let wanted = self.settings.automation && self.vault.is_some() && self.blocked.is_none();
        if !wanted {
            self.door = None;
            self.door_failed = false;
            self.cut_runs(Instant::now());
            return false;
        }
        if self.door.is_none() && !self.door_failed {
            let dir = self.vault_path.as_ref().and_then(|path| path.parent());
            if let (Some(dir), Some(waker)) = (dir, self.waker.clone()) {
                self.door = Door::open(dir, waker).ok();
                // Tried once until the switch or the vault changes, not on every frame.
                self.door_failed = self.door.is_none();
            }
        }
        let calls = self.door.as_ref().map(Door::take_calls).unwrap_or_default();
        for call in calls {
            if let Request::Run { tab, command, wait } = &call.request {
                let (tab, command) = (*tab, command.clone());
                let wait = Duration::from_secs(wait.unwrap_or(WAIT_DEFAULT).min(WAIT_MAX));
                match agent_tab(self, tab) {
                    Ok(_) => self.runs.push(Running {
                        call,
                        tab,
                        command,
                        before: None,
                        asked: Instant::now(),
                        wait,
                        seen: (0, Instant::now()),
                        typed_at: 0,
                        entry: 0,
                    }),
                    Err(why) => call.answer(Err(why)),
                }
            } else {
                let answer = handle(self, &call.request);
                call.answer(answer);
            }
        }
        self.move_runs(Instant::now());
        !self.runs.is_empty()
    }

    /// Drops every command on its way, as the door closes: the ones already typed are marked
    /// cut off in their panels, so none shows as running forever.
    pub(super) fn cut_runs(&mut self, now: Instant) {
        for run in std::mem::take(&mut self.runs) {
            if run.before.is_none() {
                continue;
            }
            if let Some(command) = self
                .tabs
                .iter_mut()
                .find(|tab| tab.uid == run.tab)
                .and_then(|tab| tab.agent.as_mut())
                .and_then(|log| log.commands.get_mut(run.entry))
                .filter(|command| matches!(command.state, Ran::Running(_)))
            {
                command.state = Ran::Cut {
                    took: now.duration_since(run.asked),
                };
            }
        }
    }

    /// Types each waiting command once its terminal has started, and answers each typed one
    /// with its output once that has settled or its wait is up.
    fn move_runs(&mut self, now: Instant) {
        for mut run in std::mem::take(&mut self.runs) {
            let Ok(tab) = agent_tab(self, run.tab) else {
                run.call.answer(Err(format!("tab {} closed", run.tab)));
                continue;
            };
            let Some(TermView::Live(panes)) = &tab.terminal else {
                run.call
                    .answer(Err(format!("tab {} holds no terminal", run.tab)));
                continue;
            };
            let Some(before) = run.before else {
                if let Some(why) = panes.focused_failure() {
                    run.call
                        .answer(Err(format!("tab {} could not connect: {why}", run.tab)));
                    continue;
                }
                // Typing before the shell's first prompt has it echo the command early and
                // print its prompt into the answer, so wait for the prompt to settle.
                let shown = panes.focused_transcript().unwrap_or_default();
                if shown.len() != run.seen.0 {
                    run.seen = (shown.len(), now);
                }
                // A command that ran out of its wait may still be printing: the next one waits
                // for it to go quiet rather than land in the middle of its output.
                let busy = tab
                    .agent
                    .as_ref()
                    .and_then(|log| log.commands.last())
                    .is_some_and(|last| matches!(last.state, Ran::TimedOut { .. }));
                let ready = if shown.trim().is_empty() {
                    now.duration_since(run.asked) >= SILENT_PROMPT
                } else {
                    let quiet = if busy { QUIET } else { PROMPT_QUIET };
                    now.duration_since(run.seen.1) >= quiet
                };
                let row = panes.focused_cursor_row().unwrap_or(0);
                let mut typed = run.command.replace("\r\n", "\r").replace('\n', "\r");
                typed.push('\r');
                if ready && panes.type_focused(typed.as_bytes()) {
                    let text = panes.focused_transcript().unwrap_or_default();
                    let before = text.trim_end().lines().count();
                    run.before = Some(before);
                    run.seen = (text.len(), now);
                    run.typed_at = text.len();
                    run.asked = now;
                    let at = crate::sync_screen::now_secs();
                    let log = tab.agent.get_or_insert_with(|| Box::new(AgentLog::new(at)));
                    run.entry = log.commands.len();
                    log.commands.push(AgentCommand {
                        text: run.command.clone(),
                        at,
                        line: row,
                        state: Ran::Running(now),
                    });
                    self.runs.push(run);
                } else if now.duration_since(run.asked) > START_WITHIN {
                    let why = if busy {
                        format!(
                            "tab {} is still busy with the command before, which ran out of \
                             its wait: read it, or interrupt it",
                            run.tab
                        )
                    } else {
                        "the terminal never started; read it to see why".to_owned()
                    };
                    run.call.answer(Err(why));
                } else {
                    self.runs.push(run);
                }
                continue;
            };
            let text = panes.focused_transcript().unwrap_or_default();
            if text.len() != run.seen.0 {
                run.seen = (text.len(), now);
            }
            // A slow shell or command may say nothing for a while: silence before any output
            // is waiting, not done, so only the wait ends it.
            let settled = text.len() != run.typed_at && now.duration_since(run.seen.1) >= QUIET;
            if settled || now.duration_since(run.asked) >= run.wait {
                let out = new_output(&text, before);
                // Done when its last output came, not when the quiet after it ran out.
                let took = if settled {
                    run.seen.1.duration_since(run.asked)
                } else {
                    now.duration_since(run.asked)
                };
                if let Some(command) = tab
                    .agent
                    .as_mut()
                    .and_then(|log| log.commands.get_mut(run.entry))
                {
                    command.state = ran(&out, took, settled);
                }
                run.call.answer(Ok(Response::Text(out)));
            } else {
                self.runs.push(run);
            }
        }
    }
}

/// How wide the panel beside an agent's terminal is, open and folded.
const PANEL_W: f32 = 320.0;
const RAIL_W: f32 = 44.0;

/// The width the panel beside an agent's terminal takes.
#[must_use]
pub fn panel_width(log: &AgentLog) -> f32 {
    if log.folded { RAIL_W } else { PANEL_W }
}

/// What the panel beside an agent's terminal asks of the window.
#[derive(Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Stop all agents: automation goes off and every agent is cut off.
    Stop,
    /// Let agents in again: automation goes back on.
    Allow,
    /// Show this row at the top of the terminal (a command was picked).
    Show(usize),
    /// Back to the newest output (the picked command was let go).
    Newest,
}

/// Commands in the panel: monospace, a touch smaller than the terminal's.
const MONO_SMALL: annulus_ui::gfx::Font = annulus_ui::gfx::Font::mono(12.0);
/// One wrapped line of a command, and the line under it saying how it ran.
const COMMAND_LINE: f32 = 17.0;
const META_LINE: f32 = 18.0;
/// The most lines a command wraps to before it is cut short (all of it is in its tooltip).
const COMMAND_LINES: usize = 3;
/// Room on a row's left for its status mark.
const MARK_COLUMN: f32 = 22.0;

/// The panel beside an agent's terminal (ADR-0020 §6, F-AUTO-004). On top: the AI mark,
/// whether agents may act (Stop, or Allow again once stopped), fold, what the tab is
/// connected to, and since when. Then every command the agent typed, newest last and kept in
/// view, each with how it ran, when in the session, how long it took and how many lines it
/// printed; picking one shows it in the terminal. `rect` is the inside of the panel's card;
/// `tab` is the tab's title, its status dot and whether automation is on.
#[allow(clippy::too_many_lines)] // The panel top to bottom: head, list, foot.
pub fn agent_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    log: &mut AgentLog,
    (title, status, allowed): (&str, Option<TabStatus>, bool),
) -> Option<PanelAction> {
    let t = ui.theme;
    if log.folded {
        return rail(ui, rect, log, allowed);
    }
    let mut action = None;
    let mut column = rect.inset(space::M, space::M);

    let mut head = column.take_top(CONTROL);
    let fold = head.take_right(CONTROL);
    head.take_right(space::XS);
    let (label, icon, kind, tip, asks) = if allowed {
        (
            "Stop",
            icons::STOP,
            Kind::Danger,
            "Stop all agents: automation goes off and every agent is cut off now",
            PanelAction::Stop,
        )
    } else {
        (
            "Allow again",
            icons::PLAY,
            Kind::Secondary,
            "Let agents use Annulus again (Settings > Security > Automation)",
            PanelAction::Allow,
        )
    };
    let switch = head.take_right(button_width(&mut ui.gfx, label, Some(icon)));
    // The switch, then fold: the keyboard goes left to right.
    if ui
        .button(Id::new("agent.stop"), switch, label, Some(icon), kind)
        .clicked
    {
        action = Some(asks);
    }
    ui.tooltip(Id::new("agent.stop"), switch, tip);
    if ui
        .icon_button(
            Id::new("agent.fold"),
            fold,
            icons::CHEVRON_RIGHT,
            "Fold the agent panel",
            false,
        )
        .clicked
    {
        log.folded = true;
    }
    let chip = ui.chip(
        Point::new(head.x, head.center().y - 10.0),
        "AI",
        if allowed { t.accent } else { t.text3 },
    );
    head.take_left(chip + space::S);
    let heading = if allowed {
        "Agent session"
    } else {
        "Agents stopped"
    };
    ui.gfx
        .text(head, heading, BODY_MEDIUM, t.text, Align::Start);

    // What the tab is connected to, then how the session stands: two lines, so neither is
    // cut short.
    column.take_top(space::S);
    let mut host = column.take_top(18.0);
    let (dot, state) = match status {
        Some(TabStatus::Error) => (t.error, "dropped"),
        Some(TabStatus::Input) => (t.warn, "waiting for an answer"),
        _ => (t.ok, "connected"),
    };
    ui.gfx
        .fill_circle(Point::new(host.x + 4.0, host.center().y), 3.5, dot);
    host.take_left(14.0);
    let fitted = annulus_ui::gfx::fit(&mut ui.gfx, title, SMALL, host.w);
    ui.gfx.text(host, &fitted, SMALL, t.text, Align::Start);
    ui.tooltip(Id::new("agent.host"), host, &format!("{title}: {state}"));
    let mut about = column.take_top(18.0);
    about.take_left(14.0);
    let (said, color) = if allowed {
        (session_said(log), t.text3)
    } else {
        ("Agents are cut off from Annulus.".to_owned(), t.warn)
    };
    ui.gfx.text(about, &said, SMALL, color, Align::Start);

    let mut foot = column.take_bottom(18.0);
    ui.gfx.icon(
        icons::LOCK,
        Rect::new(foot.x, foot.center().y - 6.0, 12.0, 12.0),
        t.text3,
    );
    foot.take_left(18.0);
    ui.gfx.text(
        foot,
        "Types only in this tab. Yours are read-only to it.",
        SMALL,
        t.text3,
        Align::Start,
    );
    column.take_bottom(space::S);
    // Dividers and the list span the card inside its border.
    ui.gfx.fill_rect(
        Rect::new(rect.x + 1.0, column.b(), rect.w - 2.0, 1.0),
        0.0,
        t.line,
    );

    column.take_top(space::M);
    ui.gfx.fill_rect(
        Rect::new(rect.x + 1.0, column.y, rect.w - 2.0, 1.0),
        0.0,
        t.line,
    );
    let list = Rect::new(rect.x + 1.0, column.y + 1.0, rect.w - 2.0, column.h - 1.0);
    if log.commands.is_empty() {
        let mut empty = list.inset(space::M, space::M);
        ui.gfx.text(
            empty.take_top(20.0),
            "Nothing typed yet",
            BODY_MEDIUM,
            t.text2,
            Align::Start,
        );
        ui.gfx.text(
            empty.take_top(18.0),
            "Each command the agent runs shows here.",
            SMALL,
            t.text3,
            Align::Start,
        );
        return action;
    }
    if let Some(picked) = list_commands(ui, list, log) {
        action = Some(picked);
    }
    action
}

/// The commands of `log` in `list`, scrolled to the newest when one arrives. Returns what
/// picking one asks for.
#[allow(clippy::too_many_lines)] // Measure, scroll, then each row top to bottom.
fn list_commands(ui: &mut Ui<'_>, list: Rect, log: &mut AgentLog) -> Option<PanelAction> {
    let t = ui.theme;
    let char_w = ui.gfx.measure("M", MONO_SMALL).width.max(1.0);
    // Room on the right for Copy, which shows on the hovered and the picked row.
    let text_w = list.w - space::M * 2.0 - MARK_COLUMN - space::S - 24.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Characters on a line.
    let per_line = (text_w / char_w).floor().max(8.0) as usize;
    let rows: Vec<Vec<String>> = log
        .commands
        .iter()
        .map(|command| wrap_command(&command.text, per_line, COMMAND_LINES))
        .collect();
    let height = |lines: &Vec<String>| {
        #[allow(clippy::cast_precision_loss)] // A few lines.
        let text = lines.len() as f32 * COMMAND_LINE;
        space::S * 2.0 + text + META_LINE
    };
    let content: f32 = rows.iter().map(height).sum();
    let id = Id::new("agent.list");
    if log.commands.len() != log.listed {
        // A new command: show it.
        ui.set_scroll_offset(id, content);
        log.listed = log.commands.len();
    }
    let offset = ui.scroll_view(id, list, content);
    let now = Instant::now();
    let mut action = None;
    ui.push_clip(list);
    let mut y = list.y - offset;
    for (index, (command, lines)) in log.commands.iter().zip(&rows).enumerate() {
        let row = Rect::new(list.x, y, list.w - 8.0, height(lines));
        y += row.h;
        if row.b() < list.y || row.y > list.b() {
            continue;
        }
        let row_id = Id::new(("agent.command", index));
        let response = ui.zone(row_id, row, Role::Button, &command.text);
        let picked = log.picked == Some(index);
        if picked {
            ui.gfx.fill_rect(row, 0.0, t.selected);
            ui.gfx
                .fill_rect(Rect::new(row.x, row.y, 2.0, row.h), 0.0, t.accent);
        } else if response.hovered {
            ui.gfx.fill_rect(row, 0.0, t.hover);
        }
        if response.focus_ring {
            ui.focus_ring(row.inset(2.0, 2.0), 4.0);
        }
        ui.gfx.fill_rect(
            Rect::new(row.x + space::M, row.b() - 1.0, row.w - space::M, 1.0),
            0.0,
            t.line,
        );
        let mut inner = row.inset(space::M, space::S);
        let mark = inner.take_left(MARK_COLUMN);
        status_mark(ui, Point::new(mark.x + 7.0, mark.y + 8.5), &command.state);
        for (n, text) in lines.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // A few lines.
            let line = Rect::new(
                inner.x,
                inner.y + n as f32 * COMMAND_LINE,
                inner.w,
                COMMAND_LINE,
            );
            ui.gfx.text(line, text, MONO_SMALL, t.text, Align::Start);
        }
        let meta = Rect::new(inner.x, inner.b() - META_LINE, inner.w, META_LINE);
        let (said, ink) = describe_run(command, log.since, now);
        let color = match ink {
            Ink::Accent => t.accent,
            Ink::Warn => t.warn,
            Ink::Quiet => t.text3,
        };
        ui.gfx.text(meta, &said, SMALL, color, Align::Start);
        if response.hovered || picked {
            let copy = Rect::new(row.r() - space::S - 24.0, row.y + space::XS, 24.0, 24.0);
            ui.gfx.fill_rect(copy, 4.0, t.chrome);
            if ui
                .icon_button(
                    row_id.with("copy"),
                    copy,
                    icons::COPY,
                    "Copy the command",
                    false,
                )
                .clicked
            {
                ui.copy(command.text.clone());
                ui.toast("Command copied", Tone::Info);
                continue;
            }
        }
        if response.clicked {
            if picked {
                log.picked = None;
                action = Some(PanelAction::Newest);
            } else {
                log.picked = Some(index);
                action = Some(PanelAction::Show(command.line));
            }
        }
    }
    ui.pop_clip();
    action
}

/// The panel folded to a rail: unfold, the AI mark, how many commands so far and how the last
/// one ran (its tooltip says it in words), and Stop at the foot.
fn rail(ui: &mut Ui<'_>, rect: Rect, log: &mut AgentLog, allowed: bool) -> Option<PanelAction> {
    let t = ui.theme;
    let mut column = rect.inset(space::XS, space::S);
    let unfold = column.take_top(CONTROL).centered(CONTROL, CONTROL);
    if ui
        .icon_button(
            Id::new("agent.fold"),
            unfold,
            icons::CHEVRON_LEFT,
            "Open the agent panel",
            false,
        )
        .clicked
    {
        log.folded = false;
    }
    column.take_top(space::S);
    let chip_row = column.take_top(20.0);
    let chip_w = ui.gfx.measure("AI", SMALL).width + 12.0;
    ui.chip(
        Point::new(chip_row.center().x - chip_w / 2.0, chip_row.y),
        "AI",
        if allowed { t.accent } else { t.text3 },
    );
    column.take_top(space::S);
    // How many commands and how the last one ran; a click opens the panel on them.
    let tally = column.take_top(if log.commands.is_empty() { 22.0 } else { 42.0 });
    let said = if allowed {
        session_said(log)
    } else {
        "Agents are cut off from Annulus".to_owned()
    };
    let id = Id::new("agent.tally");
    let response = ui.zone(id, tally, Role::Button, &said);
    if response.hovered {
        ui.gfx.fill_rect(tally, 6.0, t.hover);
    }
    if response.focus_ring {
        ui.focus_ring(tally, 6.0);
    }
    ui.tooltip(id, tally, &said);
    ui.gfx.text(
        Rect::new(tally.x, tally.y + 2.0, tally.w, 18.0),
        &log.commands.len().to_string(),
        SMALL,
        t.text2,
        Align::Center,
    );
    if let Some(last) = log.commands.last() {
        status_mark(
            ui,
            Point::new(tally.center().x, tally.y + 30.0),
            &last.state,
        );
    }
    if response.clicked {
        log.folded = false;
    }
    let switch = column.take_bottom(CONTROL).centered(CONTROL, CONTROL);
    let (icon, name, asks) = if allowed {
        (icons::STOP, "Stop all agents", PanelAction::Stop)
    } else {
        (
            icons::PLAY,
            "Let agents use Annulus again",
            PanelAction::Allow,
        )
    };
    let clicked = ui
        .icon_button(Id::new("agent.stop"), switch, icon, name, false)
        .clicked;
    clicked.then_some(asks)
}

/// The mark at `center` for how a command ran: a check once done, a clock when its wait ran
/// out, a cross when it was interrupted or cut off, a turning arc while it runs.
fn status_mark(ui: &mut Ui<'_>, center: Point, state: &Ran) {
    let t = ui.theme;
    let icon = |icon, color, ui: &mut Ui<'_>| {
        ui.gfx.icon(
            icon,
            Rect::new(center.x - 7.0, center.y - 7.0, 14.0, 14.0),
            color,
        );
    };
    match state {
        Ran::Done { .. } => icon(icons::CHECK, t.ok, ui),
        Ran::TimedOut { .. } => icon(icons::CLOCK, t.warn, ui),
        Ran::Interrupted { .. } => icon(icons::CLOSE, t.warn, ui),
        Ran::Cut { .. } => icon(icons::CLOSE, t.text3, ui),
        Ran::Running(_) => {
            ui.gfx.stroke_circle(center, 6.0, 1.5, t.accent.alpha(0.25));
            #[allow(clippy::cast_possible_truncation)] // An angle for drawing.
            let turn = (ui.time() * 300.0 % 360.0) as f32;
            let arc: Vec<Point> =
                icons::Segment::arc_points(center.x, center.y, 6.0, turn, turn + 100.0)
                    .into_iter()
                    .map(|(x, y)| Point::new(x, y))
                    .collect();
            ui.gfx.polyline(&arc, false, 1.5, t.accent);
            ui.request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// How the session stands, in a few words: since when, how many commands, how many still
/// going.
fn session_said(log: &AgentLog) -> String {
    let count = log.commands.len();
    let going = log
        .commands
        .iter()
        .filter(|command| command.state.going())
        .count();
    let mut said = format!(
        "opened {} · {count} {}",
        super::start::ago(log.since, crate::sync_screen::now_secs()),
        if count == 1 { "command" } else { "commands" },
    );
    if going > 0 {
        let _ = write!(said, " · {going} running");
    }
    said
}

/// How the line under a command is colored: quiet once done, the accent while it runs, a
/// warning when its wait ran out.
#[derive(Debug, PartialEq, Eq)]
enum Ink {
    Quiet,
    Accent,
    Warn,
}

/// The line under a command and its color: when in the session it was typed, how long it
/// took (or has run so far) and how many lines it printed.
fn describe_run(command: &AgentCommand, since: u64, now: Instant) -> (String, Ink) {
    let at = clock(command.at.saturating_sub(since));
    match &command.state {
        Ran::Running(started) => (
            format!("{at} · running {}", took(now.duration_since(*started))),
            Ink::Accent,
        ),
        Ran::Done { took: spent, lines } => (
            format!("{at} · {} · {}", took(*spent), lines_said(*lines)),
            Ink::Quiet,
        ),
        Ran::TimedOut { took: spent, lines } => (
            format!(
                "{at} · still printing after {} · {}",
                took(*spent),
                lines_said(*lines)
            ),
            Ink::Warn,
        ),
        Ran::Interrupted { took: spent } => (
            format!("{at} · interrupted after {}", took(*spent)),
            Ink::Warn,
        ),
        Ran::Cut { took: spent } => (
            format!("{at} · cut off by Stop after {}", took(*spent)),
            Ink::Quiet,
        ),
    }
}

/// `seconds` into the session as m:ss (h:mm:ss past an hour).
fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("+{h}:{m:02}:{s:02}")
    } else {
        format!("+{m}:{s:02}")
    }
}

/// A command's run time, as short as it reads well: 0.4 s, 12 s, 2 min 5 s.
fn took(spent: Duration) -> String {
    let seconds = spent.as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else if seconds < 60.0 {
        format!("{} s", spent.as_secs())
    } else {
        format!("{} min {} s", spent.as_secs() / 60, spent.as_secs() % 60)
    }
}

fn lines_said(lines: usize) -> String {
    match lines {
        0 => "no output".to_owned(),
        1 => "1 line".to_owned(),
        n => format!("{n} lines"),
    }
}

/// `text` in lines of at most `per_line` characters, at most `most` of them, the last cut
/// short with an ellipsis when there is more. A command of several lines runs on, its lines
/// two spaces apart.
fn wrap_command(text: &str, per_line: usize, most: usize) -> Vec<String> {
    let flat = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("  ");
    let chars: Vec<char> = flat.chars().collect();
    let per_line = per_line.max(1);
    let mut lines: Vec<String> = chars
        .chunks(per_line)
        .map(|chunk| chunk.iter().collect())
        .collect();
    if lines.len() > most {
        lines.truncate(most);
        if let Some(last) = lines.last_mut() {
            let kept: String = last.chars().take(per_line.saturating_sub(1)).collect();
            *last = kept + "…";
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Agent tab `uid`, or why it is not one.
fn agent_tab(app: &mut NativeApp, uid: u64) -> Result<&mut Tab, String> {
    let tab = app
        .tabs
        .iter_mut()
        .find(|tab| tab.uid == uid)
        .ok_or_else(|| format!("no tab {uid}: see `annulus tabs`"))?;
    if tab.agent.is_none() {
        return Err(format!(
            "tab {uid} is not an agent's: open one with `annulus open` (yours are read-only)"
        ));
    }
    Ok(tab)
}

/// What a terminal printed after a command was typed into it, when it held `before` lines:
/// from the line the command was typed on. If the terminal was cleared or its scrollback ran
/// over meanwhile, its last screenful or so.
fn new_output(text: &str, before: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let from = if lines.len() >= before {
        before.saturating_sub(1)
    } else {
        lines.len().saturating_sub(60)
    };
    let mut out = lines[from..].join("\n");
    out.push('\n');
    out
}

/// How a command that answered with `out` after `took` ran: done when its output `settled`,
/// timed out otherwise. Its lines leave out the command's own line and, once done, the prompt
/// that came back after it.
fn ran(out: &str, took: Duration, settled: bool) -> Ran {
    let shown = out.trim_end().lines().count();
    if settled {
        Ran::Done {
            took,
            lines: shown.saturating_sub(2),
        }
    } else {
        Ran::TimedOut {
            took,
            lines: shown.saturating_sub(1),
        }
    }
}

/// The most lines one read returns: enough for any command's output, little enough that a
/// client asking again and again cannot stall the window.
const READ_MAX: usize = 5_000;

/// The answer to `request`.
///
/// # Errors
/// What a client is told when the request cannot be answered.
pub fn handle(app: &mut NativeApp, request: &Request) -> Result<Response, String> {
    let Some(vault) = app.vault.as_ref() else {
        return Err("the vault is locked".into());
    };
    match request {
        Request::Status => Ok(Response::Status(Status {
            version: env!("CARGO_PKG_VERSION").into(),
            unlocked: true,
            tabs: app.tabs.len(),
        })),
        Request::Hosts => Ok(Response::Hosts(hosts(vault))),
        Request::Tabs => Ok(Response::Tabs(
            app.tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| tab_info(tab, index == app.active))
                .collect(),
        )),
        Request::Open { host } => {
            match host {
                Some(id) => {
                    if !vault.hosts().iter().any(|host| host.id == *id) {
                        return Err(format!("no saved host {id}: see `annulus hosts`"));
                    }
                    app.act(super::hosts::HostsAction::Connect(*id));
                }
                None => app.open_local(),
            }
            let mut opened = None;
            for (title, program, origin) in std::mem::take(&mut app.opens) {
                opened = Some(app.open_terminal(title, program, origin));
            }
            let uid = opened.ok_or("nothing opened")?;
            let index = app.tabs.iter().position(|tab| tab.uid == uid).unwrap_or(0);
            let tab = &mut app.tabs[index];
            tab.agent = Some(Box::new(AgentLog::new(crate::sync_screen::now_secs())));
            Ok(Response::Tab(tab_info(tab, index == app.active)))
        }
        Request::Run { tab, .. } => {
            // Typed and answered over the next frames by `NativeApp::control`.
            agent_tab(app, *tab).map(|_| Response::Done)
        }
        Request::Interrupt { tab } => {
            let agent = agent_tab(app, *tab)?;
            let Some(TermView::Live(panes)) = &agent.terminal else {
                return Err(format!("tab {tab} holds no terminal"));
            };
            if !panes.type_focused(b"\x03") {
                return Err(format!("tab {tab} has not started yet"));
            }
            let at = Instant::now();
            if let Some(command) = agent
                .agent
                .as_mut()
                .and_then(|log| log.commands.iter_mut().rev().find(|c| c.state.going()))
            {
                let started =
                    Duration::from_secs(crate::sync_screen::now_secs().saturating_sub(command.at));
                let took = match command.state {
                    Ran::Running(since) => at.duration_since(since),
                    _ => started,
                };
                command.state = Ran::Interrupted { took };
            }
            Ok(Response::Done)
        }
        Request::Close { tab } => {
            agent_tab(app, *tab)?;
            if let Some(index) = app.tabs.iter().position(|open| open.uid == *tab) {
                app.close_tab(index);
            }
            Ok(Response::Done)
        }
        Request::Read { tab, lines } => {
            let found = app.tabs.iter_mut().find(|open| open.uid == *tab);
            let text = match found.and_then(|open| open.terminal.as_mut()) {
                Some(TermView::Live(panes)) => {
                    let mut all = panes.transcripts();
                    let mut text = if all.len() == 1 {
                        all.remove(0).1
                    } else {
                        all.iter()
                            .map(|(title, text)| format!("--- {title} ---\n{text}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    // What the tab shows in its own panel, not in the terminal: an agent
                    // reading a tab that could not connect learns why.
                    if let Some(why) = panes.focused_failure() {
                        let _ = writeln!(text, "[Annulus] {why}");
                    }
                    text
                }
                Some(TermView::Replay(emulator)) => {
                    annulus_term::transcript::transcript(emulator.term_mut())
                }
                None => return Err(format!("no terminal tab {tab}: see `annulus tabs`")),
            };
            let wanted = lines.unwrap_or(READ_MAX).min(READ_MAX);
            Ok(Response::Text(last_lines(&text, Some(wanted))))
        }
    }
}

/// `tab` as a client sees it; `active` when it is in front.
fn tab_info(tab: &Tab, active: bool) -> TabInfo {
    TabInfo {
        id: tab.uid,
        title: tab.title.clone(),
        kind: kind_name(tab.kind).into(),
        status: tab.status.map_or("", status_name).into(),
        active,
        agent: tab.agent.is_some(),
    }
}

/// The saved hosts as the list shows them: what they inherit filled in, never a credential.
fn hosts(vault: &Vault) -> Vec<HostInfo> {
    let groups = vault.groups();
    vault
        .hosts()
        .iter()
        .map(|host: &Host| {
            let (resolved, _) = annulus_vault::groups::resolve(host, groups);
            HostInfo {
                id: host.id,
                label: host.label.clone(),
                address: host.address.clone(),
                port: resolved.port.unwrap_or(22),
                user: resolved.username.trim().to_owned(),
                group: group_path(groups, host.group.as_deref()),
                os: host.os.clone(),
            }
        })
        .collect()
}

/// The names of group `uid` and the groups above it, top level first ("Prod / Web").
fn group_path(groups: &[Group], uid: Option<&str>) -> String {
    let mut names = Vec::new();
    let mut at = uid;
    // A loop in the parents (a broken vault) stops at a depth no real tree reaches.
    while let Some(uid) = at
        && names.len() < 64
    {
        let Some(group) = groups.iter().find(|group| group.uid == uid) else {
            break;
        };
        names.push(group.name.as_str());
        at = group.parent.as_deref();
    }
    names.reverse();
    names.join(" / ")
}

/// The last `lines` lines of `text` without its trailing blank ones, or all of them.
fn last_lines(text: &str, lines: Option<usize>) -> String {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let from = lines.map_or(0, |lines| all.len().saturating_sub(lines));
    let mut out = all[from..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn kind_name(kind: TabKind) -> &'static str {
    match kind {
        TabKind::Vault => "vault",
        TabKind::Files => "files",
        TabKind::Terminal => "terminal",
        TabKind::Sftp => "sftp",
        TabKind::NewTab => "new_tab",
    }
}

fn status_name(status: TabStatus) -> &'static str {
    match status {
        TabStatus::Unread => "unread",
        TabStatus::Input => "input",
        TabStatus::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::demo::fixture_vault;
    use annulus_control::protocol::{self, Outcome};
    use annulus_vault::hosts::Secret;

    fn replay(app: &mut NativeApp, title: &str, output: &str) {
        let mut emulator = annulus_term::Emulator::new(80, 24);
        emulator.advance(output.as_bytes());
        app.push_terminal(title, TermView::Replay(Box::new(emulator)));
    }

    /// Every request, answered and written as it would go through the door.
    fn every_answer(app: &mut NativeApp) -> String {
        let tabs: Vec<u64> = app.tabs.iter().map(|tab| tab.uid).collect();
        let mut requests = vec![Request::Status, Request::Hosts, Request::Tabs];
        requests.extend(
            tabs.into_iter()
                .map(|tab| Request::Read { tab, lines: None }),
        );
        requests
            .iter()
            .map(|request| protocol::encode(&Outcome::of(handle(app, request))))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_password_proxy_login_or_key_of_the_vault_is_in_any_answer() {
        let mut vault = fixture_vault("control-secrets", true);
        let mut host = vault.hosts()[0].clone();
        host.password = Some(Secret::new("hunter2-password"));
        host.proxy = Some(Secret::new("socks5://bob:proxy-pass-77@10.9.9.9:1080"));
        vault.upsert_host(host).unwrap();
        let mut app = NativeApp::new(Some(vault));
        replay(&mut app, "web-1", "deploy@web-1:~$ uptime\r\n");
        let written = every_answer(&mut app);
        assert!(
            written.contains("prod-db"),
            "the hosts are there: {written}"
        );
        for secret in ["hunter2", "proxy-pass-77", "bob:", "10.9.9.9"] {
            assert!(!written.contains(secret), "{secret} leaked: {written}");
        }
    }

    #[test]
    fn hosts_tabs_and_terminal_text_are_answered_as_the_window_shows_them() {
        let mut app = NativeApp::new(Some(fixture_vault("control-answers", true)));
        replay(&mut app, "web-1", "one\r\ntwo\r\nthree\r\n");
        let Ok(Response::Hosts(hosts)) = handle(&mut app, &Request::Hosts) else {
            panic!("hosts");
        };
        let host = |label: &str| hosts.iter().find(|host| host.label == label).unwrap();
        let db = host("prod-db");
        assert_eq!(
            (
                db.address.as_str(),
                db.port,
                db.user.as_str(),
                db.group.as_str(),
                db.os.as_str()
            ),
            ("10.0.4.12", 22, "postgres", "", "debian")
        );
        assert_eq!(host("bastion").port, 2222);
        assert_eq!(host("worker-1").group, "Production");
        let Ok(Response::Tabs(tabs)) = handle(&mut app, &Request::Tabs) else {
            panic!("tabs");
        };
        let kinds: Vec<(&str, &str, bool)> = tabs
            .iter()
            .map(|tab| (tab.title.as_str(), tab.kind.as_str(), tab.active))
            .collect();
        assert_eq!(
            kinds,
            [
                ("Vault", "vault", false),
                ("SFTP", "files", false),
                ("web-1", "terminal", true)
            ]
        );
        let web = tabs[2].id;
        assert_eq!(
            handle(
                &mut app,
                &Request::Read {
                    tab: web,
                    lines: Some(2)
                }
            ),
            Ok(Response::Text("two\nthree\n".into()))
        );
        let vault_tab = tabs[0].id;
        assert!(
            handle(
                &mut app,
                &Request::Read {
                    tab: vault_tab,
                    lines: None
                }
            )
            .is_err()
        );
        assert!(
            handle(
                &mut app,
                &Request::Read {
                    tab: 999,
                    lines: None
                }
            )
            .is_err()
        );
        app.lock_vault();
        assert_eq!(
            handle(&mut app, &Request::Hosts),
            Err("the vault is locked".into())
        );
    }

    #[test]
    fn an_agent_opens_its_own_tab_and_may_type_into_and_close_only_that() {
        let mut app = NativeApp::new(Some(fixture_vault("control-agent", true)));
        replay(&mut app, "mine", "owner@box:~$\r\n");
        let mine = app.tabs[app.active].uid;
        let Ok(Response::Tab(opened)) = handle(&mut app, &Request::Open { host: None }) else {
            panic!("a local tab opens");
        };
        assert!(opened.agent && opened.active, "{opened:?}");
        assert_eq!(
            app.tabs[app.active].uid, opened.id,
            "in front, to be watched"
        );
        let web = app
            .vault
            .as_ref()
            .unwrap()
            .hosts()
            .iter()
            .find(|host| host.label == "web-1")
            .unwrap()
            .id;
        let Ok(Response::Tab(host_tab)) = handle(&mut app, &Request::Open { host: Some(web) })
        else {
            panic!("a host tab opens");
        };
        assert_eq!((host_tab.title.as_str(), host_tab.agent), ("web-1", true));
        assert!(handle(&mut app, &Request::Open { host: Some(999) }).is_err());
        let run = |tab| Request::Run {
            tab,
            command: "rm -rf /tmp/x".into(),
            wait: None,
        };
        let refused = handle(&mut app, &run(mine)).unwrap_err();
        assert!(refused.contains("read-only"), "{refused}");
        assert!(handle(&mut app, &Request::Close { tab: mine }).is_err());
        assert!(
            app.tabs.iter().any(|tab| tab.uid == mine),
            "the owner's tab stays"
        );
        assert_eq!(
            handle(&mut app, &Request::Close { tab: opened.id }),
            Ok(Response::Done)
        );
        assert!(!app.tabs.iter().any(|tab| tab.uid == opened.id));
    }

    #[test]
    fn a_commands_output_starts_at_the_line_it_was_typed_on() {
        let before = "old line\nuser@box:~$";
        let after = "old line\nuser@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n";
        assert_eq!(
            new_output(after, before.lines().count()),
            "user@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n"
        );
        // Cleared meanwhile (fewer lines than before): its last lines instead.
        assert_eq!(new_output("fresh\n", 40), "fresh\n");
    }

    #[test]
    fn a_finished_command_counts_its_output_and_a_timed_out_one_keeps_its_lines_so_far() {
        let out = "user@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n";
        let took = Duration::from_millis(400);
        assert!(matches!(ran(out, took, true), Ran::Done { lines: 1, .. }));
        let going = "user@box:~$ tail -f log\none\ntwo\n";
        assert!(matches!(
            ran(going, took, false),
            Ran::TimedOut { lines: 2, .. }
        ));
        assert!(matches!(
            ran("user@box:~$ true\nuser@box:~$\n", took, true),
            Ran::Done { lines: 0, .. }
        ));
    }

    #[test]
    fn each_command_says_when_in_the_session_how_long_and_how_much() {
        let command = |state| AgentCommand {
            text: "uptime".into(),
            at: 1_000 + 84,
            line: 0,
            state,
        };
        let done = command(Ran::Done {
            took: Duration::from_millis(400),
            lines: 1,
        });
        let now = Instant::now();
        assert_eq!(
            describe_run(&done, 1_000, now),
            ("+1:24 · 0.4 s · 1 line".to_owned(), Ink::Quiet)
        );
        let late = command(Ran::TimedOut {
            took: Duration::from_secs(125),
            lines: 48,
        });
        assert_eq!(
            describe_run(&late, 1_000, now),
            (
                "+1:24 · still printing after 2 min 5 s · 48 lines".to_owned(),
                Ink::Warn
            )
        );
        let running = command(Ran::Running(now));
        let (said, ink) = describe_run(&running, 1_000, now);
        assert!(said.starts_with("+1:24 · running "), "{said}");
        assert_eq!(ink, Ink::Accent);
        assert_eq!(clock(3_725), "+1:02:05");
        assert_eq!(lines_said(0), "no output");
        let stopped = command(Ran::Interrupted {
            took: Duration::from_secs(12),
        });
        assert_eq!(
            describe_run(&stopped, 1_000, now),
            ("+1:24 · interrupted after 12 s".to_owned(), Ink::Warn)
        );
        let cut = command(Ran::Cut {
            took: Duration::from_millis(2_500),
        });
        assert_eq!(
            describe_run(&cut, 1_000, now).0,
            "+1:24 · cut off by Stop after 2.5 s"
        );

        let mut log = AgentLog::new(crate::sync_screen::now_secs());
        log.commands.push(done);
        log.commands.push(running);
        assert_eq!(
            session_said(&log),
            "opened just now · 2 commands · 1 running"
        );
    }

    #[test]
    fn a_long_command_wraps_to_three_lines_and_the_rest_is_cut_short() {
        assert_eq!(wrap_command("ls -la", 10, 3), vec!["ls -la"]);
        assert_eq!(wrap_command("abcdefghij", 4, 3), vec!["abcd", "efgh", "ij"]);
        assert_eq!(
            wrap_command("abcdefghijklmnop", 4, 3),
            vec!["abcd", "efgh", "ijk…"]
        );
        assert_eq!(
            wrap_command("cd /srv\nmake\n", 40, 3),
            vec!["cd /srv  make"],
            "the lines of one command run on"
        );
    }

    #[test]
    fn the_margin_marks_every_command_and_tints_the_picked_one_up_to_the_next() {
        let mut log = AgentLog::new(0);
        for line in [3, 9, 20] {
            log.commands.push(AgentCommand {
                text: "true".into(),
                at: 0,
                line,
                state: Ran::Running(Instant::now()),
            });
        }
        assert_eq!(log.marks().lines, vec![3, 9, 20]);
        assert_eq!(log.marks().picked, None);
        log.picked = Some(1);
        assert_eq!(log.marks().picked, Some(9..20));
        log.picked = Some(2);
        assert_eq!(
            log.marks().picked,
            Some(20..usize::MAX),
            "the newest runs on"
        );
    }

    #[test]
    fn the_door_is_open_only_while_switched_on_and_unlocked() {
        let dir = std::env::temp_dir().join(format!("annulus-door-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = annulus_control::door_file(&dir);
        let mut app = NativeApp::new(Some(fixture_vault("control-door", false)));
        app.vault_path = Some(dir.join("vault.anv"));
        app.waker = Some(std::sync::Arc::new(|| {}));
        app.control();
        assert!(!file.exists(), "off by default");
        app.settings.automation = true;
        app.control();
        assert!(file.exists(), "switched on and unlocked, it opens");
        app.lock_vault();
        assert!(!file.exists(), "locking closes it at once");
        app.control();
        assert!(!file.exists(), "and it stays closed while locked");
        app.vault = Some(fixture_vault("control-door-again", false));
        app.control();
        assert!(file.exists(), "unlocked again, it opens again");
        app.settings.automation = false;
        app.control();
        assert!(!file.exists(), "switched off, it closes");
    }
}
