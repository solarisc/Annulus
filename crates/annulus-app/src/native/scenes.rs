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
use annulus_config::tokens::{Appearance, ThemeMode};
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_session::{Password, Program};
use annulus_ui::gfx::{Canvas, Point, Rect};
use annulus_ui::kit::{Button, Event, FrameOutput, Key, Modifiers, Role, Theme, Ui, UiState, Zone};
use annulus_vault::hosts::Host;

use super::demo::fixture_vault;
use super::hosts::{HostsAction, Mode};
use super::shortcuts::{self, Shortcut};
use super::{Flyer, NativeApp, TermView, WindowButtons};

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
    /// Press a shortcut's own chord on this computer's keyboard.
    Shortcut(Shortcut),
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

/// The key press of `shortcut`'s own chord on this computer's keyboard.
fn chord(shortcut: Shortcut) -> Event {
    let (key, modifiers) = shortcut.own().expect("a chord");
    Event::Key(key, modifiers)
}

/// `shortcut`'s own chord as this computer's keyboard writes it.
fn own_keys(shortcut: Shortcut) -> String {
    let (key, modifiers) = shortcut.own().expect("a chord");
    shortcuts::chord_text(key, modifiers)
}

mod catalog;
use catalog::*;

pub(super) fn split_panes(with_news: bool) -> super::panes::Panes {
    catalog::split_panes_fixture(with_news)
}

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
        let frame = self
            .0
            .render(width, height, scale, |canvas| draw(canvas))
            .unwrap();
        keep.then(|| frame.png().unwrap())
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
/// A settled frame's focus rings that a clip cut off around a widget in full view: a list
/// that leaves no room for them (`Ui::push_list_clip`).
fn cut_ring_faults(output: &FrameOutput) -> impl Iterator<Item = String> + '_ {
    output
        .cut_rings
        .iter()
        .map(|ring| format!("the focus ring around {ring:?} is cut off at a clip's edge"))
}

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
    // Scenes draw the same frame on every OS; the macOS ones ask for macOS's title bar.
    app.window_buttons = WindowButtons::Ours;
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
            app.system_theme = system;
            let theme = super::colors::app_theme(&app.settings, system);
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
            // What the act opened has arrived by now, with its controls.
            (output, _) = frame(&[], time + 0.9, false);
            faults.extend(output.faults.iter().cloned());
            faults.extend(cut_ring_faults(&output));
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
            Some(Act::Shortcut(shortcut)) => events.push(chord(*shortcut)),
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
    // Settle: what the last input moved comes to rest, hover finds what is now under the
    // pointer, and hover fades and focus glows finish.
    let _ = frame(&[], time + 1.0, false);
    let (output, png) = frame(&[], time + 1.5, true);
    faults.extend(output.faults.iter().cloned());
    faults.extend(cut_ring_faults(&output));
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
        Act::Shortcut(shortcut) => vec![chord(*shortcut)],
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
    // The menu leaves as it came, over a screen that already works.
    Scene {
        name: "motion-menu-close",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[
            Act::Click("More ways to add hosts"),
            Act::Press(Key::Escape, Modifiers::NONE),
        ],
    },
    // A session tab comes in from its side.
    Scene {
        name: "motion-tab-switch",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: with_tabs,
        acts: &[Act::Shortcut(shortcuts::NEXT_TAB)],
    },
    // A key card unfolds above the list, which glides down out of its way.
    Scene {
        name: "motion-key-card",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[Act::Click("Generate key")],
    },
    // ...and folds away again.
    Scene {
        name: "motion-key-card-close",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: keychain,
        acts: &[
            Act::Click("Generate key"),
            Act::Press(Key::Escape, Modifiers::NONE),
        ],
    },
    // A snippet's card, the same.
    Scene {
        name: "motion-snippet-card",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: snippets,
        acts: &[Act::Click("New snippet")],
    },
    // A wizard's next step comes in from the right.
    Scene {
        name: "motion-wizard-step",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: forwards,
        acts: &[
            Act::Click("New rule"),
            Act::Click("Reach a service behind a server"),
        ],
    },
    // Into a group: its hosts fill in as the others go.
    Scene {
        name: "motion-group",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Production")],
    },
    // The tag chips unfold over the hosts.
    Scene {
        name: "motion-tag-chips",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Tags")],
    },
    // A section is left for another: one fades out over the other coming in.
    Scene {
        name: "motion-section-crossfade",
        width: 1100,
        height: 720,
        scale: 1.0,
        populated: true,
        setup: nothing,
        acts: &[Act::Click("Port Forwarding")],
    },
];

/// How long after the input each frame of a filmstrip is taken, in seconds.
const FILM_AT: [f64; 4] = [0.03, 0.07, 0.14, 0.3];

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
            let _ = frame(&[], time + 0.5, false);
            (output, _) = frame(&[], time + 0.9, false);
        }
    }
    frames
}

/// What one act of a scene did on screen.
struct Pop {
    scene: &'static str,
    act: String,
    /// The share of the act's whole change the first frame after it already shows: near 1
    /// for something that just appears, small for something that moves there.
    share: f64,
    /// The share of the window the act changes.
    area: f64,
}

/// The difference between two frames: the total, and the share of pixels that visibly differ.
fn frame_change(a: &[u8], b: &[u8]) -> (f64, f64) {
    let (mut total, mut seen) = (0u64, 0u64);
    for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let d = x[..3]
            .iter()
            .zip(&y[..3])
            .map(|(p, q)| u64::from(p.abs_diff(*q)))
            .sum::<u64>();
        total += d;
        seen += u64::from(d > 24);
    }
    #[allow(clippy::cast_precision_loss)] // Frames are small.
    (total as f64, seen as f64 / (a.len() / 4) as f64)
}

/// [`Pop`]s of every act of `scene` but typing, drawn with the CPU renderer.
fn pops_of(scene: &'static Scene, text: &mut Option<TextEngine>) -> Vec<Pop> {
    let vault = fixture_vault(&format!("pops-{}", scene.name), scene.populated);
    let mut app = NativeApp::new(Some(vault));
    app.window_buttons = WindowButtons::Ours;
    (scene.setup)(&mut app);
    let theme = Theme::new(Appearance::Dark);
    let mut state = UiState::default();
    #[allow(clippy::cast_precision_loss)] // Scene sizes.
    let viewport = Rect::new(0.0, 0.0, scene.width as f32, scene.height as f32);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // Scene sizes.
    let (width, height) = (
        (scene.width as f32 * scene.scale).round() as u32,
        (scene.height as f32 * scene.scale).round() as u32,
    );
    let mut frame = |events: &[Event], time: f64| {
        let engine = text.take().unwrap_or_default();
        let mut canvas = CpuCanvas::new(width, height, scene.scale, engine).unwrap();
        state.begin_frame(events, time);
        let mut ui = Ui::new(&mut canvas, theme, &mut state, viewport);
        app.frame(&mut ui);
        let output = state.end_frame();
        let pixels = canvas.pixels().to_vec();
        *text = Some(canvas.into_text_engine());
        (output, pixels)
    };
    let mut pops = Vec::new();
    let mut time = 0.0;
    let _ = frame(&[], time);
    for act in scene.acts {
        // At rest before the act.
        let _ = frame(&[], time + 1.0);
        let (output, before) = frame(&[], time + 2.5);
        time += 3.0;
        let events = events_for(act, &output.zones);
        let _ = frame(&events, time);
        let (_, first) = frame(&[], time + 0.017);
        let _ = frame(&[], time + 1.0);
        let (_, settled) = frame(&[], time + 2.5);
        time += 3.0;
        if matches!(act, Act::Type(_)) {
            continue;
        }
        if std::env::var("ANNULUS_POPDUMP").is_ok_and(|want| want == scene.name) {
            let dir = out_dir();
            for (name, px) in [
                ("before", &before),
                ("first", &first),
                ("settled", &settled),
            ] {
                let path = dir.join(format!("pop-{}-{name}.rgba", pops.len()));
                std::fs::write(path, px).unwrap();
            }
        }
        let (whole, area) = frame_change(&before, &settled);
        let (left, _) = frame_change(&first, &settled);
        if whole > 0.0 {
            pops.push(Pop {
                scene: scene.name,
                act: format!("{act:?}"),
                share: 1.0 - left / whole,
                area,
            });
        }
    }
    pops
}

/// Whether an act is meant to show at once: the pointer and the keyboard's focus are answered
/// in a moment, which from a distance is the same thing. In focus mode another terminal does
/// fade in, but what changes most there is the old one's many lines leaving the short new one,
/// which the measure cannot tell from a card thrown on.
fn answers_at_once(scene: &str, act: &str) -> bool {
    act.starts_with("Hover")
        || act.starts_with("Press(Tab")
        || (scene == "terminal-focus-mode" && act.contains("Disconnected or failed"))
}

#[cfg(test)]
mod tests;
