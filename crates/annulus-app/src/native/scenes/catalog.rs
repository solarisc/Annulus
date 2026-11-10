//! Scene definitions and their setup state. Rendering, input, and checks stay in the parent.

use annulus_config::settings::OpenTab;
use annulus_config::tokens::ThemeMode;
use annulus_ui::kit::{Key, Modifiers};

use super::super::hosts::Mode;
use super::super::shortcuts;
use super::super::{NativeApp, TermView, WindowButtons};
use super::{Act, Scene};
use crate::native::{Section, demo};
use crate::sync_screen::Attempt;
use crate::terminal_view::TabStatus;
pub(super) fn nothing(_: &mut NativeApp) {}

pub(super) fn list_mode(app: &mut NativeApp) {
    app.hosts.mode = Mode::List;
}

pub(super) fn edit_prod_db(app: &mut NativeApp) {
    let id = app
        .vault
        .as_ref()
        .and_then(|vault| vault.hosts().iter().find(|host| host.label == "prod-db"))
        .map(|host| host.id)
        .unwrap();
    if let Some(vault) = &app.vault {
        app.hosts.edit_host(vault, id);
    }
    if let Some(editor) = &mut app.hosts.editor {
        editor.advanced = true;
    }
}

/// prod-db's editor with notes longer than a line of the box, which wrap inside it.
pub(super) fn edit_prod_db_notes(app: &mut NativeApp) {
    if let Some(vault) = app.vault.as_mut()
        && let Some(mut host) = vault
            .hosts()
            .iter()
            .find(|host| host.label == "prod-db")
            .cloned()
    {
        host.notes = "Primary Postgres for the shop; replicas on db-2 and db-3. Restart only \
                      after draining connections: see https://wiki.example.com/runbooks/\
                      postgres-restart-without-dropping-checkouts\nBackups at 02:00."
            .into();
        vault.upsert_host(host).unwrap();
    }
    edit_prod_db(app);
}

/// The Keychain with its key in use by a host: Delete asks about its servers too.
pub(super) fn keychain_in_use(app: &mut NativeApp) {
    if let Some(vault) = app.vault.as_mut()
        && let Some(key) = vault.ssh_keys().first().map(|key| key.id)
        && let Some(mut host) = vault.hosts().first().cloned()
    {
        host.key_id = Some(key);
        vault.upsert_host(host).unwrap();
    }
    section(app, Section::Keychain);
}

pub(super) fn with_tabs(app: &mut NativeApp) {
    for (title, status) in [
        ("prod-db", None),
        ("web-1", Some(TabStatus::Unread)),
        ("bastion", Some(TabStatus::Error)),
    ] {
        app.push_terminal(title, TermView::Replay(Box::new(demo::terminal())));
        if let Some(tab) = app.tabs.last_mut() {
            tab.status = status;
        }
    }
    // The hosts grid, with the tabs above it.
    app.active = 0;
}

pub(super) fn section(app: &mut NativeApp, section: Section) {
    app.ssh_files.known_hosts = demo::known_hosts("scenes");
    app.section = section;
}

pub(super) fn keychain(app: &mut NativeApp) {
    section(app, Section::Keychain);
}

pub(super) fn snippets(app: &mut NativeApp) {
    section(app, Section::Snippets);
}

pub(super) fn forwards(app: &mut NativeApp) {
    section(app, Section::PortForwarding);
}

/// Port Forwarding with a rule that listens on every interface (an imported `*:1080`).
pub(super) fn forwards_open(app: &mut NativeApp) {
    if let Some(vault) = app.vault.as_mut()
        && let Some(mut rule) = vault.forwards().first().cloned()
    {
        "0.0.0.0".clone_into(&mut rule.bind_address);
        vault.upsert_forward(rule).unwrap();
    }
    section(app, Section::PortForwarding);
}

/// Port Forwarding with hosts but no rules yet: the wizard is the screen.
pub(super) fn forwards_empty(app: &mut NativeApp) {
    if let Some(vault) = app.vault.as_mut() {
        for rule in vault.forwards().to_vec() {
            vault.remove_forward(rule.id).unwrap();
        }
    }
    section(app, Section::PortForwarding);
}

pub(super) fn known_hosts(app: &mut NativeApp) {
    section(app, Section::KnownHosts);
}

pub(super) fn logs(app: &mut NativeApp) {
    section(app, Section::Logs);
}

pub(super) fn sftp(app: &mut NativeApp) {
    app.push_sftp(demo::sftp_tab("scenes"));
}

pub(super) fn settings(app: &mut NativeApp) {
    section(app, Section::Settings);
}

/// Settings with agents allowed to close their tabs, so Tab behavior has its fifth row.
pub(super) fn settings_agents_may_close(app: &mut NativeApp) {
    settings(app);
    app.settings.stop_agents_closing_tabs = false;
}

pub(super) fn settings_account(app: &mut NativeApp) {
    settings(app);
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    // Made up, so no photo shows the CI machine's name.
    "Work laptop".clone_into(&mut app.sync.device);
}

/// A LAN server's own certificate, waiting for the user to compare its fingerprint.
pub(super) fn settings_account_certificate(app: &mut NativeApp) {
    settings_account(app);
    "https://10.1.1.20:8443".clone_into(&mut app.sync.server);
    "ana".clone_into(&mut app.sync.account);
    app.sync.untrusted = Some(((0..=255).collect(), Attempt::Create));
}

pub(super) fn locked(app: &mut NativeApp) {
    app.vault = None;
}

/// The title bar as on macOS: the OS draws the window buttons at its left end.
pub(super) fn macos_title_bar(app: &mut NativeApp) {
    with_tabs(app);
    app.window_buttons = WindowButtons::Os;
}

pub(super) fn first_run(app: &mut NativeApp) {
    app.vault = None;
    app.lock = crate::native::lock::LockView::new(crate::native::lock::Mode::Setup);
}

pub(super) fn terminal_tab(app: &mut NativeApp) {
    with_tabs(app);
    app.active = crate::native::FIXED_TABS;
}

/// An agent's tab (ADR-0020, F-AUTO-004): marked AI, with the commands it typed beside its
/// terminal: done, out of time, a long one wrapped, one picked and the newest still running.
pub(super) fn agent_tab(app: &mut NativeApp) {
    use crate::native::control::{AgentCommand, AgentLog, Ran};
    use std::time::{Duration, Instant};
    terminal_tab(app);
    let now = crate::sync_screen::now_secs();
    let tab = &mut app.tabs[crate::native::FIXED_TABS];
    "deploy-check".clone_into(&mut tab.title);
    let command = |text: &str, ago: u64, line, state| AgentCommand {
        text: text.into(),
        at: now - ago,
        line,
        state,
        typed: crate::native::control::Typed::default(),
    };
    let done = |millis, lines| Ran::Done {
        took: Duration::from_millis(millis),
        lines,
    };
    let mut log = AgentLog::new(now - 330);
    log.commands = vec![
        command("systemctl is-active nginx", 300, 0, done(210, 1)),
        command("df -h /var", 280, 2, done(380, 2)),
        command(
            "docker ps -a --format \"table {{.Names}}\\t{{.Image}}\\t{{.Status}}\\t{{.Ports}}\"",
            200,
            5,
            done(640, 6),
        ),
        command(
            "journalctl -u app -f",
            120,
            12,
            Ran::TimedOut {
                took: Duration::from_secs(30),
                lines: 48,
            },
        ),
        command(
            "curl -s http://127.0.0.1:8080/healthz",
            4,
            61,
            Ran::Running(Instant::now()),
        ),
    ];
    log.picked = Some(2);
    tab.agent = Some(Box::new(log));
    app.settings.automation = true;
}

/// The same tab after Stop: the panel says agents are cut off and offers to allow them
/// again, and the command that was running shows as cut off.
pub(super) fn agent_tab_stopped(app: &mut NativeApp) {
    use crate::native::control::Ran;
    agent_tab(app);
    app.settings.automation = false;
    if let Some(log) = app.tabs[crate::native::FIXED_TABS].agent.as_mut() {
        log.picked = None;
        if let Some(last) = log.commands.last_mut() {
            last.state = Ran::Cut {
                took: std::time::Duration::from_secs(4),
            };
        }
        if let Some(journal) = log.commands.get_mut(3) {
            journal.state = Ran::Interrupted {
                took: std::time::Duration::from_secs(41),
            };
        }
    }
}

/// The same tab with its agent panel folded to a rail.
pub(super) fn agent_tab_folded(app: &mut NativeApp) {
    agent_tab(app);
    if let Some(log) = app.tabs[crate::native::FIXED_TABS].agent.as_mut() {
        log.folded = true;
    }
}

/// An agent command waiting for the owner's choice before it reaches the terminal.
pub(super) fn agent_command_approval(app: &mut NativeApp) {
    agent_tab(app);
    app.settings.approve_commands = true;
    let tab = app.tabs[crate::native::FIXED_TABS].uid;
    app.preview_approval(tab, "sudo systemctl restart nginx");
}

/// The window and the terminal in two different dark community schemes.
pub(super) fn scheme_dark(app: &mut NativeApp) {
    terminal_tab(app);
    app.settings.app_scheme = Some("Gruvbox Dark".into());
    "Tokyo Night Storm".clone_into(&mut app.settings.terminal_theme);
}

/// The Look settings in a light community scheme.
pub(super) fn scheme_light(app: &mut NativeApp) {
    settings(app);
    app.settings.app_scheme = Some("Solarized Light".into());
}

/// A New Tab page after a few connections from this device.
pub(super) fn new_tab(app: &mut NativeApp) {
    with_tabs(app);
    let now = crate::sync_screen::now_secs();
    for (label, ago) in [
        ("staging-db", 86_400 * 3),
        ("web-1", 3 * 3600),
        ("prod-db", 240),
    ] {
        if let Some(id) = app.vault.as_ref().and_then(|vault| {
            vault
                .hosts()
                .iter()
                .find(|host| host.label == label)
                .map(|host| host.id)
        }) {
            app.settings.connected(id, now - ago);
        }
    }
    // A saved host's terminal, so its tab shows the server's mark.
    let web = app.vault.as_ref().and_then(|vault| {
        vault
            .hosts()
            .iter()
            .find(|host| host.label == "web-1")
            .map(|host| host.id)
    });
    if let Some(id) = web {
        app.push_terminal("web-1", TermView::Replay(Box::new(demo::terminal())));
        if let Some(tab) = app.tabs.last_mut() {
            tab.origin = Some(OpenTab::Host(id));
        }
    }
    app.open_new_tab();
}

/// A New Tab page with two hosts pinned to its top.
pub(super) fn new_tab_pinned(app: &mut NativeApp) {
    new_tab(app);
    for label in ["bastion", "nas"] {
        if let Some(id) = app.vault.as_ref().and_then(|vault| {
            vault
                .hosts()
                .iter()
                .find(|host| host.label == label)
                .map(|host| host.id)
        }) {
            app.settings.toggle_pin(id);
        }
    }
}

pub(super) fn sftp_picker(app: &mut NativeApp) {
    app.active = 1;
}

/// An SSH tab part-way through connecting, its log open.
pub(super) fn connecting(app: &mut NativeApp) {
    let program = crate::connect::quick("deploy@web-1.example.com", &app.ssh_files);
    let mut terminal = crate::native::live::LiveTerminal::held_at(
        program,
        1,
        &[
            "Reaching web-1.example.com:22 as deploy",
            "web-1.example.com's key is trusted",
        ],
    );
    terminal.identity = Some(("web-1".into(), crate::native::os::find("ubuntu")));
    app.push_terminal(
        "web-1",
        TermView::Live(Box::new(crate::native::panes::Panes::new(terminal))),
    );
}

/// An SFTP tab asking what to do with README.md, already on the host, and one more waiting.
pub(super) fn sftp_clash(app: &mut NativeApp) {
    sftp(app);
    if let Some(view) = app.tabs.last_mut().and_then(|tab| tab.sftp.as_mut()) {
        view.demo_clash();
    }
}

/// An SFTP tab with transfers in every state: under way, done, skipped, failed.
pub(super) fn sftp_transfers(app: &mut NativeApp) {
    sftp(app);
    if let Some(view) = app.tabs.last_mut().and_then(|tab| tab.sftp.as_mut()) {
        view.demo_transfers();
    }
}

/// A recorded session on `host`: one command, its output and the next prompt.
pub(super) fn recorded(host: &str, command: &str, output: &[&str]) -> annulus_term::Emulator {
    let prompt = format!("\x1b[1;32mdeploy@{host}\x1b[0m:\x1b[1;34m~\x1b[0m$ ");
    let mut text = format!("{prompt}{command}\r\n");
    for line in output {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.push_str(&prompt);
    let mut emulator = annulus_term::Emulator::new(80, 24);
    emulator.advance(text.as_bytes());
    emulator
}

/// A tab of four recorded panes, two by two: prod-db with the keyboard and cache-eu-1 under
/// it, web-1 and bastion on the right. Leaf order 1, 3, 2, 4.
pub(super) fn split_tab(app: &mut NativeApp) {
    app.push_terminal(
        "prod-db",
        TermView::Live(Box::new(split_panes_fixture(true))),
    );
}

/// [`split_tab`]'s panes, `with_news` of their own or all quiet.
pub(super) fn split_panes_fixture(with_news: bool) -> crate::native::panes::Panes {
    use crate::native::panes::{Panes, Replay};
    let pane = |title: &str, emulator: annulus_term::Emulator, status: Option<TabStatus>| Replay {
        title: title.into(),
        emulator,
        status: status.filter(|_| with_news),
    };
    let mut panes = Panes::replay(pane("prod-db", demo::terminal(), None));
    panes.split_replay(
        true,
        pane(
            "web-1",
            recorded(
                "web-1",
                "sudo nginx -t",
                &["nginx: configuration syntax is ok"],
            ),
            Some(TabStatus::Unread),
        ),
    );
    panes.focused = 1;
    panes.split_replay(
        false,
        pane(
            "cache-eu-1",
            recorded("cache-eu-1", "redis-cli ping", &["PONG"]),
            Some(TabStatus::Input),
        ),
    );
    panes.focused = 2;
    panes.split_replay(
        false,
        pane(
            "bastion",
            recorded(
                "bastion",
                "uptime",
                &[" 09:12:44 up 2 days,  load average: 0.08"],
            ),
            Some(TabStatus::Error),
        ),
    );
    panes.focused = 1;
    panes
}

pub(super) fn terminal_fonts(app: &mut NativeApp) {
    settings(app);
    app.settings_view.page = crate::native::settings::Page::Terminal;
    app.settings.terminal_font = Some("Fira Mono".into());
}

pub(super) fn app_fonts(app: &mut NativeApp) {
    settings(app);
    app.settings_view.page = crate::native::settings::Page::Appearance;
    app.settings.ui_font = Some("Fira Sans".into());
}

pub(super) fn theme_browser(app: &mut NativeApp) {
    scheme_light(app);
    app.section = Section::Hosts;
    app.settings.app_scheme = None;
    app.settings.theme = ThemeMode::System;
    app.settings_view.picker = Some(crate::native::theme_picker::ThemePicker::new(&app.settings));
}

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "hosts-empty",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: nothing,
        acts: &[],
    },
    // An empty vault with New host open.
    Scene {
        name: "host-editor-new",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: nothing,
        acts: &[Act::Click("New host")],
    },
    Scene {
        name: "hosts-grid",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: with_tabs,
        acts: &[Act::Hover("prod-db")],
    },
    Scene {
        name: "hosts-list",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: list_mode,
        acts: &[Act::Hover("cache-eu-1")],
    },
    Scene {
        name: "host-editor-edit",
        width: 1280,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: edit_prod_db,
        acts: &[],
    },
    Scene {
        // Long notes wrap inside their box, a long link broken where it must be.
        name: "host-editor-notes",
        width: 1280,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: edit_prod_db_notes,
        acts: &[],
    },
    Scene {
        name: "hosts-search",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Click("Search hosts, or type user@host:port to connect"),
            Act::Type("db"),
        ],
    },
    Scene {
        name: "hosts-new-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("More ways to add hosts")],
    },
    // New host and its menu are one control: only the half under the pointer lights up.
    Scene {
        name: "hosts-new-menu-hover",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("More ways to add hosts")],
    },
    // From the search, Tab reaches New host and then its menu half, each with its own ring.
    Scene {
        name: "hosts-new-menu-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Click("Search hosts, or type user@host:port to connect"),
            Act::Press(Key::Tab, Modifiers::NONE),
            Act::Press(Key::Tab, Modifiers::NONE),
        ],
    },
    // Beside the editor the actions row is too narrow for its labels: icons only.
    Scene {
        name: "hosts-toolbar-compact",
        width: 1040,
        height: 680,
        scale: 1.0,
        populated: true,
        setup: edit_prod_db,
        acts: &[],
    },
    Scene {
        name: "hosts-context-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("prod-db"), Act::RightClick("prod-db")],
    },
    Scene {
        name: "hosts-list-context-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: list_mode,
        acts: &[Act::Hover("grafana"), Act::RightClick("grafana")],
    },
    // The New Tab page: recent connections, then the newest saved hosts filling the page.
    Scene {
        name: "new-tab",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: new_tab,
        acts: &[],
    },
    Scene {
        name: "new-tab-search",
        width: 1100,
        height: 720,
        scale: 1.25,
        populated: true,
        setup: new_tab,
        acts: &[Act::Type("api"), Act::Press(Key::Down, Modifiers::NONE)],
    },
    Scene {
        name: "new-tab-quick-connect",
        width: 900,
        height: 600,
        scale: 1.0,
        populated: true,
        setup: new_tab,
        acts: &[Act::Type("root@10.9.8.7")],
    },
    Scene {
        name: "new-tab-pinned",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: new_tab_pinned,
        acts: &[],
    },
    Scene {
        name: "new-tab-empty",
        width: 900,
        height: 600,
        scale: 1.0,
        populated: false,
        setup: |app| app.open_new_tab(),
        acts: &[],
    },
    // Transfers: long names keep both ends, deep folders keep their last parts.
    Scene {
        name: "sftp-transfers",
        width: 1200,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: sftp_transfers,
        acts: &[],
    },
    Scene {
        name: "sftp-transfers-narrow",
        width: 860,
        height: 640,
        scale: 1.25,
        populated: true,
        setup: sftp_transfers,
        acts: &[Act::Hover("Retry")],
    },
    Scene {
        name: "sftp-transfers-folded",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp_transfers,
        acts: &[Act::Click("Hide transfers")],
    },
    // An SSH tab connecting: its steps, the one under way spinning, and its log.
    Scene {
        name: "terminal-connecting",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: connecting,
        acts: &[],
    },
    Scene {
        name: "terminal-connecting-narrow",
        width: 640,
        height: 560,
        scale: 1.5,
        populated: true,
        setup: connecting,
        acts: &[Act::Click("Hide log")],
    },
    // The SFTP tab beside the vault picks a host.
    Scene {
        name: "sftp-picker",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp_picker,
        acts: &[Act::Hover("nas")],
    },
    // On a narrow card the hover buttons squeezed the name and address.
    // Now only Edit shows, in room kept for it, and the tags stay.
    Scene {
        name: "hosts-card-hover-narrow",
        width: 760,
        height: 560,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("cache-eu-1")],
    },
    Scene {
        name: "hosts-card-selected",
        width: 1100,
        height: 720,
        scale: 1.25,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("nas")],
    },
    // A group opened: the breadcrumb, and Connect all for the hosts directly in it.
    Scene {
        name: "hosts-group",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Production")],
    },
    // The tag filter with a tag chosen: the hosts of every group that have it.
    Scene {
        name: "hosts-tag-filter",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Tags"), Act::Click("nginx (2)")],
    },
    // A typed target that no host matches, offered to be saved as a host.
    Scene {
        name: "hosts-save-target",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Click("Search hosts, or type user@host:port to connect"),
            Act::Type("deploy@10.0.9.9:2200"),
        ],
    },
    Scene {
        name: "tooltip",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("New tab")],
    },
    Scene {
        name: "hosts-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Press(Key::Tab, Modifiers::NONE),
            Act::Press(Key::Tab, Modifiers::NONE),
            Act::Press(Key::Tab, Modifiers::NONE),
        ],
    },
    Scene {
        name: "keychain-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "snippets-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: snippets,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "forwards-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "known-hosts-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: known_hosts,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "logs-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: logs,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "sftp-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "settings-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Press(Key::Tab, Modifiers::NONE); 11],
    },
    Scene {
        name: "lock-keyboard-focus",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: locked,
        acts: &[Act::Type("pw"), Act::Press(Key::Tab, Modifiers::NONE)],
    },
    Scene {
        name: "hosts-narrow",
        width: 640,
        height: 420,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[],
    },
    Scene {
        name: "keychain",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[],
    },
    // A saved key open to edit, its private key as dots, then shown on asking.
    Scene {
        name: "keychain-edit",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Click("Work laptop")],
    },
    Scene {
        name: "keychain-edit-shown",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Click("Work laptop"), Act::Click("Show private key")],
    },
    Scene {
        name: "keychain-import",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[
            Act::Click("Import key"),
            Act::Click("Name, e.g. id_ed25519"),
            Act::Type("deploy key"),
            Act::Click("Paste the private key (-----BEGIN \u{2026} PRIVATE KEY-----)"),
            Act::Type(
                "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\n-----END OPENSSH PRIVATE KEY-----",
            ),
        ],
    },
    Scene {
        name: "keychain-install",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Click("Install on a host"), Act::Click("Choose a host")],
    },
    Scene {
        name: "keychain-delete",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Click("Delete")],
    },
    Scene {
        // A key a host logs in with: its servers are named, "take it off" ticked.
        name: "keychain-delete-from-servers",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain_in_use,
        acts: &[Act::Click("Delete")],
    },
    Scene {
        name: "snippets",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: snippets,
        acts: &[],
    },
    Scene {
        name: "snippets-new",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: snippets,
        acts: &[
            Act::Click("New snippet"),
            Act::Click("sudo systemctl restart nginx"),
            Act::Type("cd /srv/app"),
            Act::Press(Key::Enter, Modifiers::NONE),
            Act::Type("git pull --ff-only"),
            Act::Press(Key::Enter, Modifiers::NONE),
            Act::Type("sudo systemctl restart app"),
        ],
    },
    Scene {
        name: "forwards",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards,
        acts: &[],
    },
    Scene {
        // A rule open to the network carries the warning on its row.
        name: "forwards-open-to-network",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards_open,
        acts: &[],
    },
    Scene {
        name: "forwards-new",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards,
        acts: &[Act::Click("New rule")],
    },
    Scene {
        name: "forwards-new-small",
        width: 640,
        height: 420,
        scale: 1.0,
        populated: true,
        setup: forwards,
        acts: &[Act::Click("New rule")],
    },
    Scene {
        // No rules yet: the wizard's first step, a diagram per kind, is the screen. The
        // photo is taken a second in, with the packets on their way.
        name: "forwards-wizard",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards_empty,
        acts: &[],
    },
    Scene {
        name: "forwards-wizard-ports",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards_empty,
        acts: &[
            Act::Click("Reach a service behind a server"),
            Act::Click("Port on this computer"),
            Act::Type("15432"),
            Act::Click("Destination, as the server sees it: host"),
            Act::Type("db.internal"),
            Act::Click("Destination, as the server sees it: port"),
            Act::Type("5432"),
        ],
    },
    Scene {
        name: "forwards-wizard-review",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards_empty,
        acts: &[
            Act::Click("Share a service from this computer"),
            Act::Click("Port on the server"),
            Act::Type("9000"),
            Act::Click("Service on this computer: port"),
            Act::Type("3000"),
            Act::Click("Next"),
        ],
    },
    Scene {
        name: "forwards-wizard-remote-small",
        width: 640,
        height: 520,
        scale: 1.0,
        populated: true,
        setup: forwards_empty,
        acts: &[Act::Click("Share a service from this computer")],
    },
    Scene {
        name: "known-hosts",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: known_hosts,
        acts: &[],
    },
    Scene {
        name: "logs",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: logs,
        acts: &[],
    },
    Scene {
        name: "sftp",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Hover("deploy.sh")],
    },
    Scene {
        name: "sftp-drag",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Hold("todo.txt"), Act::Hover("README.md")],
    },
    Scene {
        name: "sftp-filtered",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[
            Act::Click("Filter this folder"),
            Act::Type("nothing-like-this"),
        ],
    },
    Scene {
        name: "sftp-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Click("deploy.sh"), Act::Click("More actions")],
    },
    Scene {
        name: "sftp-wide",
        width: 1680,
        height: 820,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Hover("app.log")],
    },
    Scene {
        name: "sftp-drag-down",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Hold("deploy.sh"), Act::Hover("todo.txt")],
    },
    Scene {
        name: "sftp-clash",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp_clash,
        acts: &[],
    },
    Scene {
        name: "sftp-columns-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::RightClick("Sort by modified")],
    },
    Scene {
        name: "sftp-local-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::RightClick("todo.txt")],
    },
    Scene {
        name: "sftp-delete",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[
            Act::Click("deploy.sh"),
            Act::Press(Key::Delete, Modifiers::NONE),
        ],
    },
    Scene {
        name: "sftp-permissions",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[
            Act::Click("deploy.sh"),
            Act::Click("More actions"),
            Act::Click("Permissions\u{2026}"),
        ],
    },
    Scene {
        name: "sftp-properties",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[
            Act::Click("app.log"),
            Act::Click("More actions"),
            Act::Click("Properties"),
        ],
    },
    Scene {
        name: "settings",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[],
    },
    Scene {
        name: "scheme-light-settings",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: scheme_light,
        acts: &[],
    },
    Scene {
        // Theme browser search, family grouping and a live preview.
        name: "scheme-menu-search",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: scheme_light,
        acts: &[
            Act::Click("Solarized Light"),
            Act::Click("All"),
            Act::Type("gruvbox"),
            Act::Hover("Gruvbox dark, hard"),
            Act::Hover("Gruvbox dark, hard"),
        ],
    },
    Scene {
        name: "font-settings-terminal",
        width: 1100,
        height: 980,
        scale: 1.0,
        populated: true,
        setup: terminal_fonts,
        acts: &[],
    },
    Scene {
        name: "font-settings-app",
        width: 1100,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: app_fonts,
        acts: &[],
    },
    Scene {
        name: "theme-picker-families",
        width: 1200,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: theme_browser,
        acts: &[],
    },
    Scene {
        name: "theme-picker-hidpi",
        width: 1000,
        height: 700,
        scale: 2.0,
        populated: true,
        setup: theme_browser,
        acts: &[],
    },
    Scene {
        name: "theme-picker-empty",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: scheme_light,
        acts: &[Act::Click("Solarized Light"), Act::Type("no-such-theme")],
    },
    Scene {
        name: "theme-picker-compact",
        width: 800,
        height: 600,
        scale: 1.0,
        populated: true,
        setup: theme_browser,
        acts: &[],
    },
    Scene {
        name: "scheme-dark-terminal",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: scheme_dark,
        acts: &[],
    },
    Scene {
        name: "settings-terminal",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Terminal")],
    },
    // The vault's lock and the shared switch that opens the agent door.
    Scene {
        name: "settings-security",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Security")],
    },
    // Agent access, approval, tab behavior and the skill installer.
    Scene {
        name: "settings-agents",
        width: 1100,
        height: 900,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Agents")],
    },
    // With agents allowed to close their tabs, "close when its shell ends" shows.
    Scene {
        name: "settings-agents-may-close",
        width: 1100,
        height: 960,
        scale: 1.0,
        populated: true,
        setup: settings_agents_may_close,
        acts: &[Act::Click("Agents")],
    },
    Scene {
        name: "settings-shortcuts",
        width: 1100,
        height: 900,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Shortcuts")],
    },
    Scene {
        name: "settings-shortcuts-search",
        width: 1000,
        height: 640,
        scale: 1.25,
        populated: true,
        setup: settings,
        acts: &[
            Act::Click("Shortcuts"),
            Act::Click("Search shortcuts"),
            Act::Type("tab"),
        ],
    },
    Scene {
        name: "settings-shortcuts-recording",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[
            Act::Click("Shortcuts"),
            Act::Click("Change the keys of New tab"),
            Act::Shortcut(shortcuts::PALETTE),
        ],
    },
    Scene {
        name: "settings-shortcuts-changed",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[
            Act::Click("Shortcuts"),
            Act::Click("Change the keys of New tab"),
            Act::Press(
                Key::Char('n'),
                Modifiers {
                    ctrl: true,
                    alt: true,
                    ..Modifiers::NONE
                },
            ),
        ],
    },
    Scene {
        name: "settings-sessions-narrow",
        width: 820,
        height: 600,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Sessions")],
    },
    Scene {
        name: "settings-about",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("About")],
    },
    Scene {
        name: "settings-account",
        width: 1100,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: settings_account,
        acts: &[],
    },
    Scene {
        name: "settings-account-certificate",
        width: 1100,
        height: 900,
        scale: 1.0,
        populated: true,
        setup: settings_account_certificate,
        acts: &[],
    },
    Scene {
        name: "command-palette",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: with_tabs,
        acts: &[Act::Shortcut(shortcuts::PALETTE), Act::Type("db")],
    },
    Scene {
        name: "lock",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: locked,
        acts: &[],
    },
    Scene {
        name: "macos-title-bar",
        width: 1100,
        height: 720,
        scale: 2.0,
        populated: true,
        setup: macos_title_bar,
        acts: &[],
    },
    Scene {
        name: "lock-setup-error",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: first_run,
        acts: &[
            Act::Click("Master password (10+ characters)"),
            Act::Type("short"),
            Act::Click("Type it again"),
            Act::Type("short"),
            Act::Click("Create vault"),
        ],
    },
    Scene {
        name: "lock-setup-strength",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: false,
        setup: first_run,
        acts: &[
            Act::Click("Master password (10+ characters)"),
            Act::Type("Summer2024!!"),
        ],
    },
    Scene {
        name: "agent-tab",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: agent_tab,
        acts: &[],
    },
    Scene {
        name: "agent-tab-folded",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: agent_tab_folded,
        acts: &[],
    },
    Scene {
        name: "agent-command-approval",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: agent_command_approval,
        acts: &[],
    },
    Scene {
        name: "agent-transfer-approval",
        width: 1300,
        height: 800,
        scale: 1.0,
        populated: true,
        setup: NativeApp::preview_transfer,
        acts: &[],
    },
    Scene {
        name: "agent-tab-stopped",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: agent_tab_stopped,
        acts: &[],
    },
    Scene {
        name: "terminal",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: terminal_tab,
        acts: &[],
    },
    Scene {
        name: "terminal-hidpi",
        width: 1100,
        height: 720,
        scale: 1.5,
        populated: true,
        setup: terminal_tab,
        acts: &[],
    },
    // Four panes typing together, cache-eu-1 left out; every pane shows its dot.
    Scene {
        name: "terminal-broadcast",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: split_tab,
        acts: &[
            Act::Click("Broadcast"),
            Act::Click("Broadcast to cache-eu-1"),
        ],
    },
    // Focus mode: the failed pane large, the list of all four beside it.
    Scene {
        name: "terminal-focus-mode",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: split_tab,
        acts: &[
            Act::Click("Focus mode"),
            Act::Click("bastion, Disconnected or failed"),
        ],
    },
    Scene {
        name: "hosts-hidpi",
        width: 1100,
        height: 720,
        scale: 1.5,
        populated: true,
        setup: with_tabs,
        acts: &[],
    },
    Scene {
        name: "host-editor-small",
        width: 640,
        height: 420,
        scale: 1.0,
        populated: true,
        setup: edit_prod_db,
        acts: &[],
    },
    Scene {
        name: "hosts-wide-editor",
        width: 1920,
        height: 1080,
        scale: 1.0,
        populated: true,
        setup: edit_prod_db,
        acts: &[],
    },
];
