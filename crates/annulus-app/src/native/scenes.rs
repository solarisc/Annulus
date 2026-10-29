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
        let (output, _) = frame(&events, time, false);
        faults.extend(output.faults.iter().cloned());
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
    let zone = |label: &str, rect: Rect| Zone {
        id: annulus_ui::kit::Id::new(label),
        rect,
        role: Role::Button,
        label: label.into(),
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
