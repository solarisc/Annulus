//! The lock screen (UI-SPEC §5, F-SEC-004): create the vault on first run, unlock it after.
//! Password fields edit zeroizing buffers and are wiped once handed over.

use annulus_config::tokens::space;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Ui};
use annulus_ui::widgets::{BODY, CONTROL, FieldStyle, Kind, SMALL};
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
}

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
        }
    }

    /// Checks a new master password and its confirmation.
    fn check_new(&self) -> Result<(), String> {
        if self.password.chars().count() < MIN_PASSWORD {
            return Err(format!("Use at least {MIN_PASSWORD} characters."));
        }
        if self.password.as_str() != self.confirm.as_str() {
            return Err("The two passwords differ.".into());
        }
        Ok(())
    }

    /// Draws the screen centered in `rect`.
    #[allow(clippy::too_many_lines)] // One card, top to bottom.
    pub fn draw(&mut self, ui: &mut Ui<'_>, rect: Rect) -> Option<LockAction> {
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
        let error_h = if self.error.is_some() { 28.0 } else { 0.0 };
        let height = 64.0
            + 20.0
            + 28.0
            + 22.0
            + 28.0
            + fields * (CONTROL + space::M)
            + error_h
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
        }
        if setup {
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
        let clicked = ui
            .button(Id::new("lock.go"), go, button, None, Kind::Primary)
            .clicked;
        let mut action = None;
        if clicked || submitted {
            if setup {
                match self.check_new() {
                    Ok(()) => {
                        self.confirm = Zeroizing::new(String::new());
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
}
