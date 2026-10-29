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

use annulus_config::settings::OpenTab;
use annulus_config::tokens::Appearance;
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_session::{Password, Program};
use annulus_ui::gfx::{Canvas, Point, Rect};
use annulus_ui::kit::{Button, Event, FrameOutput, Key, Modifiers, Role, Theme, Ui, UiState, Zone};
use annulus_vault::hosts::Host;

use super::demo::fixture_vault;
use super::hosts::{HostsAction, Mode};
use super::{NativeApp, TermView};
use crate::terminal_view::TabStatus;

/// One step of a scene's script, applied between frames.
#[derive(Clone, Debug)]
enum Act {
    /// Move the pointer over the zone with this accessible name.
    Hover(&'static str),
    /// Click it.
    Click(&'static str),
    /// Type text into whatever has focus.
    Type(&'static str),
    /// Press a key.
    Press(Key, Modifiers),
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

fn settings(app: &mut NativeApp) {
    section(app, super::Section::Settings);
}

fn settings_account(app: &mut NativeApp) {
    settings(app);
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    // Made up, so no photo shows the CI machine's name.
    "Work laptop".clone_into(&mut app.sync.device);
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
    app.active = 1;
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
        acts: &[Act::Click("chevron-down")],
    },
    Scene {
        name: "hosts-more-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Hover("prod-db"), Act::Click("more")],
    },
    Scene {
        name: "hosts-list-more-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: list_mode,
        acts: &[Act::Hover("grafana"), Act::Click("more")],
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
        name: "settings",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: settings,
        acts: &[],
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
    let theme = Theme::new(appearance);
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
            None => break,
        }
    }
    // Settle: hover fades and focus glows finish.
    let (output, png) = frame(&[], time + 1.0, true);
    faults.extend(output.faults.iter().cloned());
    faults.extend(layout_faults(&output.zones, viewport));
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

#[test]
fn every_scene_renders_without_layout_faults() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderers: Vec<Box<dyn Renderer>> = vec![Box::new(Cpu(None))];
    #[cfg(windows)]
    renderers.push(Box::new(Direct2D(
        annulus_platform::d2d::D2d::new().unwrap(),
    )));
    let mut failures = Vec::new();
    for renderer in &mut renderers {
        for scene in SCENES {
            for appearance in [Appearance::Dark, Appearance::Light] {
                let (png, faults) = render(scene, appearance, renderer.as_mut());
                let theme = match appearance {
                    Appearance::Dark => "dark",
                    Appearance::Light => "light",
                };
                let name = format!("{}-{theme}{}", scene.name, renderer.suffix());
                std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
                for fault in faults {
                    failures.push(format!("{name}: {fault}"));
                }
            }
        }
    }
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

#[test]
fn a_hosts_more_menu_copies_its_ssh_command_and_duplicates_it() {
    let mut app = NativeApp::new(Some(fixture_vault("more-menu", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    // Selecting the card shows its buttons, More among them.
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 1.0);
    output = step(&mut app, &mut state, &click_on(&output, "more"), 2.0);
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
    output = step(&mut app, &mut state, &click_on(&output, "more"), 4.0);
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
fn a_hovered_cards_more_menu_stays_open_after_the_hover_fades() {
    let mut app = NativeApp::new(Some(fixture_vault("more-stays", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let on_card = [Event::PointerMoved(zone_center(&output.zones, "prod-db"))];
    output = step(&mut app, &mut state, &on_card, 1.0);
    output = step(&mut app, &mut state, &click_on(&output, "more"), 2.0);
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
        .skip(1)
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
    let more = click_within(&output, "prod-db", "more");
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
    let more = click_within(&output, "prod-db", "more");
    output = step(&mut app, &mut state, &more, 6.0);
    let menu = click_on(&output, "Move to the top level");
    step(&mut app, &mut state, &menu, 7.0);
    assert_eq!(host_named(&app, "prod-db").group, None);
}

#[test]
fn duplicate_saves_a_copy_with_a_new_id_and_the_same_address() {
    // From a row of the list, whose More menu is its own.
    let mut app = NativeApp::new(Some(fixture_vault("duplicate", true)));
    app.hosts.mode = Mode::List;
    let before = app.vault.as_ref().unwrap().hosts().len();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let hover = [Event::PointerMoved(zone_center(&output.zones, "grafana"))];
    output = step(&mut app, &mut state, &hover, 1.0);
    let more = click_within(&output, "grafana", "more");
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

/// End to end through the deployed sync server: the new UI's Settings create an account
/// from typed input (server, name, the vault's password) and the vault syncs to it.
#[test]
#[ignore = "needs the deployed sync server (ANNULUS_E2E_SYNC_URL)"]
fn e2e_the_new_settings_create_a_sync_account_and_sync() {
    let url = std::env::var("ANNULUS_E2E_SYNC_URL").expect("ANNULUS_E2E_SYNC_URL");
    let vault = fixture_vault("e2e-account", true);
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
        ("This vault's master password", "pw"),
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
        at("chevron-down"),
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
fn open_tabs_are_remembered_and_reopened_at_the_next_start() {
    let mut app = NativeApp::new(Some(fixture_vault("restore", true)));
    app.settings.restore_tabs = true;
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.act(super::hosts::HostsAction::Connect(host));
    let mut state = UiState::default();
    drive(&mut app, &mut state, &[], 0.0);
    assert!(
        app.settings
            .open_tabs
            .contains(&annulus_config::settings::OpenTab::Host(host))
    );
    // The next start: a host tab waits for the vault, then reconnects.
    let mut next = NativeApp::new(None);
    let tabs = app.settings.open_tabs.clone();
    next.reopen(&tabs);
    assert_eq!(next.pending_hosts, [host]);
    next.vault = app.vault.take();
    drive(&mut next, &mut state, &[], 1.0);
    drive(&mut next, &mut state, &[], 2.0);
    assert_eq!(next.pending_hosts, Vec::<u64>::new());
    assert_eq!(next.tabs.len(), 2, "the host's tab is open again");
}
