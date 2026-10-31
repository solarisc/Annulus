//! The app's side of the control door (ADR-0020, F-AUTO): it is open while Settings →
//! Security → Automation is on and the vault is unlocked, and [`handle`] answers what comes
//! through it. Answers carry what a person sees in the window, never a secret.
//!
//! An agent opens tabs of its own, marked as its, and types only into those; the owner's
//! tabs are read-only to it. What it typed is listed beside its tab ([`AgentLog`]).

use std::time::{Duration, Instant};

use annulus_config::tokens::space;
use annulus_control::door::{Door, Pending};
use annulus_control::protocol::{HostInfo, Request, Response, Status, TabInfo};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::{BODY_MEDIUM, CONTROL, Kind, SMALL};
use annulus_vault::groups::Group;
use annulus_vault::hosts::Host;
use annulus_vault::vault::Vault;

use super::{NativeApp, Tab, TabKind, TermView};
use crate::terminal_view::TabStatus;

/// What an agent typed into its tab, newest last. In memory only: it can hold secrets.
#[derive(Default)]
pub struct AgentLog {
    /// Each command and when it was typed (seconds since the Unix epoch).
    pub commands: Vec<(u64, String)>,
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
}

/// Output quiet this long counts as finished.
const QUIET: Duration = Duration::from_millis(1200);
/// How long the terminal's prompt must sit unchanged before a command is typed at it.
const PROMPT_QUIET: Duration = Duration::from_millis(300);
/// How long to wait for a prompt from a shell that prints none before typing anyway.
const SILENT_PROMPT: Duration = Duration::from_secs(5);
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
            self.runs.clear();
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
                // Typing before the shell's first prompt has it echo the command early and
                // print its prompt into the answer, so wait for the prompt to settle.
                let shown = panes.focused_transcript().unwrap_or_default();
                if shown.len() != run.seen.0 {
                    run.seen = (shown.len(), now);
                }
                let ready = if shown.trim().is_empty() {
                    now.duration_since(run.asked) >= SILENT_PROMPT
                } else {
                    now.duration_since(run.seen.1) >= PROMPT_QUIET
                };
                let mut typed = run.command.replace("\r\n", "\r").replace('\n', "\r");
                typed.push('\r');
                if ready && panes.type_focused(typed.as_bytes()) {
                    let text = panes.focused_transcript().unwrap_or_default();
                    run.before = Some(text.trim_end().lines().count());
                    run.seen = (text.len(), now);
                    run.asked = now;
                    let log = tab.agent.get_or_insert_with(Box::default);
                    log.commands
                        .push((crate::sync_screen::now_secs(), run.command.clone()));
                    self.runs.push(run);
                } else if now.duration_since(run.asked) > START_WITHIN {
                    run.call
                        .answer(Err("the terminal never started; read it to see why".into()));
                } else {
                    self.runs.push(run);
                }
                continue;
            };
            let text = panes.focused_transcript().unwrap_or_default();
            if text.len() != run.seen.0 {
                run.seen = (text.len(), now);
            }
            let settled = now.duration_since(run.seen.1) >= QUIET;
            if settled || now.duration_since(run.asked) >= run.wait {
                run.call
                    .answer(Ok(Response::Text(new_output(&text, before))));
            } else {
                self.runs.push(run);
            }
        }
    }
}

/// How wide the strip beside an agent's terminal is.
pub const STRIP_W: f32 = 280.0;

/// The strip beside an agent's terminal (ADR-0020 §6): what it is, each command it typed with
/// when, newest last, and Stop. Returns whether Stop was pressed: automation goes off and every
/// agent is shut out.
pub fn agent_strip(ui: &mut Ui<'_>, rect: Rect, log: &AgentLog) -> bool {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 0.0, t.chrome);
    ui.gfx
        .fill_rect(Rect::new(rect.x, rect.y, 1.0, rect.h), 0.0, t.line);
    let mut column = rect.inset(space::M, space::M);
    let head = column.take_top(22.0);
    let chip = ui.chip(Point::new(head.x, head.y + 1.0), "AI", t.accent);
    let mut title = head;
    title.take_left(chip + space::S);
    ui.gfx.text(
        title,
        "An agent is at work",
        BODY_MEDIUM,
        t.text,
        Align::Start,
    );
    column.take_top(space::XS);
    ui.gfx.text(
        column.take_top(18.0),
        "It types only in this tab.",
        SMALL,
        t.text2,
        Align::Start,
    );
    ui.gfx.text(
        column.take_top(18.0),
        "Your other tabs are read-only to it.",
        SMALL,
        t.text2,
        Align::Start,
    );
    let stop_row = column.take_bottom(CONTROL);
    let stop = ui
        .button(
            Id::new("agent.stop"),
            stop_row,
            "Stop all agents",
            Some(icons::CLOSE),
            Kind::Danger,
        )
        .clicked;
    ui.tooltip(
        Id::new("agent.stop"),
        stop_row,
        "Turns automation off: every agent is cut off now",
    );
    column.take_bottom(space::M);
    column.take_top(space::M);
    ui.section_label(column.take_top(18.0), "Commands");
    column.take_top(space::XS);
    if log.commands.is_empty() {
        ui.gfx.text(
            column.take_top(20.0),
            "Nothing typed yet.",
            SMALL,
            t.text3,
            Align::Start,
        );
        return stop;
    }
    // The newest at the bottom; as many of the latest as fit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // A count of rows.
    let fit = (column.h / STRIP_ROW).floor().max(0.0) as usize;
    let now = crate::sync_screen::now_secs();
    let shown = &log.commands[log.commands.len().saturating_sub(fit)..];
    for (at, command) in shown {
        let mut row = column.take_top(STRIP_ROW);
        let when = row.take_top(16.0);
        ui.gfx.text(
            when,
            &super::start::ago(*at, now),
            SMALL,
            t.text3,
            Align::Start,
        );
        let first = command.lines().next().unwrap_or_default();
        ui.gfx
            .text(row.take_top(20.0), first, MONO_SMALL, t.text, Align::Start);
    }
    stop
}

/// The height of one command in the strip: when, then the command.
const STRIP_ROW: f32 = 40.0;

/// Commands in the strip: monospace, a touch smaller than the terminal's.
const MONO_SMALL: annulus_ui::gfx::Font = annulus_ui::gfx::Font::mono(12.0);

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
            tab.agent = Some(Box::default());
            Ok(Response::Tab(tab_info(tab, index == app.active)))
        }
        Request::Run { tab, .. } => {
            // Typed and answered over the next frames by `NativeApp::control`.
            agent_tab(app, *tab).map(|_| Response::Done)
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
                    if all.len() == 1 {
                        all.remove(0).1
                    } else {
                        all.iter()
                            .map(|(title, text)| format!("--- {title} ---\n{text}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
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
