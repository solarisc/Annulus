//! Account & sync and the master password, in Settings (UI-SPEC §6, F-SYNC-001…006,
//! F-SEC-002). The work is `sync_screen::SyncScreen`'s and the vault's; this draws it.

use std::path::Path;

use annulus_config::tokens::space;
use annulus_sync::account;
use annulus_sync::http::HttpServer;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Ui};
use annulus_ui::motion::Layer;
use annulus_ui::widgets::{BODY, BODY_STRONG, CONTROL, FieldStyle, Kind, SMALL, button_width};
use annulus_vault::vault::{Vault, VaultError};
use zeroize::Zeroizing;

use crate::job::{Job, Pace, Poll};
use crate::password_screen;
use crate::sync_screen::{Attempt, SyncScreen, ago, now_secs, pill};

/// The forms' typed text, wiped when dropped.
#[derive(Default)]
pub struct AccountView {
    password: Zeroizing<String>,
    /// The single-use code a server with closed sign-ups needs to create an account.
    invite: Zeroizing<String>,
    current: Zeroizing<String>,
    new: Zeroizing<String>,
    confirm: Zeroizing<String>,
    changing: bool,
    change: Option<Job<Result<Vec<u8>, String>>>,
    change_message: Option<(bool, String)>,
    /// How tall the account card was last frame.
    card_h: f32,
    /// How tall the master password card was last frame.
    password_h: f32,
}

/// A small primary-or-secondary button at the left of `row`, which shrinks.
fn push_button(
    ui: &mut Ui<'_>,
    row: &mut Rect,
    id: &str,
    label: &str,
    kind: Kind,
    enabled: bool,
) -> bool {
    let width = button_width(&mut ui.gfx, label, None);
    let rect = row.take_left(width);
    row.take_left(space::S);
    // Not ready yet, it keeps its kind and rests dim: what it will do stays clear.
    ui.push_layer(Layer::fade(if enabled { 1.0 } else { 0.45 }));
    let clicked = ui.button(Id::new(id), rect, label, None, kind).clicked && enabled;
    ui.pop_layer();
    clicked
}

/// A label over the field below it, taken from the top of `column`; `column` is left where the
/// field goes.
fn labelled(ui: &mut Ui<'_>, column: &mut Rect, label: &str) -> Rect {
    let t = ui.theme;
    ui.strict_text(column.take_top(16.0), label, SMALL, t.text2, Align::Start);
    column.take_top(5.0);
    column.take_top(CONTROL)
}

impl AccountView {
    /// Draws the account card from `rect.y` down; returns the height it took.
    #[allow(clippy::too_many_lines)] // Signed out and signed in, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        (sync, vault, pace, vault_path): (&mut SyncScreen, &mut Vault, &dyn Pace, &Path),
    ) -> f32 {
        let t = ui.theme;
        let top = rect.y;
        let column = rect;
        let card_top = column.y;
        // Its height is known once its content is drawn: last frame's serves.
        ui.card_surface(Rect::new(column.x, card_top, column.w, self.card_h), 12.0);
        let mut inner = Rect::new(
            column.x + space::L,
            column.y + space::M,
            column.w - 2.0 * space::L,
            2000.0,
        );
        let busy = sync.work.as_ref().map(|(label, _)| label.clone());
        let account = vault.sync_state().account.clone();
        // The card's fill goes under its content: drawn after, from the height used.
        let mut body = Vec::new();
        match &account {
            None => {
                ui.gfx.text(
                    inner.take_top(20.0),
                    "Sync this vault through your own Annulus server.",
                    BODY_STRONG,
                    t.text,
                    Align::Start,
                );
                ui.gfx.text(inner.take_top(18.0), "The server stores only ciphertext; your master password never leaves this device.", SMALL, t.text2, Align::Start);
                inner.take_top(space::M);
                let half = (inner.w - space::M) / 2.0;
                let mut row = inner.take_top(16.0 + 5.0 + CONTROL);
                let mut left = row.take_left(half);
                row.take_left(space::M);
                let field = labelled(ui, &mut left, "Server");
                let server = ui.text_field(
                    Id::new("account.server"),
                    field,
                    &mut sync.server,
                    FieldStyle {
                        placeholder: "https://sync.example.com",
                        ..FieldStyle::default()
                    },
                );
                ui.set_name(Id::new("account.server"), "Server");
                // A trusted certificate belongs to the address it was trusted for.
                if server.changed {
                    sync.pin = None;
                    sync.untrusted = None;
                }
                let field = labelled(ui, &mut row, "Account name");
                ui.text_field(
                    Id::new("account.name"),
                    field,
                    &mut sync.account,
                    FieldStyle::default(),
                );
                ui.set_name(Id::new("account.name"), "Account name");
                inner.take_top(space::M);
                let mut row = inner.take_top(16.0 + 5.0 + CONTROL);
                let mut left = row.take_left(half);
                row.take_left(space::M);
                let field = labelled(ui, &mut left, "This device's name");
                ui.text_field(
                    Id::new("account.device"),
                    field,
                    &mut sync.device,
                    FieldStyle::default(),
                );
                ui.set_name(Id::new("account.device"), "This device's name");
                let field = labelled(ui, &mut row, "This vault's master password");
                ui.text_field(
                    Id::new("account.password"),
                    field,
                    &mut self.password,
                    FieldStyle {
                        icon: Some(icons::KEY),
                        ..FieldStyle::default()
                    },
                );
                ui.set_name(Id::new("account.password"), "This vault's master password");
                inner.take_top(space::M);
                let mut row = inner.take_top(16.0 + 5.0 + CONTROL);
                let mut left = row.take_left(half);
                let field = labelled(ui, &mut left, "Invite code, to create an account");
                ui.text_field(
                    Id::new("account.invite"),
                    field,
                    &mut self.invite,
                    FieldStyle {
                        placeholder: "Only where sign-ups are closed",
                        ..FieldStyle::default()
                    },
                );
                ui.set_name(Id::new("account.invite"), "Invite code");
                if let Some((certificate, attempt)) = sync.untrusted.clone() {
                    inner.take_top(space::M);
                    certificate_box(ui, &mut inner, sync, &certificate, attempt);
                } else if sync.pin.is_some() {
                    inner.take_top(space::S);
                    ui.gfx.text(
                        inner.take_top(18.0),
                        "This server's own certificate is trusted for this account.",
                        SMALL,
                        t.ok,
                        Align::Start,
                    );
                }
                inner.take_top(space::M);
                let ready = busy.is_none()
                    && !sync.server.trim().is_empty()
                    && !sync.account.trim().is_empty()
                    && !self.password.is_empty();
                let mut row = inner.take_top(CONTROL);
                let sign_in = push_button(
                    ui,
                    &mut row,
                    "account.join",
                    "Sign in",
                    Kind::Primary,
                    ready,
                );
                let create = push_button(
                    ui,
                    &mut row,
                    "account.create",
                    "Create account",
                    Kind::Secondary,
                    ready,
                );
                if sign_in || create {
                    sync.set_password(&self.password);
                    self.password = Zeroizing::new(String::new());
                    if sign_in {
                        sync.join(pace, vault, vault_path);
                    } else {
                        sync.create(pace, vault, &self.invite);
                        self.invite = Zeroizing::new(String::new());
                    }
                }
                inner.take_top(space::S);
                ui.gfx.text(inner.take_top(18.0), "Signing in on a device with hosts merges them into the account; the old vault file is kept.", SMALL, t.text2, Align::Start);
            }
            Some(account) => {
                let state = vault.sync_state();
                let (_, words) = pill(
                    state.last_synced,
                    state.pending_count(),
                    sync.round_failed,
                    now_secs(),
                );
                ui.gfx.text(
                    inner.take_top(20.0),
                    &format!("{} on {}", account.name, account.server),
                    BODY_STRONG,
                    t.text,
                    Align::Start,
                );
                ui.gfx
                    .text(inner.take_top(18.0), &words, SMALL, t.text2, Align::Start);
                inner.take_top(space::M);
                let idle = busy.is_none();
                let mut row = inner.take_top(CONTROL);
                if push_button(
                    ui,
                    &mut row,
                    "account.sync",
                    "Sync now",
                    Kind::Primary,
                    idle,
                ) {
                    sync.sync_now();
                }
                if push_button(
                    ui,
                    &mut row,
                    "account.devices",
                    "Devices",
                    Kind::Secondary,
                    idle,
                ) {
                    sync.load_devices(pace, account);
                }
                if push_button(
                    ui,
                    &mut row,
                    "account.deleted",
                    "Deleted items",
                    Kind::Secondary,
                    idle,
                ) {
                    sync.load_deleted(pace, account, vault);
                }
                if push_button(ui, &mut row, "account.out", "Sign out", Kind::Danger, idle) {
                    sync.sign_out(vault);
                }
                let mut revoke = None;
                for device in &sync.devices {
                    inner.take_top(space::S);
                    let mut line = inner.take_top(28.0);
                    let name = if device.current {
                        format!("{} (this device)", device.name)
                    } else {
                        device.name.clone()
                    };
                    if !device.current {
                        let out = line.take_right(button_width(&mut ui.gfx, "Sign out", None));
                        if ui
                            .button(
                                Id::new(("account.revoke", device.id)),
                                out,
                                "Sign out",
                                None,
                                Kind::Ghost,
                            )
                            .clicked
                        {
                            revoke = Some(device.id);
                        }
                    }
                    let icon = line.take_left(16.0).centered(16.0, 16.0);
                    ui.gfx.icon(icons::DEVICE, icon, t.text2);
                    line.take_left(space::S);
                    body.push((
                        line,
                        format!("{name} \u{b7} seen {}", ago(device.last_seen, now_secs())),
                    ));
                }
                if let Some(id) = revoke {
                    sync.revoke(pace, account, id);
                }
                let mut restore = None;
                let mut index = 0;
                while let Some(name) = sync.deleted_name(index) {
                    inner.take_top(space::S);
                    let mut line = inner.take_top(28.0);
                    let again = line.take_right(button_width(&mut ui.gfx, "Restore", None));
                    if ui
                        .button(
                            Id::new(("account.restore", index)),
                            again,
                            "Restore",
                            None,
                            Kind::Ghost,
                        )
                        .clicked
                    {
                        restore = Some(index);
                    }
                    let icon = line.take_left(16.0).centered(16.0, 16.0);
                    ui.gfx.icon(icons::TRASH, icon, t.text2);
                    line.take_left(space::S);
                    body.push((line, name.to_owned()));
                    index += 1;
                }
                if let Some(index) = restore {
                    sync.restore(pace, account, vault, index);
                }
            }
        }
        if let Some(label) = &busy {
            inner.take_top(space::S);
            ui.gfx.text(
                inner.take_top(18.0),
                &format!("{label}\u{2026}"),
                SMALL,
                t.accent,
                Align::Start,
            );
        }
        if let Some((error, message)) = &sync.message {
            inner.take_top(space::S);
            ui.gfx.text(
                inner.take_top(18.0),
                message,
                SMALL,
                if *error { t.error } else { t.ok },
                Align::Start,
            );
        }
        for (line, text) in body {
            ui.gfx.text(line, &text, BODY, t.text, Align::Start);
        }
        let card = Rect::new(column.x, card_top, column.w, inner.y - card_top + space::M);
        self.card_h = card.h;
        // Room to lay out in; its height becomes what the password card took.
        let mut after = Rect::new(column.x, card.b() + space::XL, column.w, 1000.0);
        after.h = self.password_card(ui, after, vault, pace);
        after.b() - top
    }

    /// The master password row, and its change form once opened.
    #[allow(clippy::too_many_lines)] // The row, the form and its result.
    fn password_card(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        pace: &dyn Pace,
    ) -> f32 {
        let t = ui.theme;
        if let Some(job) = &self.change {
            match job.poll() {
                Poll::Running => {}
                Poll::Done(result) => {
                    self.change = None;
                    self.change_message = Some(
                        match result.and_then(|key_file| {
                            vault.set_key_file(&key_file).map_err(|e| e.to_string())
                        }) {
                            Ok(()) => (false, "Master password changed.".into()),
                            Err(message) => (true, message),
                        },
                    );
                    self.changing = false;
                }
                Poll::Lost => {
                    self.change = None;
                    self.change_message = Some((true, "The change did not finish.".into()));
                }
            }
        }
        let mut column = rect;
        ui.section_label(column.take_top(18.0), "Master password");
        column.take_top(space::S);
        let card_top = column.y;
        ui.card_surface(
            Rect::new(column.x, card_top, column.w, self.password_h),
            12.0,
        );
        let mut inner = Rect::new(
            column.x + space::L,
            column.y + space::M,
            column.w - 2.0 * space::L,
            1000.0,
        );
        let mut row = inner.take_top(CONTROL);
        let change = row.take_right(button_width(&mut ui.gfx, "Change", None));
        ui.gfx.text(
            row,
            "Unlocks this vault, and your account when you sync.",
            BODY,
            t.text,
            Align::Start,
        );
        if !self.changing
            && ui
                .button(
                    Id::new("password.open"),
                    change,
                    "Change",
                    None,
                    Kind::Secondary,
                )
                .clicked
        {
            self.changing = true;
        }
        if self.changing {
            for (id, buffer, placeholder) in [
                ("password.current", &mut self.current, "Current password"),
                (
                    "password.new",
                    &mut self.new,
                    "New password (10+ characters)",
                ),
                (
                    "password.confirm",
                    &mut self.confirm,
                    "Type the new one again",
                ),
            ] {
                inner.take_top(space::S);
                ui.text_field(
                    Id::new(id),
                    inner.take_top(CONTROL),
                    buffer,
                    FieldStyle {
                        placeholder,
                        icon: Some(icons::KEY),
                        ..FieldStyle::default()
                    },
                );
            }
            inner.take_top(space::M);
            let mut row = inner.take_top(CONTROL);
            let problem = password_screen::problem(&self.new, &self.confirm);
            let ready = self.change.is_none() && problem.is_none() && !self.current.is_empty();
            if push_button(
                ui,
                &mut row,
                "password.save",
                "Change password",
                Kind::Primary,
                ready,
            ) {
                let current = std::mem::take(&mut self.current);
                let new = std::mem::take(&mut self.new);
                self.confirm = Zeroizing::new(String::new());
                let kit = vault.password_kit();
                let account = vault.sync_state().account.clone();
                self.change = Some(Job::start(pace, move || {
                    let key_file = kit.rewrap(&current, &new).map_err(|error| match error {
                        VaultError::WrongPassword => "The current password is wrong.".to_owned(),
                        other => other.to_string(),
                    })?;
                    if let Some(account) = account {
                        let mut http =
                            HttpServer::for_account(&account).map_err(|error| error.to_string())?;
                        account::change_password(
                            &mut http,
                            &account::default_ksf(),
                            &current,
                            &new,
                            key_file.clone(),
                        )
                        .map_err(|error| {
                            format!("The account could not be updated, so nothing changed: {error}")
                        })?;
                    }
                    Ok(key_file)
                }));
            }
            if push_button(ui, &mut row, "password.cancel", "Cancel", Kind::Ghost, true) {
                self.changing = false;
            }
            if let Some(problem) = problem.filter(|_| !self.new.is_empty()) {
                ui.gfx.text(row, problem, SMALL, t.text2, Align::Start);
            }
        }
        if self.change.is_some() {
            inner.take_top(space::S);
            ui.gfx.text(
                inner.take_top(18.0),
                "Changing the password\u{2026}",
                SMALL,
                t.accent,
                Align::Start,
            );
        }
        if let Some((error, message)) = &self.change_message {
            inner.take_top(space::S);
            ui.gfx.text(
                inner.take_top(18.0),
                message,
                SMALL,
                if *error { t.error } else { t.ok },
                Align::Start,
            );
        }
        let card = Rect::new(column.x, card_top, column.w, inner.y - card_top + space::M);
        self.password_h = card.h;
        card.b() - rect.y
    }
}

/// The certificate a server showed, by fingerprint, with a choice to trust it (ADR-0018);
/// trusting it says how to redo `attempt`, the step it stopped.
fn certificate_box(
    ui: &mut Ui<'_>,
    inner: &mut Rect,
    sync: &mut SyncScreen,
    certificate: &[u8],
    attempt: Attempt,
) {
    let t = ui.theme;
    let fingerprint = annulus_sync::pin::fingerprint(certificate);
    // 32 bytes as "AB:" pairs: two lines of 16 read and compare more easily than one.
    let (first, second) = fingerprint.split_at(fingerprint.len().min(47));
    let lines = [
        "Trust it only if this SHA-256 fingerprint matches what",
        "\u{201c}annulus-server fingerprint\u{201d} prints on the server:",
    ];
    let height = 20.0 + 18.0 * 4.0 + space::S * 3.0 + CONTROL + space::M * 2.0;
    let card = inner.take_top(height);
    ui.gfx.fill_rect(card, 10.0, t.warn.alpha(0.08));
    ui.gfx.stroke_rect(card, 10.0, 1.0, t.warn.alpha(0.5));
    let mut column = card.inset(space::M, space::M);
    let mut title = column.take_top(20.0);
    let icon = title.take_left(16.0).centered(14.0, 14.0);
    ui.gfx.icon(icons::SHIELD, icon, t.warn);
    title.take_left(space::S);
    ui.gfx.text(
        title,
        "This server uses its own certificate, not one from a public authority.",
        BODY_STRONG,
        t.text,
        Align::Start,
    );
    column.take_top(space::S);
    for line in lines {
        ui.gfx
            .text(column.take_top(18.0), line, SMALL, t.text2, Align::Start);
    }
    column.take_top(space::S);
    for part in [first, second.trim_start_matches(':')] {
        ui.gfx.text(
            column.take_top(18.0),
            part,
            annulus_ui::gfx::Font::mono(12.0),
            t.text,
            Align::Start,
        );
    }
    column.take_top(space::S);
    let mut row = column.take_top(CONTROL);
    if push_button(
        ui,
        &mut row,
        "account.trust",
        "Trust this certificate",
        Kind::Primary,
        true,
    ) {
        sync.pin = Some(certificate.to_vec());
        sync.untrusted = None;
        sync.message = Some((false, attempt.after_trust().into()));
    }
    if push_button(
        ui,
        &mut row,
        "account.distrust",
        "Cancel",
        Kind::Ghost,
        true,
    ) {
        sync.untrusted = None;
        sync.message = None;
    }
}
