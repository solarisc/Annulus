//! The app's side of the control door (ADR-0020, F-AUTO-001/002): it is open while Settings →
//! Security → Automation is on and the vault is unlocked, and [`handle`] answers what comes
//! through it. Answers carry what a person sees in the window, never a secret.

use annulus_control::door::Door;
use annulus_control::protocol::{HostInfo, Request, Response, Status, TabInfo};
use annulus_vault::groups::Group;
use annulus_vault::hosts::Host;
use annulus_vault::vault::Vault;

use super::{NativeApp, TabKind, TermView};
use crate::terminal_view::TabStatus;

impl NativeApp {
    /// Opens the door when it should be open and closes it when it should not, then answers
    /// every request that came through it.
    pub(super) fn control(&mut self) {
        let wanted = self.settings.automation && self.vault.is_some() && self.blocked.is_none();
        if !wanted {
            self.door = None;
            self.door_failed = false;
            return;
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
            let answer = handle(self, &call.request);
            call.answer(answer);
        }
    }
}

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
                .map(|(index, tab)| TabInfo {
                    id: tab.uid,
                    title: tab.title.clone(),
                    kind: kind_name(tab.kind).into(),
                    status: tab.status.map_or("", status_name).into(),
                    active: index == app.active,
                })
                .collect(),
        )),
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
            Ok(Response::Text(last_lines(&text, *lines)))
        }
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
