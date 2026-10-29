//! Settings (UI-SPEC §6): the sections in their own navigation down the left, the chosen
//! one beside it with a header and its cards. Account & sync comes first when there is a
//! vault file; Shortcuts lists the app's shortcut table ([`super::shortcuts`]), searchable.

use annulus_config::settings::Settings;
use annulus_config::tokens::{ThemeMode, space};
use annulus_session::profiles::Profile;
use annulus_term::alacritty_terminal::vte::ansi::Rgb;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Font, Point, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Role, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, BODY_STRONG, CONTROL, FieldStyle, KEYCAP_H, SMALL, TITLE, keys_width,
};

use super::pages;
use super::shortcuts;

/// Draws the account card into a rectangle and returns the height it took.
pub type AccountCard<'a> = &'a mut dyn FnMut(&mut Ui<'_>, Rect) -> f32;

/// A section of Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// The sync account: server, devices, password.
    #[default]
    Account,
    /// Theme and motion.
    Appearance,
    /// Colors and text size.
    Terminal,
    /// Auto-lock.
    Security,
    /// Reopening tabs and session logs.
    Sessions,
    /// The shell new terminals run.
    Shells,
    /// Every keyboard shortcut.
    Shortcuts,
    /// Version and licenses.
    About,
}

impl Page {
    const ALL: [Page; 8] = [
        Page::Account,
        Page::Appearance,
        Page::Terminal,
        Page::Security,
        Page::Sessions,
        Page::Shells,
        Page::Shortcuts,
        Page::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::Account => "Account & sync",
            Page::Appearance => "Appearance",
            Page::Terminal => "Terminal",
            Page::Security => "Security",
            Page::Sessions => "Sessions",
            Page::Shells => "Local shells",
            Page::Shortcuts => "Shortcuts",
            Page::About => "About",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Page::Account => "Your own server keeps your vault in sync, end-to-end encrypted.",
            Page::Appearance => "How Annulus looks and moves on this device.",
            Page::Terminal => "The colors programs draw with and the size of their text.",
            Page::Security => "When the vault locks itself.",
            Page::Sessions => "What Annulus keeps of your sessions.",
            Page::Shells => "What a new local terminal runs.",
            Page::Shortcuts => "Every shortcut, as the keyboard says it.",
            Page::About => "The app, its licenses and where it comes from.",
        }
    }

    fn icon(self) -> IconId {
        match self {
            Page::Account => icons::SYNC,
            Page::Appearance => icons::SUN,
            Page::Terminal => icons::TERMINAL,
            Page::Security => icons::LOCK,
            Page::Sessions => icons::CLOCK,
            Page::Shells => icons::DEVICE,
            Page::Shortcuts => icons::KEYBOARD,
            Page::About => icons::INFO,
        }
    }
}

/// Settings' own state.
#[derive(Default)]
pub struct SettingsView {
    /// The section shown.
    pub page: Page,
    /// The Shortcuts search.
    search: String,
    /// How tall the page drew last frame, for its scroll.
    content_h: f32,
}

const LOCK_MINUTES: [(u32, &str); 5] = [
    (0, "Never"),
    (5, "After 5 minutes"),
    (15, "After 15 minutes"),
    (30, "After 30 minutes"),
    (60, "After 1 hour"),
];

/// Width of the sections' navigation.
const NAV_W: f32 = 212.0;
/// Height of one setting row.
const ROW: f32 = 60.0;
/// The widest the section column gets.
const COLUMN_W: f32 = 780.0;

impl SettingsView {
    /// Draws Settings into `rect`, with the account card (`account`, which returns the height
    /// it took) when there is a vault file; `account_h` is that height from the frame before.
    /// Returns whether a setting changed and the account card's height now.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        (settings, profiles): (&mut Settings, &[Profile]),
        account: Option<AccountCard<'_>>,
        account_h: f32,
    ) -> (bool, f32) {
        let t = ui.theme;
        if account.is_none() && self.page == Page::Account {
            self.page = Page::Appearance;
        }
        let mut area = rect.inset(space::L, space::L);
        let nav = area.take_left(NAV_W);
        self.navigation(ui, nav, account.is_some());
        area.take_left(space::XL);
        // A hairline between the navigation and the section.
        ui.gfx.fill_rect(
            Rect::new(nav.r() + space::L - 1.0, area.y, 1.0, area.h),
            0.0,
            t.line,
        );
        let column = Rect::new(area.x, area.y, area.w.min(COLUMN_W), area.h);
        // Each section rises into place when chosen.
        let shown = ui.entrance(Id::new(("settings.page", self.page.title())), Motion::BASE);
        ui.push_layer(Layer::fade(shown).shifted(0.0, 10.0 * (1.0 - shown)));
        let mut body = column;
        section_header(ui, &mut body, self.page);
        let offset = ui.scroll_view(Id::new("settings.scroll"), body, self.content_h);
        ui.push_clip(body);
        let mut page = Rect::new(body.x, body.y - offset, body.w - 12.0, f32::MAX / 4.0);
        let top = page.y;
        let mut changed = false;
        let mut drawn_h = account_h;
        match self.page {
            Page::Account => {
                if let Some(account) = account {
                    drawn_h = account(ui, page);
                    page.take_top(drawn_h);
                }
            }
            Page::Appearance => changed = appearance(ui, &mut page, settings),
            Page::Terminal => changed = terminal(ui, &mut page, settings),
            Page::Security => changed = security(ui, &mut page, settings),
            Page::Sessions => changed = sessions(ui, &mut page, settings),
            Page::Shells => changed = shells(ui, &mut page, settings, profiles),
            Page::Shortcuts => shortcuts_page(ui, &mut page, &mut self.search),
            Page::About => about(ui, &mut page),
        }
        self.content_h = page.y - top + space::XL;
        ui.pop_clip();
        ui.pop_layer();
        (changed, drawn_h)
    }

    /// The sections down the left, the chosen one on a pill that glides between them.
    fn navigation(&mut self, ui: &mut Ui<'_>, rect: Rect, with_account: bool) {
        let t = ui.theme;
        let mut column = rect;
        ui.strict_text(
            column.take_top(32.0),
            "Settings",
            TITLE,
            t.text,
            Align::Start,
        );
        ui.gfx.text(
            column.take_top(18.0),
            "Saved on this device",
            SMALL,
            t.text2,
            Align::Start,
        );
        column.take_top(space::L);
        let pages: Vec<Page> = Page::ALL
            .into_iter()
            .filter(|page| with_account || *page != Page::Account)
            .collect();
        let rows: Vec<Rect> = pages.iter().map(|_| column.take_top(38.0)).collect();
        if let Some(at) = pages.iter().position(|page| *page == self.page) {
            let target = rows[at];
            let y = ui.spring(Id::new("settings.nav.pill"), target.y);
            let pill = Rect::new(target.x, y + 1.0, target.w, target.h - 2.0);
            ui.gfx.fill_rect(pill, 9.0, t.selected);
            let bar = Rect::new(pill.x, pill.y + 9.0, 3.0, pill.h - 18.0);
            ui.gfx.fill_rect(bar, 1.5, t.accent);
        }
        for (page, row) in pages.iter().zip(rows) {
            let row = row.inset(0.0, 1.0);
            let id = Id::new(("settings.nav", page.title()));
            let response = ui.zone(id, row, Role::Tab, page.title());
            if response.clicked {
                self.page = *page;
            }
            let selected = self.page == *page;
            let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
            if !selected {
                ui.gfx.fill_rect(row, 9.0, t.hover.alpha(hover * 0.8));
            }
            if response.focus_ring {
                ui.focus_ring(row, 9.0);
            }
            let mut inner = row.inset(space::M, 0.0);
            let ink = if selected { t.accent } else { t.text2 };
            ui.gfx
                .icon(page.icon(), inner.take_left(16.0).centered(16.0, 16.0), ink);
            inner.take_left(space::M - 2.0);
            let (font, color) = if selected {
                (BODY_MEDIUM, t.text)
            } else {
                (BODY, t.text.mix(t.text2, 0.35 * (1.0 - hover)))
            };
            ui.strict_text(inner, page.title(), font, color, Align::Start);
            if response.hovered {
                ui.set_cursor(Cursor::Pointer);
            }
        }
    }
}

/// The section's icon, title and what it is for; leaves `rect` below.
fn section_header(ui: &mut Ui<'_>, rect: &mut Rect, page: Page) {
    let t = ui.theme;
    let mut head = rect.take_top(52.0);
    let tile = head.take_left(44.0).centered(44.0, 44.0);
    ui.gfx.fill_rect(tile, 12.0, t.accent.alpha(0.14));
    ui.gfx.stroke_rect(tile, 12.0, 1.0, t.accent.alpha(0.3));
    ui.gfx
        .icon(page.icon(), tile.centered(22.0, 22.0), t.accent);
    head.take_left(space::M + 2.0);
    let heading = Rect::new(head.x, head.y + 3.0, head.w, 26.0);
    ui.strict_text(heading, page.title(), TITLE, t.text, Align::Start);
    let line = Rect::new(head.x, head.y + 29.0, head.w, 18.0);
    ui.gfx
        .text(line, page.subtitle(), BODY, t.text2, Align::Start);
    rect.take_top(space::XL);
}

/// A card of `rows` setting rows under a label; returns its inside.
fn card(ui: &mut Ui<'_>, page: &mut Rect, title: &str, rows: usize) -> Rect {
    #[allow(clippy::cast_precision_loss)] // A few rows.
    let height = rows as f32 * ROW + 2.0 * space::S;
    pages::card(ui, page, title, height)
}

/// One row of a card: its title and explanation on the left; returns the control's place.
fn row(
    ui: &mut Ui<'_>,
    card: &mut Rect,
    first: bool,
    (title, detail): (&str, &str),
    w: f32,
) -> Rect {
    if !first {
        pages::divider(ui, card.y, *card);
    }
    pages::setting(ui, card.take_top(ROW), title, detail, w)
}

/// A choice among a few, its selection a pill that glides to the one chosen; returns the one
/// clicked.
fn segmented(
    ui: &mut Ui<'_>,
    id: &str,
    rect: Rect,
    items: &[&str],
    selected: usize,
) -> Option<usize> {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 9.0, t.field);
    ui.gfx.stroke_rect(rect, 9.0, 1.0, t.line);
    #[allow(clippy::cast_precision_loss)] // Two to four segments.
    let width = (rect.w - 4.0) / items.len() as f32;
    #[allow(clippy::cast_precision_loss)] // As above.
    let target = rect.x + 2.0 + selected as f32 * width;
    let x = ui.spring(Id::new((id, "pill")), target);
    let pill = Rect::new(x, rect.y + 2.0, width, rect.h - 4.0);
    ui.gfx.shadow(pill, 7.0, 6.0, 1.0, t.shadow.alpha(0.5));
    ui.gfx.fill_rect(pill, 7.0, t.surface.mix(t.hover, 0.4));
    ui.gfx.stroke_rect(pill, 7.0, 1.0, t.line_strong);
    let mut chosen = None;
    for (index, item) in items.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // As above.
        let cell = Rect::new(
            rect.x + 2.0 + index as f32 * width,
            rect.y + 2.0,
            width,
            rect.h - 4.0,
        );
        let response = ui.zone(Id::new((id, index)), cell, Role::Tab, item);
        if response.clicked {
            chosen = Some(index);
        }
        if response.focus_ring {
            ui.focus_ring(cell, 7.0);
        }
        let ink = if index == selected { t.text } else { t.text2 };
        ui.strict_text(cell, item, BODY_MEDIUM, ink, Align::Center);
        if response.hovered {
            ui.set_cursor(Cursor::Pointer);
        }
    }
    chosen
}

fn appearance(ui: &mut Ui<'_>, page: &mut Rect, settings: &mut Settings) -> bool {
    let mut changed = false;
    let mut card = card(ui, page, "Look", 2);
    let control = row(
        ui,
        &mut card,
        true,
        ("Theme", "Follow the system, or always light or dark."),
        300.0,
    );
    let modes = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];
    let at = modes
        .iter()
        .position(|mode| *mode == settings.theme)
        .unwrap_or(0);
    if let Some(chosen) = segmented(
        ui,
        "settings.theme",
        control,
        &["System", "Light", "Dark"],
        at,
    ) {
        settings.theme = modes[chosen];
        changed = true;
    }
    let control = row(
        ui,
        &mut card,
        false,
        (
            "Reduce motion",
            "Changes happen at once, without animating.",
        ),
        60.0,
    );
    changed |= ui
        .switch(
            Id::new("settings.motion"),
            control,
            &mut settings.reduce_motion,
            "Reduce motion",
        )
        .clicked;
    changed
}

fn terminal(ui: &mut Ui<'_>, page: &mut Rect, settings: &mut Settings) -> bool {
    let mut changed = false;
    let mut card = card(ui, page, "Text", 2);
    let control = row(
        ui,
        &mut card,
        true,
        ("Colors", "The palette programs draw with."),
        260.0,
    );
    let themes: Vec<String> = annulus_term::themes::THEMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let at = themes
        .iter()
        .position(|name| *name == settings.terminal_theme);
    if let Some(chosen) = ui.select(
        Id::new("settings.colors"),
        control,
        &themes,
        at,
        "Annulus Dark",
    ) {
        settings.terminal_theme.clone_from(&themes[chosen]);
        changed = true;
    }
    let control = row(
        ui,
        &mut card,
        false,
        (
            "Text size",
            "Ctrl+= and Ctrl+- change it; Ctrl+0 resets it.",
        ),
        260.0,
    );
    let sizes: Vec<String> = (11..=20).map(|size| format!("{size} pt")).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 8 to 32.
    let at = (settings.font_size.round() as usize).checked_sub(11);
    if let Some(chosen) = ui.select(Id::new("settings.font"), control, &sizes, at, "14 pt") {
        #[allow(clippy::cast_precision_loss)] // Small sizes.
        let size = (chosen + 11) as f32;
        settings.font_size = size;
        changed = true;
    }
    preview(ui, page, settings);
    changed
}

/// What a terminal looks like in the chosen colors and size: a prompt, a listing and the
/// sixteen colors.
fn preview(ui: &mut Ui<'_>, page: &mut Rect, settings: &Settings) {
    let palette = annulus_term::themes::theme(&settings.terminal_theme)
        .unwrap_or_else(annulus_term::Palette::annulus_dark);
    let rgb = |c: Rgb| Color::rgba(c.r, c.g, c.b, 255);
    ui.section_label(page.take_top(18.0), "Preview");
    page.take_top(space::S);
    let shape = page.take_top(150.0);
    page.take_top(space::XL);
    ui.gfx
        .shadow(shape, 12.0, 14.0, 4.0, ui.theme.shadow.alpha(0.6));
    ui.gfx.fill_rect(shape, 12.0, rgb(palette.background));
    ui.gfx.stroke_rect(shape, 12.0, 1.0, ui.theme.line);
    let font = Font::mono(settings.font_size.clamp(8.0, 20.0));
    let line_h = ui.gfx.measure("M", font).height;
    let mut y = shape.y + space::L;
    let x = shape.x + space::L;
    let lines: [&[(&str, Rgb)]; 3] = [
        &[
            ("ana@web-1", palette.ansi[2]),
            (":", palette.foreground),
            ("~/site", palette.ansi[4]),
            ("$ ls", palette.foreground),
        ],
        &[
            ("index.html  ", palette.foreground),
            ("assets/  ", palette.ansi[12]),
            ("deploy.sh", palette.ansi[10]),
        ],
        &[
            ("error: ", palette.ansi[1]),
            ("disk at 91 %", palette.ansi[3]),
        ],
    ];
    for line in lines {
        let mut at = x;
        for (text, color) in line {
            ui.gfx.draw_text(Point::new(at, y), text, font, rgb(*color));
            at += ui.gfx.measure(text, font).width;
        }
        y += line_h;
    }
    // The sixteen colors as swatches along the bottom.
    let swatch = 16.0;
    let mut at = x;
    let bottom = shape.b() - space::L - swatch;
    for color in palette.ansi {
        ui.gfx
            .fill_rect(Rect::new(at, bottom, swatch, swatch), 4.0, rgb(color));
        at += swatch + 4.0;
        if at + swatch > shape.r() - space::L {
            break;
        }
    }
}

fn security(ui: &mut Ui<'_>, page: &mut Rect, settings: &mut Settings) -> bool {
    let mut card = card(ui, page, "Vault", 1);
    let control = row(
        ui,
        &mut card,
        true,
        (
            "Lock the vault",
            "When you have been away from the keyboard. Ctrl+Alt+L locks it now.",
        ),
        260.0,
    );
    let items: Vec<String> = LOCK_MINUTES
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let at = LOCK_MINUTES
        .iter()
        .position(|(minutes, _)| *minutes == settings.auto_lock_minutes);
    if let Some(chosen) = ui.select(Id::new("settings.lock"), control, &items, at, "Custom") {
        settings.auto_lock_minutes = LOCK_MINUTES[chosen].0;
        return true;
    }
    false
}

fn sessions(ui: &mut Ui<'_>, page: &mut Rect, settings: &mut Settings) -> bool {
    let mut changed = false;
    let mut card = card(ui, page, "Tabs and logs", 2);
    let control = row(
        ui,
        &mut card,
        true,
        (
            "Reopen my tabs",
            "Local shells and saved hosts, at the next start.",
        ),
        60.0,
    );
    changed |= ui
        .switch(
            Id::new("settings.restore"),
            control,
            &mut settings.restore_tabs,
            "Reopen my tabs",
        )
        .clicked;
    let control = row(
        ui,
        &mut card,
        false,
        (
            "Save session logs",
            "Output can hold secrets; logs stay encrypted.",
        ),
        60.0,
    );
    changed |= ui
        .switch(
            Id::new("settings.logs"),
            control,
            &mut settings.record_logs,
            "Save session logs",
        )
        .clicked;
    changed
}

fn shells(ui: &mut Ui<'_>, page: &mut Rect, settings: &mut Settings, profiles: &[Profile]) -> bool {
    let mut card = card(ui, page, "New terminals", 1);
    let control = row(
        ui,
        &mut card,
        true,
        (
            "New terminals open",
            "Ctrl+Shift+L, and Local terminal on a new tab.",
        ),
        260.0,
    );
    let names: Vec<String> = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    let at = settings
        .default_profile
        .as_ref()
        .and_then(|name| names.iter().position(|known| known == name));
    if let Some(chosen) = ui.select(
        Id::new("settings.profile"),
        control,
        &names,
        at.or(Some(0)),
        "System shell",
    ) {
        settings.default_profile = Some(names[chosen].clone());
        return true;
    }
    false
}

/// Every shortcut, grouped, each with its keys as keycaps; the search keeps those whose name
/// or keys hold every word typed.
fn shortcuts_page(ui: &mut Ui<'_>, page: &mut Rect, search: &mut String) {
    let t = ui.theme;
    ui.text_field(
        Id::new("settings.shortcuts.search"),
        page.take_top(CONTROL),
        search,
        FieldStyle {
            placeholder: "Search shortcuts",
            icon: Some(icons::SEARCH),
            ..FieldStyle::default()
        },
    );
    page.take_top(space::L);
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    let found: Vec<&shortcuts::Shortcut> = shortcuts::ALL
        .iter()
        .filter(|shortcut| {
            let text = format!("{} {} {}", shortcut.action, shortcut.keys(), shortcut.group)
                .to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .collect();
    if found.is_empty() {
        ui.gfx.text(
            page.take_top(24.0),
            "No shortcut matches.",
            BODY,
            t.text2,
            Align::Start,
        );
        return;
    }
    let mut groups: Vec<&str> = Vec::new();
    for shortcut in &found {
        if !groups.contains(&shortcut.group) {
            groups.push(shortcut.group);
        }
    }
    for group in groups {
        let rows: Vec<&&shortcuts::Shortcut> = found
            .iter()
            .filter(|shortcut| shortcut.group == group)
            .collect();
        #[allow(clippy::cast_precision_loss)] // A dozen rows.
        let height = rows.len() as f32 * 40.0 + 2.0 * space::S;
        let mut card = pages::card(ui, page, group, height);
        for (index, shortcut) in rows.into_iter().enumerate() {
            if index > 0 {
                pages::divider(ui, card.y, card);
            }
            let line = card.take_top(40.0);
            let keys = shortcut.keys();
            let width = keys_width(&mut ui.gfx, &keys);
            let mut label = line;
            let caps = label.take_right(width);
            label.take_right(space::M);
            ui.gfx
                .text(label, shortcut.action, BODY, t.text, Align::Start);
            ui.keys(Point::new(caps.x, caps.center().y - KEYCAP_H / 2.0), &keys);
        }
    }
}

fn about(ui: &mut Ui<'_>, page: &mut Rect) {
    let t = ui.theme;
    let mut card = pages::card(ui, page, "Annulus", 3.0 * 40.0 + 2.0 * space::S);
    for (index, (title, value)) in [
        ("Version", env!("CARGO_PKG_VERSION")),
        ("License", "MIT or Apache 2.0, your choice"),
        ("Source", "Free and open source, with no telemetry"),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            pages::divider(ui, card.y, card);
        }
        let mut line = card.take_top(40.0);
        let label = line.take_left(160.0);
        ui.gfx.text(label, title, BODY, t.text2, Align::Start);
        ui.gfx.text(line, value, BODY_STRONG, t.text, Align::Start);
    }
    ui.gfx.text(
        page.take_top(20.0),
        "Annulus is an independent project, not affiliated with any other terminal app.",
        SMALL,
        t.text3,
        Align::Start,
    );
}
