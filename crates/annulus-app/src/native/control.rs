//! The app's side of the control door (ADR-0020, F-AUTO): it is open while Settings →
//! Security → Automation is on and the vault is unlocked, and [`handle`] answers what comes
//! through it. Answers carry what a person sees in the window, never a secret.
//!
//! An agent opens tabs of its own, marked as its, and types only into those; the owner's
//! tabs are read-only to it. What it typed is listed beside its tab ([`AgentLog`]).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use annulus_control::door::{Door, Pending};
use annulus_control::protocol::{
    HostInfo, Request, Response, RunOutput, RunState, Status, TabInfo,
};
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
    /// The command's first line as typed, to tell its own line wrapped at the terminal's
    /// width from output; empty when unknown.
    command: String,
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
            command: String::new(),
        }
    }

    /// How many rows after `lines[at]`, the command's line, are that line wrapped at the
    /// terminal's width: a long command fills more than one row, and those rows are none of
    /// its output. 0 when the command shows on its line whole, or cannot be found there.
    fn wrapped_rows(&self, lines: &[&str], at: usize) -> usize {
        // A wrap can fall on a space: compare without any.
        let squeeze =
            |text: &str| -> String { text.chars().filter(|c| !c.is_whitespace()).collect() };
        let command = squeeze(&self.command);
        if command.is_empty() {
            return 0;
        }
        let mut joined = squeeze(lines[at]);
        let mut rows = 0;
        let start = loop {
            if let Some(start) = joined.find(&command) {
                break start;
            }
            let Some(next) = lines.get(at + rows + 1).filter(|_| rows < 8) else {
                return 0;
            };
            rows += 1;
            joined.push_str(&squeeze(next));
        };
        // Its echo follows a prompt; found after anything else, those were its output's words
        // (its echo lost), and every row after its line is output.
        if rows == 0 || looks_like_prompt(&joined[..start]) {
            rows
        } else {
            0
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

    /// The prompt in front of the command's echo in `rows` (its line and those it wrapped
    /// onto), when it was typed at a line that is no prompt (one the shell printed while it
    /// started, such as "Loading personal and system profiles took 900ms."): the prompt that
    /// ends it is the one it was echoed at, not that line.
    fn echo_prompt(&self, rows: &[&str]) -> Option<String> {
        if looks_like_prompt(&self.prompt) {
            return None;
        }
        let head = self.command.split_whitespace().next()?;
        rows.iter().find_map(|row| {
            let prompt = row[..row.find(head)?].trim_end();
            looks_like_prompt(prompt).then(|| prompt.to_owned())
        })
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
    let (count, echoed) = match typed.find(&lines) {
        Some(at) => {
            let rows = typed.wrapped_rows(&lines, at);
            (
                lines.len() - at - 1 - rows,
                typed.echo_prompt(&lines[at..=at + rows]),
            )
        }
        None => (lines.len(), None),
    };
    let back = |last: &str| {
        typed.prompt_back(last)
            || echoed
                .as_deref()
                .is_some_and(|echoed| prompt_back(last, echoed))
    };
    After {
        lines: count,
        prompt: count > 0 && lines.last().is_some_and(|last| back(last)),
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
/// Whether a command may be typed into a terminal showing `text` (`None`: not read), its
/// output quiet for `quiet`, `waited` after it was asked for; `busy` while the command before
/// still goes on (this one answers its question). Once the shell has shown text and settled,
/// it is typed even before the prompt: a fresh PowerShell drops keys that arrive as its
/// prompt appears, but keeps those typed while it starts. A command
/// typed at a line that is no prompt ends at the first prompt instead
/// ([`Typed::prompt_back`]).
fn ready_to_type(text: Option<&str>, quiet: Duration, waited: Duration, busy: bool) -> bool {
    let Some(text) = text else {
        return false;
    };
    if text.trim().is_empty() {
        return waited >= SILENT_PROMPT;
    }
    quiet >= if busy { QUIET } else { PROMPT_QUIET }
}
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
        if self.settings.closes_ended_agent_tabs() {
            self.close_ended_agent_tabs();
        }
        self.runs
            .iter()
            .any(|run| run.call.is_some() || now.duration_since(run.output.1) < QUIET * 2)
    }

    /// Closes agents' tabs whose shells have all ended, once no answer waits on them
    /// (Settings > Security > Automation, F-AUTO-006, and only while agents may close tabs).
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
        // Typing before the shell's prompt has it echo the command early, or lose it, so wait
        // for the prompt to show and settle (`ready_to_type`). Output alone is not enough: a
        // Windows console clears the screen and places the cursor before the shell says
        // anything, and PowerShell may print a line of its own before its prompt.
        let text = if queued {
            None
        } else {
            panes.focused_transcript()
        };
        let ready = ready_to_type(
            text.as_deref(),
            now.duration_since(run.output.1),
            now.duration_since(run.asked),
            busy,
        );
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
                run.command
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .clone_into(&mut typed.command);
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

mod panel;
#[cfg(test)]
use panel::{Ink, clock, describe_run, lines_said, session_said, wrap_command};
pub use panel::{PanelAction, agent_panel, panel_width};
/// Agent tab `uid`, or why it is not one.
fn agent_tab(app: &mut NativeApp, uid: u64) -> Result<&mut Tab, String> {
    let tab = app
        .tabs
        .iter_mut()
        .find(|tab| tab.uid == uid)
        .ok_or_else(|| format!("no tab {uid}: see `annulus-agent tabs`"))?;
    if tab.agent.is_none() {
        return Err(format!(
            "tab {uid} is not an agent's: open one with `annulus-agent open` (yours are read-only)"
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
                return Err(format!("no saved host {id}: see `annulus-agent hosts`"));
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
            if app.settings.stop_agents_closing_tabs {
                return Err(format!(
                    "tab {tab} stays open: agents may not close tabs (Settings > Security > \
                     Automation), so the owner can read what was done there"
                ));
            }
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
        None => return Err(format!("no terminal tab {tab}: see `annulus-agent tabs`")),
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
        .ok_or_else(|| format!("no saved host {host}: see `annulus-agent hosts`"))?;
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
        // Once the owner lets agents close tabs.
        app.settings.stop_agents_closing_tabs = false;
        assert_eq!(
            handle(&mut app, &Request::Close { tab: opened.id }),
            Ok(Response::Done)
        );
        assert!(!app.tabs.iter().any(|tab| tab.uid == opened.id));
    }

    #[test]
    fn an_agents_tab_stays_open_until_the_owner_lets_agents_close_tabs() {
        let mut app = NativeApp::new(Some(fixture_vault("control-keep", true)));
        let Ok(Response::Tab(opened)) = handle(&mut app, &Request::Open { host: None }) else {
            panic!("a local tab opens");
        };
        assert!(app.settings.stop_agents_closing_tabs, "on by default");
        let refused = handle(&mut app, &Request::Close { tab: opened.id }).unwrap_err();
        assert!(
            refused.contains("Settings > Security > Automation"),
            "{refused}"
        );
        assert!(
            app.tabs.iter().any(|tab| tab.uid == opened.id),
            "the tab stays for the owner"
        );
        app.settings.stop_agents_closing_tabs = false;
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
    fn a_command_typed_before_the_prompt_ends_at_the_prompt_it_was_echoed_at() {
        let loading = "PowerShell 7.6.9\nLoading personal and system profiles took 912ms.";
        let mut typed = Typed::of(loading);
        "echo hi".clone_into(&mut typed.command);
        // Echoed once the prompt came, then its line and the prompt back.
        let screen = format!("{loading}\nPS C:\\work> echo hi\nhi\nPS C:\\work>");
        assert_eq!(
            after(&screen, &typed),
            After {
                lines: 2,
                prompt: true
            }
        );
        // A line of its own that merely ends like a prompt does not end it.
        let going = format!("{loading}\nPS C:\\work> echo hi\nProgress 45%");
        assert!(!after(&going, &typed).prompt);
        // Typed at a prompt, only a prompt like that one ends it, as before.
        let at_prompt = Typed::of("user@box:~$");
        assert!(!at_prompt.prompt_back("50% done >"));
        assert!(at_prompt.prompt_back("user@box:/tmp$"));
    }

    #[test]
    fn a_commands_words_in_its_output_are_no_echo_when_the_echo_is_lost() {
        let mut typed = Typed::of("PS C:\\work>");
        "ls".clone_into(&mut typed.command);
        // The echo never came; "tools" holds "ls" but is output, after no prompt.
        assert_eq!(
            after("PS C:\\work>\ntools\nPS C:\\work>", &typed),
            After {
                lines: 2,
                prompt: true
            }
        );
    }

    #[test]
    fn a_command_wrapped_at_the_terminals_width_is_no_output_of_its_own() {
        let mut typed = Typed::of("PS C:\\work>");
        "Start-Sleep -Seconds 3; Write-Output said".clone_into(&mut typed.command);
        // Too long for the row, its line went on to the next one: still silent.
        let wrapped = "PS C:\\work> Start-Sleep -Seconds 3\n; Write-Output said";
        assert_eq!(
            after(wrapped, &typed),
            After {
                lines: 0,
                prompt: false
            }
        );
        // A wrap that falls on a space reads the same.
        let at_space = "PS C:\\work> Start-Sleep -Seconds\n3; Write-Output said";
        assert_eq!(after(at_space, &typed).lines, 0);
        // Then its line and its prompt back.
        assert_eq!(
            after(&format!("{wrapped}\nsaid\nPS C:\\work>"), &typed),
            After {
                lines: 2,
                prompt: true
            }
        );
        // A command whose echo is not there counts every line after, as before.
        typed.command = "something else".into();
        assert_eq!(after(wrapped, &typed).lines, 1);
    }

    #[test]
    fn a_command_is_typed_once_the_shell_has_shown_text_and_settled() {
        let ms = Duration::from_millis;
        let loading = "PowerShell 7.6.9\nLoading personal and system profiles took 912ms.";
        // While a fresh PowerShell starts its keys are kept; as its prompt appears they may
        // not be, so the command goes in once anything shows and settles.
        assert!(ready_to_type(Some(loading), PROMPT_QUIET, ms(2_000), false));
        assert!(
            !ready_to_type(Some("PS C:\\work> "), ms(100), ms(2_000), false),
            "not settled yet"
        );
        // Answering a question of the command before: its output must have stopped.
        assert!(!ready_to_type(
            Some("Password:"),
            PROMPT_QUIET,
            ms(2_000),
            true
        ));
        assert!(ready_to_type(Some("Password:"), QUIET, ms(2_000), true));
        // Nothing shown yet: a shell that prints no prompt gets the command after a while.
        assert!(!ready_to_type(Some(""), ms(5_000), ms(5_000), false));
        assert!(ready_to_type(Some("\n \n"), ms(0), SILENT_PROMPT, false));
        assert!(
            !ready_to_type(None, ms(5_000), SILENT_PROMPT, false),
            "queued behind another"
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
        // At a space where there is one, so words stay whole.
        assert_eq!(
            wrap_command("docker ps --format x", 10, 3),
            vec!["docker ps", "--format x"]
        );
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
