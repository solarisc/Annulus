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

use annulus_config::tokens::Appearance;
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_ui::gfx::{Canvas, Point, Rect};
use annulus_ui::kit::{Button, Event, FrameOutput, Key, Modifiers, Role, Theme, Ui, UiState, Zone};

use super::demo::fixture_vault;
use super::hosts::Mode;
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
        name: "sftp",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Hover("deploy.sh")],
    },
    Scene {
        name: "sftp-menu",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: sftp,
        acts: &[Act::Click("deploy.sh"), Act::Click("more")],
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
            Act::Click("more"),
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
            Act::Click("more"),
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
    output = step(&mut app, &mut state, &click_on(&output, "more"), 5.0);
    output = step(&mut app, &mut state, &click_on(&output, "Delete"), 6.0);
    assert!(card_up(&output), "the menu asks too");
    output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 7.0);
    assert!(!card_up(&output));
}

#[test]
fn a_snippet_runs_on_the_hosts_ticked_a_tab_each() {
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
    // One frame: the tabs are added at its end, and start connecting when first drawn.
    drive(
        &mut app,
        &mut state,
        &click_on(&output, "Run on 2 hosts"),
        4.0,
    );
    assert_eq!(titles(&app), ["Vault", "api-1", "bastion"]);
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
    assert_eq!(next.tabs.len(), 2, "the host's tab is open again");
    assert_eq!(next.active, 1);
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
    app.run_action(Action::Tab(1));
    assert_eq!(app.active, 1);
    app.run_action(Action::Settings);
    assert_eq!((app.active, app.section), (0, super::Section::Settings));
    app.run_action(Action::NewLocal);
    assert_eq!(app.opens.len(), 1, "a local shell opens after this frame");
}

#[test]
fn the_vault_tab_cannot_be_closed_and_closing_moves_focus_left() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.close_tab(0);
    assert_eq!(app.tabs.len(), 3);
    app.close_tab(2);
    assert_eq!((app.tabs.len(), app.active), (2, 1));
    app.close_tab(1);
    assert_eq!((titles(&app), app.active), (vec!["Vault"], 0));
}

#[test]
fn closing_a_background_tab_keeps_the_front_tab() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.active = 2;
    app.close_tab(1);
    assert_eq!(app.tabs[app.active].title, "b");
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
