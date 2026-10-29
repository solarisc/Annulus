//! The lock screen (UI-SPEC §5, F-SEC-004): create the vault on first run, unlock it after.
//! Password fields edit zeroizing buffers and are wiped once handed over.

use annulus_config::tokens::space;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Ui};
use annulus_ui::widgets::{BODY, CONTROL, FieldStyle, Kind, SMALL};
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

/// What the user asked the vault to do.
#[derive(Debug, PartialEq, Eq)]
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
    /// A new vault, waiting for its recovery key to be confirmed.
    pending: Option<Box<annulus_vault::vault::Vault>>,
    key: Zeroizing<String>,
    typed: String,
    /// How hard the new master password is to guess, while one is typed.
    strength: Option<Strength>,
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
            pending: None,
            key: Zeroizing::new(String::new()),
            typed: String::new(),
            strength: None,
        }
    }

    /// Shows `key` for the new `vault`, which opens once the key is confirmed.
    pub fn show_recovery(&mut self, vault: annulus_vault::vault::Vault, key: Zeroizing<String>) {
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
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
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
        let fields = if setup { 2.0 } else { 1.0 };
        let meter_h = if setup { space::S + METER_H } else { 0.0 };
        let error_h = if self.error.is_some() { 28.0 } else { 0.0 };
        let height = 64.0
            + 20.0
            + 28.0
            + 22.0
            + 28.0
            + fields * (CONTROL + space::M)
            + error_h
            + meter_h
            + CONTROL
            + if setup { 0.0 } else { 44.0 };
        let card = rect.centered(rect.w.min(400.0), height + 2.0 * space::XL);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL + 4.0, space::XL);

        let mark = column.take_top(56.0).centered(56.0, 56.0);
        ui.gfx.fill_rect(mark, 16.0, t.selected);
        ui.gfx.stroke_rect(mark, 16.0, 1.0, t.accent.alpha(0.35));
        ui.gfx.icon(
            if setup { icons::SHIELD } else { icons::LOCK },
            mark.centered(26.0, 26.0),
            t.accent,
        );
        column.take_top(space::L);
        ui.strict_text(
            column.take_top(26.0),
            title,
            annulus_ui::widgets::HEADING,
            t.text,
            Align::Center,
        );
        column.take_top(2.0);
        ui.strict_text(column.take_top(20.0), detail, BODY, t.text2, Align::Center);
        column.take_top(space::XL - 4.0);

        let placeholder = match (setup, self.recovery) {
            (true, _) => "Master password (10+ characters)",
            (false, false) => "Master password",
            (false, true) => "Recovery key",
        };
        let first = ui.text_field(
            Id::new("lock.password"),
            column.take_top(CONTROL),
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
                column.take_top(CONTROL),
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
        if let Some(error) = &self.error {
            column.take_top(space::S);
            let mut line = column.take_top(20.0);
            let icon = line.take_left(16.0).centered(14.0, 14.0);
            ui.gfx.icon(icons::WARNING, icon, t.error);
            line.take_left(space::S);
            ui.gfx.text(line, error, SMALL, t.error, Align::Start);
        }
        column.take_top(space::L);
        let go = column.take_top(CONTROL);
        let busy = self.working.is_some();
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
                    Err(problem) => self.error = Some(problem),
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
            let width = annulus_ui::widgets::button_width(ui.gfx, link, None);
            let row = column.take_top(CONTROL);
            let at = row.centered(width.min(row.w), CONTROL);
            if ui
                .button(Id::new("lock.recovery"), at, link, None, Kind::Ghost)
                .clicked
            {
                self.recovery = !self.recovery;
                self.error = None;
                self.password = Zeroizing::new(String::new());
            }
        }
        if ui.key(Key::Escape, Modifiers::NONE) {
            self.error = None;
        }
        action
    }
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
    if let Some(hint) = strength.hint {
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
