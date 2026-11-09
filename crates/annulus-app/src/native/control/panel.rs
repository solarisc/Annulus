//! The panel beside a terminal controlled through the automation door.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use annulus_config::tokens::space;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{FOCUS_REACH, Id, Role, Ui};
use annulus_ui::widgets::{BODY_MEDIUM, CONTROL, Kind, SMALL, Tone, button_width};

use super::{AgentCommand, AgentLog, PendingApproval, READ_MAX, Ran};
use crate::terminal_view::TabStatus;
/// How wide the panel beside an agent's terminal is, open and folded.
const PANEL_W: f32 = 320.0;
const RAIL_W: f32 = 44.0;

/// The width the panel beside an agent's terminal takes.
#[must_use]
pub fn panel_width(log: &AgentLog) -> f32 {
    if log.folded { RAIL_W } else { PANEL_W }
}

/// What the panel beside an agent's terminal asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Stop all agents: automation goes off and every agent is cut off.
    Stop,
    /// Let agents in again: automation goes back on.
    Allow,
    /// Show this row at the top of the terminal (a command was picked).
    Show(usize),
    /// Back to the newest output (the picked command was let go).
    Newest,
    /// Let the held command run.
    Approve(u64),
    /// Refuse the held command without typing it.
    Deny(u64),
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
    approval: Option<&PendingApproval>,
) -> Option<PanelAction> {
    let t = ui.theme;
    if approval.is_some() {
        log.folded = false;
    }
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
            "Let agents use Annulus again (Settings > Agents)",
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
    let mut list = Rect::new(rect.x + 1.0, column.y + 1.0, rect.w - 2.0, column.h - 1.0);
    if let Some(approval) = approval {
        let prompt = list.take_top(126.0);
        if let Some(choice) = approval_prompt(ui, prompt, approval) {
            action = Some(choice);
        }
        ui.gfx.fill_rect(
            Rect::new(list.x + space::M, list.y, list.w - space::M, 1.0),
            0.0,
            t.line,
        );
    }
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

/// The next held command, with an explicit choice before any bytes reach the terminal.
fn approval_prompt(ui: &mut Ui<'_>, rect: Rect, approval: &PendingApproval) -> Option<PanelAction> {
    let t = ui.theme;
    let mut inner = rect.inset(space::M, space::S);
    ui.gfx.text(
        inner.take_top(20.0),
        "Approve this command?",
        BODY_MEDIUM,
        t.text,
        Align::Start,
    );
    let char_w = ui.gfx.measure("M", MONO_SMALL).width.max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let per_line = (inner.w / char_w).floor().max(8.0) as usize;
    let command_rect = Rect::new(inner.x, inner.y, inner.w, COMMAND_LINE * 2.0);
    let command_id = Id::new(("agent.approval.command", approval.id));
    ui.zone(
        command_id,
        command_rect,
        Role::Backdrop,
        &format!("Command: {}", approval.command),
    );
    for line in wrap_command(&approval.command, per_line, 2) {
        ui.gfx.text(
            inner.take_top(COMMAND_LINE),
            &line,
            MONO_SMALL,
            t.text2,
            Align::Start,
        );
    }
    ui.tooltip(command_id.with("full"), command_rect, &approval.command);
    let mut buttons = inner.take_bottom(CONTROL);
    let approve_w = button_width(&mut ui.gfx, "Approve", Some(icons::CHECK));
    let deny_w = button_width(&mut ui.gfx, "Deny", Some(icons::CLOSE));
    let approve = buttons.take_right(approve_w);
    buttons.take_right(space::S);
    let deny = buttons.take_right(deny_w);
    if ui
        .button(
            Id::new(("agent.deny", approval.id)),
            deny,
            "Deny",
            Some(icons::CLOSE),
            Kind::Secondary,
        )
        .clicked
    {
        return Some(PanelAction::Deny(approval.id));
    }
    ui.button(
        Id::new(("agent.approve", approval.id)),
        approve,
        "Approve",
        Some(icons::CHECK),
        Kind::Primary,
    )
    .clicked
    .then_some(PanelAction::Approve(approval.id))
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
        // Inside the row: the list runs from one edge of the panel to the other, with no room
        // around it for a ring.
        if response.focus_ring {
            ui.focus_ring(row.inset(FOCUS_REACH, FOCUS_REACH), 4.0);
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
                // A command may hold a password: kept out of clipboard history, and cleared.
                ui.copy_secret(command.text.clone());
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

/// The mark at `center` for how a command ran: a check once done, a keyboard while it waits
/// with no prompt back (for input, maybe), a clock when its wait ran out, a cross when it was
/// interrupted, cut off or its shell ended, a turning arc while it runs.
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
        Ran::Waiting { .. } => icon(icons::KEYBOARD, t.warn, ui),
        Ran::TimedOut { .. } => icon(icons::CLOCK, t.warn, ui),
        Ran::Interrupted { .. } => icon(icons::CLOSE, t.warn, ui),
        Ran::Cut { .. } | Ran::Ended { .. } => icon(icons::CLOSE, t.text3, ui),
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
pub(super) fn session_said(log: &AgentLog) -> String {
    let count = log.commands.len();
    let going = log
        .commands
        .iter()
        .filter(|command| command.state.going())
        .count();
    let mut said = format!(
        "opened {} · {count} {}",
        crate::native::start::ago(log.since, crate::sync_screen::now_secs()),
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
pub(super) enum Ink {
    Quiet,
    Accent,
    Warn,
}

/// The line under a command and its color: when in the session it was typed, how long it
/// took (or has run so far) and how many lines it printed.
pub(super) fn describe_run(command: &AgentCommand, since: u64, now: Instant) -> (String, Ink) {
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
        Ran::Waiting { took: spent, lines } => (
            format!(
                "{at} · no prompt after {} · {}",
                took(*spent),
                lines_said(*lines)
            ),
            Ink::Warn,
        ),
        Ran::Ended { took: spent } => (
            format!("{at} · its shell ended after {}", took(*spent)),
            Ink::Quiet,
        ),
    }
}

/// `seconds` into the session as m:ss (h:mm:ss past an hour).
pub(super) fn clock(seconds: u64) -> String {
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

/// How many lines a command printed, in words; [`READ_MAX`] or more, as many as its answer
/// holds and more (its oldest lines were cut).
pub(super) fn lines_said(lines: usize) -> String {
    match lines {
        0 => "no output".to_owned(),
        1 => "1 line".to_owned(),
        n if n >= READ_MAX => format!("{}+ lines", grouped(READ_MAX)),
        n => format!("{} lines", grouped(n)),
    }
}

/// `n` with a comma between each three digits (4,998).
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `text` in lines of at most `per_line` characters, at most `most` of them, the last cut
/// short with an ellipsis when there is more. A command of several lines runs on, its lines
/// two spaces apart.
pub(super) fn wrap_command(text: &str, per_line: usize, most: usize) -> Vec<String> {
    let flat = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("  ");
    let chars: Vec<char> = flat.chars().collect();
    let per_line = per_line.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut rest: &[char] = &chars;
    while rest.len() > per_line {
        // At the last space that fits, so a word stays whole; a word too long for a line is
        // broken where the line ends.
        let cut = rest[..=per_line]
            .iter()
            .rposition(|c| *c == ' ')
            .filter(|at| *at > 0)
            .map_or(per_line, |at| at + 1);
        lines.push(rest[..cut].iter().collect::<String>().trim_end().to_owned());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        lines.push(rest.iter().collect());
    }
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
