//! Settings (UI-SPEC §6): the sections in their own navigation down the left, the chosen
//! one beside it with a header and its cards. Account & sync comes first when there is a
//! vault file; Shortcuts lists the app's shortcut table ([`super::shortcuts`]), searchable,
//! and a click on a shortcut's keys records others for it (F-SET-004).

use annulus_config::settings::{KeyBinding, Settings};
use annulus_config::tokens::{ThemeMode, space};
use annulus_session::profiles::Profile;
use annulus_term::alacritty_terminal::vte::ansi::Rgb;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Font, Point, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Typed, Ui};
use annulus_ui::motion::{Feel, Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, BODY_STRONG, CONTROL, FieldStyle, HEADING, KEYCAP_H, Kind, SMALL, TITLE,
    Tone, keys_width,
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
            Page::Security => "When the vault locks itself, and what else may use Annulus.",
            Page::Sessions => "What Annulus keeps of your sessions.",
            Page::Shells => "What a new local terminal runs.",
            Page::Shortcuts => "Every shortcut. Click one's keys to give it others.",
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
    /// The shortcut whose new keys are being recorded, by its id.
    recording: Option<&'static str>,
    /// Why the keys pressed last were not taken.
    refused: Option<String>,
    /// Refusals so far, so each one shakes the field anew.
    refusals: u32,
    /// The keys pressed this frame while recording, before any shortcut saw them.
    caught: Option<(Key, Modifiers)>,
    /// The entry of the window's scheme menu to show this frame: the window shows it until
    /// the menu closes ([`super::colors::previewed_theme`]).
    pub previewing: Option<usize>,
    /// Which entry of that menu the pointer rests on.
    scheme: SchemePreview,
}

/// How long the pointer rests on a scheme before the window takes its colors, in seconds.
const PREVIEW_REST: f64 = 0.18;

/// The entry of the window's scheme menu the pointer (or the keyboard) has rested on: the one
/// the window previews. Entries passed over on the way change nothing, so sweeping the list
/// never flashes the window through every scheme it crosses.
#[derive(Default)]
struct SchemePreview {
    /// The entry under the pointer, and since when.
    hot: Option<(usize, f64)>,
    /// The entry the window shows.
    shown: Option<usize>,
}

impl SchemePreview {
    /// Follows the menu at `now`: whether it is `open`, and its entry under the pointer.
    /// Returns the entry to show, and how long until the one under the pointer shows.
    fn follow(&mut self, open: bool, hot: Option<usize>, now: f64) -> (Option<usize>, Option<f64>) {
        if !open {
            *self = Self::default();
            return (None, None);
        }
        let Some(entry) = hot else {
            // Off the list (in its search): what shows stays.
            self.hot = None;
            return (self.shown, None);
        };
        let since = match self.hot {
            Some((known, since)) if known == entry => since,
            _ => now,
        };
        self.hot = Some((entry, since));
        let left = PREVIEW_REST - (now - since);
        if left > 0.0 {
            return (self.shown, Some(left));
        }
        self.shown = Some(entry);
        (self.shown, None)
    }
}

/// The scrollback choices (F-TERM-004), each with what a full tab 120 columns wide costs.
const SCROLLBACK: [(usize, &str); 4] = [
    (1_000, "1,000 lines (about 3 MB)"),
    (10_000, "10,000 lines (about 30 MB)"),
    (50_000, "50,000 lines (about 150 MB)"),
    (100_000, "100,000 lines (about 300 MB)"),
];

const LOCK_MINUTES: [(u32, &str); 8] = [
    (0, "Never"),
    (5, "After 5 minutes"),
    (15, "After 15 minutes"),
    (30, "After 30 minutes"),
    (60, "After 1 hour"),
    (120, "After 2 hours"),
    (240, "After 4 hours"),
    (480, "After 8 hours"),
];

/// Width of the sections' navigation.
const NAV_W: f32 = 212.0;
/// The place of a switch in a setting row: as wide as the switch, so it ends where the menus
/// and segmented controls above and below it end.
const SWITCH_W: f32 = 34.0;
/// Height of one setting row.
const ROW: f32 = 60.0;
/// The widest the section column gets.
const COLUMN_W: f32 = 780.0;
/// Height of one shortcut row.
const KEY_ROW: f32 = 40.0;
/// Width of the field that waits for new keys.
const RECORD_W: f32 = 172.0;

impl SettingsView {
    /// While keys are being recorded, takes every key press of this frame before any shortcut
    /// can act on it. Recording stops when Settings → Shortcuts is not `showing`.
    pub fn catch_keys(&mut self, ui: &mut Ui<'_>, showing: bool) {
        if self.recording.is_none() {
            return;
        }
        if !showing || self.page != Page::Shortcuts {
            self.stop_recording();
            return;
        }
        for typed in ui.take_typed() {
            if let Typed::Key(key, modifiers) = typed
                && self.caught.is_none()
            {
                self.caught = Some((key, modifiers));
            }
        }
    }

    /// Whether a shortcut's new keys are being recorded: every key goes to the recording then.
    #[must_use]
    pub fn recording(&self) -> bool {
        self.recording.is_some()
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.refused = None;
        self.caught = None;
    }

    /// Gives the shortcut being recorded the keys caught this frame, if they may be its;
    /// Escape stops recording. Returns whether the settings changed.
    fn take_recorded(&mut self, ui: &mut Ui<'_>, settings: &mut Settings) -> bool {
        let (Some(id), Some((key, modifiers))) = (self.recording, self.caught.take()) else {
            return false;
        };
        if (key, modifiers) == (Key::Escape, Modifiers::NONE) {
            self.stop_recording();
            return false;
        }
        if let Err(why) = shortcuts::check(id, key, modifiers) {
            self.refused = Some(why);
            self.refusals = self.refusals.wrapping_add(1);
            return false;
        }
        let Some(shortcut) = shortcuts::ALL.iter().find(|shortcut| shortcut.id == id) else {
            self.stop_recording();
            return false;
        };
        settings.keys.retain(|binding| binding.action != id);
        // Its own keys again need no entry.
        if shortcut.own() != Some((key, modifiers)) {
            settings.keys.push(KeyBinding {
                action: id.to_owned(),
                keys: shortcuts::chord_text(key, modifiers),
            });
        }
        shortcuts::choose(&settings.keys);
        ui.toast(
            format!("{} is now {}", shortcut.action, shortcut.keys()),
            Tone::Done,
        );
        self.stop_recording();
        true
    }
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
        // Room for the scroll bar only while there is one, so cards end with the header.
        let page_w = if self.content_h > body.h {
            body.w - 12.0
        } else {
            body.w
        };
        let mut page = Rect::new(body.x, body.y - offset, page_w, f32::MAX / 4.0);
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
            Page::Appearance => {
                changed = appearance(
                    ui,
                    &mut page,
                    settings,
                    (&mut self.previewing, &mut self.scheme),
                );
            }
            Page::Terminal => changed = terminal(ui, &mut page, settings),
            Page::Security => changed = security(ui, &mut page, settings),
            Page::Sessions => changed = sessions(ui, &mut page, settings),
            Page::Shells => changed = shells(ui, &mut page, settings, profiles),
            Page::Shortcuts => changed = self.shortcuts_page(ui, &mut page, settings),
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
            HEADING,
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
            let y = ui.spring_with(Id::new("settings.nav.pill"), target.y, Feel::Soft);
            let pill = Rect::new(target.x, y + 1.0, target.w, target.h - 2.0);
            ui.gfx.fill_rect(pill, 8.0, t.selected);
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
                ui.gfx.fill_rect(row, 8.0, t.hover.alpha(hover));
            }
            if response.focus_ring {
                ui.focus_ring(row, 8.0);
            }
            let mut inner = row.inset(space::M, 0.0);
            let ink = if selected { t.text } else { t.text2 };
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
    let head = rect.take_top(58.0);
    let heading = Rect::new(head.x, head.y, head.w, 34.0);
    ui.strict_text(heading, page.title(), TITLE, t.text, Align::Start);
    let line = Rect::new(head.x, head.y + 36.0, head.w, 18.0);
    ui.gfx
        .text(line, page.subtitle(), BODY, t.text2, Align::Start);
    rect.take_top(space::L);
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
    #[allow(clippy::cast_precision_loss)] // Two to four segments.
    let width = (rect.w - 4.0) / items.len() as f32;
    #[allow(clippy::cast_precision_loss)] // As above.
    let target = rect.x + 2.0 + selected as f32 * width;
    let x = ui.spring_with(Id::new((id, "pill")), target, Feel::Soft);
    let pill = Rect::new(x, rect.y + 2.0, width, rect.h - 4.0);
    ui.gfx.shadow(pill, 7.0, 6.0, 1.0, t.shadow.alpha(0.5));
    ui.gfx.fill_rect(pill, 7.0, t.raised);
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

fn appearance(
    ui: &mut Ui<'_>,
    page: &mut Rect,
    settings: &mut Settings,
    (previewing, scheme): (&mut Option<usize>, &mut SchemePreview),
) -> bool {
    let mut changed = false;
    let mut card = card(ui, page, "Look", 3);
    let control = row(
        ui,
        &mut card,
        true,
        ("Theme", "Light or dark for Annulus's own colors."),
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
            "Color scheme",
            "Recolors the whole window; Annulus is our own.",
        ),
        260.0,
    );
    let schemes = super::colors::app_schemes();
    let at = settings
        .app_scheme
        .as_ref()
        .map_or(Some(0), |name| schemes.iter().position(|s| s == name));
    let id = Id::new("settings.scheme");
    // The window takes the colors the pointer rests on, so each can be seen before it is
    // chosen; the window blends into them (`colors::ThemeFade`).
    let (shown, wait) = scheme.follow(ui.menu_open(id), ui.menu_hot(id), ui.time());
    *previewing = shown;
    if let Some(wait) = wait {
        ui.request_repaint_after(std::time::Duration::from_secs_f64(wait));
    }
    if let Some(chosen) = ui.select_with_swatches(
        id,
        control,
        schemes,
        super::colors::app_swatches(),
        at,
        super::colors::OWN_COLORS,
    ) {
        settings.app_scheme = (chosen > 0).then(|| schemes[chosen].clone());
        *previewing = None;
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
        SWITCH_W,
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
    let themes = super::colors::terminal_themes();
    let at = themes
        .iter()
        .position(|name| *name == settings.terminal_theme);
    let id = Id::new("settings.colors");
    // The preview below shows the colors under the pointer before they are chosen.
    let hovered = ui.menu_hot(id).and_then(|index| themes.get(index));
    let shown = hovered.unwrap_or(&settings.terminal_theme).clone();
    if let Some(chosen) = ui.select_with_swatches(
        id,
        control,
        themes,
        super::colors::terminal_swatches(),
        at,
        "Annulus Dark",
    ) {
        settings.terminal_theme.clone_from(&themes[chosen]);
        changed = true;
    }
    let how = format!(
        "{} and {} change it; {} resets it.",
        shortcuts::ZOOM_IN.keys(),
        shortcuts::ZOOM_OUT.keys(),
        shortcuts::ZOOM_RESET.keys()
    );
    let control = row(ui, &mut card, false, ("Text size", &how), 260.0);
    let sizes: Vec<String> = (11..=20).map(|size| format!("{size} pt")).collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 8 to 32.
    let at = (settings.font_size.round() as usize).checked_sub(11);
    if let Some(chosen) = ui.select(Id::new("settings.font"), control, &sizes, at, "14 pt") {
        #[allow(clippy::cast_precision_loss)] // Small sizes.
        let size = (chosen + 11) as f32;
        settings.font_size = size;
        changed = true;
    }
    preview(ui, page, &shown, settings.font_size);
    let mut history = self::card(ui, page, "History", 1);
    let control = row(
        ui,
        &mut history,
        true,
        ("Lines each tab keeps", "How far back each tab scrolls."),
        260.0,
    );
    let names: Vec<String> = SCROLLBACK
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let at = SCROLLBACK
        .iter()
        .position(|(lines, _)| *lines == settings.scrollback);
    if let Some(chosen) = ui.select(
        Id::new("settings.scrollback"),
        control,
        &names,
        at,
        "Custom",
    ) {
        settings.scrollback = SCROLLBACK[chosen].0;
        changed = true;
    }
    changed
}

/// What a terminal looks like in the colors called `theme` at `font_size`: a prompt, a
/// listing and the sixteen colors.
fn preview(ui: &mut Ui<'_>, page: &mut Rect, theme: &str, font_size: f32) {
    let palette = super::colors::terminal_palette(theme);
    let rgb = |c: Rgb| Color::rgba(c.r, c.g, c.b, 255);
    ui.section_label(page.take_top(18.0), "Preview");
    page.take_top(space::S);
    let shape = page.take_top(150.0);
    page.take_top(space::XL);
    ui.gfx
        .shadow(shape, 12.0, 14.0, 4.0, ui.theme.shadow.alpha(0.6));
    ui.gfx.fill_rect(shape, 12.0, rgb(palette.background));
    ui.gfx.stroke_rect(shape, 12.0, 1.0, ui.theme.line);
    let font = Font::mono(font_size.clamp(8.0, 20.0));
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
    let when = format!(
        "After this long idle. {} locks it now.",
        shortcuts::LOCK.keys()
    );
    let control = row(ui, &mut card, true, ("Lock the vault", &when), 260.0);
    let items: Vec<String> = LOCK_MINUTES
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let at = LOCK_MINUTES
        .iter()
        .position(|(minutes, _)| *minutes == settings.auto_lock_minutes);
    let mut changed = false;
    if let Some(chosen) = ui.select(Id::new("settings.lock"), control, &items, at, "Custom") {
        settings.auto_lock_minutes = LOCK_MINUTES[chosen].0;
        changed = true;
    }
    // Whether a tab closes when its shell ends is asked only once agents may close tabs:
    // until then nothing closes an agent's tab, not even `exit`.
    let closing = !settings.stop_agents_closing_tabs;
    let rows = if closing { 6 } else { 5 };
    let mut automation = self::card(ui, page, "Automation", rows);
    let control = row(
        ui,
        &mut automation,
        true,
        (
            "Allow the command line and AI agents",
            "Only while the vault is unlocked; locking shuts them out.",
        ),
        SWITCH_W,
    );
    changed |= ui
        .switch(
            Id::new("settings.automation"),
            control,
            &mut settings.automation,
            "Allow the command line and AI agents",
        )
        .clicked;
    // How agents' tabs behave (F-AUTO-006).
    let mut switches = vec![
        (
            "settings.agent.front",
            "Agents' tabs open in front",
            "Off, they open behind the tab you are in, with their AI mark.",
            &mut settings.agent_tabs_in_front,
        ),
        (
            "settings.agent.read",
            "Agents may read my tabs",
            "Off, agents read only the tabs they opened.",
            &mut settings.agents_read_my_tabs,
        ),
        (
            "settings.agent.keep",
            "Stop agents closing their tabs",
            "Their tabs stay for you to read, even after exit. Off, agents may close them.",
            &mut settings.stop_agents_closing_tabs,
        ),
    ];
    if closing {
        switches.push((
            "settings.agent.close",
            "Also close an agent's tab when its shell ends",
            "After exit or a lost connection. Off, it stays open with Start again.",
            &mut settings.close_ended_agent_tabs,
        ));
    }
    switches.push((
        "settings.agent.folded",
        "Agent panel starts folded",
        "New agent tabs show the slim rail, not the list.",
        &mut settings.agent_panel_folded,
    ));
    for (id, name, about, value) in switches {
        let control = row(ui, &mut automation, false, (name, about), SWITCH_W);
        changed |= ui.switch(Id::new(id), control, value, name).clicked;
    }
    changed
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
        SWITCH_W,
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
        SWITCH_W,
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
    let which = format!(
        "{}, and Local terminal on a new tab.",
        shortcuts::LOCAL.keys()
    );
    let control = row(ui, &mut card, true, ("New terminals open", &which), 260.0);
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

impl SettingsView {
    /// Every shortcut, grouped, each with its keys as keycaps; the search keeps those whose
    /// name or keys hold every word typed. Returns whether the settings changed.
    fn shortcuts_page(
        &mut self,
        ui: &mut Ui<'_>,
        page: &mut Rect,
        settings: &mut Settings,
    ) -> bool {
        const RESET_ALL: &str = "Reset all";
        let t = ui.theme;
        let mut changed = self.take_recorded(ui, settings);
        let mut bar = page.take_top(CONTROL);
        let reset = (!settings.keys.is_empty()).then(|| {
            let width = ui.gfx.measure(RESET_ALL, BODY_MEDIUM).width + 2.0 * space::L;
            let reset = bar.take_right(width);
            bar.take_right(space::M);
            reset
        });
        ui.text_field(
            Id::new("settings.shortcuts.search"),
            bar,
            &mut self.search,
            FieldStyle {
                placeholder: "Search shortcuts",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        let id = Id::new("settings.keys.reset_all");
        if let Some(reset) = reset
            && ui
                .button(id, reset, RESET_ALL, None, Kind::Secondary)
                .clicked
        {
            settings.keys.clear();
            shortcuts::choose(&settings.keys);
            self.stop_recording();
            ui.toast("Every shortcut has its own keys again", Tone::Done);
            changed = true;
        }
        page.take_top(space::L);
        let words: Vec<String> = self
            .search
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
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
            return changed;
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
            let heights: Vec<f32> = rows.iter().map(|shortcut| self.row_h(shortcut)).collect();
            let height = heights.iter().sum::<f32>() + 2.0 * space::S;
            let mut card = pages::card(ui, page, group, height);
            for (index, (shortcut, height)) in rows.into_iter().zip(heights).enumerate() {
                if index > 0 {
                    pages::divider(ui, card.y, card);
                }
                let line = card.take_top(height);
                changed |= self.key_row(ui, line, shortcut, settings);
            }
        }
        changed
    }

    /// A shortcut row's height: taller while it says why keys were refused.
    fn row_h(&self, shortcut: &shortcuts::Shortcut) -> f32 {
        if self.recording == Some(shortcut.id) && self.refused.is_some() {
            KEY_ROW + 18.0
        } else {
            KEY_ROW
        }
    }

    /// One shortcut: what it does and its keys, which a click records anew; Reset beside
    /// keys of the user's choosing. Returns whether the settings changed.
    fn key_row(
        &mut self,
        ui: &mut Ui<'_>,
        line: Rect,
        shortcut: &shortcuts::Shortcut,
        settings: &mut Settings,
    ) -> bool {
        let t = ui.theme;
        let mut head = Rect::new(line.x, line.y, line.w, KEY_ROW);
        let keys = shortcut.keys();
        let caps_w = keys_width(&mut ui.gfx, &keys) + 12.0;
        if shortcut.own().is_none() {
            // One a widget or the window handles: listed, not changed here.
            let caps = head.take_right(caps_w);
            head.take_right(space::M);
            ui.gfx
                .text(head, shortcut.action, BODY, t.text, Align::Start);
            ui.keys(
                Point::new(caps.x + 6.0, caps.center().y - KEYCAP_H / 2.0),
                &keys,
            );
            return false;
        }
        let recording = self.recording == Some(shortcut.id);
        let id = Id::new(("settings.keys", shortcut.id));
        let width = if recording { RECORD_W } else { caps_w };
        let target = head.take_right(width).centered(width, KEYCAP_H + 10.0);
        head.take_right(space::M);
        // What sits left of the keys comes first, so Tab runs left to right.
        let mut changed = false;
        if recording {
            let hint = "Esc cancels";
            let w = ui.gfx.measure(hint, SMALL).width;
            ui.gfx
                .text(head.take_right(w), hint, SMALL, t.text3, Align::End);
            head.take_right(space::M);
        } else if shortcut.changed() {
            changed = reset_keys(ui, &mut head, id, shortcut, settings);
        }
        let name = if recording {
            format!("Recording keys for {}", shortcut.action)
        } else {
            format!("Change the keys of {}", shortcut.action)
        };
        let response = ui.zone(id, target, Role::Button, &name);
        // A screen reader hears why keys were refused, not only the sighted.
        if recording && let Some(why) = &self.refused {
            ui.set_value(id, why);
        }
        if response.clicked {
            self.stop_recording();
            if !recording {
                self.recording = Some(shortcut.id);
            }
        }
        if response.hovered {
            ui.set_cursor(Cursor::Pointer);
        }
        let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        if recording {
            // The field shakes its head at keys it cannot take.
            let refused = self.refused.is_some();
            let shake = ui.entrance(id.with(("shake", self.refusals)), Motion::SLOW);
            let nudge = if refused {
                5.0 * (1.0 - shake) * (shake * 18.0).sin()
            } else {
                0.0
            };
            let shown = ui.entrance(id.with("recording"), Motion::FAST);
            let field = target.offset(nudge, 0.0);
            let ink = if refused { t.error } else { t.accent };
            ui.gfx.fill_rect(field, 8.0, ink.alpha(0.12 * shown));
            ui.gfx
                .stroke_rect(field, 8.0, 1.0, ink.alpha(0.35 + 0.45 * shown));
            ui.strict_text(field, "Press the new keys", BODY_MEDIUM, ink, Align::Center);
        } else {
            ui.gfx.fill_rect(target, 8.0, t.hover.alpha(hover));
            ui.gfx
                .stroke_rect(target, 8.0, 1.0, t.line_strong.alpha(hover));
            ui.keys(
                Point::new(target.x + 6.0, target.center().y - KEYCAP_H / 2.0),
                &keys,
            );
        }
        if response.focus_ring {
            ui.focus_ring(target, 8.0);
        }
        ui.gfx
            .text(head, shortcut.action, BODY, t.text, Align::Start);
        if recording && let Some(why) = &self.refused {
            let shown = ui.entrance(id.with(("why", why.as_str())), Motion::FAST);
            let below = Rect::new(line.x, line.y + KEY_ROW - 10.0, line.w, 18.0);
            ui.gfx
                .text(below, why, SMALL, t.error.alpha(shown), Align::Start);
        }
        changed
    }
}

/// Reset, at the right of `head`, for a shortcut with keys of the user's choosing: a click
/// gives it its own again. Returns whether it did.
fn reset_keys(
    ui: &mut Ui<'_>,
    head: &mut Rect,
    id: Id,
    shortcut: &shortcuts::Shortcut,
    settings: &mut Settings,
) -> bool {
    let t = ui.theme;
    let own = shortcut
        .own()
        .map(|(key, modifiers)| shortcuts::chord_text(key, modifiers))
        .unwrap_or_default();
    let w = ui.gfx.measure("Reset", SMALL).width + 14.0;
    let reset = head.take_right(w).centered(w, 24.0);
    head.take_right(space::S);
    let label = format!("Reset {} to {own}", shortcut.action);
    let response = ui.zone(id.with("reset"), reset, Role::Button, &label);
    let lit = ui.ease(
        id.with("reset.hover"),
        if response.hovered { 1.0 } else { 0.0 },
    );
    ui.gfx.fill_rect(reset, 6.0, t.hover.alpha(lit));
    ui.strict_text(
        reset,
        "Reset",
        SMALL,
        t.text2.mix(t.text, lit),
        Align::Center,
    );
    if response.focus_ring {
        ui.focus_ring(reset, 6.0);
    }
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    if !response.clicked {
        return false;
    }
    // Another shortcut may have taken its own keys meanwhile.
    if let Some((key, modifiers)) = shortcut.own()
        && let Err(why) = shortcuts::check(shortcut.id, key, modifiers)
    {
        ui.toast(
            format!("Can't reset {}. {why}", shortcut.action),
            Tone::Problem,
        );
        return false;
    }
    settings
        .keys
        .retain(|binding| binding.action != shortcut.id);
    shortcuts::choose(&settings.keys);
    ui.toast(format!("{} is {own} again", shortcut.action), Tone::Done);
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheme_shows_once_the_pointer_rests_on_it_not_while_it_passes() {
        let mut preview = SchemePreview::default();
        assert_eq!(
            preview.follow(true, Some(3), 0.0),
            (None, Some(PREVIEW_REST))
        );
        assert_eq!(preview.follow(true, Some(3), 0.2), (Some(3), None));
        // Sweeping past entries keeps what shows.
        for (step, entry) in (4..12).enumerate() {
            #[allow(clippy::cast_precision_loss)] // A few steps.
            let now = 0.3 + step as f64 * 0.03;
            assert_eq!(preview.follow(true, Some(entry), now).0, Some(3));
        }
        // The last entry passed, 11, came at 0.51.
        assert_eq!(preview.follow(true, Some(11), 0.6).0, Some(3));
        assert_eq!(
            preview.follow(true, Some(11), 0.75).0,
            Some(11),
            "rested on"
        );
        // In the search, what shows stays; closed, the window goes back.
        assert_eq!(preview.follow(true, None, 0.8), (Some(11), None));
        assert_eq!(preview.follow(false, None, 0.85), (None, None));
        assert_eq!(preview.follow(true, Some(5), 0.9).0, None);
    }
}
