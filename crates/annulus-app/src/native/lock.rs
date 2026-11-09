//! The lock screen (UI-SPEC §5, F-SEC-004): create the vault on first run, unlock it after.
//! Password fields edit zeroizing buffers and are wiped once handed over.

use annulus_config::tokens::{motion as motion_ms, space};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Ui};
use annulus_ui::motion::{Curve, Layer, Motion};
use annulus_ui::widgets::{
    BODY, CONTROL, FieldStyle, HEADING, Kind, SEARCH, SMALL, TITLE, shake_offset,
};
use annulus_vault::strength::{self, Strength};
use zeroize::Zeroizing;

/// The shortest master password accepted.
pub const MIN_PASSWORD: usize = 10;

/// What the screen asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No vault yet: choose a master password.
    Setup,
    /// A vault exists: type its password (or the recovery key).
    Unlock,
    /// A vault was just made: its recovery key is shown this once.
    Recovery,
}

/// What the user asked the vault to do. Its `Debug` never shows the password or key.
#[derive(PartialEq, Eq)]
pub enum LockAction {
    /// Create a vault with this master password.
    Create(Zeroizing<String>),
    /// Open the vault with this password, or recovery key when `recovery`.
    Unlock {
        /// The password or key typed.
        secret: Zeroizing<String>,
        /// It is the recovery key.
        recovery: bool,
    },
    /// The recovery key was confirmed: the new vault can open.
    Confirmed,
}

impl std::fmt::Debug for LockAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create(_) => f.write_str("Create(..)"),
            Self::Unlock { recovery, .. } => f
                .debug_struct("Unlock")
                .field("recovery", recovery)
                .finish_non_exhaustive(),
            Self::Confirmed => f.write_str("Confirmed"),
        }
    }
}

/// The lock screen's state.
pub struct LockView {
    /// Setup or unlock.
    pub mode: Mode,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    recovery: bool,
    /// The last problem, shown under the fields.
    pub error: Option<String>,
    /// Creating or unlocking, on another thread.
    pub working: Option<crate::job::Job<Opened>>,
    /// When the current password check began, on the window clock.
    working_since: Option<f64>,
    /// A new vault, waiting for its recovery key to be confirmed.
    pending: Option<Box<annulus_vault::vault::Vault>>,
    key: Zeroizing<String>,
    typed: String,
    /// How hard the new master password is to guess, while one is typed.
    strength: Option<Strength>,
    /// The screen was just shown: the password field takes the keyboard, so typing the
    /// password needs no click first.
    fresh: bool,
    /// How many times what was typed was refused: each one shakes the field.
    refusals: u32,
    /// How far the lock has opened, 0.0 to 1.0, as the screen leaves.
    opened: f32,
}

/// What creating or unlocking gives: the vault, and a new vault's recovery key.
pub type Opened = Result<
    (annulus_vault::vault::Vault, Option<Zeroizing<String>>),
    annulus_vault::vault::VaultError,
>;

impl LockView {
    /// A screen in `mode`.
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            password: Zeroizing::new(String::new()),
            confirm: Zeroizing::new(String::new()),
            recovery: false,
            error: None,
            working: None,
            working_since: None,
            pending: None,
            key: Zeroizing::new(String::new()),
            typed: String::new(),
            strength: None,
            fresh: true,
            refusals: 0,
            opened: 0.0,
        }
    }

    /// `why` the last try failed: said under the field, and the field shakes.
    pub fn refuse(&mut self, why: String) {
        self.working_since = None;
        self.error = Some(why);
        self.refusals += 1;
    }

    /// Starts the visual state used while the vault works on a password.
    pub fn start_working(&mut self, now: f64) {
        self.working_since = Some(now);
        self.error = None;
    }

    /// Ends the password-check state before success or an error takes over.
    pub fn finish_working(&mut self) {
        self.working_since = None;
    }

    /// Draws the screen on its way out, `t` (0.0 to 1.0) of the way: the shackle lifts, then
    /// the lock fades while a soft band opens from its center to show the vault below. It takes
    /// no input.
    pub fn draw_leaving(&mut self, ui: &mut Ui<'_>, rect: Rect, t: f32) {
        let t = t.clamp(0.0, 1.0);
        self.opened = Curve::Overshoot.at((t / 0.34).min(1.0)).max(0.001);
        let reveal = Curve::Emphasized.at(((t - 0.04) / 0.9).clamp(0.0, 1.0));
        let half = rect.h * 0.5 * reveal;
        let top_h = (rect.center().y - half - rect.y).max(0.0);
        let bottom_y = rect.center().y + half;
        let bottom_h = (rect.b() - bottom_y).max(0.0);
        ui.gfx
            .fill_rect(Rect::new(rect.x, rect.y, rect.w, top_h), 0.0, ui.theme.bg);
        ui.gfx.fill_rect(
            Rect::new(rect.x, bottom_y, rect.w, bottom_h),
            0.0,
            ui.theme.bg,
        );

        // A low light at each moving edge keeps the reveal clear on both light and dark themes.
        if reveal > 0.0 && reveal < 1.0 {
            let edge = ui
                .theme
                .accent
                .alpha(0.14 * (std::f32::consts::PI * reveal).sin());
            let top_y = rect.center().y - half;
            ui.gfx.fill_gradient(
                Rect::new(rect.x, top_y, rect.w, 18.0),
                0.0,
                edge,
                edge.alpha(0.0),
                false,
            );
            ui.gfx.fill_gradient(
                Rect::new(rect.x, bottom_y - 18.0, rect.w, 18.0),
                0.0,
                edge.alpha(0.0),
                edge,
                false,
            );
        }

        let fade = 1.0 - Curve::Out.at((t / 0.42).clamp(0.0, 1.0));
        ui.push_inert();
        ui.push_layer(
            Layer::fade(fade)
                .scaled(rect.center(), 1.0 + 0.014 * t)
                .grouped(rect),
        );
        let _ = self.draw(ui, rect);
        ui.pop_layer();
        ui.pop_inert();
    }

    /// Shows `key` for the new `vault`, which opens once the key is confirmed.
    pub fn show_recovery(&mut self, vault: annulus_vault::vault::Vault, key: Zeroizing<String>) {
        self.finish_working();
        self.mode = Mode::Recovery;
        self.pending = Some(Box::new(vault));
        self.key = key;
        self.typed.clear();
        self.error = None;
    }

    /// The new vault, after its recovery key was confirmed.
    pub fn take_vault(&mut self) -> Option<annulus_vault::vault::Vault> {
        self.key = Zeroizing::new(String::new());
        self.pending.take().map(|vault| *vault)
    }

    /// The recovery key shown, for tests that confirm it.
    #[cfg(test)]
    pub fn recovery_key_for_tests(&self) -> String {
        self.key.to_string()
    }

    /// The recovery key is being used to unlock.
    #[must_use]
    pub fn recovery_mode(&self) -> bool {
        self.recovery
    }

    /// The recovery key, once, and a check that it was written down.
    fn recovery_card(&mut self, ui: &mut Ui<'_>, rect: Rect) -> Option<LockAction> {
        let t = ui.theme;
        let card = rect.centered(rect.w.min(460.0), 330.0);
        ui.raised_surface(card, 14.0);
        let mut column = card.inset(space::XL + 4.0, space::XL);
        ui.strict_text(
            column.take_top(26.0),
            "Save your recovery key",
            annulus_ui::widgets::HEADING,
            t.text,
            Align::Center,
        );
        column.take_top(space::XS);
        ui.gfx.text(
            column.take_top(20.0),
            "If you forget the master password, it is the only way in.",
            SMALL,
            t.text2,
            Align::Center,
        );
        ui.gfx.text(
            column.take_top(18.0),
            "It is shown only now.",
            SMALL,
            t.text2,
            Align::Center,
        );
        column.take_top(space::L);
        let key_box = column.take_top(48.0);
        ui.gfx.fill_rect(key_box, 8.0, t.field);
        ui.gfx.stroke_rect(key_box, 8.0, 1.0, t.line_strong);
        let mut inside = key_box.inset(space::M, 0.0);
        let copy = inside.take_right(30.0).centered(30.0, 30.0);
        ui.gfx.text(
            inside,
            &self.key,
            annulus_ui::gfx::Font::mono(14.0),
            t.text,
            Align::Center,
        );
        if ui
            .icon_button(
                Id::new("recovery.copy"),
                copy,
                icons::COPY,
                "Copy the recovery key",
                false,
            )
            .clicked
        {
            ui.copy_secret(self.key.as_str().to_owned());
        }
        column.take_top(space::L);
        let field = ui.text_field(
            Id::new("recovery.typed"),
            column.take_top(CONTROL),
            &mut self.typed,
            FieldStyle {
                placeholder: "Type its first group to confirm you saved it",
                ..FieldStyle::default()
            },
        );
        if let Some(error) = &self.error {
            column.take_top(space::XS);
            ui.gfx
                .text(column.take_top(18.0), error, SMALL, t.error, Align::Start);
        }
        column.take_top(space::M);
        let go = column.take_top(CONTROL);
        let clicked = ui
            .button(
                Id::new("recovery.go"),
                go,
                "I saved it, open the vault",
                None,
                Kind::Primary,
            )
            .clicked;
        if clicked || field.submitted {
            let first = self.key.split('-').next().unwrap_or_default();
            if self.typed.trim().eq_ignore_ascii_case(first) {
                return Some(LockAction::Confirmed);
            }
            self.error = Some("That does not match the first group.".into());
        }
        None
    }

    /// Checks a new master password and its confirmation.
    fn check_new(&self) -> Result<(), String> {
        if self.password.chars().count() < MIN_PASSWORD {
            return Err(format!("Use at least {MIN_PASSWORD} characters."));
        }
        if !strength::estimate(&self.password).is_strong() {
            return Err("Too easy to guess. Make it longer or less common.".into());
        }
        if self.password.as_str() != self.confirm.as_str() {
            return Err("The two passwords differ.".into());
        }
        Ok(())
    }

    /// Draws the screen centered in `rect`.
    #[allow(clippy::too_many_lines)] // One card, top to bottom.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect) -> Option<LockAction> {
        if self.mode == Mode::Recovery {
            return self.recovery_card(ui, rect);
        }
        let t = ui.theme;
        let setup = self.mode == Mode::Setup;
        let (title, detail, button) = match (setup, self.recovery) {
            (true, _) => (
                "Create your vault",
                "Hosts and keys are encrypted with a master password.",
                "Create vault",
            ),
            (false, false) => (
                "Unlock your vault",
                "Local terminals work while the vault is locked.",
                "Unlock",
            ),
            (false, true) => (
                "Unlock with your recovery key",
                "Type the key you saved when you created the vault.",
                "Unlock",
            ),
        };
        let busy = self.working_since.is_some() || self.working.is_some() || self.opened > 0.0;
        let fields = if setup { 2.0 } else { 1.0 };
        let meter_h = if setup { space::S + METER_H } else { 0.0 };
        // A problem takes the place of the line under the title, so nothing moves for it.
        let says = ui.animate(
            Id::new("lock.says"),
            if self.error.is_some() { 1.0 } else { 0.0 },
            Motion::BASE,
        );
        let height = 44.0
            + space::M
            + 36.0
            + 2.0
            + 20.0
            + space::XL
            + fields * SEARCH
            + (fields - 1.0) * space::M
            + meter_h
            + space::M
            + SEARCH
            + if setup { 0.0 } else { space::M + CONTROL };
        masthead(ui, rect);
        let mut column = rect.centered(rect.w.min(380.0), height);

        let mark = column.take_top(44.0).centered(40.0, 40.0);
        if setup {
            ui.gfx
                .icon(icons::SHIELD, mark.centered(32.0, 32.0), t.text);
        } else {
            if busy && self.opened <= 0.0 {
                let since = self.working_since.unwrap_or_else(|| ui.time());
                working_mark(ui, mark.center(), (ui.time() - since).max(0.0));
            }
            lock_mark(ui, mark, self.opened);
        }
        column.take_top(space::M);
        // The password or the recovery key: the words change softly.
        let words = ui.entrance(Id::new(("lock.words", self.recovery)), Motion::BASE);
        ui.strict_text(
            column.take_top(36.0),
            title,
            TITLE,
            t.text.alpha(words),
            Align::Center,
        );
        column.take_top(2.0);
        let saying = column.take_top(20.0);
        if says < 1.0 {
            let checking = ui.animate(
                Id::new("lock.checking.words"),
                if busy { 1.0 } else { 0.0 },
                Motion::BASE,
            );
            if checking < 1.0 {
                ui.push_layer(Layer::fade((1.0 - says) * words * (1.0 - checking)));
                ui.strict_text(saying, detail, BODY, t.text2, Align::Center);
                ui.pop_layer();
            }
            if checking > 0.0 {
                let progress = if setup {
                    "Encrypting your new vault\u{2026}"
                } else {
                    "Checking and opening saved hosts\u{2026}"
                };
                ui.push_layer(Layer::fade((1.0 - says) * words * checking));
                ui.strict_text(saying, progress, BODY, t.accent, Align::Center);
                ui.pop_layer();
            }
        }
        if let Some(error) = &self.error {
            ui.push_layer(Layer::fade(says));
            let width = (ui.gfx.measure(error, BODY).width + 20.0).min(saying.w);
            let mut at = saying.centered(width, saying.h);
            let icon = at.take_left(14.0).centered(14.0, 14.0);
            ui.gfx.icon(icons::WARNING, icon, t.error);
            at.take_left(6.0);
            ui.gfx.text(at, error, BODY, t.error, Align::Start);
            ui.pop_layer();
        }
        column.take_top(space::XL);

        let placeholder = match (setup, self.recovery) {
            (true, _) => "Master password (10+ characters)",
            (false, false) => "Master password",
            (false, true) => "Recovery key",
        };
        if std::mem::take(&mut self.fresh) {
            ui.state.set_focus(Some(Id::new("lock.password")), false);
        }
        // Nothing typed during the password check can change the value being checked.
        if busy {
            ui.push_inert();
        }
        let controls = ui.animate(
            Id::new("lock.controls.ready"),
            if busy { 0.38 } else { 1.0 },
            Motion::BASE,
        );
        ui.push_layer(Layer::fade(controls));
        // Refused, the field says no the way a head shakes.
        let shake = if self.refusals > 0 {
            let id = Id::new(("lock.shake", self.refusals));
            ui.entrance(id, Motion::new(motion_ms::SLOW + 60, Curve::Linear))
        } else {
            1.0
        };
        ui.push_layer(Layer::NONE.shifted(shake_offset(shake), 0.0));
        let first = ui.text_field(
            Id::new("lock.password"),
            column.take_top(SEARCH),
            &mut self.password,
            FieldStyle {
                placeholder,
                icon: Some(icons::KEY),
                error: self.error.is_some(),
                ..FieldStyle::default()
            },
        );
        let mut submitted = first.submitted;
        if first.changed {
            self.error = None;
            self.strength =
                (setup && !self.password.is_empty()).then(|| strength::estimate(&self.password));
        }
        if setup {
            column.take_top(space::S);
            let meter = column.take_top(METER_H);
            if let Some(strength) = &self.strength {
                strength_meter(ui, meter, strength);
            }
            column.take_top(space::M);
            let second = ui.text_field(
                Id::new("lock.confirm"),
                column.take_top(SEARCH),
                &mut self.confirm,
                FieldStyle {
                    placeholder: "Type it again",
                    icon: Some(icons::KEY),
                    error: self.error.is_some(),
                    ..FieldStyle::default()
                },
            );
            submitted |= second.submitted;
            if second.changed {
                self.error = None;
            }
        }
        ui.pop_layer();
        ui.pop_layer();
        if busy {
            ui.pop_inert();
        }
        column.take_top(space::M);
        let go = column.take_top(SEARCH);
        let label = if busy {
            if setup {
                "Creating the vault\u{2026}"
            } else {
                "Unlocking\u{2026}"
            }
        } else {
            button
        };
        let clicked = ui
            .button(Id::new("lock.go"), go, label, None, Kind::Primary)
            .clicked
            && !busy;
        // The key is being derived: that takes a moment, and nothing can say how long.
        if self.working.is_some() {
            ui.sweep(go);
        }
        let mut action = None;
        if (clicked || submitted) && !busy {
            if setup {
                match self.check_new() {
                    Ok(()) => {
                        self.confirm = Zeroizing::new(String::new());
                        self.strength = None;
                        action = Some(LockAction::Create(std::mem::replace(
                            &mut self.password,
                            Zeroizing::new(String::new()),
                        )));
                    }
                    Err(problem) => self.refuse(problem),
                }
            } else if !self.password.is_empty() {
                action = Some(LockAction::Unlock {
                    secret: std::mem::replace(&mut self.password, Zeroizing::new(String::new())),
                    recovery: self.recovery,
                });
            }
        }
        if !setup {
            column.take_top(space::M);
            let link = if self.recovery {
                "Use the master password"
            } else {
                "Forgot the password? Use the recovery key"
            };
            let width = annulus_ui::widgets::button_width(&mut ui.gfx, link, None);
            let row = column.take_top(CONTROL);
            let at = row.centered(width.min(row.w), CONTROL);
            if busy {
                ui.push_inert();
                ui.push_layer(Layer::fade(0.38));
            }
            if quiet_link(ui, Id::new("lock.recovery"), at, link) {
                self.recovery = !self.recovery;
                self.error = None;
                self.password = Zeroizing::new(String::new());
            }
            if busy {
                ui.pop_layer();
                ui.pop_inert();
            }
        }
        if ui.key(Key::Escape, Modifiers::NONE) {
            self.error = None;
        }
        action
    }
}

/// Whose window this is, at the top of the screen.
fn masthead(ui: &mut Ui<'_>, rect: Rect) {
    let t = ui.theme;
    let name_w = ui.gfx.measure("Annulus", HEADING).width + 2.0;
    let mut row = Rect::new(rect.x, rect.y + space::XL, rect.w, 28.0)
        .centered(26.0 + space::S + name_w, 28.0);
    let mark = row.take_left(26.0).centered(26.0, 26.0);
    super::titlebar::paint_mark(ui, mark);
    row.take_left(space::S);
    ui.gfx.text(row, "Annulus", HEADING, t.text, Align::Start);
}

/// A link-like button: quiet text that wakes under the pointer. Returns whether it was
/// clicked.
fn quiet_link(ui: &mut Ui<'_>, id: Id, rect: Rect, label: &str) -> bool {
    let response = ui.zone(id, rect, annulus_ui::kit::Role::Button, label);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let ink = t.text2.mix(t.text, hover);
    ui.strict_text(rect, label, BODY, ink, Align::Center);
    if response.hovered {
        ui.set_cursor(annulus_ui::kit::Cursor::Pointer);
    }
    response.clicked
}

/// A quiet loop around the lock while password work has no known end time.
fn working_mark(ui: &mut Ui<'_>, center: Point, elapsed: f64) {
    const PERIOD: f64 = 1.8;
    let accent = ui.theme.accent;
    if ui.state.reduce_motion {
        ui.gfx.stroke_circle(center, 25.0, 1.5, accent.alpha(0.34));
        return;
    }
    #[allow(clippy::cast_possible_truncation)] // A repeating visual phase.
    let phase = ((elapsed % PERIOD) / PERIOD) as f32;
    for step in 0..3_u8 {
        let wave = (phase + f32::from(step) / 3.0) % 1.0;
        let gone = Curve::Out.at(wave);
        ui.gfx.stroke_circle(
            center,
            22.0 + 17.0 * gone,
            1.25,
            accent.alpha(0.24 * (1.0 - gone)),
        );
    }
    let angle = phase * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    for step in 0..3_u8 {
        let at = angle + f32::from(step) * std::f32::consts::TAU / 3.0;
        let point = Point::new(center.x + 25.0 * at.cos(), center.y + 25.0 * at.sin());
        ui.gfx
            .fill_circle(point, 2.25, accent.alpha(0.5 + f32::from(step) * 0.16));
    }
    ui.request_repaint();
}

/// The lock, drawn in `rect` with its shackle swung `opened` (0.0 to 1.0, a little past when it
/// overshoots) about its right leg.
fn lock_mark(ui: &mut Ui<'_>, rect: Rect, opened: f32) {
    let ink = ui.theme.text;
    let unit = rect.w / 32.0;
    let at = |x: f32, y: f32| Point::new(rect.x + x * unit, rect.y + y * unit);
    let body = Rect::new(at(6.5, 0.0).x, at(0.0, 14.5).y, 19.0 * unit, 14.0 * unit);
    ui.gfx.stroke_rect(body, 4.0 * unit, 2.0 * unit, ink);
    let (top, bottom) = (at(16.0, 20.0), at(16.0, 23.5));
    ui.gfx.polyline(&[top, bottom], false, 2.0 * unit, ink);
    // Up the left leg, over the arch, down the right leg.
    let (center_x, arch_y, reach) = (16.0, 9.5, 5.5);
    let mut points = vec![(center_x - reach, 14.5)];
    for step in 0..=12_u8 {
        let angle = std::f32::consts::PI * f32::from(step) / 12.0;
        points.push((center_x - reach * angle.cos(), arch_y - reach * angle.sin()));
    }
    points.push((center_x + reach, 14.5));
    // It lifts out of the body, then swings about the right leg.
    let (sin, cos) = (opened * 0.5).sin_cos();
    let (pivot_x, pivot_y) = (center_x + reach, 14.5);
    let lift = 2.5 * opened;
    let shackle: Vec<Point> = points
        .into_iter()
        .map(|(x, y)| {
            let (dx, dy) = (x - pivot_x, y - pivot_y);
            at(
                pivot_x + dx * cos - dy * sin,
                pivot_y + dx * sin + dy * cos - lift,
            )
        })
        .collect();
    ui.gfx.polyline(&shackle, false, 2.0 * unit, ink);
}

/// The height of [`strength_meter`].
pub const METER_H: f32 = 36.0;

/// Four bars and a word for how hard a new password is to guess, and what would help.
pub fn strength_meter(ui: &mut Ui<'_>, rect: Rect, strength: &Strength) {
    let t = ui.theme;
    let color = match strength.score {
        0 | 1 => t.error,
        2 | 3 => t.warn,
        _ => t.ok,
    };
    let mut column = rect;
    let mut row = column.take_top(16.0);
    let bars = row.take_left(120.0);
    let gap = 4.0;
    let width = (bars.w - 3.0 * gap) / 4.0;
    let filled = strength.score.max(1);
    for bar in 0..4_u8 {
        let x = bars.x + f32::from(bar) * (width + gap);
        let on = bar < filled;
        ui.gfx.fill_rect(
            Rect::new(x, bars.y + (bars.h - 4.0) / 2.0, width, 4.0),
            2.0,
            if on { color } else { t.line_strong },
        );
    }
    row.take_left(space::M);
    ui.strict_text(row, strength.label(), SMALL, color, Align::Start);
    if let Some(hint) = strength.hint.as_deref() {
        column.take_top(2.0);
        ui.strict_text(column, hint, SMALL, t.text2, Align::Start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_password_needs_ten_characters_and_a_matching_confirmation() {
        let mut view = LockView::new(Mode::Setup);
        view.password = Zeroizing::new("short".into());
        view.confirm = Zeroizing::new("short".into());
        assert_eq!(
            view.check_new(),
            Err("Use at least 10 characters.".to_owned())
        );
        view.password = Zeroizing::new("long enough pw".into());
        assert_eq!(
            view.check_new(),
            Err("The two passwords differ.".to_owned())
        );
        view.confirm = Zeroizing::new("long enough pw".into());
        assert_eq!(view.check_new(), Ok(()));
    }

    #[test]
    fn what_the_lock_screen_hands_over_never_prints_the_secret() {
        let create = LockAction::Create(Zeroizing::new("hunter2-but-longer".into()));
        let unlock = LockAction::Unlock {
            secret: Zeroizing::new("ABCD-EFGH-JKLM".into()),
            recovery: true,
        };
        let printed = format!("{create:?} {unlock:?}");
        assert!(
            !printed.contains("hunter2") && !printed.contains("ABCD"),
            "{printed}"
        );
        assert!(printed.contains("recovery: true"), "{printed}");
    }

    #[test]
    fn a_long_but_common_master_password_is_refused() {
        let mut view = LockView::new(Mode::Setup);
        view.password = Zeroizing::new("Password2024!".into());
        view.confirm = Zeroizing::new("Password2024!".into());
        assert_eq!(
            view.check_new(),
            Err("Too easy to guess. Make it longer or less common.".to_owned())
        );
    }
}
