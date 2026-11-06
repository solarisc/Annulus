//! The README's clips and the repository's film: the app itself over the demo vault, driven by
//! a script of real pointer and key input, frame by frame on a fixed clock, so their motion is
//! the app's own and never drops a frame. Nothing connects anywhere: what a server would show
//! is recorded.
//!
//! Each writes raw RGBA frames into a command's standard input, so any encoder will do. In the
//! command, `{name}`, `{w}`, `{h}` and `{fps}` stand for the clip's own:
//!
//! ```text
//! ANNULUS_FILM_PIPE="ffmpeg -y -f rawvideo -pix_fmt rgba -s {w}x{h} -r {fps} -i - docs/media/{name}.gif" \
//!   cargo test -p annulus-app --release readme_clips -- --ignored --nocapture
//! ```
//!
//! `official_film` makes the film the same way, and `social_preview` the one picture GitHub
//! shows when the repository is linked. `ANNULUS_FILM_ONLY=keys` makes only the clips whose
//! names contain it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::process::{Child, Command, Stdio};

use annulus_config::tokens::Appearance;
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_ui::gfx::{Align, Canvas as _, Color, Font, Point, Rect, Weight};
use annulus_ui::kit::{Button, Event, FrameOutput, Key, Modifiers, Theme, Ui, UiState, Zone};
use annulus_ui::motion::{Curve, Layer};

use super::demo::fixture_vault;
use super::live::LiveTerminal;
use super::panes::Panes;
use super::{Flyer, NativeApp, Section, TermView, WindowButtons};

/// How long captions take to come and go, in seconds.
const CAPTION_FADE: f64 = 0.32;
/// The picture around the window in the film.
const BACKDROP: Color = Color::rgba(10, 10, 9, 255);

/// How a clip is framed.
#[derive(Clone, Copy)]
struct Look {
    name: &'static str,
    /// The picture, in pixels.
    size: (usize, usize),
    /// Pixels per unit of the layout.
    scale: f32,
    /// The app's window on the picture, in units.
    window: Rect,
    corner: f32,
    fps: f64,
    /// The picture around the window, which is see-through without one.
    backdrop: Option<Color>,
    captions: bool,
}

impl Look {
    /// A README clip: the window alone, at the size a README shows it.
    const fn clip(name: &'static str) -> Self {
        Self {
            name,
            size: (1560, 900),
            scale: 1.5,
            window: Rect {
                x: 0.0,
                y: 0.0,
                w: 1040.0,
                h: 600.0,
            },
            corner: 10.0,
            // A GIF counts in hundredths of a second, so three of them to each frame keeps
            // the motion even.
            fps: 100.0 / 3.0,
            backdrop: None,
            captions: false,
        }
    }

    /// The film: 1080p with room for a caption under the window.
    const FILM: Self = Self {
        name: "annulus-demo",
        size: (1920, 1080),
        scale: 1.5,
        window: Rect {
            x: 80.0,
            y: 18.0,
            w: 1120.0,
            h: 630.0,
        },
        corner: 12.0,
        fps: 60.0,
        backdrop: Some(BACKDROP),
        captions: true,
    };

    /// The picture for link previews: the name over the window, which runs off the bottom.
    const SOCIAL: Self = Self {
        name: "social-preview",
        size: (1280, 640),
        scale: 1.0,
        window: Rect {
            x: 120.0,
            y: 196.0,
            w: 1040.0,
            h: 600.0,
        },
        corner: 12.0,
        fps: 30.0,
        backdrop: Some(BACKDROP),
        captions: false,
    };

    /// The size of the whole picture, in units.
    #[allow(clippy::cast_precision_loss)] // The picture's size.
    fn whole(&self) -> Rect {
        Rect::new(
            0.0,
            0.0,
            self.size.0 as f32 / self.scale,
            self.size.1 as f32 / self.scale,
        )
    }

    /// `value` in units as pixels.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On the picture.
    fn pixels(&self, value: f32) -> usize {
        (value * self.scale).round() as usize
    }

    /// The command that takes this clip's frames, from `variable`.
    fn encoder(&self, variable: &str) -> Child {
        let line = std::env::var(variable)
            .unwrap_or_else(|_| panic!("{variable} names the encoder that takes the frames"));
        let filled = line
            .replace("{name}", self.name)
            .replace("{w}", &self.size.0.to_string())
            .replace("{h}", &self.size.1.to_string())
            .replace("{fps}", &self.fps.to_string());
        let mut words = filled.split_whitespace();
        Command::new(words.next().expect("a command"))
            .args(words)
            .stdin(Stdio::piped())
            .spawn()
            .expect("the encoder starts")
    }
}

/// What the picture around the window shows.
#[derive(Clone, PartialEq)]
struct Around {
    caption: Option<&'static str>,
    /// How much the caption shows, in 1/64ths.
    caption_shown: u8,
    /// A card: its lines, and how much of it shows in 1/64ths.
    card: Option<(&'static [&'static str], u8)>,
}

struct Film {
    look: Look,
    app: NativeApp,
    state: UiState,
    around_state: UiState,
    text: Option<TextEngine>,
    encoder: Child,
    /// Whether frames go to the encoder, or only move the clock on.
    rolling: bool,
    frame: u64,
    zones: Vec<Zone>,
    pointer: Point,
    /// How much the pointer's dot shows.
    dot: f32,
    /// Where and when the last click landed.
    ripple: Option<(Point, f64)>,
    /// How much the window shows.
    window: f32,
    caption: Option<&'static str>,
    caption_since: f64,
    /// What the caption said before, while it fades out.
    caption_was: Option<(&'static str, f64)>,
    card: Option<(&'static [&'static str], f32)>,
    drawn: Option<(Around, Vec<u8>)>,
    picture: Vec<u8>,
}

impl Film {
    fn new(look: Look, mut app: NativeApp, variable: &str) -> Self {
        app.window_buttons = WindowButtons::Ours;
        app.ssh_files.known_hosts = super::demo::known_hosts(look.name);
        let encoder = look.encoder(variable);
        Self {
            look,
            app,
            state: UiState::default(),
            around_state: UiState::default(),
            text: None,
            encoder,
            rolling: true,
            frame: 0,
            zones: Vec::new(),
            pointer: Point::new(look.window.w * 0.5, look.window.h * 0.86),
            dot: 0.0,
            ripple: None,
            window: 1.0,
            caption: None,
            caption_since: 0.0,
            caption_was: None,
            card: None,
            drawn: None,
            picture: vec![0; look.size.0 * look.size.1 * 4],
        }
    }

    #[allow(clippy::cast_precision_loss)] // A short film.
    fn time(&self) -> f64 {
        self.frame as f64 / self.look.fps
    }

    /// How many frames `seconds` is.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // A short film.
    fn frames(&self, seconds: f64) -> u64 {
        (seconds * self.look.fps).round() as u64
    }

    /// One frame: the app, its pointer, then the picture around it.
    fn shoot(&mut self, events: &[Event]) {
        let time = self.time();
        let look = self.look;
        let size = (look.pixels(look.window.w), look.pixels(look.window.h));
        let viewport = Rect::new(0.0, 0.0, look.window.w, look.window.h);
        #[allow(clippy::cast_possible_truncation)] // The picture's size.
        let mut canvas = CpuCanvas::new(
            size.0 as u32,
            size.1 as u32,
            look.scale,
            self.text.take().unwrap_or_default(),
        )
        .unwrap();
        self.state.begin_frame(events, time);
        let theme = super::colors::previewed_theme(
            &self.app.settings,
            self.app.settings_view.previewing,
            Theme::new(Appearance::Dark),
        );
        let mut ui = Ui::new(&mut canvas, theme, &mut self.state, viewport);
        self.app.frame(&mut ui);
        Self::draw_pointer(&mut ui, self.pointer, self.dot, self.ripple, time);
        drop(ui);
        let output: FrameOutput = self.state.end_frame();
        self.zones = output.zones;
        self.compose(canvas.pixels());
        self.text = Some(canvas.into_text_engine());
        if self.rolling {
            self.encoder
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&self.picture)
                .unwrap();
        }
        self.frame += 1;
    }

    /// A soft dot where the pointer is, and a ring leaving where it clicked.
    fn draw_pointer(ui: &mut Ui<'_>, at: Point, dot: f32, ripple: Option<(Point, f64)>, now: f64) {
        let ink = ui.theme.text;
        if let Some((from, since)) = ripple {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = ((now - since) / 0.5) as f32;
            if share < 1.0 {
                let grown = Curve::Out.at(share);
                let radius = 8.0 + 20.0 * grown;
                let ring = Rect::new(from.x - radius, from.y - radius, radius * 2.0, radius * 2.0);
                ui.gfx
                    .stroke_rect(ring, radius, 2.0, ink.alpha(0.5 * (1.0 - grown)));
            }
        }
        if dot > 0.0 {
            ui.gfx.fill_circle(at, 12.0, ink.alpha(0.13 * dot));
            ui.gfx.fill_circle(at, 5.5, ink.alpha(0.92 * dot));
        }
    }

    /// The picture around the window, drawn again only when it changes.
    fn around(&mut self) -> &[u8] {
        let time = self.time();
        let look = self.look;
        let shown = |since: f64| -> f32 {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = ((time - since) / CAPTION_FADE) as f32;
            Curve::Out.at(share.clamp(0.0, 1.0))
        };
        if let Some((_, since)) = self.caption_was
            && time - since >= CAPTION_FADE
        {
            self.caption_was = None;
        }
        // One leaves before the next comes.
        let (caption, caption_shown) = match self.caption_was {
            Some((was, since)) => (Some(was), 1.0 - shown(since)),
            None => (self.caption, shown(self.caption_since)),
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 to 64.
        let bucket = |share: f32| (share * 64.0).round() as u8;
        let wanted = Around {
            caption,
            caption_shown: bucket(caption_shown),
            card: self.card.map(|(lines, share)| (lines, bucket(share))),
        };
        if self.drawn.as_ref().is_none_or(|(had, _)| *had != wanted) {
            #[allow(clippy::cast_possible_truncation)] // The picture's size.
            let mut canvas = CpuCanvas::new(
                look.size.0 as u32,
                look.size.1 as u32,
                look.scale,
                self.text.take().unwrap_or_default(),
            )
            .unwrap();
            let theme = Theme::new(Appearance::Dark);
            let whole = look.whole();
            let mut ui = Ui::new(&mut canvas, theme, &mut self.around_state, whole);
            if let Some(backdrop) = look.backdrop {
                ui.gfx.fill_rect(whole, 0.0, backdrop);
            }
            if self.window > 0.0 && look.backdrop.is_some() {
                ui.gfx.stroke_rect(
                    look.window.outset(0.5),
                    look.corner + 0.5,
                    1.0,
                    Color::WHITE.alpha(0.09),
                );
            }
            if let Some(words) = wanted.caption {
                let share = f32::from(wanted.caption_shown) / 64.0;
                let rise = 6.0 * (1.0 - share);
                let band = look.window.b();
                ui.gfx.text(
                    Rect::new(
                        look.window.x,
                        band + 2.0 + rise,
                        look.window.w,
                        whole.h - band,
                    ),
                    words,
                    Font::ui(20.0),
                    theme.text.alpha(share),
                    Align::Center,
                );
            }
            if let Some((lines, share)) = wanted.card {
                let fills = look.size.1 == Look::FILM.size.1;
                draw_card(&mut ui, lines, f32::from(share) / 64.0, fills);
            }
            drop(ui);
            let pixels = canvas.pixels().to_vec();
            self.text = Some(canvas.into_text_engine());
            self.drawn = Some((wanted, pixels));
        }
        &self.drawn.as_ref().unwrap().1
    }

    /// The window on the picture around it, with rounded corners.
    fn compose(&mut self, window: &[u8]) {
        let shown = self.window;
        let look = self.look;
        let around = self.around().to_vec();
        self.picture.copy_from_slice(&around);
        if shown <= 0.0 {
            return;
        }
        let (x0, y0) = (look.pixels(look.window.x), look.pixels(look.window.y));
        let (w, h) = (look.pixels(look.window.w), look.pixels(look.window.h));
        let radius = look.corner * look.scale;
        let see_through = look.backdrop.is_none();
        #[allow(clippy::cast_precision_loss)] // The picture's size.
        for y in 0..h {
            if y0 + y >= look.size.1 {
                break;
            }
            let to = (y0 + y) * look.size.0 * 4 + x0 * 4;
            let from = y * w * 4;
            let row = from..from + w * 4;
            // Whole rows between the corners, at rest: the most of it, cheaply.
            let in_corners = (y as f32) < radius || (y as f32) >= h as f32 - radius;
            if shown >= 1.0 && !in_corners {
                self.picture[to..to + w * 4].copy_from_slice(&window[row]);
                continue;
            }
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let dx = (radius - px).max(px - (w as f32 - radius)).max(0.0);
                let dy = (radius - py).max(py - (h as f32 - radius)).max(0.0);
                let edge = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                let a = edge * shown;
                let at = to + x * 4;
                let src = from + x * 4;
                for c in 0..3 {
                    let over = f32::from(window[src + c]) * a
                        + f32::from(self.picture[at + c]) * (1.0 - a);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 to 255.
                    let over = over.round() as u8;
                    self.picture[at + c] = over;
                }
                self.picture[at + 3] = if see_through && a < 0.5 { 0 } else { 255 };
            }
        }
    }

    fn say(&mut self, words: &'static str) {
        if !self.look.captions {
            return;
        }
        if let Some(was) = self.caption {
            self.caption_was = Some((was, self.time()));
        }
        self.caption = Some(words);
        self.caption_since = self.time()
            + if self.caption_was.is_some() {
                CAPTION_FADE
            } else {
                0.0
            };
    }

    fn quiet(&mut self) {
        if let Some(was) = self.caption.take() {
            self.caption_was = Some((was, self.time()));
        }
    }

    /// Lets the app come to rest before the clip starts.
    fn settle(&mut self) {
        self.rolling = false;
        self.hold(1.5);
        self.rolling = true;
    }

    /// `seconds` with no input.
    fn hold(&mut self, seconds: f64) {
        for _ in 0..self.frames(seconds) {
            self.shoot(&[]);
        }
    }

    /// Where the zone named `name` is.
    fn place(&self, name: &str) -> Rect {
        let Some(zone) = self.zones.iter().find(|zone| zone.label == name) else {
            let names: Vec<&str> = self.zones.iter().map(|zone| zone.label.as_str()).collect();
            panic!("no zone named {name:?}: {names:?}")
        };
        zone.rect
    }

    /// Moves the pointer to `to` over `seconds`, as a hand does.
    fn glide(&mut self, to: Point, seconds: f64) {
        let from = self.pointer;
        let steps = self.frames(seconds).max(1);
        for step in 1..=steps {
            #[allow(clippy::cast_precision_loss)] // A short film.
            let share = Curve::InOut.at(step as f32 / steps as f32);
            self.pointer = Point::new(
                from.x + (to.x - from.x) * share,
                from.y + (to.y - from.y) * share,
            );
            self.dot = (self.dot + 0.14).min(1.0);
            self.shoot(&[Event::PointerMoved(self.pointer)]);
        }
    }

    fn go_to(&mut self, name: &str) {
        let to = self.place(name).center();
        self.glide(to, 0.7);
    }

    /// Presses and lets go where the pointer is.
    fn click(&mut self) {
        self.ripple = Some((self.pointer, self.time()));
        self.shoot(&[Event::PointerDown(Button::Primary)]);
        self.hold(0.07);
        self.shoot(&[Event::PointerUp(Button::Primary)]);
    }

    fn click_on(&mut self, name: &str) {
        self.go_to(name);
        self.hold(0.14);
        self.click();
    }

    /// Clicks tab `title` in the strip.
    fn click_tab(&mut self, title: &str) {
        let uid = self
            .app
            .tabs
            .iter()
            .find(|tab| tab.title == title)
            .map(|tab| tab.uid)
            .expect("the tab");
        let (_, place) = self
            .app
            .tab_places
            .iter()
            .find(|(at, _)| *at == uid)
            .copied()
            .expect("its place");
        self.glide(place.center(), 0.7);
        self.hold(0.14);
        self.click();
    }

    /// Types `words` as a person does.
    fn type_in(&mut self, words: &str) {
        for letter in words.chars() {
            self.shoot(&[Event::Text(letter.to_string())]);
            self.hold(0.085);
        }
    }

    fn press(&mut self, key: Key) {
        self.shoot(&[Event::Key(key, Modifiers::NONE)]);
    }

    /// A card over the whole picture: in, held and, when it `leaves`, out.
    fn play_card(&mut self, lines: &'static [&'static str], seconds: f64, leaves: bool) {
        const FADE: f64 = 0.6;
        for frame in 0..self.frames(seconds) {
            #[allow(clippy::cast_precision_loss)] // A short film.
            let at = frame as f64 / self.look.fps;
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = if at < FADE {
                Curve::InOut.at((at / FADE) as f32)
            } else if leaves && at > seconds - FADE {
                1.0 - Curve::InOut.at(((at - (seconds - FADE)) / FADE) as f32)
            } else {
                1.0
            };
            self.card = Some((lines, share));
            self.shoot(&[]);
        }
        if leaves {
            self.card = None;
        }
    }

    /// The window fading in or out over `seconds`.
    fn fade_window(&mut self, to: f32, seconds: f64) {
        let from = self.window;
        let steps = self.frames(seconds).max(1);
        for step in 1..=steps {
            #[allow(clippy::cast_precision_loss)] // A short film.
            let share = Curve::InOut.at(step as f32 / steps as f32);
            self.window = from + (to - from) * share;
            self.drawn = None;
            self.shoot(&[]);
        }
    }

    fn finish(mut self) {
        drop(self.encoder.stdin.take());
        let status = self.encoder.wait().unwrap();
        assert!(status.success(), "the encoder said {status}");
        println!("{}: {} frames", self.look.name, self.frame);
    }
}

/// The title's and the closing's card: the mark, the name and a line or two. In the film it
/// `fills` the picture, in the middle; otherwise it sits at the top.
fn draw_card(ui: &mut Ui<'_>, lines: &[&str], share: f32, fills: bool) {
    let t = ui.theme;
    let whole = ui.viewport;
    ui.gfx.fill_rect(whole, 0.0, BACKDROP.alpha(share));
    let rise = 10.0 * (1.0 - share);
    let (top, mark, name, line): (f32, f32, f32, f32) = if fills {
        (244.0, 68.0, 60.0, 22.0)
    } else {
        (20.0, 44.0, 40.0, 19.0)
    };
    ui.push_layer(Layer::fade(share));
    super::titlebar::paint_mark(
        ui,
        Rect::new(whole.w / 2.0 - mark / 2.0, top + rise, mark, mark),
    );
    let below = top + mark + rise + mark * 0.2;
    ui.gfx.text(
        Rect::new(0.0, below, whole.w, name * 1.2),
        "Annulus",
        Font::display(name).weight(Weight::Semibold),
        t.text,
        Align::Center,
    );
    for (index, words) in lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // A line or two.
        let y = below + name * 1.3 + index as f32 * 34.0;
        let (font, ink) = if index == 0 {
            (Font::ui(line), t.text2)
        } else {
            (Font::ui(15.0), t.text3)
        };
        ui.gfx.text(
            Rect::new(0.0, y, whole.w, 30.0),
            words,
            font,
            ink,
            Align::Center,
        );
    }
    ui.pop_layer();
}

/// The tab connecting to prod-db, at `stage` of its steps.
fn connecting(app: &NativeApp, stage: u8) -> TermView {
    const LOG: [&str; 4] = [
        "Reaching prod-db.example.com:22 as deploy",
        "prod-db.example.com's key is trusted",
        "Logged in with Work laptop",
        "Opening a shell",
    ];
    let program = crate::connect::quick("deploy@prod-db.example.com", &app.ssh_files);
    let log = &LOG[..=usize::from(stage)];
    let mut terminal = LiveTerminal::held_at(program, stage, log);
    terminal.identity = Some(("prod-db".into(), super::os::find("ubuntu")));
    TermView::Live(Box::new(Panes::new(terminal)))
}

/// Puts `view` in the tab in front, which fades in.
fn replace_view(film: &mut Film, view: TermView) {
    let active = film.app.active;
    film.app.tabs[active].terminal = Some(view);
    film.app.switched = Some((film.time(), 0.0));
}

/// The app unlocked at `section`, on its made-up hosts.
fn unlocked(name: &str, section: Section) -> NativeApp {
    let mut app = NativeApp::new(Some(fixture_vault(name, true)));
    app.section = section;
    app
}

/// The vault opens: a password, and the hosts come in as the lock goes.
fn shot_unlock(film: &mut Film) {
    let vault = fixture_vault(&format!("{}-vault", film.look.name), true);
    film.say("Your hosts and keys live in an encrypted vault.");
    film.hold(0.8);
    film.type_in("correct horse");
    film.hold(0.3);
    film.go_to("Unlock");
    film.hold(0.2);
    film.ripple = Some((film.pointer, film.time()));
    film.app.vault = Some(vault);
    film.app.lock_left = Some(film.time());
    film.hold(1.5);
}

/// Into a group and back, then the tag filter.
fn shot_hosts(film: &mut Film) {
    film.say("Groups and tags keep a long list of servers easy to walk.");
    film.hold(0.4);
    film.click_on("Production");
    film.hold(1.3);
    film.click_on("All hosts");
    film.hold(1.0);
    film.click_on("Tags");
    film.hold(0.9);
    film.click_on("nginx (2)");
    film.hold(1.3);
    film.click_on("Tags");
    film.hold(1.0);
}

/// The host flies to its tab, and each step of the connection shows.
fn shot_connect(film: &mut Film) {
    film.say("A connection shows you each step as it happens.");
    film.go_to("prod-db");
    film.hold(0.4);
    film.ripple = Some((film.pointer, film.time()));
    let from = film.place("prod-db");
    let view = connecting(&film.app, 0);
    film.app.push_terminal("prod-db", view);
    let uid = film.app.tabs[film.app.active].uid;
    film.app.flyer = Some(Flyer {
        uid,
        from,
        title: "prod-db".into(),
        os: super::os::find("ubuntu"),
        since: film.time(),
    });
    film.hold(0.9);
    for stage in 1..=3 {
        let view = connecting(&film.app, stage);
        let active = film.app.active;
        film.app.tabs[active].terminal = Some(view);
        film.hold(0.75);
    }
    replace_view(film, TermView::Replay(Box::new(super::demo::terminal())));
    film.hold(1.6);
}

/// The palette finds things by a few letters.
fn shot_palette(film: &mut Film) {
    film.say("The command palette reaches everything from the keyboard.");
    film.dot = 0.0;
    film.hold(0.5);
    let (key, modifiers) = super::shortcuts::PALETTE.own().expect("a chord");
    film.shoot(&[Event::Key(key, modifiers)]);
    film.hold(0.7);
    film.type_in("prod");
    film.hold(1.3);
    film.press(Key::Escape);
    film.hold(0.8);
}

/// Panes: broadcast on and off, then focus on one.
fn shot_panes(film: &mut Film) {
    film.say("Split a tab, type into every pane at once, or focus on one.");
    film.hold(1.1);
    film.click_on("Broadcast");
    film.hold(1.5);
    film.click_on("Broadcast");
    film.hold(0.5);
    film.click_on("Focus mode");
    film.hold(1.2);
    film.click_on("web-1");
    film.hold(1.2);
    film.click_on("Focus mode");
    film.hold(0.9);
}

/// Files: the two panes, a row chosen, then transfers rise.
fn shot_files(film: &mut Film) {
    film.say("Files side by side, with a queue that shows what is moving.");
    film.hold(0.6);
    film.click_on("deploy.sh");
    film.hold(0.7);
    if let Some(view) = film.app.tabs[1].sftp.as_mut() {
        view.demo_transfers_going_well();
    }
    film.hold(2.2);
    film.click_on("Hide transfers");
    film.hold(1.1);
    film.click_on("Show transfers");
    film.hold(1.3);
}

/// Keys: the card unfolds, a new key rises into the list, and the next card opens for it.
fn shot_keys(film: &mut Film) {
    film.say("Make a key, keep it in your vault, and install it on a server.");
    film.hold(0.5);
    film.click_on("Generate key");
    film.hold(0.9);
    film.click_on("Name, e.g. work laptop");
    film.type_in("Build server");
    film.hold(0.3);
    film.click_on("Generate");
    film.hold(1.9);
    film.go_to("Build server");
    film.hold(0.4);
    film.click_on("Install on a host");
    film.hold(1.3);
    film.press(Key::Escape);
    film.hold(0.9);
}

/// Tunnels: the kinds, each a little diagram, then the next step.
fn shot_tunnels(film: &mut Film) {
    film.say("Tunnels are drawn for you as you set them up.");
    film.hold(0.4);
    film.click_on("New rule");
    film.hold(1.9);
    film.click_on("Reach a service behind a server");
    film.hold(2.2);
    film.press(Key::Escape);
    film.hold(0.8);
}

/// Colors: the whole window takes the scheme under the pointer.
fn shot_colors(film: &mut Film) {
    film.say("Nearly 500 color schemes, previewed on the whole window.");
    film.hold(0.4);
    // Not the mark in the title bar, which is named the same: the scheme's menu, further on.
    let scheme = film
        .zones
        .iter()
        .rfind(|zone| zone.label == "Annulus")
        .map(|zone| zone.rect.center())
        .unwrap();
    film.glide(scheme, 0.7);
    film.hold(0.14);
    film.click();
    film.hold(0.4);
    film.type_in("gruvbox");
    film.hold(0.2);
    film.go_to("Gruvbox Light");
    film.hold(1.5);
    film.go_to("Gruvbox dark, hard");
    film.hold(1.3);
    film.press(Key::Escape);
    film.hold(1.0);
}

/// A clip of one shot: its name, the shot, and the section it starts at.
type Clip = (&'static str, fn(&mut Film), Section);

/// Whether to make the clip `name`, as `ANNULUS_FILM_ONLY` asks.
fn wanted(name: &str) -> bool {
    std::env::var("ANNULUS_FILM_ONLY").map_or(true, |only| name.contains(&only))
}

#[test]
#[ignore = "makes the README's clips; ANNULUS_FILM_PIPE names the encoder"]
fn readme_clips() {
    const PIPE: &str = "ANNULUS_FILM_PIPE";
    // The first thing a reader sees.
    if wanted("hero") {
        let mut film = Film::new(Look::clip("hero"), NativeApp::new(None), PIPE);
        film.settle();
        shot_unlock(&mut film);
        shot_connect(&mut film);
        film.finish();
    }
    let clips: [Clip; 3] = [
        ("hosts", shot_hosts, Section::Hosts),
        ("keys", shot_keys, Section::Keychain),
        ("tunnels", shot_tunnels, Section::PortForwarding),
    ];
    for (name, shot, section) in clips {
        if wanted(name) {
            let mut film = Film::new(Look::clip(name), unlocked(name, section), PIPE);
            film.settle();
            shot(&mut film);
            film.finish();
        }
    }
    if wanted("colors") {
        let app = unlocked("colors", Section::Settings);
        let mut film = Film::new(Look::clip("colors"), app, PIPE);
        film.settle();
        shot_colors(&mut film);
        film.finish();
    }
    if wanted("palette") {
        let app = unlocked("palette", Section::Hosts);
        let mut film = Film::new(Look::clip("palette"), app, PIPE);
        film.settle();
        shot_palette(&mut film);
        film.finish();
    }
    if wanted("panes") {
        let mut app = unlocked("panes", Section::Hosts);
        let panes = super::scenes::split_panes(false);
        app.push_terminal("prod-db", TermView::Live(Box::new(panes)));
        let mut film = Film::new(Look::clip("panes"), app, PIPE);
        film.settle();
        shot_panes(&mut film);
        film.finish();
    }
    if wanted("files") {
        let mut app = unlocked("files", Section::Hosts);
        app.tabs[1].sftp = Some(Box::new(super::sftp::SftpView::new(super::demo::sftp_tab(
            "files",
        ))));
        app.active = 1;
        let mut film = Film::new(Look::clip("files"), app, PIPE);
        film.settle();
        shot_files(&mut film);
        film.finish();
    }
}

#[test]
#[ignore = "makes the film; ANNULUS_FILM_PIPE names the encoder"]
fn official_film() {
    let mut film = Film::new(Look::FILM, NativeApp::new(None), "ANNULUS_FILM_PIPE");
    film.window = 0.0;
    film.shoot(&[]);
    film.play_card(&["Terminals and SSH, in one calm window."], 3.2, true);
    film.fade_window(1.0, 0.5);
    shot_unlock(&mut film);
    shot_hosts(&mut film);
    shot_connect(&mut film);
    shot_palette(&mut film);

    let panes = super::scenes::split_panes(false);
    replace_view(&mut film, TermView::Live(Box::new(panes)));
    shot_panes(&mut film);

    film.app.tabs[1].sftp = Some(Box::new(super::sftp::SftpView::new(super::demo::sftp_tab(
        "film",
    ))));
    film.click_tab("SFTP");
    shot_files(&mut film);

    film.click_tab("Vault");
    film.hold(0.3);
    film.click_on("Keychain");
    shot_keys(&mut film);

    film.click_on("Port Forwarding");
    shot_tunnels(&mut film);

    film.click_on("Settings");
    shot_colors(&mut film);

    film.quiet();
    film.dot = 0.0;
    film.fade_window(0.0, 0.5);
    film.play_card(
        &[
            "Free and open source. No telemetry. Sync through your own server.",
            "In development. Shown with demo data.",
        ],
        3.6,
        false,
    );
    film.finish();
}

#[test]
#[ignore = "makes the link preview; ANNULUS_STILL_PIPE names the encoder"]
fn social_preview() {
    let app = unlocked("social", Section::Hosts);
    let mut film = Film::new(Look::SOCIAL, app, "ANNULUS_STILL_PIPE");
    film.card = Some((&["Terminals and SSH, in one calm window."], 1.0));
    film.settle();
    // The encoder keeps the last frame.
    film.shoot(&[]);
    film.finish();
}
