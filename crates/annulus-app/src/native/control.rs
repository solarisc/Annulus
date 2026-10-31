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
use annulus_control::protocol::{
    HostInfo, Request, Response, RunOutput, RunState, Status, TabInfo,
};
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

    /// Settles the commands before `entry` still going, now that the one at `entry` has
    /// `ending`: all of them when its shell ended, and when it is done, those whose own prompt
    /// is back (the `ssh` an `exit` came back from, the `sudo` a password went to). `text` is
    /// the terminal's text now.
    fn settle_before(&mut self, entry: usize, text: &str, ending: Ending) {
        let now = crate::sync_screen::now_secs();
        let last = text.trim_end().lines().next_back().unwrap_or_default();
        for command in self.commands.iter_mut().take(entry) {
            if !command.state.going() {
                continue;
            }
            let took = Duration::from_secs(now.saturating_sub(command.at));
            match ending {
                Ending::Ended => command.state = Ran::Ended { took },
                Ending::Done if prompt_back(last, &command.typed.prompt) => {
                    command.state = ran(&output_of(text, &command.typed), took, Ending::Done);
                }
                Ending::Done | Ending::Waiting | Ending::TimedOut => {}
            }
        }
    }
}

/// One command an agent typed.
pub struct AgentCommand {
    pub text: String,
    /// When it was typed (seconds since the Unix epoch).
    pub at: u64,
    /// The row it was typed on.
    pub line: usize,
    pub state: Ran,
    /// What the terminal showed as it was typed, to find its output again.
    pub typed: Typed,
}

/// How far a command got.
pub enum Ran {
    /// Typed at this instant; its output has not settled yet.
    Running(Instant),
    /// Its prompt came back after `took` (to its last output), `lines` printed.
    Done { took: Duration, lines: usize },
    /// Its output stopped after `took`, `lines` long, with no prompt back: it may be asking
    /// for input or still working.
    Waiting { took: Duration, lines: usize },
    /// Its wait ran out after `took` with output still coming, `lines` long so far: it may
    /// still be running.
    TimedOut { took: Duration, lines: usize },
    /// The agent pressed Ctrl+C on it after `took`.
    Interrupted { took: Duration },
    /// Stop (or the vault locking) cut the agent off after `took`, before it settled.
    Cut { took: Duration },
    /// Its shell exited or its connection ended after `took`.
    Ended { took: Duration },
}

impl Ran {
    /// Typed and not known to have ended: running, waiting with no prompt back, or out of
    /// its wait and maybe still going.
    #[must_use]
    pub fn going(&self) -> bool {
        matches!(
            self,
            Self::Running(_) | Self::Waiting { .. } | Self::TimedOut { .. }
        )
    }

    /// How far it got, as an agent hears it.
    fn state(&self) -> RunState {
        match self {
            Self::Done { .. } => RunState::Done,
            Self::Waiting { .. } => RunState::Waiting,
            Self::Running(_) | Self::TimedOut { .. } => RunState::Running,
            Self::Interrupted { .. } => RunState::Interrupted,
            Self::Cut { .. } | Self::Ended { .. } => RunState::Ended,
        }
    }

    /// How long it ran: so far while it runs.
    fn took(&self, now: Instant) -> Duration {
        match self {
            Self::Running(since) => now.duration_since(*since),
            Self::Done { took, .. }
            | Self::Waiting { took, .. }
            | Self::TimedOut { took, .. }
            | Self::Interrupted { took }
            | Self::Cut { took }
            | Self::Ended { took } => *took,
        }
    }
}

/// What a terminal showed just before a command was typed into it: enough to find the
/// command's line again however much output follows, even once the scrollback runs over and
/// drops lines from its top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    /// How many lines the terminal held.
    lines: usize,
    /// The line the command went on, as it stood.
    line: String,
    /// Up to two lines above it.
    above: Vec<String>,
    /// The shell prompt that comes back once it is done: that line, unless the command
    /// answered a question (a password, a yes or no) of the one before.
    prompt: String,
    /// Other prompts its tab showed before: one of them back ends it too (the local shell's,
    /// after `exit` from an `ssh`).
    known: Vec<String>,
}

impl Typed {
    /// What `text`, a terminal's text just before a command is typed, says of where it goes.
    fn of(text: &str) -> Self {
        let lines: Vec<&str> = text.trim_end().lines().collect();
        let last = lines.len().saturating_sub(1);
        let line = lines.last().map_or("", |line| line.trim_end()).to_owned();
        Self {
            lines: lines.len(),
            above: lines[last.saturating_sub(2)..last]
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
            prompt: line.clone(),
            line,
            known: Vec::new(),
        }
    }

    /// The index in `lines` of the line the command was typed on: where it was, or higher
    /// up once lines dropped off the top. `None` once it is gone (cleared, or more output
    /// than the scrollback holds).
    fn find(&self, lines: &[&str]) -> Option<usize> {
        let Some(expected) = self.lines.checked_sub(1) else {
            return (!lines.is_empty()).then_some(0);
        };
        // The lines above it that are still there: near the top, some have dropped off.
        let above_at = |at: usize| {
            let kept = self.above.len().min(at);
            lines[at - kept..at] == self.above[self.above.len() - kept..]
        };
        let last = lines.len().checked_sub(1)?;
        (0..=expected.min(last))
            .rev()
            .find(|&at| lines[at].starts_with(self.line.as_str()) && above_at(at))
            // A prompt redrawn as Enter was pressed (a transient prompt) no longer matches:
            // nothing moved, so the line is where it was.
            .or_else(|| (expected <= last && above_at(expected)).then_some(expected))
    }

    /// Typed after `before` (`busy`: while it still went on), in a tab whose commands were
    /// typed at `earlier`, newest last. Its line may be that one's prompt behind output with
    /// no newline (`okuser@box:~$`): it ends at the prompt itself. Typed at a question of a
    /// command still going (a password, a yes or no), it ends when that one's prompt comes
    /// back; at a prompt of its own (the remote shell `ssh` opened, `sudo -i`, a REPL), when
    /// that prompt does.
    fn following(&mut self, before: &Typed, busy: bool, earlier: &[AgentCommand]) {
        let behind_output =
            before.prompt.chars().count() > 2 && self.line.ends_with(before.prompt.as_str());
        if behind_output || (busy && !looks_like_prompt(&self.line)) {
            self.prompt.clone_from(&before.prompt);
        }
        // The tab's other prompts, newest first and a few at most.
        for command in earlier.iter().rev() {
            let prompt = &command.typed.prompt;
            if self.known.len() < 4
                && !prompt.is_empty()
                && *prompt != self.prompt
                && !self.known.contains(prompt)
            {
                self.known.push(prompt.clone());
            }
        }
    }

    /// Whether `line`, the terminal's last line, is its prompt or one its tab showed before.
    fn prompt_back(&self, line: &str) -> bool {
        prompt_back(line, &self.prompt) || self.known.iter().any(|known| prompt_back(line, known))
    }
}

/// Whether `line` is shaped like a shell prompt: it ends with a prompt's character or starts
/// with a prompt's mark. A question (`Password:`, `[Y/n]`) is not.
fn looks_like_prompt(line: &str) -> bool {
    let line = line.trim_end();
    line.chars()
        .last()
        .is_some_and(|end| PROMPT_ENDS.contains(&end))
        || line
            .chars()
            .next()
            .is_some_and(|mark| PROMPT_MARKS.contains(&mark))
}

/// What followed a command's line, worked out once its output paused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct After {
    /// How many lines came after the command's own.
    lines: usize,
    /// The last of them is a shell prompt like the one it was typed at.
    prompt: bool,
}

/// What followed the command `typed` in `text`, the terminal's text now.
fn after(text: &str, typed: &Typed) -> After {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let count = match typed.find(&lines) {
        Some(at) => lines.len() - at - 1,
        None => lines.len(),
    };
    After {
        lines: count,
        prompt: count > 0 && lines.last().is_some_and(|last| typed.prompt_back(last)),
    }
}

/// Characters a shell prompt ends with: `$` and `#` (sh, bash), `%` (zsh), `>` (fish, cmd,
/// PowerShell), `❯` and `»` (starship and its kind).
const PROMPT_ENDS: [char; 6] = ['$', '#', '%', '>', '❯', '»'];
/// Marks some prompts start with instead (oh-my-zsh's arrow, starship's chevron, lambda).
const PROMPT_MARKS: [char; 3] = ['➜', '❯', 'λ'];

/// Whether `line`, the terminal's last line, is a shell prompt like `prompt`, the one the
/// command was typed at: the same (also after output with no newline, `okuser@box:~$`), or
/// the same kind of prompt in another folder (`~$`, then `/tmp$`). A question (`[Y/n]`,
/// `password:`) or another user's shell is not.
fn prompt_back(line: &str, prompt: &str) -> bool {
    let line = line.trim_end();
    if prompt.is_empty() || line.is_empty() {
        return false;
    }
    // A one- or two-character prompt (`$`) is too short to tell from output it ends.
    if line == prompt || (prompt.chars().count() > 2 && line.ends_with(prompt)) {
        return true;
    }
    let shared = line
        .chars()
        .zip(prompt.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let ends_alike = prompt
        .chars()
        .last()
        .is_some_and(|end| PROMPT_ENDS.contains(&end) && line.ends_with(end));
    let marked = prompt
        .chars()
        .next()
        .is_some_and(|mark| PROMPT_MARKS.contains(&mark) && line.starts_with(mark));
    (shared >= 2 && ends_alike) || marked
}

/// How a command settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    /// Its prompt is back.
    Done,
    /// Its output stopped with no prompt back.
    Waiting,
}

/// Whether a command whose lines after it are `after` has settled, its output quiet for
/// `quiet`. `fresh`: output came since it was typed or last waited on. A command with
/// nothing past its own line (a silent `sleep`) never settles by quiet alone.
fn settled(after: After, quiet: Duration, fresh: bool) -> Option<Settled> {
    if after.prompt && quiet >= PROMPT_QUIET {
        Some(Settled::Done)
    } else if fresh && after.lines > 0 && quiet >= QUIET {
        Some(Settled::Waiting)
    } else {
        None
    }
}

/// What a command printed, from the line it was typed on, as found in `text`: the last
/// [`READ_MAX`] lines. Once its line is gone, everything the terminal holds is newer.
fn output_of(text: &str, typed: &Typed) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let from = typed
        .find(&lines)
        .unwrap_or(0)
        .max(lines.len().saturating_sub(READ_MAX));
    let mut out = lines[from..].join("\n");
    out.push('\n');
    out
}

/// How a command was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Done,
    Waiting,
    TimedOut,
    Ended,
}

/// An agent's command on its way: waiting for its turn and its terminal, then for its output
/// to settle. Answered while the command goes on (waiting, or out of its wait), it is still
/// watched with no call, so its row turns done once its prompt comes back; `wait` takes it
/// up again.
pub struct Running {
    call: Option<Pending>,
    tab: u64,
    command: String,
    /// The longest to wait for output.
    wait: Duration,
    /// When it was asked for; once typed, when it was typed.
    asked: Instant,
    /// When to answer at the latest, once typed (or waited on).
    until: Instant,
    /// What the terminal showed as it was typed; `None` until it is.
    typed: Option<Typed>,
    /// Its place in its tab's [`AgentLog`], once typed.
    entry: usize,
    /// The terminal's output count when last looked at, and when it last went up.
    output: (u64, Instant),
    /// The output count as it was typed or last waited on.
    base: u64,
    /// What followed it when its output last paused, and whether output came since.
    after: After,
    stale: bool,
}

impl Running {
    fn new(call: Pending, tab: u64, command: String, wait: Duration, now: Instant) -> Self {
        Self {
            call: Some(call),
            tab,
            command,
            wait,
            asked: now,
            until: now + wait,
            typed: None,
            entry: 0,
            output: (u64::MAX, now),
            base: 0,
            after: After::default(),
            stale: true,
        }
    }

    /// Answers its call, if one waits.
    fn answer(&mut self, answer: Result<Response, String>) {
        if let Some(call) = self.call.take() {
            call.answer(answer);
        }
    }
}

/// Output quiet this long with no prompt back answers a command as waiting.
const QUIET: Duration = Duration::from_millis(1200);
/// How long a prompt must sit unchanged: before a command is typed at it, and once it is
/// back after one, before the command counts as done.
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
    /// request that came through it, and moves agents' commands along. Returns whether to look
    /// again soon: a command waits for its answer, or one just printed.
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
        let now = Instant::now();
        let calls = self.door.as_ref().map(Door::take_calls).unwrap_or_default();
        for call in calls {
            match &call.request {
                Request::Run { tab, command, wait } => {
                    let (tab, command, wait) = (*tab, command.clone(), waited(*wait));
                    match agent_tab(self, tab) {
                        Ok(_) => self.runs.push(Running::new(call, tab, command, wait, now)),
                        Err(why) => call.answer(Err(why)),
                    }
                }
                Request::Wait { tab, wait } => {
                    let (tab, wait) = (*tab, waited(*wait));
                    self.wait_for(call, tab, wait, now);
                }
                _ => {
                    let answer = handle(self, &call.request);
                    call.answer(answer);
                }
            }
        }
        self.move_runs(now);
        if self.settings.close_ended_agent_tabs {
            self.close_ended_agent_tabs();
        }
        self.runs
            .iter()
            .any(|run| run.call.is_some() || now.duration_since(run.output.1) < QUIET * 2)
    }

    /// Closes agents' tabs whose shells have all ended, once no answer waits on them
    /// (Settings > Security > Automation, F-AUTO-006).
    fn close_ended_agent_tabs(&mut self) {
        let ended: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| {
                tab.agent.is_some()
                    && matches!(&tab.terminal, Some(TermView::Live(panes)) if panes.all_ended())
                    && !self
                        .runs
                        .iter()
                        .any(|run| run.tab == tab.uid && run.call.is_some())
            })
            .map(|(index, _)| index)
            .collect();
        for index in ended.into_iter().rev() {
            self.close_tab(index);
        }
    }

    /// Takes up a `wait` for the last command in agent tab `uid`: answered once it settles,
    /// or at once if it has.
    fn wait_for(&mut self, call: Pending, uid: u64, wait: Duration, now: Instant) {
        if let Err(why) = agent_tab(self, uid) {
            call.answer(Err(why));
            return;
        }
        if self
            .runs
            .iter()
            .any(|run| run.tab == uid && run.call.is_some())
        {
            call.answer(Err(format!(
                "tab {uid} already has a command waiting for its answer"
            )));
            return;
        }
        if let Some(run) = self.runs.iter_mut().find(|run| run.tab == uid) {
            // Still watched: only output from now on counts as a new pause.
            run.call = Some(call);
            run.until = now + wait;
            run.base = run.output.0;
            return;
        }
        let answer = last_answer(self, uid, now);
        call.answer(answer);
    }

    /// Drops every command on its way, as the door closes: the ones already typed are marked
    /// cut off in their panels, so none shows as running forever.
    pub(super) fn cut_runs(&mut self, now: Instant) {
        for run in std::mem::take(&mut self.runs) {
            if run.typed.is_none() {
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

    /// Types each waiting command when its turn comes, and follows each typed one.
    fn move_runs(&mut self, now: Instant) {
        for run in std::mem::take(&mut self.runs) {
            if run.typed.is_none() {
                self.start_run(run, now);
            } else {
                self.watch_run(run, now);
            }
        }
    }

    /// Types `run`'s command once its terminal has started, its prompt has settled and no
    /// command before it in the tab still waits for its answer; gives it up when that takes
    /// too long.
    fn start_run(&mut self, mut run: Running, now: Instant) {
        // One command at a time per tab: a second one queues behind the first.
        let queued = self
            .runs
            .iter()
            .any(|other| other.tab == run.tab && other.call.is_some());
        let Ok(tab) = agent_tab(self, run.tab) else {
            run.answer(Err(format!("tab {} closed", run.tab)));
            return;
        };
        let Some(TermView::Live(panes)) = &tab.terminal else {
            run.answer(Err(format!("tab {} holds no terminal", run.tab)));
            return;
        };
        if let Some(why) = panes.focused_failure() {
            run.answer(Err(format!("tab {} could not connect: {why}", run.tab)));
            return;
        }
        if panes.focused_ended() {
            run.answer(Err(format!(
                "the shell in tab {} has ended: close the tab and open another",
                run.tab
            )));
            return;
        }
        let count = panes.focused_output();
        if count != run.output.0 {
            run.output = (count, now);
        }
        // A command answered while it went on may still print, or ask for the answer this
        // one types: its output must stop first, so this never lands in the middle of it.
        let busy = tab
            .agent
            .as_ref()
            .and_then(|log| log.commands.last())
            .is_some_and(|last| last.state.going());
        // Typing before the shell's first prompt has it echo the command early, or lose it,
        // so wait for the prompt to show and settle. Output alone is not enough: a Windows
        // console clears the screen and places the cursor before the shell says anything.
        let text = if queued {
            None
        } else {
            panes.focused_transcript()
        };
        let shown = text.as_deref().is_some_and(|text| !text.trim().is_empty());
        let ready = !queued
            && if shown {
                now.duration_since(run.output.1) >= if busy { QUIET } else { PROMPT_QUIET }
            } else {
                now.duration_since(run.asked) >= SILENT_PROMPT
            };
        if let Some(text) = text.filter(|_| ready) {
            // A trailing newline would press Enter twice.
            let mut keys = run
                .command
                .trim_end_matches(['\r', '\n'])
                .replace("\r\n", "\r")
                .replace('\n', "\r");
            keys.push('\r');
            let row = panes.focused_cursor_row().unwrap_or(0);
            if panes.type_focused(keys.as_bytes()) {
                let mut typed = Typed::of(&text);
                let at = crate::sync_screen::now_secs();
                let log = tab.agent.get_or_insert_with(|| Box::new(AgentLog::new(at)));
                if let Some(before) = log.commands.last() {
                    typed.following(&before.typed, busy, &log.commands);
                }
                run.entry = log.commands.len();
                log.commands.push(AgentCommand {
                    text: run.command.clone(),
                    at,
                    line: row,
                    state: Ran::Running(now),
                    typed: typed.clone(),
                });
                run.typed = Some(typed);
                run.asked = now;
                run.until = now + run.wait;
                run.base = count;
                run.output = (count, now);
                run.stale = true;
                // The command before is done with: no more watching it.
                let uid = run.tab;
                self.runs
                    .retain(|other| other.tab != uid || other.call.is_some());
                self.runs.push(run);
                return;
            }
        }
        if now.duration_since(run.asked) > START_WITHIN.max(run.wait) {
            let why = if queued || busy {
                format!(
                    "tab {} is still busy with the command before: wait for it, read it, or \
                     interrupt it",
                    run.tab
                )
            } else {
                "the terminal never started; read it to see why".to_owned()
            };
            run.answer(Err(why));
        } else {
            self.runs.push(run);
        }
    }

    /// Follows a typed command: answers its call once its prompt is back, its output stops
    /// with no prompt back, its shell ends or its wait runs out. Answered while it goes on,
    /// it is watched on until its prompt comes back, so its row turns done.
    fn watch_run(&mut self, mut run: Running, now: Instant) {
        let Ok(tab) = agent_tab(self, run.tab) else {
            run.answer(Err(format!("tab {} closed", run.tab)));
            return;
        };
        let Some(TermView::Live(panes)) = &tab.terminal else {
            run.answer(Err(format!("tab {} holds no terminal", run.tab)));
            return;
        };
        let Some(typed) = run.typed.clone() else {
            return;
        };
        let count = panes.focused_output();
        if count != run.output.0 {
            run.output = (count, now);
            run.stale = true;
        }
        let quiet = now.duration_since(run.output.1);
        // Its text is read once its output pauses, not on every frame while it streams.
        if run.stale && quiet >= PROMPT_QUIET {
            if let Some(text) = panes.focused_transcript() {
                run.after = after(&text, &typed);
            }
            run.stale = false;
        }
        let settles = if run.stale {
            None
        } else {
            settled(run.after, quiet, count != run.base)
        };
        let ending = if panes.focused_ended() || panes.focused_failure().is_some() {
            Some(Ending::Ended)
        } else {
            match settles {
                Some(Settled::Done) => Some(Ending::Done),
                Some(Settled::Waiting) if run.call.is_some() => Some(Ending::Waiting),
                _ if run.call.is_some() && now >= run.until => Some(Ending::TimedOut),
                _ => None,
            }
        };
        let Some(ending) = ending else {
            self.runs.push(run);
            return;
        };
        // Settled: it ran until its last output, not until the quiet after it ran out.
        let took = match ending {
            Ending::Done | Ending::Waiting => run.output.1.duration_since(run.asked),
            Ending::TimedOut | Ending::Ended => now.duration_since(run.asked),
        };
        let text = panes.focused_transcript().unwrap_or_default();
        let out = output_of(&text, &typed);
        let state = ran(&out, took, ending);
        let mut heard = state.state();
        let mut by_hand = false;
        if let Some(log) = tab.agent.as_mut() {
            if let Some(command) = log.commands.get_mut(run.entry) {
                // Ctrl+C or Stop already said how it ended.
                by_hand = matches!(command.state, Ran::Interrupted { .. } | Ran::Cut { .. });
                if by_hand {
                    heard = RunState::Interrupted;
                } else {
                    command.state = state;
                }
            }
            log.settle_before(run.entry, &text, ending);
        }
        run.answer(Ok(Response::Ran(RunOutput {
            output: out,
            state: heard,
            millis: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        })));
        if !by_hand && matches!(ending, Ending::Waiting | Ending::TimedOut) {
            self.runs.push(run);
        }
    }
}

/// The seconds `run` or `wait` asked to wait at most, as a wait.
fn waited(wait: Option<u64>) -> Duration {
    Duration::from_secs(wait.unwrap_or(WAIT_DEFAULT).min(WAIT_MAX))
}

/// The answer for the last command typed into agent tab `uid`, as it stands now.
fn last_answer(app: &mut NativeApp, uid: u64, now: Instant) -> Result<Response, String> {
    let tab = agent_tab(app, uid)?;
    let Some(TermView::Live(panes)) = &tab.terminal else {
        return Err(format!("tab {uid} holds no terminal"));
    };
    let Some(last) = tab.agent.as_ref().and_then(|log| log.commands.last()) else {
        return Err(format!("nothing was run in tab {uid} yet"));
    };
    let text = panes.focused_transcript().unwrap_or_default();
    Ok(Response::Ran(RunOutput {
        output: output_of(&text, &last.typed),
        state: last.state.state(),
        millis: u64::try_from(last.state.took(now).as_millis()).unwrap_or(u64::MAX),
    }))
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

/// How many lines a command printed, in words; [`READ_MAX`] or more, as many as its answer
/// holds and more (its oldest lines were cut).
fn lines_said(lines: usize) -> String {
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

/// How a command answered with `out` after `took` ran, as it ended. Its lines leave out the
/// command's own line and, once done, the prompt that came back after it; an answer cut to
/// its last [`READ_MAX`] lines counts as that many (the command printed more).
fn ran(out: &str, took: Duration, ending: Ending) -> Ran {
    let shown = out.trim_end().lines().count();
    let lines = |besides: usize| {
        if shown >= READ_MAX {
            READ_MAX
        } else {
            shown.saturating_sub(besides)
        }
    };
    match ending {
        Ending::Done => Ran::Done {
            took,
            lines: lines(2),
        },
        Ending::Waiting => Ran::Waiting {
            took,
            lines: lines(1),
        },
        Ending::TimedOut => Ran::TimedOut {
            took,
            lines: lines(1),
        },
        Ending::Ended => Ran::Ended { took },
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
        Request::SetNotes { host, notes } => set_notes(app, *host, notes),
        Request::Open { host } => {
            if let Some(id) = host
                && !vault.hosts().iter().any(|host| host.id == *id)
            {
                return Err(format!("no saved host {id}: see `annulus hosts`"));
            }
            open(app, *host)
        }
        Request::Run { tab, .. } => {
            // Typed and answered over the next frames by `NativeApp::control`.
            agent_tab(app, *tab).map(|_| Response::Done)
        }
        // Taken up by `NativeApp::control` while the command goes on; as it stands otherwise.
        Request::Wait { tab, .. } => last_answer(app, *tab, Instant::now()),
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
        Request::Read { tab, lines } => read(app, *tab, *lines),
    }
}

/// Opens an agent's tab: an SSH session to saved host `host` (known to exist), or the default
/// local shell. In front, or behind the tab in use, and with its panel open or folded, as
/// Settings say.
fn open(app: &mut NativeApp, host: Option<u64>) -> Result<Response, String> {
    let in_use = app.tabs.get(app.active).map(|tab| tab.uid);
    match host {
        Some(id) => app.act(super::hosts::HostsAction::Connect(id)),
        None => app.open_local(),
    }
    let mut opened = None;
    for (title, program, origin) in std::mem::take(&mut app.opens) {
        opened = Some(app.open_terminal(title, program, origin));
    }
    let uid = opened.ok_or("nothing opened")?;
    if !app.settings.agent_tabs_in_front
        && let Some(at) = in_use.and_then(|uid| app.tabs.iter().position(|tab| tab.uid == uid))
    {
        app.active = at;
    }
    let index = app.tabs.iter().position(|tab| tab.uid == uid).unwrap_or(0);
    let mut log = AgentLog::new(crate::sync_screen::now_secs());
    log.folded = app.settings.agent_panel_folded;
    let tab = &mut app.tabs[index];
    tab.agent = Some(Box::new(log));
    Ok(Response::Tab(tab_info(tab, index == app.active)))
}

/// The text of terminal tab `tab`, its last `lines` lines ([`READ_MAX`] at most). The owner's
/// tabs only while Settings let agents read them.
fn read(app: &mut NativeApp, tab: u64, lines: Option<usize>) -> Result<Response, String> {
    let mine_only = !app.settings.agents_read_my_tabs;
    let found = app.tabs.iter_mut().find(|open| open.uid == tab);
    if mine_only && found.as_ref().is_some_and(|open| open.agent.is_none()) {
        return Err(format!(
            "tab {tab} is not an agent's, and reading the owner's tabs is switched off \
             (Settings > Security > Automation)"
        ));
    }
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
            // What the tab shows in its own panel, not in the terminal: an agent reading a
            // tab that could not connect learns why.
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

/// Replaces saved host `host`'s notes, cleaned and 2,000 characters at most (F-HOST-011),
/// and tells the owner; the host as it is now.
fn set_notes(app: &mut NativeApp, host: u64, notes: &str) -> Result<Response, String> {
    let notes = annulus_vault::hosts::clean_notes(notes)?;
    let vault = app.vault.as_mut().ok_or("the vault is locked")?;
    let mut saved = vault
        .hosts()
        .iter()
        .find(|known| known.id == host)
        .cloned()
        .ok_or_else(|| format!("no saved host {host}: see `annulus hosts`"))?;
    if saved.notes != notes {
        saved.notes = notes;
        let label = saved.label.clone();
        vault
            .upsert_host(saved)
            .map_err(|error| format!("the notes could not be saved: {error}"))?;
        // The owner hears of every change an agent makes to the vault.
        app.agent_news
            .push(format!("An agent updated the notes of {label}"));
    }
    let vault = app.vault.as_ref().ok_or("the vault is locked")?;
    hosts(vault)
        .into_iter()
        .find(|info| info.id == host)
        .map(Response::Host)
        .ok_or_else(|| format!("no saved host {host}"))
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
                notes: host.notes.clone(),
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
    fn an_agent_reads_a_hosts_notes_and_rewrites_them_within_their_length() {
        let mut app = NativeApp::new(Some(fixture_vault("control-notes", true)));
        let web = app
            .vault
            .as_ref()
            .unwrap()
            .hosts()
            .iter()
            .find(|host| host.label == "web-1")
            .unwrap()
            .id;
        let set = |notes: &str| Request::SetNotes {
            host: web,
            notes: notes.into(),
        };
        let Ok(Response::Host(saved)) = handle(
            &mut app,
            &set("  nginx front end\r\nlogs in /var/log/nginx  "),
        ) else {
            panic!("the notes are saved");
        };
        let clean = "nginx front end\nlogs in /var/log/nginx";
        assert_eq!(saved.notes, clean);
        let Ok(Response::Hosts(hosts)) = handle(&mut app, &Request::Hosts) else {
            panic!("hosts");
        };
        let listed = hosts.iter().find(|host| host.id == web).unwrap();
        assert_eq!(listed.notes, clean, "the next agent reads them");
        assert_eq!(app.agent_news.len(), 1, "the owner hears of the change");
        let long = "x".repeat(annulus_vault::hosts::NOTES_MAX + 1);
        let refused = handle(&mut app, &set(&long)).unwrap_err();
        assert!(refused.contains("shorten"), "{refused}");
        assert!(
            handle(
                &mut app,
                &Request::SetNotes {
                    host: 999,
                    notes: String::new()
                }
            )
            .is_err()
        );
        app.lock_vault();
        assert!(
            handle(&mut app, &set("later")).is_err(),
            "never while locked"
        );
    }

    #[test]
    fn agents_tabs_open_behind_and_folded_and_the_owners_tabs_close_to_them_when_set() {
        let mut app = NativeApp::new(Some(fixture_vault("control-settings", true)));
        replay(&mut app, "mine", "owner@box:~$ uptime\r\n");
        let mine = app.tabs[app.active].uid;
        app.settings.agent_tabs_in_front = false;
        app.settings.agent_panel_folded = true;
        app.settings.agents_read_my_tabs = false;
        let Ok(Response::Tab(opened)) = handle(&mut app, &Request::Open { host: None }) else {
            panic!("a local tab opens");
        };
        assert!(!opened.active, "behind: {opened:?}");
        assert_eq!(
            app.tabs[app.active].uid, mine,
            "the tab in use stays in front"
        );
        let folded = app
            .tabs
            .iter()
            .find(|tab| tab.uid == opened.id)
            .and_then(|tab| tab.agent.as_ref())
            .is_some_and(|log| log.folded);
        assert!(folded, "its panel starts as the rail");
        let refused = handle(
            &mut app,
            &Request::Read {
                tab: mine,
                lines: None,
            },
        )
        .unwrap_err();
        assert!(refused.contains("switched off"), "{refused}");
        assert!(
            handle(
                &mut app,
                &Request::Read {
                    tab: opened.id,
                    lines: None
                }
            )
            .is_ok(),
            "its own tab it still reads"
        );
    }

    #[test]
    fn a_commands_output_starts_at_the_line_it_was_typed_on() {
        let typed = Typed::of("old line\nuser@box:~$");
        let after = "old line\nuser@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n";
        assert_eq!(
            output_of(after, &typed),
            "user@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n"
        );
        // Cleared meanwhile: all the terminal shows is newer than the command.
        let long = Typed::of(&format!("{}user@box:~$", "old\n".repeat(40)));
        assert_eq!(output_of("fresh\n", &long), "fresh\n");
    }

    #[test]
    fn a_commands_output_is_found_by_its_line_once_the_full_scrollback_drops_lines() {
        // 100 lines and the prompt; the scrollback is full, so as the command prints 6 lines,
        // 6 drop off the top and the count stays the same.
        let lines = |from: usize| -> String {
            let numbered: Vec<String> = (from..100).map(|n| format!("line {n}")).collect();
            numbered.join("\n") + "\n"
        };
        let typed = Typed::of(&format!("{}user@box:~$", lines(0)));
        let now = format!(
            "{}user@box:~$ seq 5\n1\n2\n3\n4\n5\nuser@box:~$\n",
            lines(6)
        );
        assert_eq!(now.lines().count(), typed.lines, "as many lines as before");
        assert_eq!(
            output_of(&now, &typed),
            "user@box:~$ seq 5\n1\n2\n3\n4\n5\nuser@box:~$\n"
        );
        assert_eq!(
            after(&now, &typed),
            After {
                lines: 6,
                prompt: true
            }
        );
        // Output enough to push the command's line to the very top: the lines above it are
        // gone, and it is still found.
        let top = "user@box:~$ seq 2\n1\n2\nuser@box:~$\n";
        assert_eq!(typed.find(&top.lines().collect::<Vec<_>>()), Some(0));
    }

    #[test]
    fn a_prompt_is_back_when_the_last_line_is_one_like_it_even_in_another_folder() {
        assert!(prompt_back("user@box:~$", "user@box:~$"));
        assert!(prompt_back("user@box:/tmp$", "user@box:~$"), "after cd");
        assert!(prompt_back(r"PS C:\Windows>", r"PS C:\Users\me>"));
        assert!(prompt_back("➜  tmp", "➜  ~"), "a prompt led by its mark");
        assert!(!prompt_back("[sudo] password for me:", "user@box:~$"));
        assert!(!prompt_back("Continue? [Y/n]", "user@box:~$"));
        assert!(
            !prompt_back("root@box:~#", "user@box:~$"),
            "another user's shell"
        );
        assert!(
            !prompt_back("</html>", r"PS C:\Users\me>"),
            "output that only ends like one"
        );
        assert!(!prompt_back("anything", ""), "no prompt was seen");
        assert!(
            prompt_back("okuser@box:~$", "user@box:~$"),
            "after output with no newline"
        );
        assert!(
            !prompt_back("cost 5$", "$"),
            "too short a prompt to find in output"
        );
        // A password typed at sudo's question ends at the shell prompt from before.
        let shell = Typed::of("user@box:~$");
        let mut answer = Typed::of("user@box:~$ sudo ls\n[sudo] password for me:");
        answer.following(&shell, true, &[]);
        // A command typed in the shell `ssh` opened ends at that shell's prompt.
        let mut remote = Typed::of("user@box:~$ ssh web-1\ndeploy@web-1:~$");
        remote.following(&shell, true, &[]);
        assert_eq!(remote.prompt, "deploy@web-1:~$");
        // Typed at the prompt behind output with no newline: it ends at the prompt itself.
        let mut behind = Typed::of("user@box:~$ printf ok\nokuser@box:~$");
        behind.following(&shell, false, &[]);
        assert_eq!(behind.prompt, "user@box:~$");
        let now = "user@box:~$ sudo ls\n[sudo] password for me:\nfile\nuser@box:~$\n";
        assert_eq!(
            after(now, &answer),
            After {
                lines: 2,
                prompt: true
            }
        );
    }

    #[test]
    fn a_command_is_done_once_its_prompt_is_back_and_a_silent_one_never_by_quiet_alone() {
        let after = |lines, prompt| After { lines, prompt };
        let long = Duration::from_secs(20);
        assert_eq!(
            settled(after(0, false), long, true),
            None,
            "nothing past its own line: a silent sleep runs on"
        );
        assert_eq!(
            settled(after(3, true), PROMPT_QUIET, true),
            Some(Settled::Done)
        );
        assert_eq!(
            settled(after(3, true), PROMPT_QUIET / 3, true),
            None,
            "the prompt may still be drawing"
        );
        assert_eq!(
            settled(after(2, false), QUIET, true),
            Some(Settled::Waiting),
            "output, then quiet with no prompt: a question, or a silent step"
        );
        assert_eq!(settled(after(2, false), QUIET / 2, true), None);
        assert_eq!(
            settled(after(2, false), long, false),
            None,
            "waited on again, only output since then counts"
        );
    }

    #[test]
    fn an_exit_from_ssh_is_done_at_the_local_prompt_and_settles_the_ssh_before_it() {
        let local = r"PS C:\Windows>";
        let command = |text: &str, typed: Typed, state| AgentCommand {
            text: text.into(),
            at: 0,
            line: 0,
            state,
            typed,
        };
        let waiting = || Ran::Waiting {
            took: Duration::from_millis(600),
            lines: 30,
        };
        let mut log = AgentLog::new(0);
        log.commands
            .push(command("ssh web-1", Typed::of(local), waiting()));
        let screen = format!("{local} ssh web-1\nWelcome\ndeploy@web-1:~$");
        let mut exit = Typed::of(&screen);
        exit.following(&log.commands[0].typed, true, &log.commands);
        assert_eq!(exit.prompt, "deploy@web-1:~$", "typed in the remote shell");
        let now = format!("{screen} exit\nlogout\n{local}\n");
        assert_eq!(
            after(&now, &exit),
            After {
                lines: 2,
                prompt: true
            },
            "the local prompt the tab showed before counts as back"
        );
        log.commands
            .push(command("exit", exit, Ran::Running(Instant::now())));
        log.settle_before(1, &now, Ending::Done);
        assert!(
            matches!(log.commands[0].state, Ran::Done { .. }),
            "the ssh it left is done too"
        );
        // A shell that ended takes everything still going with it.
        log.commands[0].state = waiting();
        log.settle_before(1, &now, Ending::Ended);
        assert!(matches!(log.commands[0].state, Ran::Ended { .. }));
    }

    #[test]
    fn a_finished_command_counts_its_output_and_a_timed_out_one_keeps_its_lines_so_far() {
        let out = "user@box:~$ uptime\n 10:00 up 2 days\nuser@box:~$\n";
        let took = Duration::from_millis(400);
        assert!(matches!(
            ran(out, took, Ending::Done),
            Ran::Done { lines: 1, .. }
        ));
        let going = "user@box:~$ tail -f log\none\ntwo\n";
        assert!(matches!(
            ran(going, took, Ending::TimedOut),
            Ran::TimedOut { lines: 2, .. }
        ));
        assert!(matches!(
            ran("user@box:~$ true\nuser@box:~$\n", took, Ending::Done),
            Ran::Done { lines: 0, .. }
        ));
        assert!(matches!(
            ran(
                "user@box:~$ sudo ls\n[sudo] password for me:\n",
                took,
                Ending::Waiting
            ),
            Ran::Waiting { lines: 1, .. }
        ));
    }

    #[test]
    fn each_command_says_when_in_the_session_how_long_and_how_much() {
        let command = |state| AgentCommand {
            text: "uptime".into(),
            at: 1_000 + 84,
            line: 0,
            state,
            typed: Typed::default(),
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
        assert_eq!(lines_said(4_998), "4,998 lines");
        // An answer cut to its last lines says the command printed more.
        let cut = "x\n".repeat(READ_MAX);
        let Ran::Done { lines, .. } = ran(&cut, Duration::ZERO, Ending::Done) else {
            panic!("done");
        };
        assert_eq!(lines_said(lines), "5,000+ lines");
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
                typed: Typed::default(),
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
