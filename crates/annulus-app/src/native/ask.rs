//! The questions an SSH connection asks before it can go on (UI-SPEC §5): a server key seen
//! for the first time, and a password. Terminals and port-forwarding rules both ask them.

use annulus_config::tokens::space;
use annulus_session::{Answers, HostKeyQuestion, SessionEvent};
use annulus_ssh::Trust;
use annulus_ui::gfx::{Align, Font, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Role, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, HEADING, Kind, SMALL, button_width};
use zeroize::Zeroizing;

/// A question a connection is waiting on.
pub enum Ask {
    /// A server key seen for the first time.
    HostKey(HostKeyQuestion),
    /// A password for `user@host`.
    Password {
        /// Login name.
        user: String,
        /// Host.
        host: String,
        /// What is typed, wiped when dropped.
        secret: Zeroizing<String>,
    },
}

impl Ask {
    /// The question `event` asks, if it is one.
    #[must_use]
    pub fn from_event(event: SessionEvent) -> Option<Self> {
        match event {
            SessionEvent::HostKey(question) => Some(Ask::HostKey(question)),
            SessionEvent::Password { user, host } => Some(Ask::Password {
                user,
                host,
                secret: Zeroizing::new(String::new()),
            }),
            _ => None,
        }
    }
}

/// Draws the question over `rect` and sends the answer through `answers`; returns whether it
/// was answered. Its zones' ids are made from `id` (the pane or screen asking), so two
/// questions on screen at once never share one.
#[allow(clippy::too_many_lines)] // Both questions' cards, each top to bottom.
pub fn card(ui: &mut Ui<'_>, id: Id, rect: Rect, ask: &mut Ask, answers: &Answers) -> bool {
    let t = ui.theme;
    let card = rect.centered(rect.w.min(460.0), 236.0);
    ui.gfx.fill_rect(rect, 0.0, t.scrim);
    ui.zone(id.with("ask.backdrop"), rect, Role::Backdrop, "");
    ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
    ui.gfx.fill_rect(card, 14.0, t.raised);
    ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
    let mut column = card.inset(space::XL, space::XL);
    let mut answered = false;
    match ask {
        Ask::HostKey(question) => {
            ui.strict_text(
                column.take_top(24.0),
                &format!("First connection to {}", question.host),
                HEADING,
                t.text,
                Align::Start,
            );
            column.take_top(space::XS);
            ui.gfx.text(
                column.take_top(20.0),
                "Check this key with the server's owner before trusting it.",
                SMALL,
                t.text2,
                Align::Start,
            );
            column.take_top(space::M);
            let key = column.take_top(52.0);
            ui.gfx.fill_rect(key, 8.0, t.field);
            let mut lines = key.inset(space::M, space::S);
            ui.gfx.text(
                lines.take_top(18.0),
                &question.algorithm,
                SMALL,
                t.text2,
                Align::Start,
            );
            ui.gfx.text(
                lines.take_top(18.0),
                &question.fingerprint,
                Font::mono(12.0),
                t.text,
                Align::Start,
            );
            column.take_top(space::L);
            let mut row = column.take_top(CONTROL);
            let trust_w = button_width(ui.gfx, "Trust and connect", None);
            let once_w = button_width(ui.gfx, "Connect once", None);
            let cancel_w = button_width(ui.gfx, "Cancel", None);
            let cancel = row.take_left(cancel_w);
            let trust = row.take_right(trust_w);
            row.take_right(space::S);
            let once = row.take_right(once_w);
            if ui
                .button(id.with("ask.cancel"), cancel, "Cancel", None, Kind::Ghost)
                .clicked
                || ui.key(Key::Escape, Modifiers::NONE)
            {
                answers.host_key_answer(Trust::Reject);
                answered = true;
            }
            if ui
                .button(
                    id.with("ask.once"),
                    once,
                    "Connect once",
                    None,
                    Kind::Secondary,
                )
                .clicked
            {
                answers.host_key_answer(Trust::Once);
                answered = true;
            }
            if ui
                .button(
                    id.with("ask.trust"),
                    trust,
                    "Trust and connect",
                    None,
                    Kind::Primary,
                )
                .clicked
            {
                answers.host_key_answer(Trust::Save);
                answered = true;
            }
        }
        Ask::Password { user, host, secret } => {
            ui.strict_text(
                column.take_top(24.0),
                &format!("Password for {user}@{host}"),
                HEADING,
                t.text,
                Align::Start,
            );
            column.take_top(space::XS);
            ui.gfx.text(
                column.take_top(20.0),
                "Used for this connection only; it is not saved.",
                SMALL,
                t.text2,
                Align::Start,
            );
            column.take_top(space::L);
            let field = ui.text_field(
                id.with("ask.password"),
                column.take_top(CONTROL),
                secret,
                FieldStyle {
                    placeholder: "Password",
                    icon: Some(icons::KEY),
                    ..FieldStyle::default()
                },
            );
            if ui.state.focus().is_none() {
                ui.state.set_focus(Some(id.with("ask.password")), false);
            }
            column.take_top(space::XL);
            let mut row = column.take_top(CONTROL);
            let connect = row.take_right(button_width(ui.gfx, "Connect", None).max(96.0));
            row.take_right(space::S);
            let cancel = row.take_right(button_width(ui.gfx, "Cancel", None));
            if ui
                .button(id.with("ask.cancel"), cancel, "Cancel", None, Kind::Ghost)
                .clicked
                || ui.key(Key::Escape, Modifiers::NONE)
            {
                answers.password(None);
                answered = true;
            }
            if ui
                .button(
                    id.with("ask.connect"),
                    connect,
                    "Connect",
                    None,
                    Kind::Primary,
                )
                .clicked
                || field.submitted
            {
                answers.password(Some(std::mem::replace(
                    secret,
                    Zeroizing::new(String::new()),
                )));
                answered = true;
            }
        }
    }
    answered
}
