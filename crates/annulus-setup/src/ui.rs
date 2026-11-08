//! The installer's window, drawn with our own toolkit like the app (ADR-0017): where to
//! install and what to add, then a bar while it works, then done; or, started from Apps &
//! features, what removing takes away.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use annulus_config::tokens::{Appearance, space};
use annulus_platform::window::{Waker, WindowApp, WindowRequest};
use annulus_ui::gfx::{Align, Canvas, Color, Font, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Role, Theme, Ui};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, CONTROL, FieldStyle, Kind, SMALL, TITLE, button_width,
};

use crate::install::{self, Choices, WindowsDesk};
use crate::payload;

/// The zone that moves the window.
const DRAG: &str = "setup.drag";
/// No maximize button: the window keeps its size.
const MAXIMIZE: &str = "setup.maximize";

/// What the window is for.
pub enum Mode {
    /// Install, or update what is there.
    Install,
    /// Remove what is in this folder (started from Apps & features).
    Remove(PathBuf),
}

/// How far it got.
enum Stage {
    /// The first page: choices, or what removing takes.
    Ask,
    /// At work on its own thread: how far (0 to 1) and doing what, and how it ended.
    Working(Arc<Mutex<(f32, String)>>, Receiver<Result<(), String>>),
    Done,
    Failed(String),
}

/// The installer.
pub struct Setup {
    mode: Mode,
    /// This program, copied as the uninstaller (or removed, when it is one).
    me: PathBuf,
    choices: Choices,
    /// The folder as typed.
    dir: String,
    stage: Stage,
    waker: Option<Waker>,
    appearance: Appearance,
    /// The window should close once this frame is drawn.
    closing: bool,
}

impl Setup {
    /// The installer in `mode`, run from `me`.
    #[must_use]
    pub fn new(mode: Mode, me: PathBuf) -> Self {
        let choices = Choices::new();
        Self {
            dir: choices.dir.display().to_string(),
            choices,
            mode,
            me,
            stage: Stage::Ask,
            waker: None,
            appearance: os_appearance(),
            closing: false,
        }
    }

    /// The title the window takes.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self.mode {
            Mode::Install => "Install Annulus",
            Mode::Remove(_) => "Remove Annulus",
        }
    }

    /// Starts installing (or removing) on a thread of its own.
    fn start(&mut self) {
        if !cfg!(windows) {
            self.stage = Stage::Failed(install::NOT_HERE.into());
            return;
        }
        let progress = Arc::new(Mutex::new((0.0, String::from("Starting"))));
        let (done, ended) = channel();
        let waker = self.waker.clone();
        let wake = move || {
            if let Some(waker) = &waker {
                waker();
            }
        };
        let shared = progress.clone();
        let me = self.me.clone();
        let job: Box<dyn FnOnce() -> Result<(), String> + Send> = match &self.mode {
            Mode::Install => {
                self.choices.dir = PathBuf::from(self.dir.trim());
                let choices = self.choices.clone();
                let wake = wake.clone();
                Box::new(move || {
                    install::install(
                        &choices,
                        payload::FILES,
                        &me,
                        &mut WindowsDesk,
                        &mut |share, what| {
                            if let Ok(mut now) = shared.lock() {
                                *now = (share, what.to_owned());
                            }
                            wake();
                        },
                    )
                })
            }
            Mode::Remove(dir) => {
                let dir = dir.clone();
                Box::new(move || install::uninstall(&dir, &installed_names(), &mut WindowsDesk))
            }
        };
        let spawned = std::thread::Builder::new()
            .name("setup".into())
            .spawn(move || {
                let _ = done.send(job());
                wake();
            });
        self.stage = match spawned {
            Ok(_) => Stage::Working(progress, ended),
            Err(error) => Stage::Failed(format!("Could not start: {error}")),
        };
    }

    /// What the first page asks, the bar while it works, or how it ended; returns what the
    /// window should do.
    #[allow(clippy::too_many_lines)] // Four pages, each top to bottom.
    fn page(&mut self, ui: &mut Ui<'_>, area: Rect) {
        let t = ui.theme;
        if let Stage::Working(_, ended) = &self.stage
            && let Ok(result) = ended.try_recv()
        {
            self.stage = match result {
                Ok(()) => Stage::Done,
                Err(why) => Stage::Failed(why),
            };
        }
        let mut column = area.inset(space::XL, space::L);
        let mut buttons = Rect::new(column.x, column.b() - CONTROL, column.w, CONTROL);
        // The mark and the name.
        let mut head = column.take_top(56.0);
        let mark = head.take_left(56.0);
        let colors = annulus_ui::mark::Colors::for_theme(&ui.theme);
        annulus_ui::mark::paint(&mut ui.gfx, mark, &colors);
        head.take_left(space::M);
        let (title, line) = match (&self.mode, &self.stage) {
            (Mode::Install, Stage::Done) => ("Annulus is installed", self.dir.clone()),
            (Mode::Remove(_), Stage::Done) => ("Annulus is removed", String::new()),
            (Mode::Install, _) => (
                "Install Annulus",
                format!("Version {}", env!("CARGO_PKG_VERSION")),
            ),
            (Mode::Remove(dir), _) => ("Remove Annulus", dir.display().to_string()),
        };
        let mut words = head.inset(0.0, 4.0);
        ui.strict_text(words.take_top(30.0), title, TITLE, t.text, Align::Start);
        let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &line, SMALL, words.w);
        ui.gfx
            .text(words.take_top(18.0), &fitted, SMALL, t.text2, Align::Start);
        column.take_top(space::L);
        let say = |ui: &mut Ui<'_>, column: &mut Rect, text: &str| {
            ui.gfx
                .text(column.take_top(20.0), text, BODY, t.text2, Align::Start);
        };
        match (&self.mode, &mut self.stage) {
            (Mode::Install, Stage::Ask) => {
                say(
                    ui,
                    &mut column,
                    "Annulus and its command-line agent, annulus-agent,",
                );
                say(
                    ui,
                    &mut column,
                    "for you on this PC. No administrator is needed.",
                );
                column.take_top(space::L);
                ui.gfx.text(
                    column.take_top(18.0),
                    "Install to",
                    SMALL,
                    t.text2,
                    Align::Start,
                );
                column.take_top(space::XS);
                let field_id = Id::new("setup.dir");
                ui.text_field(
                    field_id,
                    column.take_top(CONTROL),
                    &mut self.dir,
                    FieldStyle {
                        mono: true,
                        ..FieldStyle::default()
                    },
                );
                ui.set_name(field_id, "Install to");
                column.take_top(space::M);
                for (id, value, label) in [
                    (
                        "setup.menu",
                        &mut self.choices.start_menu,
                        "A shortcut in the Start menu",
                    ),
                    (
                        "setup.desktop",
                        &mut self.choices.desktop,
                        "A shortcut on the desktop",
                    ),
                    (
                        "setup.path",
                        &mut self.choices.path,
                        "annulus-agent in any terminal (adds the folder to your PATH)",
                    ),
                    (
                        "setup.open",
                        &mut self.choices.open_after,
                        "Open Annulus when done",
                    ),
                ] {
                    ui.checkbox(Id::new(id), column.take_top(28.0), value, label);
                }
                let valid = PathBuf::from(self.dir.trim()).is_absolute();
                if !valid {
                    ui.gfx.text(
                        column.take_top(18.0),
                        "A full path, such as C:\\Users\\you\\Programs\\Annulus.",
                        SMALL,
                        t.error,
                        Align::Start,
                    );
                }
                let install =
                    buttons.take_right(button_width(&mut ui.gfx, "Install", None).max(104.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                if ui
                    .button(Id::new("setup.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE)
                {
                    self.closing = true;
                }
                let go = ui
                    .button(
                        Id::new("setup.install"),
                        install,
                        "Install",
                        None,
                        Kind::Primary,
                    )
                    .clicked;
                if go && valid {
                    self.start();
                }
            }
            (Mode::Remove(dir), Stage::Ask) => {
                say(
                    ui,
                    &mut column,
                    "Takes away the app, annulus-agent, the shortcuts and",
                );
                say(
                    ui,
                    &mut column,
                    "the PATH entry. Your vault and settings stay.",
                );
                column.take_top(space::S);
                let folder = format!("From {}", dir.display());
                let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &folder, SMALL, column.w);
                ui.gfx
                    .text(column.take_top(18.0), &fitted, SMALL, t.text3, Align::Start);
                let remove =
                    buttons.take_right(button_width(&mut ui.gfx, "Remove", None).max(104.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                if ui
                    .button(Id::new("setup.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE)
                {
                    self.closing = true;
                }
                if ui
                    .button(
                        Id::new("setup.remove"),
                        remove,
                        "Remove",
                        None,
                        Kind::Danger,
                    )
                    .clicked
                {
                    self.start();
                }
            }
            (_, Stage::Working(progress, _)) => {
                let (share, what) = progress.lock().map(|now| now.clone()).unwrap_or_default();
                column.take_top(space::L);
                ui.gfx
                    .text(column.take_top(20.0), &what, BODY, t.text2, Align::Start);
                column.take_top(space::S);
                let track = column.take_top(6.0);
                let shown = ui.animate(
                    Id::new("setup.bar"),
                    share,
                    annulus_ui::motion::Motion::BASE,
                );
                ui.gfx.fill_rect(track, 3.0, t.hover);
                let fill = Rect::new(track.x, track.y, track.w * shown.clamp(0.0, 1.0), 6.0);
                ui.gfx.fill_rect(fill, 3.0, t.accent);
                ui.request_repaint();
            }
            (mode, Stage::Done) => {
                let badge = column.take_top(40.0).take_left(40.0);
                ui.gfx.fill_rect(badge, 20.0, t.ok.alpha(0.16));
                ui.gfx.icon(icons::CHECK, badge.centered(20.0, 20.0), t.ok);
                column.take_top(space::M);
                match mode {
                    Mode::Install => {
                        say(
                            ui,
                            &mut column,
                            "Open it from the Start menu, or run annulus-agent",
                        );
                        say(ui, &mut column, "in a terminal opened from now on.");
                    }
                    Mode::Remove(_) => {
                        say(
                            ui,
                            &mut column,
                            "Your vault and settings are still on this PC, so",
                        );
                        say(ui, &mut column, "installing again finds them as they were.");
                    }
                }
                let label = match mode {
                    Mode::Install if self.choices.open_after => "Open Annulus",
                    _ => "Close",
                };
                let done = buttons.take_right(button_width(&mut ui.gfx, label, None).max(104.0));
                let finish = ui
                    .button(Id::new("setup.finish"), done, label, None, Kind::Primary)
                    .clicked
                    || ui.key(Key::Enter, Modifiers::NONE);
                if finish {
                    if matches!(mode, Mode::Install) && self.choices.open_after {
                        let app = self.choices.dir.join(install::APP);
                        let _ = std::process::Command::new(app)
                            .current_dir(&self.choices.dir)
                            .spawn();
                    }
                    self.closing = true;
                }
            }
            (_, Stage::Failed(why)) => {
                let mut line = column.take_top(24.0);
                ui.gfx.icon(
                    icons::WARNING,
                    line.take_left(20.0).centered(16.0, 16.0),
                    t.error,
                );
                line.take_left(space::S);
                ui.gfx.text(
                    line,
                    "That did not work",
                    BODY_STRONG,
                    t.error,
                    Align::Start,
                );
                column.take_top(space::S);
                for part in wrap(&mut ui.gfx, why, BODY, column.w) {
                    ui.gfx
                        .text(column.take_top(20.0), &part, BODY, t.text2, Align::Start);
                }
                let again =
                    buttons.take_right(button_width(&mut ui.gfx, "Try again", None).max(104.0));
                buttons.take_right(space::S);
                let close = buttons.take_right(button_width(&mut ui.gfx, "Close", None));
                if ui
                    .button(Id::new("setup.close"), close, "Close", None, Kind::Ghost)
                    .clicked
                {
                    self.closing = true;
                }
                if ui
                    .button(
                        Id::new("setup.again"),
                        again,
                        "Try again",
                        None,
                        Kind::Primary,
                    )
                    .clicked
                {
                    self.stage = Stage::Ask;
                }
            }
        }
    }

    /// The title bar: it moves the window, and its button closes it (not while at work).
    fn title_bar(&mut self, ui: &mut Ui<'_>, bar: Rect) {
        let t = ui.theme;
        ui.zone(Id::new(DRAG), bar, Role::Backdrop, self.title());
        let mut row = bar;
        let close = row.take_right(46.0);
        ui.gfx.text(
            row.inset(space::L, 0.0),
            self.title(),
            SMALL,
            t.text2,
            Align::Start,
        );
        if matches!(self.stage, Stage::Working(..)) {
            return;
        }
        let id = Id::new("setup.window.close");
        let response = ui.zone(id, close, Role::WindowButton, "Close");
        let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let red = Color::rgba(0xC4, 0x2B, 0x1C, 255);
        ui.gfx.fill_rect(close, 0.0, red.alpha(hover));
        let ink = t.text2.mix(Color::WHITE, hover);
        let c = close.center();
        let s = 5.0;
        for (from, to) in [((-s, -s), (s, s)), ((-s, s), (s, -s))] {
            ui.gfx.polyline(
                &[
                    Point::new(c.x + from.0, c.y + from.1),
                    Point::new(c.x + to.0, c.y + to.1),
                ],
                false,
                1.0,
                ink,
            );
        }
        if response.clicked {
            self.closing = true;
        }
    }
}

impl WindowApp for Setup {
    fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    fn frame(&mut self, ui: &mut Ui<'_>, _maximized: bool) -> Option<WindowRequest> {
        let t = ui.theme;
        let mut area = ui.viewport;
        ui.gfx.fill_rect(area, 0.0, t.bg);
        let bar = area.take_top(40.0);
        self.title_bar(ui, bar);
        self.page(ui, area);
        ui.overlays();
        if self.closing {
            if let (Mode::Remove(dir), Stage::Done) = (&self.mode, &self.stage) {
                install::schedule_self_removal(&self.me, dir);
            }
            return Some(WindowRequest::Close);
        }
        None
    }

    fn drag_zone(&self) -> Id {
        Id::new(DRAG)
    }

    fn maximize_zone(&self) -> Id {
        Id::new(MAXIMIZE)
    }

    fn theme(&self) -> Theme {
        Theme::new(self.appearance)
    }
}

/// The names the installer puts in its folder, to take away again.
fn installed_names() -> Vec<&'static str> {
    if payload::FILES.is_empty() {
        vec![
            install::APP,
            "annulus-agent.exe",
            "LICENSE-MIT",
            "LICENSE-APACHE",
        ]
    } else {
        payload::FILES.iter().map(|(name, _, _)| *name).collect()
    }
}

/// Light or dark, as Windows shows apps.
fn os_appearance() -> Appearance {
    #[cfg(windows)]
    {
        if annulus_platform::install::apps_use_light_theme() {
            return Appearance::Light;
        }
    }
    Appearance::Dark
}

/// `text` broken at spaces into lines no wider than `width` in `font`.
fn wrap<C: Canvas + ?Sized>(canvas: &mut C, text: &str, font: Font, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let tried = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if canvas.measure(&tried, font).width > width && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = tried;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_ui::kit::{Button, Event, UiState};

    use super::*;

    /// Draws `setup` once with `events`; the frame's zones and what it asked of the window.
    fn frame(
        setup: &mut Setup,
        state: &mut UiState,
        events: &[Event],
        time: f64,
    ) -> (Vec<annulus_ui::kit::Zone>, Option<WindowRequest>) {
        let mut canvas = CpuCanvas::new(560, 480, 1.0, TextEngine::new()).unwrap();
        state.begin_frame(events, time);
        let viewport = Rect::new(0.0, 0.0, 560.0, 480.0);
        let mut ui = Ui::new(&mut canvas, Theme::new(Appearance::Dark), state, viewport);
        let asked = setup.frame(&mut ui, false);
        (state.end_frame().zones, asked)
    }

    fn click(zones: &[annulus_ui::kit::Zone], name: &str) -> Vec<Event> {
        let zone = zones.iter().rev().find(|zone| zone.label == name).unwrap();
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    }

    #[test]
    fn the_first_page_offers_the_folder_and_what_to_add_and_cancel_closes() {
        let mut setup = Setup::new(Mode::Install, PathBuf::from("annulus-setup.exe"));
        let mut state = UiState::default();
        let (zones, asked) = frame(&mut setup, &mut state, &[], 0.0);
        assert_eq!(asked, None);
        for name in [
            "Install to",
            "A shortcut in the Start menu",
            "A shortcut on the desktop",
            "Open Annulus when done",
            "Install",
            "Cancel",
            "Close",
        ] {
            assert!(zones.iter().any(|zone| zone.label == name), "{name}");
        }
        assert!(setup.choices.start_menu && !setup.choices.desktop && setup.choices.path);
        let (zones, _) = frame(
            &mut setup,
            &mut state,
            &click(&zones, "A shortcut on the desktop"),
            1.0,
        );
        assert!(setup.choices.desktop, "a click ticks it");
        let (_, asked) = frame(&mut setup, &mut state, &click(&zones, "Cancel"), 2.0);
        assert_eq!(asked, Some(WindowRequest::Close));
    }

    #[test]
    fn a_folder_that_is_not_a_full_path_is_not_installed_to() {
        let mut setup = Setup::new(Mode::Install, PathBuf::from("annulus-setup.exe"));
        setup.dir = "Annulus".into();
        let mut state = UiState::default();
        let (zones, _) = frame(&mut setup, &mut state, &[], 0.0);
        frame(&mut setup, &mut state, &click(&zones, "Install"), 1.0);
        assert!(matches!(setup.stage, Stage::Ask), "still asking");
    }

    /// A page's name and how to bring the installer to it.
    type Page = (&'static str, fn(&mut Setup));

    /// Every page in both themes, written as PNGs beside the app's scenes for review.
    #[test]
    fn every_page_draws_and_is_photographed() {
        let shots = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/scenes");
        std::fs::create_dir_all(&shots).unwrap();
        let folder = r"C:\Users\ana\AppData\Local\Programs\Annulus";
        let pages: [Page; 5] = [
            ("install", |_| {}),
            ("working", |setup| {
                let progress = Arc::new(Mutex::new((0.6, "Copying annulus-agent.exe".into())));
                setup.stage = Stage::Working(progress, channel().1);
            }),
            ("done", |setup| setup.stage = Stage::Done),
            ("failed", |setup| {
                setup.stage = Stage::Failed(
                    "Annulus is in use. Close Annulus (and any annulus-agent running), then try again."
                        .into(),
                );
            }),
            ("remove", |setup| {
                setup.mode = Mode::Remove(PathBuf::from(
                    r"C:\Users\ana\AppData\Local\Programs\Annulus",
                ));
            }),
        ];
        for (name, set) in pages {
            for appearance in [Appearance::Dark, Appearance::Light] {
                let mut setup = Setup::new(Mode::Install, PathBuf::from("annulus-setup.exe"));
                setup.dir = folder.into();
                set(&mut setup);
                let mut state = UiState::default();
                let mut text = Some(TextEngine::new());
                let mut png = Vec::new();
                // The second frame, once things have eased in.
                for time in [0.0, 2.0] {
                    let mut canvas = CpuCanvas::new(560, 480, 1.0, text.take().unwrap()).unwrap();
                    state.begin_frame(&[], time);
                    let viewport = Rect::new(0.0, 0.0, 560.0, 480.0);
                    let mut ui = Ui::new(&mut canvas, Theme::new(appearance), &mut state, viewport);
                    setup.frame(&mut ui, false);
                    let faults = state.end_frame().faults;
                    assert!(faults.is_empty(), "{name}: {faults:?}");
                    png = canvas.encode_png().unwrap();
                    text = Some(canvas.into_text_engine());
                }
                let theme = if appearance == Appearance::Dark {
                    "dark"
                } else {
                    "light"
                };
                std::fs::write(shots.join(format!("setup-{name}-{theme}.png")), png).unwrap();
            }
        }
    }

    #[test]
    fn long_messages_wrap_at_spaces() {
        let mut canvas = CpuCanvas::new(100, 100, 1.0, TextEngine::new()).unwrap();
        let mut state = UiState::default();
        state.begin_frame(&[], 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        let lines = wrap(
            &mut ui.gfx,
            "Annulus is in use. Close Annulus, then try again.",
            BODY,
            120.0,
        );
        assert!(lines.len() > 1, "{lines:?}");
        assert_eq!(
            lines.join(" "),
            "Annulus is in use. Close Annulus, then try again."
        );
    }
}
