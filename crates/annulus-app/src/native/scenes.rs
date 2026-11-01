//! The scene catalogue ([ADR-0017] decision 6): every screen and state, drawn through the CPU
//! renderer at several sizes, scales and both themes. Each scene drives the UI with real
//! input (hover, click, type) found by the accessible names of the zones, so what is
//! photographed is what a user would see.
//!
//! The PNGs land in `target/tmp/scenes/`, which CI uploads with every run for review. Every
//! scene also fails on layout faults: a button or heading cut off, controls that partly
//! overlap, or a control outside the window.
//!
//! [ADR-0017]: ../../../../docs/adr/0017-native-ui.md

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use annulus_config::settings::{KeyBinding, OpenTab};
use annulus_config::tokens::Appearance;
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_session::{Password, Program};
use annulus_ui::gfx::{Canvas, Point, Rect};
use annulus_ui::kit::{Button, Event, FrameOutput, Key, Modifiers, Role, Theme, Ui, UiState, Zone};
use annulus_vault::hosts::Host;

use super::demo::fixture_vault;
use super::hosts::{HostsAction, Mode};
use super::{NativeApp, TermView};
use crate::sync_screen::Attempt;
use crate::terminal_view::TabStatus;

/// One step of a scene's script, applied between frames.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Move the pointer over the zone with this accessible name.
    Hover(&'static str),
    /// Click it.
    Click(&'static str),
    /// Type text into whatever has focus.
    Type(&'static str),
    /// Press a key.
    Press(Key, Modifiers),
    /// Press the primary button on the zone with this name and keep it down (a drag starts;
    /// a later `Hover` moves it).
    Hold(&'static str),
    /// Right-click it (a context menu).
    RightClick(&'static str),
}

struct Scene {
    name: &'static str,
    width: u32,
    height: u32,
    scale: f32,
    populated: bool,
    setup: fn(&mut NativeApp),
    acts: &'static [Act],
}

fn nothing(_: &mut NativeApp) {}

fn list_mode(app: &mut NativeApp) {
    app.hosts.mode = Mode::List;
}

fn edit_prod_db(app: &mut NativeApp) {
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

fn with_tabs(app: &mut NativeApp) {
    for (title, status) in [
        ("prod-db", None),
        ("web-1", Some(TabStatus::Unread)),
        ("bastion", Some(TabStatus::Error)),
    ] {
        app.push_terminal(title, TermView::Replay(Box::new(super::demo::terminal())));
        if let Some(tab) = app.tabs.last_mut() {
            tab.status = status;
        }
    }
    // The hosts grid, with the tabs above it.
    app.active = 0;
}

fn section(app: &mut NativeApp, section: super::Section) {
    app.ssh_files.known_hosts = super::demo::known_hosts("scenes");
    app.section = section;
}

fn keychain(app: &mut NativeApp) {
    section(app, super::Section::Keychain);
}

fn snippets(app: &mut NativeApp) {
    section(app, super::Section::Snippets);
}

fn forwards(app: &mut NativeApp) {
    section(app, super::Section::PortForwarding);
}

fn known_hosts(app: &mut NativeApp) {
    section(app, super::Section::KnownHosts);
}

fn logs(app: &mut NativeApp) {
    section(app, super::Section::Logs);
}

fn sftp(app: &mut NativeApp) {
    app.push_sftp(super::demo::sftp_tab("scenes"));
}

fn settings(app: &mut NativeApp) {
    section(app, super::Section::Settings);
}

fn settings_account(app: &mut NativeApp) {
    settings(app);
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    // Made up, so no photo shows the CI machine's name.
    "Work laptop".clone_into(&mut app.sync.device);
}

/// A LAN server's own certificate, waiting for the user to compare its fingerprint.
fn settings_account_certificate(app: &mut NativeApp) {
    settings_account(app);
    "https://10.1.1.20:8443".clone_into(&mut app.sync.server);
    "ana".clone_into(&mut app.sync.account);
    app.sync.untrusted = Some(((0..=255).collect(), Attempt::Create));
}

fn locked(app: &mut NativeApp) {
    app.vault = None;
}

fn first_run(app: &mut NativeApp) {
    app.vault = None;
    app.lock = super::lock::LockView::new(super::lock::Mode::Setup);
}

fn terminal_tab(app: &mut NativeApp) {
    with_tabs(app);
    app.active = super::FIXED_TABS;
}

/// An agent's tab (ADR-0020, F-AUTO-004): marked AI, with the commands it typed beside its
/// terminal: done, out of time, a long one wrapped, one picked and the newest still running.
fn agent_tab(app: &mut NativeApp) {
    use super::control::{AgentCommand, AgentLog, Ran};
    use std::time::{Duration, Instant};
    terminal_tab(app);
    let now = crate::sync_screen::now_secs();
    let tab = &mut app.tabs[super::FIXED_TABS];
    "deploy-check".clone_into(&mut tab.title);
    let command = |text: &str, ago: u64, line, state| AgentCommand {
        text: text.into(),
        at: now - ago,
        line,
        state,
        typed: super::control::Typed::default(),
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
fn agent_tab_stopped(app: &mut NativeApp) {
    use super::control::Ran;
    agent_tab(app);
    app.settings.automation = false;
    if let Some(log) = app.tabs[super::FIXED_TABS].agent.as_mut() {
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
fn agent_tab_folded(app: &mut NativeApp) {
    agent_tab(app);
    if let Some(log) = app.tabs[super::FIXED_TABS].agent.as_mut() {
        log.folded = true;
    }
}

/// The window and the terminal in two different dark community schemes.
fn scheme_dark(app: &mut NativeApp) {
    terminal_tab(app);
    app.settings.app_scheme = Some("Gruvbox Dark".into());
    "Tokyo Night Storm".clone_into(&mut app.settings.terminal_theme);
}

/// The Look settings in a light community scheme.
fn scheme_light(app: &mut NativeApp) {
    settings(app);
    app.settings.app_scheme = Some("Solarized Light".into());
}

/// A New Tab page after a few connections from this device.
fn new_tab(app: &mut NativeApp) {
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
        app.push_terminal("web-1", TermView::Replay(Box::new(super::demo::terminal())));
        if let Some(tab) = app.tabs.last_mut() {
            tab.origin = Some(OpenTab::Host(id));
        }
    }
    app.open_new_tab();
}

/// A New Tab page with two hosts pinned to its top.
fn new_tab_pinned(app: &mut NativeApp) {
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

fn sftp_picker(app: &mut NativeApp) {
    app.active = 1;
}

/// An SSH tab part-way through connecting, its log open.
fn connecting(app: &mut NativeApp) {
    let program = crate::connect::quick("deploy@web-1.example.com", &app.ssh_files);
    let mut terminal = super::live::LiveTerminal::held_at(
        program,
        1,
        &[
            "Reaching web-1.example.com:22 as deploy",
            "web-1.example.com's key is trusted",
        ],
    );
    terminal.identity = Some(("web-1".into(), super::os::find("ubuntu")));
    app.push_terminal(
        "web-1",
        TermView::Live(Box::new(super::panes::Panes::new(terminal))),
    );
}

/// An SFTP tab with transfers in every state: under way, done, skipped, failed.
fn sftp_transfers(app: &mut NativeApp) {
    sftp(app);
    if let Some(view) = app.tabs.last_mut().and_then(|tab| tab.sftp.as_mut()) {
        view.demo_transfers();
    }
}

/// A recorded session on `host`: one command, its output and the next prompt.
fn recorded(host: &str, command: &str, output: &[&str]) -> annulus_term::Emulator {
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
fn split_tab(app: &mut NativeApp) {
    use super::panes::{Panes, Replay};
    let pane = |title: &str, emulator: annulus_term::Emulator, status: Option<TabStatus>| Replay {
        title: title.into(),
        emulator,
        status,
    };
    let mut panes = Panes::replay(pane("prod-db", super::demo::terminal(), None));
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
    app.push_terminal("prod-db", TermView::Live(Box::new(panes)));
}

const SCENES: &[Scene] = &[
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
        // The scheme menu searched, its swatches, and the window previewing the one the
        // pointer rested on (the second hover is a second later).
        name: "scheme-menu-search",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: scheme_light,
        acts: &[
            Act::Click("Solarized Light"),
            Act::Type("gruvbox"),
            Act::Hover("Gruvbox dark, hard"),
            Act::Hover("Gruvbox dark, hard"),
        ],
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
    // The vault's lock and the Automation card with the agent settings (F-AUTO-006).
    Scene {
        name: "settings-security",
        width: 1100,
        height: 760,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Security")],
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
            Act::Press(Key::Char('k'), Modifiers::CTRL_SHIFT),
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
        acts: &[
            Act::Press(Key::Char('k'), Modifiers::CTRL_SHIFT),
            Act::Type("db"),
        ],
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

/// A renderer the scenes are drawn with: the CPU one everywhere, Direct2D on Windows.
trait Renderer {
    /// Its name in file names: empty for the CPU renderer.
    fn suffix(&self) -> &'static str;
    /// Runs `draw` on a fresh `width` x `height` pixel frame; returns the PNG when `keep`.
    fn frame(
        &mut self,
        size: (u32, u32, f32),
        keep: bool,
        draw: &mut dyn FnMut(&mut dyn Canvas),
    ) -> Option<Vec<u8>>;
}

struct Cpu(Option<TextEngine>);

impl Renderer for Cpu {
    fn suffix(&self) -> &'static str {
        ""
    }

    fn frame(
        &mut self,
        (width, height, scale): (u32, u32, f32),
        keep: bool,
        draw: &mut dyn FnMut(&mut dyn Canvas),
    ) -> Option<Vec<u8>> {
        let text = self.0.take().unwrap_or_default();
        let mut canvas = CpuCanvas::new(width, height, scale, text).unwrap();
        draw(&mut canvas);
        let png = keep.then(|| canvas.encode_png().unwrap());
        self.0 = Some(canvas.into_text_engine());
        png
    }
}

#[cfg(windows)]
struct Direct2D(annulus_platform::d2d::D2d);

#[cfg(windows)]
impl Renderer for Direct2D {
    fn suffix(&self) -> &'static str {
        "-d2d"
    }

    fn frame(
        &mut self,
        (width, height, scale): (u32, u32, f32),
        keep: bool,
        draw: &mut dyn FnMut(&mut dyn Canvas),
    ) -> Option<Vec<u8>> {
        let png = self
            .0
            .render_png(width, height, scale, |canvas| draw(canvas))
            .unwrap();
        keep.then_some(png)
    }
}

fn zone_center(zones: &[Zone], name: &str) -> Point {
    zones
        .iter()
        .rev()
        .find(|zone| zone.label == name)
        .map_or_else(
            || {
                let names: Vec<&str> = zones.iter().map(|zone| zone.label.as_str()).collect();
                panic!("no zone named {name:?}; zones: {names:?}")
            },
            |zone| zone.rect.center(),
        )
}

/// Places where Tab goes backwards in reading order: to a zone above and to the left
/// of the one before, or to its left on the same row. Moving up into a new column to the right
/// is fine (the content beside the navigation), and a backdrop starts a new layer (a menu).
fn tab_order_faults(zones: &[Zone]) -> Vec<String> {
    let mut faults = Vec::new();
    let mut last: Option<&Zone> = None;
    for zone in zones {
        if zone.role == Role::Backdrop {
            last = None;
            continue;
        }
        if !zone.role.focusable() {
            continue;
        }
        if let Some(before) = last {
            let (a, b) = (before.rect, zone.rect);
            let overlap = a.b().min(b.b()) - a.y.max(b.y);
            let same_row = overlap > 0.5 * a.h.min(b.h);
            let left = b.r() <= a.x + 0.5 || (same_row && b.center().x < a.center().x);
            let above = b.b() <= a.y + 0.5;
            if (same_row && left) || (above && b.x < a.x - 0.5) {
                faults.push(format!(
                    "Tab goes backwards from {:?} {:?} to {:?} {:?}",
                    before.label, a, zone.label, b
                ));
            }
        }
        last = Some(zone);
    }
    faults
}

/// Partial overlaps between interactive zones (one fully inside another is fine: a button
/// on a card), and controls outside the window. A zone drawn after a backdrop that covers it
/// (an open menu) sits above everything drawn before that backdrop, so those never clash.
fn layout_faults(zones: &[Zone], viewport: Rect) -> Vec<String> {
    let inside = |a: &Rect, b: &Rect| {
        a.x >= b.x - 0.5 && a.y >= b.y - 0.5 && a.r() <= b.r() + 0.5 && a.b() <= b.b() + 0.5
    };
    let interactive = |zone: &Zone| !matches!(zone.role, Role::Backdrop | Role::ScrollView);
    let mut faults = Vec::new();
    for (index, a) in zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| interactive(zone))
    {
        if !inside(&a.rect, &viewport) {
            faults.push(format!(
                "{:?} sticks out of the window: {:?}",
                a.label, a.rect
            ));
        }
        for (later, b) in zones.iter().enumerate().skip(index + 1) {
            if !interactive(b) {
                continue;
            }
            let layered = zones[index + 1..later]
                .iter()
                .any(|between| between.role == Role::Backdrop && inside(&b.rect, &between.rect));
            let shared = a.rect.intersect(&b.rect);
            if !layered
                && shared.w > 0.5
                && shared.h > 0.5
                && !inside(&a.rect, &b.rect)
                && !inside(&b.rect, &a.rect)
            {
                faults.push(format!(
                    "{:?} {:?} overlaps {:?} {:?}",
                    a.label, a.rect, b.label, b.rect
                ));
            }
        }
    }
    faults
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("tmp")
        .join("scenes")
}

/// Renders `scene` in `appearance` with `renderer`: returns the PNG and every fault found.
fn render(
    scene: &Scene,
    appearance: Appearance,
    renderer: &mut dyn Renderer,
) -> (Vec<u8>, Vec<String>) {
    let vault = fixture_vault(
        &format!("{}-{appearance:?}{}", scene.name, renderer.suffix()),
        scene.populated,
    );
    let mut app = NativeApp::new(Some(vault));
    (scene.setup)(&mut app);
    // A scene that picks a color scheme draws in it, like the window would, and so does one
    // whose scheme menu has the pointer on an entry.
    let system = Theme::new(appearance);
    let mut state = UiState::default();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // Scene sizes.
    let size = (
        (scene.width as f32 * scene.scale).round() as u32,
        (scene.height as f32 * scene.scale).round() as u32,
        scene.scale,
    );
    #[allow(clippy::cast_precision_loss)] // Scene sizes.
    let viewport = Rect::new(0.0, 0.0, scene.width as f32, scene.height as f32);
    let mut frame = |events: &[Event], time: f64, keep: bool| {
        let mut output = FrameOutput::default();
        let png = renderer.frame(size, keep, &mut |canvas| {
            state.begin_frame(events, time);
            let theme =
                super::colors::previewed_theme(&app.settings, app.settings_view.previewing, system);
            let mut ui = Ui::new(canvas, theme, &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        (output, png)
    };
    let mut time = 0.0;
    let mut events: Vec<Event> = Vec::new();
    let mut acts = scene.acts.iter();
    let mut faults = Vec::new();
    // One frame to lay out, one per act (resolved against the frame before), then frames
    // until animations settle.
    loop {
        let (mut output, _) = frame(&events, time, false);
        faults.extend(output.faults.iter().cloned());
        // The frame after input, as the window draws it (see `UiState::begin_frame`).
        if !events.is_empty() {
            (output, _) = frame(&[], time + 0.5, false);
            faults.extend(output.faults.iter().cloned());
        }
        events.clear();
        time += 1.0;
        match acts.next() {
            Some(Act::Hover(name)) => {
                events.push(Event::PointerMoved(zone_center(&output.zones, name)));
            }
            Some(Act::Click(name)) => {
                events.push(Event::PointerMoved(zone_center(&output.zones, name)));
                events.push(Event::PointerDown(Button::Primary));
                events.push(Event::PointerUp(Button::Primary));
            }
            Some(Act::Type(text)) => events.push(Event::Text((*text).to_owned())),
            Some(Act::Press(key, modifiers)) => events.push(Event::Key(*key, *modifiers)),
            Some(Act::Hold(name)) => {
                events.push(Event::PointerMoved(zone_center(&output.zones, name)));
                events.push(Event::PointerDown(Button::Primary));
            }
            Some(Act::RightClick(name)) => {
                events.push(Event::PointerMoved(zone_center(&output.zones, name)));
                events.push(Event::PointerDown(Button::Secondary));
                events.push(Event::PointerUp(Button::Secondary));
            }
            None => break,
        }
    }
    // Settle: hover fades and focus glows finish.
    let (output, png) = frame(&[], time + 1.0, true);
    faults.extend(output.faults.iter().cloned());
    // Once settled, nothing asks for frames (P-06): an animation that never ends would keep
    // the CPU busy while the window sits idle.
    let (idle, _) = frame(&[], time + 2.0, false);
    if idle.repaint {
        faults.push("still asks for frames two seconds after the last input".into());
    }
    faults.extend(layout_faults(&output.zones, viewport));
    faults.extend(tab_order_faults(&output.zones));
    // Two zones with one id would take each other's input and screen-reader node.
    let mut ids = std::collections::HashSet::new();
    for zone in &output.zones {
        if !ids.insert(zone.id) {
            faults.push(format!("{:?} shares its id with another zone", zone.label));
        }
    }
    // What a screen reader gets of the same frame.
    let tree = annulus_ui::access::tree(&output.zones, state.focus(), scene.scale, "Annulus");
    faults.extend(
        annulus_ui::access::faults(&tree)
            .into_iter()
            .map(|fault| format!("accessibility: {fault}")),
    );
    (png.unwrap(), faults)
}

/// The events of `act` on the frame whose zones are `zones`.
fn events_for(act: &Act, zones: &[Zone]) -> Vec<Event> {
    match act {
        Act::Hover(name) => vec![Event::PointerMoved(zone_center(zones, name))],
        Act::Click(name) => vec![
            Event::PointerMoved(zone_center(zones, name)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ],
        Act::Type(text) => vec![Event::Text((*text).to_owned())],
        Act::Press(key, modifiers) => vec![Event::Key(*key, *modifiers)],
        Act::Hold(name) => vec![
            Event::PointerMoved(zone_center(zones, name)),
            Event::PointerDown(Button::Primary),
        ],
        Act::RightClick(name) => vec![
            Event::PointerMoved(zone_center(zones, name)),
            Event::PointerDown(Button::Secondary),
            Event::PointerUp(Button::Secondary),
        ],
    }
}

/// Transitions to photograph mid-flight: the last act of each starts the motion.
const FILMS: &[Scene] = &[
    Scene {
        name: "motion-section",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Keychain")],
    },
    Scene {
        name: "motion-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("More ways to add hosts")],
    },
    Scene {
        name: "motion-new-tab",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: with_tabs,
        acts: &[Act::Click("New tab")],
    },
    Scene {
        name: "motion-editor",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("prod-db"), Act::Click("Edit")],
    },
    Scene {
        name: "motion-editor-close",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Hover("prod-db"),
            Act::Click("Edit"),
            Act::Press(Key::Escape, Modifiers::NONE),
        ],
    },
    Scene {
        name: "motion-settings",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[Act::Click("Terminal")],
    },
];

/// How long after the input each frame of a filmstrip is taken, in seconds.
const FILM_AT: [f64; 4] = [0.03, 0.07, 0.12, 0.3];

/// Frames of `film` as its last act's motion plays, taken [`FILM_AT`] after the input.
fn filmstrip(film: &Scene, renderer: &mut dyn Renderer) -> Vec<Vec<u8>> {
    let vault = fixture_vault(
        &format!("film-{}{}", film.name, renderer.suffix()),
        film.populated,
    );
    let mut app = NativeApp::new(Some(vault));
    (film.setup)(&mut app);
    let theme = Theme::new(Appearance::Dark);
    let mut state = UiState::default();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // Scene sizes.
    let size = (
        (film.width as f32 * film.scale).round() as u32,
        (film.height as f32 * film.scale).round() as u32,
        film.scale,
    );
    #[allow(clippy::cast_precision_loss)] // Scene sizes.
    let viewport = Rect::new(0.0, 0.0, film.width as f32, film.height as f32);
    let mut frame = |events: &[Event], time: f64, keep: bool| {
        let mut output = FrameOutput::default();
        let png = renderer.frame(size, keep, &mut |canvas| {
            state.begin_frame(events, time);
            let mut ui = Ui::new(canvas, theme, &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        (output, png)
    };
    let mut time = 0.0;
    let (mut output, _) = frame(&[], time, false);
    let mut frames = Vec::new();
    for (index, act) in film.acts.iter().enumerate() {
        time += 1.0;
        let events = events_for(act, &output.zones);
        (output, _) = frame(&events, time, false);
        if index + 1 == film.acts.len() {
            for at in FILM_AT {
                let (_, png) = frame(&[], time + at, true);
                frames.push(png.unwrap());
            }
        } else {
            (output, _) = frame(&[], time + 0.5, false);
        }
    }
    frames
}

#[test]
fn transitions_move_through_frames_and_are_photographed_mid_flight() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderer = Cpu(None);
    let mut still = Vec::new();
    for film in FILMS {
        let frames = filmstrip(film, &mut renderer);
        for (index, png) in frames.iter().enumerate() {
            std::fs::write(dir.join(format!("{}-{index}.png", film.name)), png).unwrap();
        }
        // Moving: the first frames differ from each other, and the last is where it settles.
        if frames[0] == frames[1] || frames[1] == frames[2] {
            still.push(film.name);
        }
    }
    assert!(still.is_empty(), "these did not move: {still:?}");
}

/// The renderers every scene is drawn with: ours, and Direct2D on Windows. Each thread makes
/// its own (Direct2D's factory is single-threaded).
fn renderers() -> Vec<Box<dyn Renderer>> {
    vec![
        Box::new(Cpu(None)),
        #[cfg(windows)]
        Box::new(Direct2D(annulus_platform::d2d::D2d::new().unwrap())),
    ]
}

/// How many threads a heavy test spreads its independent work over: every core, but no more
/// than 8, since the CI runner runs other jobs beside this one.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8))
}

#[test]
fn every_scene_renders_without_layout_faults() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    // `ANNULUS_SCENES=forwards` renders only the scenes whose names contain it.
    let only = std::env::var("ANNULUS_SCENES").ok();
    let jobs: Vec<(&Scene, Appearance, usize)> = (0..renderers().len())
        .flat_map(|renderer| {
            SCENES
                .iter()
                .filter(|scene| only.as_deref().is_none_or(|part| scene.name.contains(part)))
                .flat_map(move |scene| {
                    [Appearance::Dark, Appearance::Light].map(|look| (scene, look, renderer))
                })
        })
        .collect();
    // Each render has its own vault (named by scene, look and renderer), so they run side by
    // side: thread n takes every n-th one, which spreads the heavy scenes out.
    let threads = workers();
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let (jobs, dir) = (&jobs, &dir);
                scope.spawn(move || {
                    let mut renderers = renderers();
                    let mut failures = Vec::new();
                    for &(scene, appearance, index) in jobs.iter().skip(first).step_by(threads) {
                        let renderer = renderers[index].as_mut();
                        let (png, faults) = render(scene, appearance, renderer);
                        let theme = match appearance {
                            Appearance::Dark => "dark",
                            Appearance::Light => "light",
                        };
                        let name = format!("{}-{theme}{}", scene.name, renderer.suffix());
                        std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
                        failures.extend(faults.into_iter().map(|fault| format!("{name}: {fault}")));
                    }
                    failures
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    assert!(
        failures.is_empty(),
        "layout faults:
{}",
        failures.join(
            "
"
        )
    );
}

/// Draws one frame of `app` with input `events`; returns what the toolkit reported.
fn drive(app: &mut NativeApp, state: &mut UiState, events: &[Event], time: f64) -> FrameOutput {
    let mut renderer = Cpu(None);
    let viewport = Rect::new(0.0, 0.0, 1100.0, 800.0);
    let mut output = FrameOutput::default();
    renderer.frame((1100, 800, 1.0), false, &mut |canvas| {
        state.begin_frame(events, time);
        let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), state, viewport);
        app.frame(&mut ui);
        output = state.end_frame();
    });
    output
}

/// Draws a frame with `events`, then the frame after input that the window draws too.
fn step(app: &mut NativeApp, state: &mut UiState, events: &[Event], time: f64) -> FrameOutput {
    drive(app, state, events, time);
    drive(app, state, &[], time)
}

/// Lays out with the real fonts but draws no pixels: the random-input test runs thousands of
/// frames, and only what the toolkit decides matters there.
struct Measure(TextEngine);

impl Canvas for Measure {
    fn scale(&self) -> f32 {
        1.0
    }
    fn fill_rect(&mut self, _: Rect, _: f32, _: annulus_ui::gfx::Color) {}
    fn stroke_rect(&mut self, _: Rect, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn shadow(&mut self, _: Rect, _: f32, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn fill_circle(&mut self, _: Point, _: f32, _: annulus_ui::gfx::Color) {}
    fn polyline(&mut self, _: &[Point], _: bool, _: f32, _: annulus_ui::gfx::Color) {}
    fn stroke_circle(&mut self, _: Point, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn measure(&mut self, text: &str, font: annulus_ui::gfx::Font) -> annulus_ui::gfx::TextSize {
        self.0.measure(text, font)
    }
    fn draw_text(
        &mut self,
        _: Point,
        _: &str,
        _: annulus_ui::gfx::Font,
        _: annulus_ui::gfx::Color,
    ) {
    }
    fn push_clip(&mut self, _: Rect) {}
    fn pop_clip(&mut self) {}
}

/// A seeded generator (xorshift64*), so a failing run replays from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        #[allow(clippy::cast_possible_truncation)] // A list index.
        let at = (self.next() % n.max(1) as u64) as usize;
        at
    }
}

/// What the random user types: plain words, addresses, accents, other scripts, emoji, a
/// combining mark, spaces, a very long name and several lines.
const TYPED: &[&str] = &[
    "a",
    "web-1",
    "root@10.0.4.12:2222",
    "\u{c4}rger.txt",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{1f642}",
    "e\u{301}",
    " ",
    "--",
    "a-very-long-host-name-that-goes-on-and-on-and-on.internal.example.com",
    "644",
    "line one\nline two",
];

/// Controls the random user leaves alone: they reach the network or other machines, write
/// outside the test's folders, or lock the vault for the rest of the run.
const HANDS_OFF: &[&str] = &[
    "Create account",
    "Sign in",
    "Sync now",
    "Export to Downloads",
    "Lock",
    "Install",
    "Run this rule",
    "Download to this computer",
    "Upload to the host",
];

/// One random action against the last frame's zones; also says what it was, for the report.
fn random_action(rng: &mut Rng, zones: &[Zone]) -> (Vec<Event>, String) {
    let targets: Vec<&Zone> = zones
        .iter()
        .filter(|zone| zone.role != Role::WindowButton && zone.rect.w > 1.0)
        .filter(|zone| !HANDS_OFF.iter().any(|name| zone.label.starts_with(name)))
        .collect();
    let click = |zone: &Zone, twice: bool| {
        let at = zone.rect.center();
        let mut events = vec![Event::PointerMoved(at)];
        for _ in 0..if twice { 2 } else { 1 } {
            events.push(Event::PointerDown(Button::Primary));
            events.push(Event::PointerUp(Button::Primary));
        }
        events
    };
    let ctrl_shift = |c| Event::Key(Key::Char(c), Modifiers::CTRL_SHIFT);
    match rng.below(24) {
        0..=9 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (click(zone, false), format!("click {:?}", zone.label))
        }
        10 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (click(zone, true), format!("double-click {:?}", zone.label))
        }
        11..=13 => {
            let text = TYPED[rng.below(TYPED.len())];
            (vec![Event::Text(text.to_owned())], format!("type {text:?}"))
        }
        14 => {
            let text = TYPED[rng.below(TYPED.len())];
            (
                vec![Event::Paste(text.to_owned())],
                format!("paste {text:?}"),
            )
        }
        15 => (vec![Event::Key(Key::Tab, Modifiers::NONE)], "Tab".into()),
        16 => (
            vec![Event::Key(Key::Tab, Modifiers::SHIFT)],
            "Shift+Tab".into(),
        ),
        17 => (
            vec![Event::Key(Key::Escape, Modifiers::NONE)],
            "Escape".into(),
        ),
        18 => (
            vec![Event::Key(Key::Enter, Modifiers::NONE)],
            "Enter".into(),
        ),
        19 => {
            let key = [Key::Backspace, Key::Delete, Key::F(2)][rng.below(3)];
            (vec![Event::Key(key, Modifiers::NONE)], format!("{key:?}"))
        }
        20 => {
            let key = [
                Key::Left,
                Key::Right,
                Key::Up,
                Key::Down,
                Key::Home,
                Key::End,
            ][rng.below(6)];
            let modifiers = [Modifiers::NONE, Modifiers::SHIFT, Modifiers::CTRL][rng.below(3)];
            (
                vec![Event::Key(key, modifiers)],
                format!("{key:?} {modifiers:?}"),
            )
        }
        21 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (
                vec![
                    Event::PointerMoved(zone.rect.center()),
                    Event::Wheel { x: 0.0, y: -120.0 },
                ],
                format!("scroll over {:?}", zone.label),
            )
        }
        _ => {
            let c = ['k', 't', 'w', 'f', 'm', 'b', 'd', 'e', '.'][rng.below(9)];
            (vec![ctrl_shift(c)], format!("Ctrl+Shift+{c}"))
        }
    }
}

/// One frame of `app` on `canvas` in a window of `viewport`.
fn measured_frame(
    canvas: &mut Measure,
    state: &mut UiState,
    app: &mut NativeApp,
    (events, time): (&[Event], f64),
    viewport: Rect,
) -> FrameOutput {
    state.begin_frame(events, time);
    let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), state, viewport);
    app.frame(&mut ui);
    state.end_frame()
}

/// Drives the whole app with `steps` random actions from `seed`: every frame must be
/// free of panics, layout faults, accessibility-tree faults and shared zone ids. New terminal
/// and SFTP tabs are closed before they first draw, so nothing connects anywhere.
fn random_user(seed: u64, steps: usize, (width, height): (f32, f32)) {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut vault = fixture_vault(&format!("monkey-{seed}"), true);
    // Rules that start by themselves would connect to the made-up hosts.
    for rule in vault.forwards().to_vec() {
        let rule = annulus_vault::forwards::PortForward {
            auto_start: false,
            ..rule
        };
        vault.upsert_forward(rule).unwrap();
    }
    let mut app = NativeApp::new(Some(vault));
    app.ssh_files.known_hosts = super::demo::known_hosts(&format!("monkey-{seed}"));
    app.ssh_files.key_dir = None;
    app.settings.auto_lock_minutes = 0;
    app.push_sftp(super::demo::sftp_tab(&format!("monkey-{seed}")));
    app.push_terminal(
        "replay",
        TermView::Replay(Box::new(super::demo::terminal())),
    );
    app.active = 0;
    let keep: Vec<u64> = app.tabs.iter().map(|tab| tab.uid).collect();
    let viewport = Rect::new(0.0, 0.0, width, height);
    let mut canvas = Measure(TextEngine::new());
    let mut state = UiState::default();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut zones: Vec<Zone> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    // What the run reached, printed at the end so a narrow run shows.
    let mut reached: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut sections = std::collections::BTreeSet::new();
    for step in 0..steps {
        let (events, what) = random_action(&mut rng, &zones);
        history.push(what);
        if history.len() > 16 {
            history.remove(0);
        }
        #[allow(clippy::cast_precision_loss)] // Step counts are small.
        let time = step as f64 * 0.25;
        let frames = catch_unwind(AssertUnwindSafe(|| {
            let first =
                measured_frame(&mut canvas, &mut state, &mut app, (&events, time), viewport);
            // Nothing new connects: terminal and SFTP tabs opened by the step go before they draw.
            app.tabs
                .retain(|tab| keep.contains(&tab.uid) || tab.kind == super::TabKind::Vault);
            app.active = app.active.min(app.tabs.len().saturating_sub(1));
            let second = measured_frame(
                &mut canvas,
                &mut state,
                &mut app,
                (&[], time + 0.1),
                viewport,
            );
            (first, second)
        }));
        let Ok((first, second)) = frames else {
            panic!("seed {seed}, step {step}: a panic after {history:#?}");
        };
        let mut faults = first.faults;
        faults.extend(second.faults);
        faults.extend(layout_faults(&second.zones, viewport));
        let tree = annulus_ui::access::tree(&second.zones, state.focus(), 1.0, "Annulus");
        faults.extend(annulus_ui::access::faults(&tree));
        let mut ids = std::collections::HashSet::new();
        for zone in &second.zones {
            if !ids.insert(zone.id) {
                faults.push(format!("{:?} shares its id", zone.label));
            }
        }
        assert!(
            faults.is_empty(),
            "seed {seed}, step {step}: {faults:#?}\nafter: {history:#?}"
        );
        reached.extend(second.zones.iter().map(|zone| zone.label.clone()));
        sections.insert(format!("{:?}", app.section));
        zones = second.zones;
    }
    eprintln!(
        "seed {seed}: {steps} steps, {} distinct controls, sections {sections:?}",
        reached.len()
    );
}

/// A few thousand random steps on every push; `ANNULUS_MONKEY_STEPS` and `ANNULUS_MONKEY_SEED`
/// make the nightly run longer and replay a failure.
#[test]
fn random_input_finds_no_panic_or_fault() {
    let steps = std::env::var("ANNULUS_MONKEY_STEPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    let seeds: Vec<u64> = match std::env::var("ANNULUS_MONKEY_SEED") {
        Ok(seed) => vec![seed.parse().expect("ANNULUS_MONKEY_SEED is a number")],
        Err(_) => vec![1, 2, 3],
    };
    // Each run has its own app and vault: they run side by side.
    std::thread::scope(|scope| {
        for seed in seeds {
            scope.spawn(move || random_user(seed, steps, (1100.0, 760.0)));
            // The smallest window the app allows, where layouts break first.
            scope.spawn(move || random_user(seed + 1000, steps / 2, (640.0, 420.0)));
        }
    });
}

/// A click on the zone named `name` that lies inside the zone named `within` (a button of
/// one list row among many with the same buttons).
fn click_within(output: &FrameOutput, within: &str, name: &str) -> Vec<Event> {
    let outer = output
        .zones
        .iter()
        .find(|zone| zone.label == within)
        .unwrap_or_else(|| panic!("no zone named {within:?}"))
        .rect;
    let inner = output
        .zones
        .iter()
        .find(|zone| {
            zone.label == name
                && zone.rect.x >= outer.x
                && zone.rect.y >= outer.y
                && zone.rect.r() <= outer.r()
                && zone.rect.b() <= outer.b()
        })
        .unwrap_or_else(|| panic!("no {name:?} inside {within:?}"));
    vec![
        Event::PointerMoved(inner.rect.center()),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ]
}

/// A click on the zone named `name` in the last frame.
fn click_on(output: &FrameOutput, name: &str) -> Vec<Event> {
    vec![
        Event::PointerMoved(zone_center(&output.zones, name)),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ]
}

/// A right click on the zone named `name` in the last frame.
fn right_click_on(output: &FrameOutput, name: &str) -> Vec<Event> {
    vec![
        Event::PointerMoved(zone_center(&output.zones, name)),
        Event::PointerDown(Button::Secondary),
        Event::PointerUp(Button::Secondary),
    ]
}

#[test]
fn a_hosts_context_menu_copies_its_ssh_command_and_duplicates_it() {
    let mut app = NativeApp::new(Some(fixture_vault("more-menu", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        2.0,
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Copy ssh command"),
        3.0,
    );
    let copied = state.take_clipboard().unwrap();
    assert!(
        copied.starts_with("ssh ") && copied.contains("10.0.4.12"),
        "{copied}"
    );
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        4.0,
    );
    step(&mut app, &mut state, &click_on(&output, "Duplicate"), 5.0);
    let vault = app.vault.as_ref().unwrap();
    let original = vault
        .hosts()
        .iter()
        .find(|host| host.label == "prod-db")
        .unwrap();
    let copy = vault
        .hosts()
        .iter()
        .find(|host| host.label == "prod-db copy")
        .unwrap();
    assert_ne!(copy.id, original.id);
    assert_eq!(
        (&copy.address, copy.port),
        (&original.address, original.port)
    );
}

#[test]
fn a_hovered_card_shows_only_edit_and_its_other_actions_are_in_its_context_menu() {
    let mut app = NativeApp::new(Some(fixture_vault("card-hover", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let card = zone_center(&output.zones, "prod-db");
    output = step(&mut app, &mut state, &[Event::PointerMoved(card)], 1.0);
    let card_rect = output
        .zones
        .iter()
        .find(|zone| zone.label == "prod-db")
        .unwrap()
        .rect;
    let inside: Vec<&str> = output
        .zones
        .iter()
        .filter(|zone| zone.label != "prod-db" && card_rect.overlaps(&zone.rect))
        .map(|zone| zone.label.as_str())
        .collect();
    assert_eq!(inside, ["Edit"], "the card's own buttons");
    // Shift+F10 on the focused card opens the same menu as a right click.
    state.set_focus(
        output
            .zones
            .iter()
            .find(|zone| zone.label == "prod-db")
            .map(|zone| zone.id),
        true,
    );
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::F(10), Modifiers::SHIFT)],
        2.0,
    );
    let menu: Vec<&str> = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::MenuItem)
        .map(|zone| zone.label.as_str())
        .collect();
    assert_eq!(
        &menu[..5],
        [
            "Connect",
            "Open SFTP",
            "Edit",
            "Copy ssh command",
            "Duplicate"
        ]
    );
    output = step(&mut app, &mut state, &click_on(&output, "Edit"), 3.0);
    assert!(
        app.hosts.editor.is_some(),
        "Edit from the menu opens the editor"
    );
    assert!(output.zones.iter().any(|zone| zone.label == "Label"));
}

#[test]
fn a_hovered_cards_context_menu_stays_open_after_the_hover_fades() {
    let mut app = NativeApp::new(Some(fixture_vault("more-stays", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let on_card = [Event::PointerMoved(zone_center(&output.zones, "prod-db"))];
    output = step(&mut app, &mut state, &on_card, 1.0);
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        2.0,
    );
    // The menu's backdrop now covers the card; well past any fade, the menu is still there.
    for time in [2.5, 3.0, 4.0] {
        output = step(&mut app, &mut state, &[], time);
    }
    assert!(
        output.zones.iter().any(|zone| zone.label == "Duplicate"),
        "the menu closed itself"
    );
}

/// The uid of the group named `name`.
fn group_uid(app: &NativeApp, name: &str) -> String {
    let vault = app.vault.as_ref().unwrap();
    let group = vault.groups().iter().find(|group| group.name == name);
    group.unwrap().uid.clone()
}

/// The saved host labelled `label`.
fn host_named<'a>(app: &'a NativeApp, label: &str) -> &'a Host {
    let vault = app.vault.as_ref().unwrap();
    vault
        .hosts()
        .iter()
        .find(|host| host.label == label)
        .unwrap()
}

/// The tab queued last: its title, who it logs in as where, the password it brings, and the
/// saved host it came from.
fn queued(app: &NativeApp) -> (String, String, Option<Password>, Option<OpenTab>) {
    let (title, program, origin) = app.opens.last().unwrap();
    let Program::Ssh {
        target, password, ..
    } = program
    else {
        panic!("an ssh program")
    };
    let login = format!("{}@{}:{}", target.user, target.host, target.port);
    (title.clone(), login, password.clone(), origin.clone())
}

#[test]
fn connect_all_opens_every_host_directly_in_the_group() {
    let mut app = NativeApp::new(Some(fixture_vault("connect-all", true)));
    let production = group_uid(&app, "Production");
    // A host of a group inside Production is not directly in it.
    let vault = app.vault.as_mut().unwrap();
    let mut inner = vault.draft_group(Some(production)).unwrap();
    inner.name = "Inner".into();
    vault.upsert_group(inner.clone()).unwrap();
    let mut deep = vault.draft_host();
    (deep.label, deep.address, deep.group) = ("deep".into(), "deep.lan".into(), Some(inner.uid));
    vault.upsert_host(deep).unwrap();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Production"), 1.0);
    // The tabs open at the end of the click's own frame.
    drive(&mut app, &mut state, &click_on(&output, "Connect all"), 2.0);
    let opened: Vec<(&str, Option<OpenTab>)> = app
        .tabs
        .iter()
        .skip(super::FIXED_TABS)
        .map(|tab| (tab.title.as_str(), tab.origin.clone()))
        .collect();
    let expected: Vec<(&str, Option<OpenTab>)> = ["api-1", "api-2", "worker-1"]
        .into_iter()
        .map(|label| (label, Some(OpenTab::Host(host_named(&app, label).id))))
        .collect();
    assert_eq!(opened, expected);
}

#[test]
fn the_tag_filter_works_by_keyboard() {
    let mut app = NativeApp::new(Some(fixture_vault("tags-keyboard", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), 1.0);
    // Tab goes on from the Tags button to the chips.
    let chip = |output: &FrameOutput| {
        let zone = output.zones.iter().find(|zone| zone.label == "nginx (2)");
        zone.unwrap().id
    };
    let mut time = 2.0;
    while state.focus() != Some(chip(&output)) {
        assert!(time < 40.0, "Tab never reached the chip");
        output = step(
            &mut app,
            &mut state,
            &[Event::Key(Key::Tab, Modifiers::NONE)],
            time,
        );
        time += 1.0;
    }
    let space = [Event::Key(Key::Space, Modifiers::NONE)];
    output = step(&mut app, &mut state, &space, time);
    assert_eq!(app.hosts.tags, ["nginx"]);
    let listed = |label: &str| output.zones.iter().any(|zone| zone.label == label);
    assert!(listed("web-1") && listed("staging-web"), "from every group");
    assert!(!listed("prod-db"));
    // Clear drops the filter; so does closing the chips.
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Clear"),
        time + 1.0,
    );
    assert_eq!(app.hosts.tags, Vec::<String>::new());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "nginx (2)"),
        time + 2.0,
    );
    assert_eq!(app.hosts.tags, ["nginx"]);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), time + 3.0);
    assert_eq!(app.hosts.tags, Vec::<String>::new());
    assert!(!output.zones.iter().any(|zone| zone.label == "nginx (2)"));
}

#[test]
fn move_to_group_puts_the_host_in_the_chosen_group_or_back_on_top() {
    let mut app = NativeApp::new(Some(fixture_vault("move-to", true)));
    let production = group_uid(&app, "Production");
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 1.0);
    let more = right_click_on(&output, "prod-db");
    output = step(&mut app, &mut state, &more, 2.0);
    let menu = click_on(&output, "Move to Production");
    output = step(&mut app, &mut state, &menu, 3.0);
    assert_eq!(
        host_named(&app, "prod-db").group.as_deref(),
        Some(production.as_str())
    );
    // It left the top level: open Production to find it, then move it back up.
    output = step(&mut app, &mut state, &click_on(&output, "Production"), 4.0);
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 5.0);
    let more = right_click_on(&output, "prod-db");
    output = step(&mut app, &mut state, &more, 6.0);
    let menu = click_on(&output, "Move to the top level");
    step(&mut app, &mut state, &menu, 7.0);
    assert_eq!(host_named(&app, "prod-db").group, None);
}

#[test]
fn duplicate_saves_a_copy_with_a_new_id_and_the_same_address() {
    // From a row of the list, whose context menu is its own.
    let mut app = NativeApp::new(Some(fixture_vault("duplicate", true)));
    app.hosts.mode = Mode::List;
    let before = app.vault.as_ref().unwrap().hosts().len();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let hover = [Event::PointerMoved(zone_center(&output.zones, "grafana"))];
    output = step(&mut app, &mut state, &hover, 1.0);
    let more = right_click_on(&output, "grafana");
    output = step(&mut app, &mut state, &more, 2.0);
    step(&mut app, &mut state, &click_on(&output, "Duplicate"), 3.0);
    assert_eq!(app.vault.as_ref().unwrap().hosts().len(), before + 1);
    let (original, copy) = (
        host_named(&app, "grafana"),
        host_named(&app, "grafana copy"),
    );
    assert_ne!(copy.id, original.id);
    assert_eq!(
        (&copy.address, &copy.username, copy.port),
        (&original.address, &original.username, original.port)
    );
}

#[test]
fn a_typed_target_can_be_saved_as_a_host() {
    let mut app = NativeApp::new(Some(fixture_vault("save-target", true)));
    let before = app.vault.as_ref().unwrap().hosts().len();
    let search = "Search hosts, or type user@host:port to connect";
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let mut events = click_on(&output, search);
    events.push(Event::Text("deploy@10.0.9.9:2200".into()));
    output = step(&mut app, &mut state, &events, 1.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Save as host"),
        2.0,
    );
    // The editor opens on it, filled in; Save keeps it.
    output = step(&mut app, &mut state, &click_on(&output, "Save"), 3.0);
    assert_eq!(app.vault.as_ref().unwrap().hosts().len(), before + 1);
    let saved = host_named(&app, "10.0.9.9");
    assert_eq!(
        (saved.address.as_str(), saved.username.as_str(), saved.port),
        ("10.0.9.9", "deploy", Some(2200))
    );
    // Words that match nothing are no target: nothing to save.
    for (at, typed) in [(4.0, "just words"), (5.0, "zzz")] {
        let mut events = click_on(&output, search);
        events.extend([
            Event::Key(Key::Char('a'), Modifiers::CTRL),
            Event::Key(Key::Backspace, Modifiers::NONE),
            Event::Text(typed.into()),
        ]);
        output = step(&mut app, &mut state, &events, at);
        assert!(
            !output.zones.iter().any(|zone| zone.label == "Save as host"),
            "{typed} was offered"
        );
    }
}

#[test]
fn quick_connect_to_a_saved_label_uses_its_details() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-saved", true)));
    let id = host_named(&app, "prod-db").id;
    app.act(HostsAction::QuickConnect("PROD-DB".into()));
    let (title, login, _, origin) = queued(&app);
    assert_eq!(
        (title.as_str(), login.as_str(), origin),
        ("prod-db", "postgres@10.0.4.12:22", Some(OpenTab::Host(id)))
    );
}

#[test]
fn a_saved_password_goes_with_the_saved_host_only() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-password", true)));
    let mut host = host_named(&app, "prod-db").clone();
    host.password = Some(annulus_vault::hosts::Secret::new("hunter2"));
    app.vault.as_mut().unwrap().upsert_host(host).unwrap();
    app.act(HostsAction::QuickConnect("prod-db".into()));
    assert_eq!(queued(&app).2, Some(Password::new("hunter2")));
    app.act(HostsAction::QuickConnect("postgres@10.0.4.12:22".into()));
    assert_eq!(queued(&app).2, None);
}

#[test]
fn quick_connect_to_anything_else_is_an_ssh_target() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-other", true)));
    app.act(HostsAction::QuickConnect("root@other:22".into()));
    let (title, login, password, origin) = queued(&app);
    assert_eq!(
        (title.as_str(), login.as_str(), password, origin),
        ("root@other:22", "root@other:22", None, None)
    );
}

#[test]
fn the_lock_screens_tree_names_the_password_fields_but_never_their_text() {
    let mut app = NativeApp::new(None);
    app.lock = super::lock::LockView::new(super::lock::Mode::Setup);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let mut events = click_on(&output, "Master password (10+ characters)");
    events.push(Event::Text("correct horse battery".into()));
    output = step(&mut app, &mut state, &events, 1.0);
    let tree = annulus_ui::access::tree(&output.zones, state.focus(), 1.0, "Annulus");
    assert_eq!(annulus_ui::access::faults(&tree), Vec::<String>::new());
    let (id, field) = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Master password (10+ characters)"))
        .unwrap();
    assert_eq!(
        field.role(),
        annulus_ui::access::accesskit::Role::PasswordInput
    );
    assert_eq!(tree.focus, *id, "the field typed into has focus");
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.value().is_none_or(|value| !value.contains("horse"))),
        "the password reached the tree"
    );
}

#[test]
fn the_hosts_listing_sorts_by_its_headers_and_filters_by_name() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-sort", false)));
    app.push_sftp(super::demo::sftp_tab("sort"));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let y = |output: &FrameOutput, name: &str| zone_center(&output.zones, name).y;
    // Folders first, then by name.
    assert!(y(&output, "config") < y(&output, "app.log"));
    assert!(y(&output, "app.log") < y(&output, "deploy.sh"));
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Sort by size"),
        1.0,
    );
    assert!(y(&output, "README.md") < y(&output, "deploy.sh"));
    assert!(y(&output, "deploy.sh") < y(&output, "app.log"));
    assert!(
        y(&output, "config") < y(&output, "README.md"),
        "folders stay first"
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Sort by size"),
        2.0,
    );
    assert!(
        y(&output, "app.log") < y(&output, "deploy.sh"),
        "again: largest first"
    );
    let mut events = click_on(&output, "Filter this folder");
    events.push(Event::Text("DEPLOY".into()));
    output = step(&mut app, &mut state, &events, 3.0);
    let names: Vec<&str> = output
        .zones
        .iter()
        .map(|zone| zone.label.as_str())
        .collect();
    assert!(
        names.contains(&"deploy.sh") && !names.contains(&"app.log"),
        "{names:?}"
    );
}

#[test]
fn deleting_on_the_host_asks_first() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-delete", false)));
    app.push_sftp(super::demo::sftp_tab("delete"));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let card_up = |output: &FrameOutput| output.zones.iter().any(|zone| zone.label == "Delete");
    output = step(&mut app, &mut state, &click_on(&output, "deploy.sh"), 1.0);
    assert!(!card_up(&output));
    let delete = [Event::Key(Key::Delete, Modifiers::NONE)];
    output = step(&mut app, &mut state, &delete, 2.0);
    assert!(card_up(&output), "the Delete key asks");
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Escape, Modifiers::NONE)],
        3.0,
    );
    assert!(!card_up(&output), "Escape keeps the file");
    assert!(output.zones.iter().any(|zone| zone.label == "deploy.sh"));
    // The More menu's Delete asks the same.
    output = step(&mut app, &mut state, &click_on(&output, "deploy.sh"), 4.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "More actions"),
        5.0,
    );
    output = step(&mut app, &mut state, &click_on(&output, "Delete"), 6.0);
    assert!(card_up(&output), "the menu asks too");
    output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 7.0);
    assert!(!card_up(&output));
}

#[test]
fn a_snippet_runs_on_the_hosts_ticked_in_one_tab_of_panes() {
    let mut app = NativeApp::new(Some(fixture_vault("snippet-hosts", true)));
    app.section = super::Section::Snippets;
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Run on hosts"),
        1.0,
    );
    output = step(&mut app, &mut state, &click_on(&output, "api-1"), 2.0);
    output = step(&mut app, &mut state, &click_on(&output, "bastion"), 3.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Run on 2 hosts")
    );
    // One frame: the tab is added at its end, and its panes start connecting when drawn.
    drive(
        &mut app,
        &mut state,
        &click_on(&output, "Run on 2 hosts"),
        4.0,
    );
    assert_eq!(
        titles(&app),
        ["Vault", "SFTP", "Tail the app log on 2 hosts"]
    );
    let panes = front_panes(&app);
    assert_eq!(panes.len(), 2, "a pane per host, side by side");
    assert_eq!(app.active, 2);
}

#[test]
fn trusting_a_servers_certificate_pins_it_until_the_address_changes() {
    let mut app = NativeApp::new(Some(fixture_vault("pin-certificate", true)));
    settings_account_certificate(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Trust this certificate"),
        1.0,
    );
    assert_eq!(app.sync.pin, Some((0..=255).collect::<Vec<u8>>()));
    assert!(app.sync.untrusted.is_none());
    // The certificate stopped a Create account: that, not Sign in, is what to redo.
    let (_, message) = app.sync.message.clone().unwrap_or_default();
    assert!(message.contains("not created yet"), "{message}");
    assert!(message.contains("Create account"), "{message}");
    assert!(!message.contains("Sign in"), "{message}");
    // Another address: the certificate trusted for the old one goes.
    let output = step(&mut app, &mut state, &[], 2.0);
    let mut events = click_on(&output, "Server, e.g. https://sync.example.com");
    events.push(Event::Text("1".into()));
    step(&mut app, &mut state, &events, 3.0);
    assert_eq!(app.sync.pin, None);
}

#[test]
fn the_lock_screen_takes_the_password_without_a_click() {
    let vault = fixture_vault("type-to-unlock", true);
    let mut app = NativeApp::new(None);
    app.vault_path = Some(vault.path().to_path_buf());
    drop(vault);
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    let typed = [
        Event::Text("not the password".into()),
        Event::Key(Key::Enter, Modifiers::NONE),
    ];
    step(&mut app, &mut state, &typed, 1.0);
    assert!(
        app.lock.working.is_some() || app.lock.error.is_some(),
        "typing went into the password field and Enter tried it"
    );
}

#[test]
fn the_plus_button_opens_a_new_tab_whose_search_connects_in_its_place() {
    let mut app = NativeApp::new(Some(fixture_vault("plus-hosts", true)));
    replay(&mut app, "build");
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(app.active, 2, "a terminal in front");
    let output = step(&mut app, &mut state, &click_on(&output, "New tab"), 1.0);
    assert_eq!(titles(&app), ["Vault", "SFTP", "build", "New Tab"]);
    assert_eq!(app.tabs[app.active].kind, super::TabKind::NewTab);
    // The search has the keyboard at once, and the saved hosts are listed newest first.
    let focused = output
        .zones
        .iter()
        .find(|zone| Some(zone.id) == state.focus());
    assert_eq!(
        focused.map(|zone| zone.role),
        Some(Role::TextInput),
        "the search is focused"
    );
    assert!(output.zones.iter().any(|zone| zone.label == "router"));
    // Typing a saved host's name and Enter connects to it, in the new tab's place.
    let typed = [
        Event::Text("prod-db".into()),
        Event::Key(Key::Enter, Modifiers::NONE),
    ];
    step(&mut app, &mut state, &typed, 2.0);
    step(&mut app, &mut state, &[], 3.0);
    assert_eq!(titles(&app), ["Vault", "SFTP", "build", "prod-db"]);
    assert_eq!(app.active, 3);
    // And it is the first of the recent connections now.
    let prod = host_named(&app, "prod-db").id;
    assert_eq!(app.settings.recent[0].host, prod);
}

#[test]
fn the_sftp_tab_picks_a_host_and_goes_back_to_the_picker() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-tab", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "SFTP"), 1.0);
    assert_eq!(app.active, 1);
    assert!(output.zones.iter().any(|zone| zone.label == "nas"));
    let output = step(&mut app, &mut state, &click_on(&output, "nas"), 2.0);
    assert!(
        app.tabs[1].sftp.is_some(),
        "its files, in the SFTP tab itself"
    );
    assert_eq!(app.tabs.len(), super::FIXED_TABS, "no tab was added");
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Back to the hosts"),
        3.0,
    );
    assert!(app.tabs[1].sftp.is_none());
}

#[test]
fn escape_with_the_palette_open_closes_only_the_palette() {
    let mut app = NativeApp::new(Some(fixture_vault("palette-escape", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &click_on(&output, "New host"), 1.0);
    assert!(app.hosts.editor.is_some());
    let palette = [Event::Key(Key::Char('k'), Modifiers::CTRL_SHIFT)];
    step(&mut app, &mut state, &palette, 2.0);
    assert!(app.palette.is_some());
    let escape = [Event::Key(Key::Escape, Modifiers::NONE)];
    step(&mut app, &mut state, &escape, 3.0);
    assert!(app.palette.is_none(), "the palette closed");
    assert!(app.hosts.editor.is_some(), "the draft under it stays");
}

#[test]
fn enter_in_an_empty_host_search_connects_nowhere() {
    let mut app = NativeApp::new(Some(fixture_vault("empty-search", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let tabs = app.tabs.len();
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Search hosts, or type user@host:port to connect"),
        1.0,
    );
    let enter = [Event::Key(Key::Enter, Modifiers::NONE)];
    step(&mut app, &mut state, &enter, 2.0);
    step(&mut app, &mut state, &[], 3.0);
    assert_eq!(app.tabs.len(), tabs, "no tab opened");
}

#[test]
fn escape_closes_the_open_card_and_keeps_what_a_delete_would_remove() {
    let escape = [Event::Key(Key::Escape, Modifiers::NONE)];
    let mut app = NativeApp::new(Some(fixture_vault("escape", true)));
    app.section = super::Section::Keychain;
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Import key"), 1.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Name, e.g. id_ed25519")
    );
    output = step(&mut app, &mut state, &escape, 2.0);
    assert!(
        !output
            .zones
            .iter()
            .any(|zone| zone.label == "Name, e.g. id_ed25519")
    );
    output = step(&mut app, &mut state, &click_on(&output, "Delete"), 3.0);
    assert!(output.zones.iter().any(|zone| zone.label == "Delete key"));
    output = step(&mut app, &mut state, &escape, 4.0);
    assert!(!output.zones.iter().any(|zone| zone.label == "Delete key"));
    assert_eq!(
        app.vault.as_ref().unwrap().ssh_keys().len(),
        1,
        "the key stays"
    );
    // Snippets and Port Forwarding cards too.
    app.section = super::Section::Snippets;
    output = step(&mut app, &mut state, &[], 5.0);
    output = step(&mut app, &mut state, &click_on(&output, "New snippet"), 6.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Label, e.g. Restart nginx")
    );
    output = step(&mut app, &mut state, &escape, 7.0);
    assert!(
        !output
            .zones
            .iter()
            .any(|zone| zone.label == "Label, e.g. Restart nginx")
    );
    app.section = super::Section::PortForwarding;
    output = step(&mut app, &mut state, &[], 8.0);
    output = step(&mut app, &mut state, &click_on(&output, "New rule"), 9.0);
    assert!(output.zones.iter().any(|zone| zone.label == "Save"));
    output = step(&mut app, &mut state, &escape, 10.0);
    assert!(!output.zones.iter().any(|zone| zone.label == "Save"));
}

#[test]
fn a_tag_is_deleted_from_every_host_only_after_a_yes() {
    let mut app = NativeApp::new(Some(fixture_vault("untag", true)));
    let tagged = |app: &NativeApp| {
        app.vault
            .as_ref()
            .unwrap()
            .hosts()
            .iter()
            .filter(|host| host.tags.iter().any(|tag| tag == "nginx"))
            .count()
    };
    let before = tagged(&app);
    assert!(before > 0);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), 1.0);
    let chip = output
        .zones
        .iter()
        .find(|zone| zone.label.starts_with("nginx ("))
        .map(|zone| zone.label.clone())
        .unwrap();
    output = step(&mut app, &mut state, &click_on(&output, &chip), 2.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Delete tag\u{2026}"),
        3.0,
    );
    // Cancel keeps it.
    output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 4.0);
    assert_eq!(tagged(&app), before);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Delete tag\u{2026}"),
        5.0,
    );
    let yes = output
        .zones
        .iter()
        .find(|zone| zone.label.starts_with("Delete \u{201c}nginx\u{201d} from"))
        .map(|zone| zone.label.clone())
        .unwrap();
    step(&mut app, &mut state, &click_on(&output, &yes), 6.0);
    assert_eq!(tagged(&app), 0, "gone from every host");
}

#[test]
fn a_locked_vault_hides_every_open_tab_and_the_palette() {
    let mut app = NativeApp::new(Some(fixture_vault("locked-tabs", true)));
    replay(&mut app, "web-1 session");
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    app.lock_vault();
    let mut output = step(&mut app, &mut state, &[], 1.0);
    let names: Vec<&str> = output
        .zones
        .iter()
        .map(|zone| zone.label.as_str())
        .collect();
    assert!(!names.contains(&"web-1 session"), "{names:?}");
    assert!(
        !names.contains(&"New tab") && !names.contains(&"SFTP"),
        "no new tab while locked"
    );
    for keys in [
        vec![Event::Key(Key::Tab, Modifiers::CTRL)],
        vec![Event::Key(Key::Char('k'), Modifiers::CTRL_SHIFT)],
        vec![Event::Key(Key::Char('t'), Modifiers::CTRL_SHIFT)],
    ] {
        output = step(&mut app, &mut state, &keys, 2.0);
        assert_eq!(app.active, 0, "the lock screen stays in front");
        assert!(app.palette.is_none() && app.opens.is_empty());
    }
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label.contains("password"))
    );
}

#[test]
fn locking_drops_the_secrets_open_tabs_hold_and_unlocking_brings_them_back() {
    let mut vault = fixture_vault("locked-secrets", false);
    let mut host = vault.draft_host();
    host.label = "db".into();
    host.address = "10.0.4.12".into();
    host.username = "deploy".into();
    host.password = Some(annulus_vault::hosts::Secret::new("hunter2"));
    let id = host.id;
    vault.upsert_host(host).unwrap();
    let mut app = NativeApp::new(Some(vault));
    app.act(super::hosts::HostsAction::Connect(id));
    let mut state = UiState::default();
    // One frame adds the tab; it is not drawn yet, so it does not connect.
    drive(&mut app, &mut state, &[], 0.0);
    let password = |app: &NativeApp| {
        app.tabs.iter().find_map(|tab| match &tab.terminal {
            Some(TermView::Live(panes)) => panes.programs().into_iter().next(),
            _ => None,
        })
    };
    let has_password = |program: Option<annulus_session::Program>| {
        matches!(
            program,
            Some(annulus_session::Program::Ssh {
                password: Some(_),
                ..
            })
        )
    };
    assert!(has_password(password(&app)), "the tab can log in");
    let path = app.vault.as_ref().unwrap().path().to_path_buf();
    app.lock_vault();
    assert!(
        !has_password(password(&app)),
        "locked: the password is gone from the tab"
    );
    // Unlocking brings the login back from the vault (the lock screen's job, done by hand).
    app.vault = Some(annulus_vault::vault::Vault::unlock(path, "pw").unwrap());
    app.unlocked();
    assert!(
        has_password(password(&app)),
        "unlocked: it can log in again"
    );
}

#[test]
fn the_palette_cannot_lock_while_the_recovery_key_is_shown() {
    let mut app = NativeApp::new(None);
    app.lock = super::lock::LockView::new(super::lock::Mode::Setup);
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Char('k'), Modifiers::CTRL_SHIFT)],
        1.0,
    );
    assert!(app.palette.is_none(), "no palette before a vault is open");
}

#[test]
fn a_second_window_cannot_open_the_same_data_folder() {
    let dir = std::env::temp_dir().join(format!("annulus-instance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let first = super::instance_lock(&dir);
    assert!(first.is_some(), "the first window takes the folder");
    assert!(
        super::instance_lock(&dir).is_none(),
        "a second one is refused"
    );
    drop(first);
    // Other tests start shells meanwhile: a child forked while the lock was open shares it
    // until it starts its program (the file closes on exec), so allow it a moment.
    let started = std::time::Instant::now();
    while super::instance_lock(&dir).is_none() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "free again once the first closes"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_key_pasted_into_the_keychain_is_saved_under_its_name() {
    let mut app = NativeApp::new(Some(fixture_vault("import-key", true)));
    app.section = super::Section::Keychain;
    let pair = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "me@test").unwrap();
    let before = app.vault.as_ref().unwrap().ssh_keys().len();
    let mut state = UiState::default();
    let mut output = drive(&mut app, &mut state, &[], 0.0);
    output = drive(&mut app, &mut state, &click_on(&output, "Import key"), 1.0);
    let mut events = click_on(&output, "Name, e.g. id_ed25519");
    events.push(Event::Text("deploy".into()));
    output = drive(&mut app, &mut state, &events, 2.0);
    let mut events = click_on(
        &output,
        "Paste the private key (-----BEGIN \u{2026} PRIVATE KEY-----)",
    );
    events.push(Event::Paste(pair.private.to_string()));
    output = drive(&mut app, &mut state, &events, 3.0);
    drive(&mut app, &mut state, &click_on(&output, "Import"), 4.0);
    let keys = app.vault.as_ref().unwrap().ssh_keys();
    assert_eq!(keys.len(), before + 1);
    let saved = keys.iter().find(|key| key.label == "deploy").unwrap();
    assert_eq!(
        (saved.label.as_str(), saved.public.as_str()),
        ("deploy", pair.public.as_str())
    );
}

/// End to end on test VM 1: the new Keychain installs a fresh key on a real host through
/// the UI (Install on a host, choose it, trust its key, Install), the host is set to log in
/// with it, and a second connection does log in with it (and takes the key off again).
#[test]
#[ignore = "needs the test servers"]
fn e2e_the_new_keychain_installs_a_key_on_a_real_host() {
    use crate::keychain_screen::stored;
    use annulus_ssh::keys;

    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let mut vault = fixture_vault("e2e-install", false);
    // The e2e key logs in the first time; the fresh key is the one installed.
    let login = keys::import(
        &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        None,
    )
    .unwrap();
    let login = stored(&vault, "e2e", &login);
    let login_id = login.id;
    vault.upsert_key(login).unwrap();
    let marker = format!("annulus-ui-e2e-{}", std::process::id());
    let fresh = keys::generate(keys::KeyKind::Ed25519, &marker).unwrap();
    let fresh = stored(&vault, "fresh", &fresh);
    let fresh_id = fresh.id;
    vault.upsert_key(fresh).unwrap();
    let mut host = vault.draft_host();
    host.label = "test-1".into();
    host.address = var("ANNULUS_E2E_VM1");
    host.username = var("ANNULUS_E2E_USER");
    host.key_id = Some(login_id);
    let host_id = host.id;
    vault.upsert_host(host).unwrap();
    let mut app = NativeApp::new(Some(vault));
    app.section = super::Section::Keychain;
    let dir = std::env::temp_dir().join(format!("annulus-e2e-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.ssh_files.known_hosts = dir.join("known_hosts");
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = step(&mut app, &mut state, &[], clock());
    let install = click_within(&output, "fresh", "Install on a host");
    output = step(&mut app, &mut state, &install, clock());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Choose a host"),
        clock(),
    );
    output = step(&mut app, &mut state, &click_on(&output, "test-1"), clock());
    output = step(&mut app, &mut state, &click_on(&output, "Install"), clock());
    let key_of = |app: &NativeApp| {
        app.vault
            .as_ref()
            .and_then(|vault| vault.hosts().iter().find(|host| host.id == host_id))
            .and_then(|host| host.key_id)
    };
    while key_of(&app) != Some(fresh_id) {
        assert!(clock() < 60.0, "the key was not installed");
        let trust = output
            .zones
            .iter()
            .any(|zone| zone.label == "Trust and connect");
        let events = if trust {
            click_on(&output, "Trust and connect")
        } else {
            Vec::new()
        };
        output = step(&mut app, &mut state, &events, clock());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Log in again, now with the fresh key, and take it off the host.
    let vault = app.vault.as_ref().unwrap();
    let host = vault
        .hosts()
        .iter()
        .find(|host| host.id == host_id)
        .unwrap();
    let task = crate::ssh_task::SshTask::exec(
        crate::connect::host(vault, &app.ssh_files, host),
        format!("sed -i '/{marker}/d' ~/.ssh/authorized_keys && echo removed"),
        || {},
    );
    let outcome = loop {
        assert!(clock() < 90.0, "the second login never finished");
        if let Some(outcome) = task.outcome() {
            break outcome;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let exec = outcome.unwrap();
    assert_eq!(
        exec.status,
        Some(0),
        "{}",
        String::from_utf8_lossy(&exec.output)
    );
}

/// How many SSH sessions `host`'s user has on the server, counted over one more connection
/// (the count includes it, the same way every time). Host-key questions are trusted.
fn sessions_on(app: &NativeApp, host_id: u64) -> usize {
    let vault = app.vault.as_ref().unwrap();
    let host = vault
        .hosts()
        .iter()
        .find(|host| host.id == host_id)
        .unwrap();
    let user = host.username.clone();
    let mut task = crate::ssh_task::SshTask::exec(
        crate::connect::host(vault, &app.ssh_files, host),
        format!("pgrep -fc '^sshd: {user}' || true"),
        || {},
    );
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed().as_secs() < 30,
            "the count never came back"
        );
        while let Some(event) = task.question() {
            if let annulus_session::SessionEvent::HostKey(_) = event {
                task.answers().host_key(true);
            }
        }
        if let Some(outcome) = task.outcome() {
            let exec = outcome.unwrap();
            return String::from_utf8_lossy(&exec.output)
                .trim()
                .parse()
                .unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// End to end on test VM 1: a terminal tab, an SFTP tab and a port forward opened through the
/// app, then closed the way a user closes them (the tab's close, the rule's switch), leave
/// nothing behind: the server is back to the sessions it had, and the forwarded port is free.
#[test]
#[ignore = "needs the test servers"]
#[allow(clippy::too_many_lines)] // Open four connections, check them, close them, check again.
fn e2e_closing_terminals_sftp_and_forwards_leaves_nothing_on_the_server() {
    use std::io::Read as _;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let mut vault = fixture_vault("e2e-cleanup", false);
    let login = annulus_ssh::keys::import(
        &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        None,
    )
    .unwrap();
    let login = crate::keychain_screen::stored(&vault, "e2e", &login);
    let key_id = login.id;
    vault.upsert_key(login).unwrap();
    let mut host = vault.draft_host();
    host.label = "test-1".into();
    host.address = var("ANNULUS_E2E_VM1");
    host.username = var("ANNULUS_E2E_USER");
    host.key_id = Some(key_id);
    let host_id = host.id;
    vault.upsert_host(host).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut rule = vault.draft_forward(host_id);
    rule.label = "ssh through test-1".into();
    rule.kind = annulus_vault::forwards::ForwardKind::Local;
    rule.bind_port = port;
    rule.destination_host = "127.0.0.1".into();
    rule.destination_port = 22;
    vault.upsert_forward(rule).unwrap();
    let mut app = NativeApp::new(Some(vault));
    let dir = std::env::temp_dir().join(format!("annulus-e2e-cleanup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.ssh_files.known_hosts = dir.join("known_hosts");
    app.ssh_files.key_dir = None;
    let before = sessions_on(&app, host_id);
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    // A terminal, split into two panes, and an SFTP tab.
    app.act(super::hosts::HostsAction::Connect(host_id));
    for _ in 0..3 {
        drive(&mut app, &mut state, &[], clock());
    }
    let split = [Event::Key(Key::Char('d'), Modifiers::CTRL_SHIFT)];
    drive(&mut app, &mut state, &split, clock());
    app.act(super::hosts::HostsAction::Sftp(host_id));
    for _ in 0..3 {
        drive(&mut app, &mut state, &[], clock());
    }
    // The forward, switched on from its row, carries a connection to the server's sshd.
    app.active = 0;
    app.section = super::Section::PortForwarding;
    let mut output = step(&mut app, &mut state, &[], clock());
    step(
        &mut app,
        &mut state,
        &click_within(&output, "ssh through test-1", "Run this rule"),
        clock(),
    );
    let banner = loop {
        assert!(clock() < 60.0, "the forward never answered");
        drive(&mut app, &mut state, &[], clock());
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0_u8; 8];
            if stream.read_exact(&mut bytes).is_ok() {
                break String::from_utf8_lossy(&bytes).into_owned();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert!(banner.starts_with("SSH-2.0"), "{banner:?}");
    // Every one of them shows on the server: two panes, the files tab and the forward.
    let during = loop {
        drive(&mut app, &mut state, &[], clock());
        let count = sessions_on(&app, host_id);
        if count >= before + 4 || clock() > 60.0 {
            break count;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    assert!(
        during >= before + 4,
        "{during} sessions while open, {before} before"
    );
    // Closed as a user closes them.
    output = step(&mut app, &mut state, &[], clock());
    step(
        &mut app,
        &mut state,
        &click_within(&output, "ssh through test-1", "Run this rule"),
        clock(),
    );
    // The fixed tabs (Vault, SFTP) never close.
    while app.tabs.len() > super::FIXED_TABS {
        assert!(clock() < 90.0, "{} tabs left open", app.tabs.len());
        app.close_tab(app.tabs.len() - 1);
        drive(&mut app, &mut state, &[], clock());
    }
    loop {
        drive(&mut app, &mut state, &[], clock());
        let count = sessions_on(&app, host_id);
        let free = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
        if count == before && free {
            break;
        }
        assert!(
            clock() < 120.0,
            "after closing: {count} sessions on the server (before: {before}), port free: {free}"
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// End to end through the deployed sync server: the new UI's Settings create an account
/// from typed input (server, name, the vault's password) and the vault syncs to it.
#[test]
#[ignore = "needs the deployed sync server (ANNULUS_E2E_SYNC_URL)"]
fn e2e_the_new_settings_create_a_sync_account_and_sync() {
    let url = std::env::var("ANNULUS_E2E_SYNC_URL").expect("ANNULUS_E2E_SYNC_URL");
    // An account on a server needs a master password that is hard to guess.
    let strong = "violet-anchor-pebble-tundra";
    let mut vault = fixture_vault("e2e-account", true);
    vault.change_password(strong).unwrap();
    let path = vault.path().to_path_buf();
    let mut app = NativeApp::new(Some(vault));
    app.vault_path = Some(path);
    app.section = super::Section::Settings;
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = drive(&mut app, &mut state, &[], clock());
    let name = format!("e2e-{}", std::process::id());
    let clear_device = vec![
        Event::Key(Key::Char('a'), Modifiers::CTRL),
        Event::Key(Key::Backspace, Modifiers::NONE),
    ];
    for (field, text) in [
        ("Server, e.g. https://sync.example.com", url.as_str()),
        ("Account name", name.as_str()),
        ("This device's name", "e2e device"),
        ("This vault's master password", strong),
    ] {
        let mut events = vec![
            Event::PointerMoved(zone_center(&output.zones, field)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        drive(&mut app, &mut state, &events, clock());
        events = clear_device.clone();
        events.push(Event::Text(text.to_owned()));
        output = drive(&mut app, &mut state, &events, clock());
    }
    let press_on = |zones: &[Zone], name: &str| {
        vec![
            Event::PointerMoved(zone_center(zones, name)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    let events = press_on(&output.zones, "Create account");
    drive(&mut app, &mut state, &events, clock());
    loop {
        assert!(clock() < 90.0, "no sync; message: {:?}", app.sync.message);
        drive(&mut app, &mut state, &[], clock());
        let synced = app.vault.as_ref().is_some_and(|vault| {
            let state = vault.sync_state();
            state.account.is_some() && state.last_synced > 0 && state.pending_count() == 0
        });
        if synced {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(app.sync.message.as_ref().filter(|(error, _)| *error), None);
}

#[test]
fn tab_crosses_the_hosts_toolbar_left_to_right() {
    let mut app = NativeApp::new(Some(fixture_vault("tab-order", true)));
    let mut state = UiState::default();
    let mut renderer = Cpu(None);
    let viewport = Rect::new(0.0, 0.0, 1100.0, 720.0);
    let mut focused = Vec::new();
    for step in 0..40 {
        let events = if step == 0 {
            Vec::new()
        } else {
            vec![Event::Key(Key::Tab, Modifiers::NONE)]
        };
        let mut output = FrameOutput::default();
        renderer.frame((1100, 720, 1.0), false, &mut |canvas| {
            #[allow(clippy::cast_precision_loss)] // Step counts are small.
            state.begin_frame(&events, f64::from(step));
            let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        if let Some(id) = state.focus()
            && let Some(zone) = output.zones.iter().find(|zone| zone.id == id)
        {
            focused.push(zone.label.clone());
        }
    }
    let at = |label: &str| {
        focused
            .iter()
            .position(|seen| seen == label)
            .unwrap_or_else(|| panic!("{label:?} never focused: {focused:?}"))
    };
    let order = [
        at("Vault"),
        at("Hosts"),
        at("Settings"),
        at("Search hosts, or type user@host:port to connect"),
        at("New host"),
        at("More ways to add hosts"),
        at("Terminal"),
        at("Homelab"),
    ];
    assert!(order.is_sorted(), "{order:?}: {focused:?}");
}

#[test]
fn the_lint_catches_partial_overlaps_but_not_nesting() {
    let zone = |label: &str, rect: Rect| {
        Zone::new(annulus_ui::kit::Id::new(label), rect, Role::Button, label)
    };
    let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
    let card = zone("card", Rect::new(0.0, 0.0, 50.0, 50.0));
    let inner = zone("inner", Rect::new(10.0, 10.0, 10.0, 10.0));
    let across = zone("across", Rect::new(40.0, 0.0, 20.0, 20.0));
    let outside = zone("outside", Rect::new(90.0, 90.0, 20.0, 20.0));
    assert_eq!(
        layout_faults(&[card.clone(), inner], viewport),
        Vec::<String>::new()
    );
    assert_eq!(
        layout_faults(&[card.clone(), across.clone()], viewport).len(),
        1
    );
    assert_eq!(layout_faults(&[outside], viewport).len(), 1);
    // A menu over the card, above a backdrop, is fine.
    let backdrop = Zone {
        role: Role::Backdrop,
        ..zone("backdrop", viewport)
    };
    assert_eq!(
        layout_faults(&[card, backdrop, across], viewport),
        Vec::<String>::new()
    );
}

#[test]
fn the_tab_order_lint_catches_a_mirrored_row_but_not_a_new_column() {
    let zone = |label: &str, rect: Rect| {
        Zone::new(annulus_ui::kit::Id::new(label), rect, Role::Button, label)
    };
    let nav = zone("nav", Rect::new(0.0, 500.0, 100.0, 30.0));
    let content = zone("content", Rect::new(200.0, 50.0, 100.0, 30.0));
    assert_eq!(
        tab_order_faults(&[nav, content.clone()]),
        Vec::<String>::new()
    );
    let right = zone("right", Rect::new(400.0, 50.0, 80.0, 30.0));
    let left = zone("left", Rect::new(300.0, 50.0, 80.0, 30.0));
    assert_eq!(tab_order_faults(&[right.clone(), left.clone()]).len(), 1);
    let backdrop = Zone {
        role: Role::Backdrop,
        ..zone("backdrop", Rect::new(0.0, 0.0, 800.0, 600.0))
    };
    assert_eq!(
        tab_order_faults(&[right, backdrop, left]),
        Vec::<String>::new()
    );
}

#[test]
fn first_run_creates_the_vault_and_opens_it_only_after_the_recovery_key_is_confirmed() {
    let dir = std::env::temp_dir().join(format!("annulus-first-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = NativeApp::new(None);
    app.vault_path = Some(dir.join("vault.anv"));
    app.lock = super::lock::LockView::new(super::lock::Mode::Setup);
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = drive(&mut app, &mut state, &[], clock());
    let type_into = |output: &FrameOutput, field: &str, text: &str| {
        vec![
            Event::PointerMoved(zone_center(&output.zones, field)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
            Event::Text(text.to_owned()),
        ]
    };
    let events = type_into(
        &output,
        "Master password (10+ characters)",
        "correct horse battery",
    );
    output = drive(&mut app, &mut state, &events, clock());
    let events = type_into(&output, "Type it again", "correct horse battery");
    output = drive(&mut app, &mut state, &events, clock());
    let create = vec![
        Event::PointerMoved(zone_center(&output.zones, "Create vault")),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ];
    output = drive(&mut app, &mut state, &create, clock());
    while app.lock.mode != super::lock::Mode::Recovery {
        assert!(
            clock() < 60.0,
            "the vault was not created: {:?}",
            app.lock.error
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        output = drive(&mut app, &mut state, &[], clock());
    }
    assert!(app.vault.is_none(), "not open before the key is confirmed");
    // Copying the key copies it as a secret: kept out of clipboard history, cleared later.
    output = drive(
        &mut app,
        &mut state,
        &click_on(&output, "Copy the recovery key"),
        clock(),
    );
    assert_eq!(
        state.take_copied(),
        Some((app.lock.recovery_key_for_tests(), true))
    );
    // A wrong first group is refused.
    let events = type_into(
        &output,
        "Type its first group to confirm you saved it",
        "nope",
    );
    output = drive(&mut app, &mut state, &events, clock());
    let confirm = |output: &FrameOutput| {
        vec![
            Event::PointerMoved(zone_center(&output.zones, "I saved it, open the vault")),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    output = drive(&mut app, &mut state, &confirm(&output), clock());
    assert!(app.vault.is_none());
    // The key's first group, from the file the vault now has on disk: reopen proves it.
    assert!(dir.join("vault.anv").exists());
    let key = app.lock.recovery_key_for_tests();
    let first = key.split('-').next().unwrap().to_owned();
    let mut events = vec![
        Event::PointerMoved(zone_center(
            &output.zones,
            "Type its first group to confirm you saved it",
        )),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
        Event::Key(Key::Char('a'), Modifiers::CTRL),
        Event::Key(Key::Backspace, Modifiers::NONE),
    ];
    events.push(Event::Text(first));
    output = drive(&mut app, &mut state, &events, clock());
    drive(&mut app, &mut state, &confirm(&output), clock());
    assert!(app.vault.is_some());
}

#[test]
fn closing_a_tab_saves_its_log_only_when_logging_is_on() {
    for record in [false, true] {
        let mut app = NativeApp::new(Some(fixture_vault(&format!("logs-{record}"), false)));
        app.settings.record_logs = record;
        let program = if cfg!(windows) {
            annulus_session::Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into(), "echo annulus-log-line".into()],
            }
        } else {
            annulus_session::Program::Command {
                program: "sh".into(),
                args: vec!["-c".into(), "echo annulus-log-line; exec sh".into()],
            }
        };
        app.open("Shell", program);
        let mut state = UiState::default();
        let started = std::time::Instant::now();
        let mut seen = false;
        while !seen {
            assert!(
                started.elapsed().as_secs() < 20,
                "the shell printed nothing"
            );
            drive(&mut app, &mut state, &[], started.elapsed().as_secs_f64());
            seen = app
                .tabs
                .last()
                .and_then(|tab| match &tab.terminal {
                    Some(super::TermView::Live(panes)) => Some(panes.transcripts()),
                    _ => None,
                })
                .is_some_and(|logs| {
                    logs.iter()
                        .any(|(_, text)| text.contains("annulus-log-line"))
                });
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        app.close_tab(app.tabs.len() - 1);
        let logs = app.vault.as_ref().unwrap().logs();
        assert_eq!(
            logs.iter().any(|log| log.text.contains("annulus-log-line")),
            record,
            "record_logs = {record}"
        );
    }
}

#[test]
fn a_log_of_a_tab_closed_while_locked_is_saved_once_the_vault_opens() {
    let mut app = NativeApp::new(Some(fixture_vault("logs-locked", false)));
    app.settings.record_logs = true;
    replay(&mut app, "build");
    let vault = app.vault.take();
    app.save_logs(vec![("build".into(), "make: done".into())]);
    assert!(app.vault.is_none());
    app.vault = vault;
    app.unlocked();
    let logs = app.vault.as_ref().unwrap().logs();
    assert!(logs.iter().any(|log| log.text == "make: done"), "{logs:?}");
}

#[test]
fn tabs_are_listed_for_the_next_launch_and_host_tabs_wait_for_the_vault() {
    let mut app = NativeApp::new(Some(fixture_vault("restore", true)));
    app.settings.restore_tabs = true;
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.act(super::hosts::HostsAction::Connect(host));
    // A tab with no origin to reopen it from is not listed.
    app.push_terminal("plain", TermView::Replay(Box::new(super::demo::terminal())));
    let mut state = UiState::default();
    drive(&mut app, &mut state, &[], 0.0);
    assert_eq!(
        app.settings.open_tabs,
        [annulus_config::settings::OpenTab::Host(host)]
    );
    // The next start: a host tab waits for the vault, then reconnects.
    let mut next = NativeApp::new(None);
    next.settings = app.settings.clone();
    next.restore();
    assert_eq!(next.pending_hosts, [host]);
    next.vault = app.vault.take();
    drive(&mut next, &mut state, &[], 1.0);
    drive(&mut next, &mut state, &[], 2.0);
    assert_eq!(next.pending_hosts, Vec::<u64>::new());
    assert_eq!(next.tabs.len(), 3, "the host's tab is open again");
    assert_eq!(next.active, 2);
    // Quitting at the lock screen keeps a host tab that is still waiting for the vault.
    let mut locked = NativeApp::new(None);
    locked.settings = next.settings.clone();
    locked.restore();
    drive(&mut locked, &mut state, &[], 3.0);
    assert_eq!(
        locked.settings.open_tabs,
        [annulus_config::settings::OpenTab::Host(host)]
    );
    // With the setting off, nothing comes back.
    let mut off = NativeApp::new(None);
    off.settings = next.settings.clone();
    off.settings.restore_tabs = false;
    off.restore();
    assert!(off.pending_hosts.is_empty() && off.opens.is_empty());
}

fn replay(app: &mut NativeApp, title: &str) {
    app.push_terminal(title, TermView::Replay(Box::new(super::demo::terminal())));
}

fn titles(app: &NativeApp) -> Vec<&str> {
    app.tabs.iter().map(|tab| tab.title.as_str()).collect()
}

#[test]
fn the_palette_lists_tabs_and_commands_and_runs_them() {
    use crate::command_palette::Action;
    let mut app = NativeApp::new(None);
    replay(&mut app, "build");
    app.active = 0;
    let items = app.palette_items();
    let kinds: Vec<(&str, &str)> = items
        .iter()
        .map(|item| (item.label.as_str(), item.kind))
        .collect();
    assert_eq!(
        kinds[0],
        ("build", "Tab"),
        "locked: no hosts, the tabs come first"
    );
    assert!(kinds.contains(&("Settings", "Command")));
    assert!(kinds.contains(&("SFTP", "Command")));
    app.run_action(Action::Tab(2));
    assert_eq!(app.active, 2);
    app.run_action(Action::Settings);
    assert_eq!((app.active, app.section), (0, super::Section::Settings));
    app.run_action(Action::NewLocal);
    assert_eq!(app.opens.len(), 1, "a local shell opens after this frame");
}

#[test]
fn the_fixed_tabs_cannot_be_closed_and_closing_moves_focus_left() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.close_tab(0);
    app.close_tab(1);
    assert_eq!(titles(&app), ["Vault", "SFTP", "a", "b"]);
    app.close_tab(3);
    assert_eq!((app.tabs.len(), app.active), (3, 2));
    // The last session gone, the vault is in front, not the SFTP picker.
    app.close_tab(2);
    assert_eq!((titles(&app), app.active), (vec!["Vault", "SFTP"], 0));
}

#[test]
fn closing_a_background_tab_keeps_the_front_tab() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.active = 3;
    app.close_tab(2);
    assert_eq!(app.tabs[app.active].title, "b");
}

#[test]
fn a_log_is_deleted_only_after_a_yes() {
    let mut app = NativeApp::new(Some(fixture_vault("delete-log", true)));
    app.section = super::Section::Logs;
    let logs = |app: &NativeApp| app.vault.as_ref().unwrap().logs().len();
    assert_eq!(logs(&app), 2);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        1.0,
    );
    let output = step(&mut app, &mut state, &click_on(&output, "Delete"), 2.0);
    assert_eq!(logs(&app), 2, "asked first, nothing gone yet");
    // Cancel keeps it.
    let output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 3.0);
    assert_eq!(logs(&app), 2);
    // From its row's button this time, and yes.
    let output = step(
        &mut app,
        &mut state,
        &click_within(&output, "prod-db", "Delete"),
        4.0,
    );
    step(&mut app, &mut state, &click_on(&output, "Delete log"), 5.0);
    assert_eq!(logs(&app), 1);
    assert!(
        app.vault
            .as_ref()
            .unwrap()
            .logs()
            .iter()
            .all(|log| log.title != "prod-db")
    );
}

#[test]
fn a_shortcut_takes_new_keys_in_settings_and_its_own_again_on_reset() {
    let mut app = NativeApp::new(Some(fixture_vault("rebind", false)));
    app.section = super::Section::Settings;
    let mut state = UiState::default();
    let press = |key, modifiers| vec![Event::Key(key, modifiers)];
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "Shortcuts"), 1.0);
    // Escape stops recording and changes nothing.
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Change the keys of Local terminal"),
        2.0,
    );
    let output = step(
        &mut app,
        &mut state,
        &press(Key::Escape, Modifiers::NONE),
        3.0,
    );
    assert!(
        output
            .zones
            .iter()
            .all(|zone| !zone.label.starts_with("Recording"))
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Change the keys of New tab"),
        4.0,
    );
    // Keys another shortcut has are refused, and reach that one no more than any other.
    step(
        &mut app,
        &mut state,
        &press(Key::Char('k'), Modifiers::CTRL_SHIFT),
        5.0,
    );
    assert!(app.palette.is_none(), "the palette's keys opened nothing");
    assert_eq!(app.settings.keys, []);
    let tabs = app.tabs.len();
    let output = step(&mut app, &mut state, &press(Key::Char('n'), ctrl_alt), 6.0);
    assert_eq!(app.tabs.len(), tabs, "recorded, not acted on");
    assert_eq!(
        app.settings.keys,
        vec![KeyBinding {
            action: "NEW_TAB".into(),
            keys: "Ctrl+Alt+N".into(),
        }]
    );
    let reset = "Reset New tab to Ctrl+Shift+T";
    assert!(output.zones.iter().any(|zone| zone.label == reset));
    // The old keys open nothing now; the new ones open a tab.
    step(
        &mut app,
        &mut state,
        &press(Key::Char('t'), Modifiers::CTRL_SHIFT),
        7.0,
    );
    assert_eq!(app.tabs.len(), tabs);
    step(&mut app, &mut state, &press(Key::Char('n'), ctrl_alt), 8.0);
    assert_eq!(app.tabs.len(), tabs + 1);
    // Back in Settings, Reset gives it its own keys again.
    app.active = 0;
    let output = step(&mut app, &mut state, &[], 9.0);
    step(&mut app, &mut state, &click_on(&output, reset), 10.0);
    assert_eq!(app.settings.keys, []);
    step(
        &mut app,
        &mut state,
        &press(Key::Char('t'), Modifiers::CTRL_SHIFT),
        11.0,
    );
    assert_eq!(app.tabs.len(), tabs + 2);
}

#[test]
fn a_reset_that_would_share_keys_is_refused_and_reset_all_puts_everything_back() {
    let mut app = NativeApp::new(Some(fixture_vault("rebind-clash", false)));
    app.section = super::Section::Settings;
    let mut state = UiState::default();
    let press = |key, modifiers| vec![Event::Key(key, modifiers)];
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let output = step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &click_on(&output, "Shortcuts"), 1.0);
    // New tab moves away and Local terminal takes its old keys: New tab's Reset would make
    // the two share them.
    let mut time = 2.0;
    for (name, keys) in [
        ("Change the keys of New tab", (Key::Char('n'), ctrl_alt)),
        (
            "Change the keys of Local terminal",
            (Key::Char('t'), Modifiers::CTRL_SHIFT),
        ),
    ] {
        let output = step(&mut app, &mut state, &[], time);
        step(&mut app, &mut state, &click_on(&output, name), time + 0.5);
        step(&mut app, &mut state, &press(keys.0, keys.1), time + 1.0);
        time += 2.0;
    }
    assert_eq!(app.settings.keys.len(), 2);
    let output = step(&mut app, &mut state, &[], time);
    let reset = "Reset New tab to Ctrl+Shift+T";
    step(&mut app, &mut state, &click_on(&output, reset), time + 1.0);
    assert_eq!(app.settings.keys.len(), 2, "no reset into a clash");
    assert_eq!(super::shortcuts::NEW_TAB.keys(), "Ctrl+Alt+N");
    let output = step(&mut app, &mut state, &[], time + 2.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Reset all"),
        time + 3.0,
    );
    assert_eq!(app.settings.keys, []);
    assert_eq!(
        (
            super::shortcuts::NEW_TAB.keys(),
            super::shortcuts::LOCAL.keys()
        ),
        ("Ctrl+Shift+T".to_owned(), "Ctrl+Shift+L".to_owned())
    );
}

#[test]
fn many_tabs_share_the_strip_and_none_shrinks_to_nothing() {
    let mut app = NativeApp::new(Some(fixture_vault("many-tabs", false)));
    for index in 0..6 {
        replay(&mut app, &format!("session number {index}"));
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let tabs: Vec<&Zone> = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::Tab && zone.label.starts_with("session"))
        .collect();
    assert_eq!(tabs.len(), 6, "every tab fits in a 1100-wide window");
    let widths: Vec<f32> = tabs.iter().map(|zone| zone.rect.w).collect();
    assert!(
        widths.iter().all(|width| *width >= 96.0 - 0.5),
        "{widths:?}"
    );
    assert!(
        widths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1.0),
        "they share it evenly: {widths:?}"
    );
}

#[test]
fn tabs_past_the_strips_end_wait_in_the_all_tabs_menu_and_come_back_when_picked() {
    let mut app = NativeApp::new(Some(fixture_vault("all-tabs", false)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert!(
        !output.zones.iter().any(|zone| zone.label == "All tabs"),
        "no menu while every tab fits"
    );
    for index in 0..16 {
        replay(&mut app, &format!("session {index:02}"));
    }
    app.active = 2;
    let output = step(&mut app, &mut state, &[], 1.0);
    let on_strip = |output: &annulus_ui::kit::FrameOutput| -> Vec<String> {
        output
            .zones
            .iter()
            .filter(|zone| zone.role == Role::Tab && zone.label.starts_with("session"))
            .map(|zone| zone.label.clone())
            .collect()
    };
    assert!(
        on_strip(&output).len() < 16,
        "not every tab fits: {:?}",
        on_strip(&output)
    );
    assert!(!on_strip(&output).contains(&"session 15".to_owned()));
    let output = step(&mut app, &mut state, &click_on(&output, "All tabs"), 2.0);
    let listed = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::MenuItem)
        .count();
    assert!(listed > 0, "the menu lists the tabs");
    step(&mut app, &mut state, &click_on(&output, "session 15"), 3.0);
    assert_eq!(
        app.tabs[app.active].title, "session 15",
        "picked, it is in front"
    );
    let output = step(&mut app, &mut state, &[], 4.0);
    assert!(
        on_strip(&output).contains(&"session 15".to_owned()),
        "and on the strip: {:?}",
        on_strip(&output)
    );
    assert_eq!(app.tabs.len(), super::FIXED_TABS + 16, "no tab lost");
}

#[test]
fn an_agent_runs_a_command_through_the_door_in_its_own_tab_and_gets_its_output() {
    use annulus_control::client::Client;
    let mut app = NativeApp::new(Some(fixture_vault("agent-run", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        let ran = client
            .run(tab.id, "echo agent-was-here", Some(30))
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, ran))
    });
    let started = std::time::Instant::now();
    let mut time = 1.0;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, ran) = agent.join().unwrap().unwrap();
    assert!(
        ran.output
            .lines()
            .any(|line| line.trim() == "agent-was-here"),
        "the echo's answer comes back: {ran:?}"
    );
    assert_eq!(
        ran.state,
        annulus_control::protocol::RunState::Done,
        "its prompt came back: {ran:?}"
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref())
        .unwrap();
    assert_eq!(log.commands.len(), 1);
    let command = &log.commands[0];
    assert_eq!(command.text, "echo agent-was-here", "listed beside the tab");
    assert!(
        matches!(command.state, super::control::Ran::Done { lines: 1.., .. }),
        "done, with the lines it printed"
    );
    let line = command.line;
    assert_eq!(log.marks().lines, vec![line], "marked in the margin");

    // Picking it in the panel shows it; picking it again lets it go.
    let output = step(&mut app, &mut state, &[], time);
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "echo agent-was-here"),
        time + 0.1,
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref());
    assert_eq!(log.and_then(|log| log.picked), Some(0));
    assert_eq!(
        log.and_then(|log| log.marks().picked)
            .map(|lines| lines.start),
        Some(line),
        "its lines are tinted from the one it was typed on"
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "echo agent-was-here"),
        time + 0.2,
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref());
    assert_eq!(log.and_then(|log| log.picked), None);
}

#[test]
fn an_agent_hears_why_its_tab_could_not_connect_and_its_command_is_refused_at_once() {
    use annulus_control::client::Client;
    let mut app = NativeApp::new(Some(fixture_vault("agent-failed", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let broken = annulus_session::Program::Command {
        program: "annulus-no-such-program".into(),
        args: Vec::new(),
    };
    let tab = app.open_terminal("broken".into(), broken, None);
    if let Some(open) = app.tabs.iter_mut().find(|open| open.uid == tab) {
        open.agent = Some(Box::new(super::control::AgentLog::new(0)));
    }
    let mut state = UiState::default();
    // The first frame opens the door and tries to start the program.
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let read = client.read(tab, None).map_err(|e| e.to_string())?;
        let run = client.run(tab, "echo never", Some(30));
        Ok::<_, String>((read, run.map_err(|e| e.to_string())))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(20) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (read, run) = agent.join().unwrap().unwrap();
    assert!(read.contains("[Annulus] Could not start"), "{read:?}");
    let refused = run.unwrap_err();
    assert!(
        refused.contains("could not connect") && refused.contains("Could not start"),
        "{refused}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "refused at once, not after the 30 s start wait"
    );
}

#[test]
fn an_agents_command_runs_out_of_its_wait_is_interrupted_and_stop_cuts_off_the_next() {
    use super::control::Ran;
    use annulus_control::client::Client;
    // A line a second for a while, in every local shell the runners have.
    let ticking = if cfg!(windows) {
        "ping -n 40 127.0.0.1"
    } else {
        "for i in $(seq 40); do echo tick $i; sleep 1; done"
    };
    let mut app = NativeApp::new(Some(fixture_vault("agent-stop", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let mut time = 0.05;
    let mut frames = |app: &mut NativeApp, until: &dyn Fn(&NativeApp) -> bool| {
        let started = std::time::Instant::now();
        while !until(app) && started.elapsed() < std::time::Duration::from_secs(90) {
            step(app, &mut state, &[], time);
            time += 0.05;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    };
    let states = |app: &NativeApp| -> Vec<String> {
        app.tabs
            .iter()
            .filter_map(|tab| tab.agent.as_ref())
            .flat_map(|log| &log.commands)
            .map(|command| match command.state {
                Ran::Running(_) => "running",
                Ran::Done { .. } => "done",
                Ran::Waiting { .. } => "waiting",
                Ran::TimedOut { .. } => "timed out",
                Ran::Interrupted { .. } => "interrupted",
                Ran::Cut { .. } => "cut",
                Ran::Ended { .. } => "ended",
            })
            .map(str::to_owned)
            .collect()
    };

    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        let out = client
            // A wait shorter than the quiet that counts as done: it always runs out, however
            // slowly a busy runner draws frames.
            .run(tab.id, ticking, Some(1))
            .map_err(|e| e.to_string())?;
        client.interrupt(tab.id).map_err(|e| e.to_string())?;
        // Answered only once this one settles, which Stop never lets happen.
        let cut = client.run(tab.id, ticking, Some(60));
        Ok::<_, String>((out, cut.is_err()))
    });
    frames(&mut app, &|app| {
        states(app).len() == 2 || agent.is_finished()
    });
    assert!(!agent.is_finished(), "{:?}", agent.join());
    assert_eq!(states(&app), ["interrupted", "running"]);

    // Stop, as the panel's button does.
    app.settings.automation = false;
    frames(&mut app, &|app| {
        states(app).last().is_some_and(|last| last == "cut")
    });
    assert_eq!(
        states(&app),
        ["interrupted", "cut"],
        "nothing is left spinning"
    );
    let (out, refused) = agent.join().unwrap().unwrap();
    // Its first line may or may not come within the second a busy runner gives it: what
    // counts is that the answer starts at the command and says it still runs.
    assert!(out.output.contains(ticking), "{out:?}");
    assert_eq!(
        out.state,
        annulus_control::protocol::RunState::Running,
        "out of its wait, it is still running: {out:?}"
    );
    assert!(refused, "the cut-off agent hears no answer");
}

#[test]
fn an_agents_silent_command_is_not_done_until_its_prompt_is_back_and_wait_follows_it() {
    use annulus_control::client::Client;
    use annulus_control::protocol::{RunOutput, RunState};
    let mut app = NativeApp::new(Some(fixture_vault("agent-silent", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    // Three seconds of silence, then one line, in the shell an agent's tab opens.
    let shell = app
        .profiles
        .first()
        .map(|profile| profile.name.clone())
        .unwrap_or_default();
    let command = if !cfg!(windows) {
        "sleep 3; echo slept-then-said"
    } else if shell.contains("PowerShell") {
        "Start-Sleep -Seconds 3; Write-Output slept-then-said"
    } else {
        "ping -n 4 127.0.0.1 >nul & echo slept-then-said"
    };
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        // Its wait runs out in the silence: answered, but not as done.
        let early = client
            .run(tab.id, command, Some(1))
            .map_err(|e| e.to_string())?;
        let late = client.wait(tab.id, Some(30)).map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, early, late))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, early, late) = agent.join().unwrap().unwrap();
    // The typed command holds the word too: only a line of its own is the answer.
    let said = |ran: &RunOutput| {
        ran.output
            .lines()
            .any(|line| line.trim() == "slept-then-said")
    };
    assert_eq!(early.state, RunState::Running, "{early:?}");
    assert!(!said(&early), "{early:?}");
    assert_eq!(late.state, RunState::Done, "{late:?}");
    assert!(said(&late), "the line after the silence: {late:?}");
    assert!(
        late.millis >= 2_000,
        "done once its prompt came back, after the silence: {late:?}"
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref())
        .unwrap();
    assert!(
        matches!(log.commands[0].state, super::control::Ran::Done { .. }),
        "its row turned done too"
    );
}

#[test]
fn an_agents_tab_opened_behind_starts_unseen_and_closes_once_its_shell_ends() {
    use annulus_control::client::Client;
    use annulus_control::protocol::RunState;
    let mut app = NativeApp::new(Some(fixture_vault("agent-behind", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    app.settings.agent_tabs_in_front = false;
    app.settings.close_ended_agent_tabs = true;
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let in_use = app.tabs[app.active].uid;
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        let said = client
            .run(tab.id, "echo unseen-ok", Some(30))
            .map_err(|e| e.to_string())?;
        let left = client
            .run(tab.id, "exit", Some(30))
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, said, left))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, said, left) = agent.join().unwrap().unwrap();
    assert_eq!(app.tabs[app.active].uid, in_use, "it never came in front");
    assert_eq!(said.state, RunState::Done, "its shell ran unseen: {said:?}");
    assert!(
        said.output.lines().any(|line| line.trim() == "unseen-ok"),
        "{said:?}"
    );
    assert_eq!(left.state, RunState::Ended, "{left:?}");
    step(&mut app, &mut state, &[], time);
    assert!(
        !app.tabs.iter().any(|open| open.uid == tab),
        "closed once its shell ended"
    );
}

#[test]
fn the_folded_agent_rail_is_a_card_clear_of_the_window_edge_and_its_tally_opens_the_panel() {
    let mut app = NativeApp::new(Some(fixture_vault("agent-rail", false)));
    agent_tab_folded(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let stop = output
        .zones
        .iter()
        .find(|zone| zone.label == "Stop all agents")
        .map(|zone| zone.rect)
        .expect("Stop sits on the rail");
    // A card like the terminal's beside it, clear of the window's right and bottom edges
    // (the drawn window is 1100 by 800).
    assert!(stop.r() <= 1100.0 - super::PANEL_GAP, "{stop:?}");
    assert!(stop.b() <= 800.0 - super::PANEL_GAP, "{stop:?}");
    let tally = output
        .zones
        .iter()
        .find(|zone| zone.label.contains("5 commands"))
        .map(|zone| zone.label.clone())
        .expect("the rail says how many commands, and how many still run");
    assert!(tally.contains("2 running"), "{tally}");
    step(&mut app, &mut state, &click_on(&output, &tally), 1.0);
    assert!(
        app.tabs[super::FIXED_TABS]
            .agent
            .as_ref()
            .is_some_and(|log| !log.folded),
        "a click on the tally opens the panel"
    );
}

#[test]
fn a_host_pinned_from_a_new_tab_heads_the_next_one_and_a_second_click_unpins_it() {
    let mut app = NativeApp::new(Some(fixture_vault("new-tab-pin", true)));
    app.open_new_tab();
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    // The highlighted (first) row shows its pin.
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Pin to the top of New Tab"),
        1.0,
    );
    assert_eq!(app.settings.pinned.len(), 1, "pinned");
    let pinned = app.settings.pinned[0];
    assert_eq!(
        app.tabs[app.active].kind,
        super::TabKind::NewTab,
        "the page stays to pin more"
    );
    let output = step(&mut app, &mut state, &[], 2.0);
    let label = app
        .vault
        .as_ref()
        .unwrap()
        .hosts()
        .iter()
        .find(|host| host.id == pinned)
        .unwrap()
        .label
        .clone();
    let first = output
        .zones
        .iter()
        .find(|zone| zone.role == Role::ListItem)
        .map(|zone| zone.label.clone());
    assert_eq!(first, Some(label), "it heads the list");
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Unpin from New Tab"),
        3.0,
    );
    assert!(app.settings.pinned.is_empty(), "and unpins");
}

#[test]
fn an_empty_vaults_new_tab_welcomes_and_offers_the_ssh_config_import() {
    let mut app = NativeApp::new(Some(fixture_vault("new-tab-welcome", false)));
    app.open_new_tab();
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "New host" && zone.role == Role::Button)
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Import ssh_config"),
        1.0,
    );
    assert_eq!(app.active, 0, "to the vault");
    assert_eq!(app.section, super::Section::Hosts);
    assert!(
        !app.tabs
            .iter()
            .any(|tab| tab.kind == super::TabKind::NewTab),
        "the new tab gave way"
    );
}

#[test]
fn a_tabs_menu_closes_the_tabs_to_its_right_or_all_the_others() {
    let mut app = NativeApp::new(Some(fixture_vault("tab-menu", false)));
    for title in ["a", "b", "c", "d"] {
        replay(&mut app, title);
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &right_click_on(&output, "b"), 1.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Close tabs to the right"),
        2.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "a", "b"]);
    assert_eq!(app.tabs[app.active].title, "b");
    replay(&mut app, "e");
    let output = step(&mut app, &mut state, &[], 3.0);
    let output = step(&mut app, &mut state, &right_click_on(&output, "b"), 4.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Close other tabs"),
        5.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "b"]);
}

#[test]
fn a_tab_moves_among_the_sessions_and_the_one_in_front_stays_in_front() {
    let mut app = NativeApp::new(None);
    for title in ["a", "b", "c"] {
        replay(&mut app, title);
    }
    app.active = 2;
    app.move_tab(2, 4);
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "c", "a"]);
    assert_eq!(app.tabs[app.active].title, "a");
    // Fixed tabs never move, and nothing moves in among them.
    app.move_tab(0, 3);
    app.move_tab(4, 1);
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "c", "a"]);
    // By keyboard: the tab in front, one place left.
    let mut state = UiState::default();
    app.vault = Some(fixture_vault("move-tab", false));
    step(&mut app, &mut state, &[], 0.0);
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::PageUp, Modifiers::CTRL_SHIFT)],
        1.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "a", "c"]);
    assert_eq!(app.tabs[app.active].title, "a");
}

#[test]
fn a_tab_dragged_past_its_neighbour_takes_its_place() {
    let mut app = NativeApp::new(Some(fixture_vault("drag-tab", false)));
    for title in ["first", "second"] {
        replay(&mut app, title);
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let from = zone_center(&output.zones, "first");
    let past = zone_center(&output.zones, "second");
    let drag = [
        Event::PointerMoved(from),
        Event::PointerDown(Button::Primary),
        Event::PointerMoved(Point::new(past.x + 40.0, past.y)),
    ];
    step(&mut app, &mut state, &drag, 1.0);
    step(
        &mut app,
        &mut state,
        &[Event::PointerUp(Button::Primary)],
        2.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "second", "first"]);
    assert_eq!(
        app.tabs[app.active].title, "first",
        "the dragged tab is in front"
    );
}

#[test]
fn idle_time_at_or_past_the_timeout_locks() {
    let mut app = NativeApp::new(Some(fixture_vault("idle", false)));
    app.settings.auto_lock_minutes = 1;
    let mut state = UiState::default();
    // The clock may start at zero: that first frame still counts as the last input.
    drive(&mut app, &mut state, &[], 0.0);
    drive(&mut app, &mut state, &[], 59.5);
    assert!(app.vault.is_some(), "a minute has not passed");
    drive(&mut app, &mut state, &[], 60.0);
    assert!(app.vault.is_none(), "idle for the whole minute: locked");
}

#[test]
fn locking_drops_the_unlocked_hosts() {
    let mut app = NativeApp::new(Some(fixture_vault("lock-drops", true)));
    if let Some(vault) = &app.vault {
        let id = vault.hosts()[0].id;
        app.hosts.edit_host(vault, id);
    }
    assert!(app.hosts.editor.is_some());
    app.lock_vault();
    assert!(app.vault.is_none());
    assert!(
        app.hosts.editor.is_none(),
        "the editor's draft (and password) went too"
    );
}

#[test]
fn settings_is_last_so_it_sits_at_the_bottom() {
    assert!(!super::Section::MAIN.contains(&super::Section::Settings));
    let mut app = NativeApp::new(Some(fixture_vault("nav", false)));
    let mut state = UiState::default();
    let output = drive(&mut app, &mut state, &[], 0.0);
    let logs = zone_center(&output.zones, "Logs");
    let settings = zone_center(&output.zones, "Settings");
    assert!(
        settings.y > logs.y && settings.y > 800.0 - 60.0,
        "{settings:?}"
    );
}

/// The panes of the terminal tab in front.
fn front_panes(app: &NativeApp) -> &super::panes::Panes {
    match app
        .tabs
        .get(app.active)
        .and_then(|tab| tab.terminal.as_ref())
    {
        Some(TermView::Live(panes)) => panes,
        _ => panic!("no live terminal tab in front"),
    }
}

#[test]
fn focus_mode_and_broadcast_are_a_click_or_a_shortcut_away() {
    let mut app = NativeApp::new(Some(fixture_vault("split-panes", false)));
    split_tab(&mut app);
    let uid = app.tabs[app.active].uid;
    let terminal = |pane: u64| annulus_ui::kit::Id::new(("terminal", uid, pane));
    let viewport = Rect::new(0.0, 0.0, 1100.0, 800.0);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Focus mode: one large pane beside the list; a row brings its pane in front, keyboard
    // and all.
    output = step(&mut app, &mut state, &click_on(&output, "Focus mode"), 1.0);
    assert!(front_panes(&app).in_focus_mode());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "bastion, Disconnected or failed"),
        2.0,
    );
    assert_eq!(front_panes(&app).focused, 4);
    assert_eq!(state.focus(), Some(terminal(4)));
    let shown = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::Terminal)
        .count();
    assert_eq!(shown, 1, "only the large pane is drawn");
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Ctrl+Shift+M goes back to the split.
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Char('m'), Modifiers::CTRL_SHIFT)],
        3.0,
    );
    assert!(!front_panes(&app).in_focus_mode());
    // Broadcast types into the other three, until one is left out with its own toggle.
    output = step(&mut app, &mut state, &click_on(&output, "Broadcast"), 4.0);
    assert!(front_panes(&app).broadcasting());
    assert_eq!(front_panes(&app).receivers(), [1, 3, 2]);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Broadcast to web-1"),
        5.0,
    );
    assert_eq!(front_panes(&app).receivers(), [1, 3]);
    let toggle = output
        .zones
        .iter()
        .find(|zone| zone.label == "Broadcast to web-1")
        .unwrap();
    assert_eq!(toggle.toggled, Some(false));
    assert_eq!(
        state.focus(),
        Some(terminal(4)),
        "the keyboard stays with the pane typed in"
    );
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Ctrl+Alt+Shift+B leaves the focused pane (bastion) out by keyboard; another pane typed
    // in then does not reach it, and the same keys take it back.
    let leave_out = Modifiers {
        alt: true,
        ..Modifiers::CTRL_SHIFT
    };
    let into = |output: &FrameOutput, pane: u64| {
        let zone = output
            .zones
            .iter()
            .find(|zone| zone.id == terminal(pane))
            .unwrap();
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Char('b'), leave_out)],
        5.5,
    );
    output = step(&mut app, &mut state, &into(&output, 1), 5.6);
    assert_eq!(front_panes(&app).receivers(), [3], "bastion is left out");
    step(&mut app, &mut state, &into(&output, 4), 5.7);
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Char('b'), leave_out)],
        5.8,
    );
    step(&mut app, &mut state, &into(&output, 1), 5.9);
    assert_eq!(front_panes(&app).receivers(), [3, 4], "back in");
    assert!(
        front_panes(&app).broadcasting(),
        "broadcast itself stays on"
    );
    // Ctrl+Shift+B stops it.
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Char('b'), Modifiers::CTRL_SHIFT)],
        6.0,
    );
    assert!(!front_panes(&app).broadcasting());
}

#[test]
fn connecting_or_unlocking_brings_the_accounts_latest_at_once() {
    let mut app = NativeApp::new(Some(fixture_vault("sync-on-connect", true)));
    app.sync.last_round = 5.0;
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.act(super::hosts::HostsAction::Connect(host));
    assert!(
        app.sync.last_round.is_infinite(),
        "a round on the next frame"
    );
    app.sync.last_round = 5.0;
    app.lock_vault();
    assert!(
        app.sync.last_round.is_infinite(),
        "and the first frame after an unlock"
    );
}
