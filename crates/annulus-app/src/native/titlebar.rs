//! The title bar is the tab strip (UI-SPEC §1): our mark, the tabs, new-tab buttons, the sync
//! badge and the window buttons. Empty space drags the window; the window host finds it, and
//! the buttons, by their zone ids ([`DRAG`], [`MINIMIZE`], [`MAXIMIZE`], [`CLOSE`]).

use annulus_config::settings::OpenTab;
use annulus_config::tokens::space;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Point, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Role, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{BODY, BODY_MEDIUM};

use super::{FIXED_TABS, NativeApp, SyncBadge, TabKind, WindowAction};
use crate::terminal_view::TabStatus;

/// The zone of the title bar's empty space: dragging it moves the window.
pub const DRAG: &str = "titlebar.drag";
/// The minimize button's zone.
pub const MINIMIZE: &str = "caption.minimize";
/// The maximize / restore button's zone (Windows shows Snap Layouts over it).
pub const MAXIMIZE: &str = "caption.maximize";
/// The close button's zone.
pub const CLOSE: &str = "caption.close";

/// Width of one window button.
const CAPTION_W: f32 = 46.0;
/// Tab height; tabs sit centered in the bar.
const TAB_H: f32 = 30.0;

/// Draws the title bar into `bar`; returns what the window must do.
#[allow(clippy::too_many_lines)] // Left to right: window buttons, mark, tabs, +, badge.
pub fn draw(app: &mut NativeApp, ui: &mut Ui<'_>, bar: Rect) -> Option<WindowAction> {
    let drag = ui.zone(Id::new(DRAG), bar, Role::Backdrop, "Annulus");
    let mut action = drag.double_clicked.then_some(WindowAction::ToggleMaximize);
    let mut row = bar;

    let close = row.take_right(CAPTION_W);
    let maximize = row.take_right(CAPTION_W);
    let minimize = row.take_right(CAPTION_W);
    for (id, rect, glyph, name, act) in [
        (
            MINIMIZE,
            minimize,
            Glyph::Minimize,
            "Minimize",
            WindowAction::Minimize,
        ),
        (
            MAXIMIZE,
            maximize,
            if app.maximized {
                Glyph::Restore
            } else {
                Glyph::Maximize
            },
            if app.maximized { "Restore" } else { "Maximize" },
            WindowAction::ToggleMaximize,
        ),
        (CLOSE, close, Glyph::Close, "Close", WindowAction::Close),
    ] {
        if caption_button(ui, Id::new(id), rect, glyph, name) {
            action = Some(act);
        }
    }

    row.take_right(space::S);

    row.take_left(space::M + 2.0);
    let mark = row.take_left(20.0).centered(20.0, 20.0);
    paint_mark(ui, mark);
    row.take_left(space::M);

    let mut close_tab = None;
    // While the vault is locked, open sessions stay out of sight and out of reach.
    let locked = app.vault.is_none();
    let shown = if locked {
        1.min(app.tabs.len())
    } else {
        app.tabs.len()
    };
    // Where each tab belongs, left to right. Tabs glide to their places (a tab closed, moved
    // or renamed), but take clicks in them, so what is clicked never moves under the pointer.
    let mut layout = slots_for(app, ui, row, shown, 0.0);
    // More tabs than the strip holds: the ones past its end wait in the all-tabs menu, and
    // the tab in front is always on the strip, in its last place.
    let overflow = layout.0.len() < shown;
    if overflow {
        layout = slots_for(app, ui, row, shown, MORE_W);
        let last = layout.0.len().saturating_sub(1);
        if app.active > last && last >= FIXED_TABS {
            app.move_tab(app.active, last);
        }
    }
    let (slots, x) = layout;
    if slots.len() > FIXED_TABS {
        // The fixed tabs, then a hairline, then the sessions.
        let line = Rect::new(
            slots[FIXED_TABS].x - DIVIDER / 2.0 - 1.0,
            row.center().y - 9.0,
            1.0,
            18.0,
        );
        ui.gfx.fill_rect(line, 0.0, ui.theme.line_strong);
    }
    let dragged = drag_tab(app, ui, &slots);
    // A context menu's "close the others": the tab kept, and whether only those to its right go.
    let mut close_others = None;
    for (index, slot) in slots.iter().enumerate() {
        if dragged.is_some_and(|(at, _)| at == index) {
            continue;
        }
        let place = Id::new(("tab", app.tabs[index].uid)).with("x");
        let at = ui.spring(place, slot.x);
        match tab(ui, *slot, at, app, index) {
            TabEvent::Select => app.active = index,
            TabEvent::Close | TabEvent::Menu(0) => close_tab = Some(index),
            TabEvent::Menu(1) => close_others = Some((app.tabs[index].uid, true)),
            TabEvent::Menu(_) => close_others = Some((app.tabs[index].uid, false)),
            TabEvent::Sync => {
                app.active = 0;
                app.section = super::Section::Settings;
                app.settings_view.page = super::settings::Page::Account;
            }
            TabEvent::None => {}
        }
    }
    if let Some((keep, right_only)) = close_others {
        app.close_others(keep, right_only);
    }
    // The dragged tab last, over its neighbours.
    if let Some((index, at)) = dragged
        && let TabEvent::Close = tab(ui, slots[index], at, app, index)
    {
        close_tab = Some(index);
    }
    if !locked {
        let slot = Rect::new(x + space::XS, row.center().y - 15.0, 30.0, 30.0);
        // It glides left as a tab closes; when one opens it is at once out of the new tab's
        // way, which grows where it was.
        let place = Id::new("tab.new").with("x");
        let mut at = ui.spring(place, slot.x);
        if at < slot.x - 0.5 {
            ui.spring_set(place, slot.x);
            at = slot.x;
        }
        let id = Id::new("tab.new");
        let response = ui.zone(id, slot, Role::Button, "New tab");
        let tip = format!("New tab ({})", super::shortcuts::NEW_TAB.keys());
        ui.tooltip(id, slot, &tip);
        plus_button(ui, Rect::new(at, slot.y, slot.w, slot.h), &response);
        if response.clicked {
            app.open_new_tab();
        }
        if overflow {
            let more = Rect::new(slot.r() + 2.0, slot.y, 30.0, 30.0);
            if let Some(index) = all_tabs(app, ui, more, slots.len()) {
                app.active = index;
            }
        }
    }
    if let Some(index) = close_tab {
        app.close_tab(index);
    }
    action
}

/// Space between the fixed tabs and the sessions, where the hairline goes.
const DIVIDER: f32 = 13.0;
/// Room kept for the + button after the tabs.
const PLUS_W: f32 = 36.0;
/// Room kept, besides, for the all-tabs button while some tabs do not fit.
const MORE_W: f32 = 32.0;
/// The narrowest a session tab gets when many share the strip.
const MIN_TAB: f32 = 96.0;

/// The places of the first `shown` tabs of `app` in `row`, with `reserve` kept at the end
/// besides the + button: as many as fit, and the x where the + button goes after them.
fn slots_for(
    app: &NativeApp,
    ui: &mut Ui<'_>,
    row: Rect,
    shown: usize,
    reserve: f32,
) -> (Vec<Rect>, f32) {
    let end = row.r() - PLUS_W - space::S - reserve;
    let mut slots: Vec<Rect> = Vec::with_capacity(shown);
    // The Vault tab holds the sync mark too, while the vault syncs with a server.
    let syncs = app.sync_badge() != SyncBadge::Local;
    let wanted: Vec<f32> = (0..shown)
        .map(|index| {
            let width = tab_width(ui, &app.tabs[index]);
            if index == 0 && syncs {
                width + CLOUD_W
            } else {
                width
            }
        })
        .collect();
    // Too many sessions for the strip: they share it evenly, down to a narrowest width,
    // rather than the last one shrinking to nothing.
    let fixed: f32 = wanted
        .iter()
        .take(FIXED_TABS)
        .map(|width| width + 2.0)
        .sum();
    let room = end - row.x - fixed - DIVIDER;
    let sessions = shown.saturating_sub(FIXED_TABS);
    let asked: f32 = wanted
        .iter()
        .skip(FIXED_TABS)
        .map(|width| width + 2.0)
        .sum();
    #[allow(clippy::cast_precision_loss)] // A few dozen tabs.
    let share = (asked > room && sessions > 0).then(|| (room / sessions as f32 - 2.0).max(MIN_TAB));
    let mut x = row.x;
    for (index, mut width) in wanted.into_iter().enumerate() {
        if index == FIXED_TABS {
            x += DIVIDER;
        }
        if index >= FIXED_TABS
            && let Some(share) = share
        {
            width = width.min(share);
        }
        if x + width > end + 0.5 {
            break;
        }
        slots.push(Rect::new(x, row.center().y - TAB_H / 2.0, width, TAB_H));
        x += width + 2.0;
    }
    (slots, x)
}

/// The all-tabs button in `rect`, shown while some tabs do not fit on the strip (the first
/// `on_strip` do): a menu of every tab, the one in front checked, with a search when it is
/// long. A dot on it says a tab off the strip wants attention. Returns the tab picked.
fn all_tabs(app: &NativeApp, ui: &mut Ui<'_>, rect: Rect, on_strip: usize) -> Option<usize> {
    let t = ui.theme;
    let id = Id::new("tab.all");
    let response = ui.zone(id, rect, Role::Button, "All tabs");
    ui.tooltip(id, rect, "All tabs");
    let hover = ui.ease(
        id.with("hover"),
        if response.hovered || ui.menu_open(id) {
            1.0
        } else {
            0.0
        },
    );
    ui.gfx.fill_rect(rect, 8.0, t.hover.alpha(hover));
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let glyph = rect.centered(16.0, 16.0);
    ui.gfx.icon(icons::MORE, glyph, t.text2.mix(t.text, hover));
    // The most urgent status among the tabs out of sight.
    let waiting = app.tabs[on_strip.min(app.tabs.len())..]
        .iter()
        .filter_map(|tab| tab.status)
        .max_by_key(|status| match status {
            TabStatus::Unread => 0,
            TabStatus::Input => 1,
            TabStatus::Error => 2,
        });
    if let Some(status) = waiting {
        let color = match status {
            TabStatus::Unread => t.accent,
            TabStatus::Input => t.warn,
            TabStatus::Error => t.error,
        };
        let at = Point::new(glyph.r(), glyph.y);
        ui.gfx.fill_circle(at, 4.5, t.chrome);
        ui.gfx.fill_circle(at, 3.0, color);
    }
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    let items: Vec<String> = app.tabs.iter().map(|tab| tab.title.clone()).collect();
    let width = items
        .iter()
        .map(|item| ui.gfx.measure(item, BODY).width + 2.0 * space::L + 24.0)
        .fold(240.0, f32::max);
    ui.menu_for(id, &response, rect, &items, Some(app.active), width)
}

/// The + button: a quiet square that lights on hover, its plus turning a little as it does.
fn plus_button(ui: &mut Ui<'_>, rect: Rect, response: &annulus_ui::kit::Response) {
    let t = ui.theme;
    let id = Id::new("tab.new");
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let press = ui.ease(id.with("press"), if response.pressed { 1.0 } else { 0.0 });
    ui.gfx.fill_rect(rect, 8.0, t.hover.alpha(hover));
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let size = 14.0 - 2.0 * press;
    ui.gfx.icon(
        icons::PLUS,
        rect.centered(size, size),
        t.text2.mix(t.text, hover),
    );
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
}

/// A session tab dragged along the strip follows the pointer, and passing the middle of a
/// neighbour swaps the two, the neighbour gliding into the gap (F-APP-001). Fixed tabs never
/// move. Returns the tab being dragged and where to draw it.
fn drag_tab(app: &mut NativeApp, ui: &mut Ui<'_>, slots: &[Rect]) -> Option<(usize, f32)> {
    let id_of = |app: &NativeApp, index: usize| Id::new(("tab", app.tabs[index].uid));
    for index in FIXED_TABS..slots.len() {
        if ui.drag_released(id_of(app, index)).is_some() {
            app.tab_grab = None;
        }
    }
    let index =
        (FIXED_TABS..slots.len()).find(|index| ui.drag_moved(id_of(app, *index)).is_some())?;
    let uid = app.tabs[index].uid;
    let pointer = ui.drag_moved(id_of(app, index))?;
    let grab = match app.tab_grab {
        Some((held, grab)) if held == uid => grab,
        _ => {
            let held = ui.pressed_at().unwrap_or(pointer);
            let grab = held.x - slots[index].x;
            app.tab_grab = Some((uid, grab));
            app.active = index;
            grab
        }
    };
    let first = slots[FIXED_TABS].x;
    let last = slots.last().map_or(first, |slot| slot.r() - slots[index].w);
    let x = (pointer.x - grab).clamp(first, last.max(first));
    // Let go, it glides from where it was dropped into its place.
    ui.spring_set(Id::new(("tab", uid)).with("x"), x);
    // Where the pointer has it, past the strip's ends too: drawn inside the strip, it can
    // still pass the middle of the last tab.
    let center = pointer.x - grab + slots[index].w / 2.0;
    let to = if index > FIXED_TABS && center < slots[index - 1].center().x {
        index - 1
    } else if index + 1 < slots.len() && center > slots[index + 1].center().x {
        index + 1
    } else {
        index
    };
    app.move_tab(index, to);
    Some((to, x))
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A window button: a thin glyph, a soft fill on hover, red for close.
fn caption_button(ui: &mut Ui<'_>, id: Id, rect: Rect, glyph: Glyph, name: &str) -> bool {
    let response = ui.zone(id, rect, Role::WindowButton, name);
    ui.tooltip(id, rect, name);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let close = matches!(glyph, Glyph::Close);
    let red = Color::rgba(0xC4, 0x2B, 0x1C, 255);
    let fill = if close {
        red.alpha(hover)
    } else {
        t.hover.alpha(hover)
    };
    ui.gfx.fill_rect(rect, 0.0, fill);
    let ink = if close {
        t.text2.mix(Color::WHITE, hover)
    } else {
        t.text2.mix(t.text, hover)
    };
    let c = rect.center();
    let s = 5.0;
    match glyph {
        Glyph::Minimize => ui.gfx.polyline(
            &[Point::new(c.x - s, c.y), Point::new(c.x + s, c.y)],
            false,
            1.0,
            ink,
        ),
        Glyph::Maximize => {
            ui.gfx
                .stroke_rect(Rect::new(c.x - s, c.y - s, 2.0 * s, 2.0 * s), 1.5, 1.0, ink);
        }
        Glyph::Restore => {
            ui.gfx.stroke_rect(
                Rect::new(c.x - s, c.y - s + 2.0, 2.0 * s - 2.0, 2.0 * s - 2.0),
                1.5,
                1.0,
                ink,
            );
            ui.gfx.polyline(
                &[
                    Point::new(c.x - s + 2.0, c.y - s),
                    Point::new(c.x + s, c.y - s),
                    Point::new(c.x + s, c.y + s - 2.0),
                ],
                false,
                1.0,
                ink,
            );
        }
        Glyph::Close => {
            ui.gfx.polyline(
                &[Point::new(c.x - s, c.y - s), Point::new(c.x + s, c.y + s)],
                false,
                1.0,
                ink,
            );
            ui.gfx.polyline(
                &[Point::new(c.x + s, c.y - s), Point::new(c.x - s, c.y + s)],
                false,
                1.0,
                ink,
            );
        }
    }
    if response.focus_ring {
        ui.focus_ring(rect.inset(4.0, 4.0), 4.0);
    }
    response.clicked
}

/// Our mark: a teal tile with a prompt. Ours alone (ADR-0010).
fn paint_mark(ui: &mut Ui<'_>, rect: Rect) {
    let t = ui.theme;
    ui.gfx.fill_rect(rect, 6.0, t.accent);
    let ink = t.on_accent;
    let (x, y) = (rect.x, rect.y);
    ui.gfx.polyline(
        &[
            Point::new(x + 5.5, y + 6.5),
            Point::new(x + 9.0, y + 10.0),
            Point::new(x + 5.5, y + 13.5),
        ],
        false,
        1.8,
        ink,
    );
    ui.gfx.polyline(
        &[
            Point::new(x + 10.5, y + 14.0),
            Point::new(x + 14.5, y + 14.0),
        ],
        false,
        1.8,
        ink,
    );
}

/// The width a tab wants: fixed tabs fit their name, the others have a close button too and
/// stay between a narrowest and a widest.
fn tab_width(ui: &mut Ui<'_>, tab: &super::Tab) -> f32 {
    let (title, kind) = (tab.title.as_str(), tab.kind);
    // Room for the AI mark on an agent's tab.
    let mark = if tab.agent.is_some() { 30.0 } else { 0.0 };
    label_width(ui, title, kind) + mark
}

fn label_width(ui: &mut Ui<'_>, title: &str, kind: TabKind) -> f32 {
    let text = ui.gfx.measure(title, BODY_MEDIUM).width;
    match kind {
        TabKind::Vault | TabKind::Files => (text + 16.0 + 8.0 + 2.0 * space::M).ceil(),
        TabKind::Terminal | TabKind::Sftp | TabKind::NewTab => {
            (text + 16.0 + 8.0 + 18.0 + 6.0 + 2.0 * space::M).clamp(132.0, 220.0)
        }
    }
}

enum TabEvent {
    None,
    Select,
    Close,
    /// An item of the tab's context menu, [`TAB_MENU`].
    Menu(usize),
    /// The Vault tab's sync mark: the account in Settings.
    Sync,
}

/// A session tab's context menu (right click, or Shift+F10).
const TAB_MENU: [&str; 3] = ["Close tab", "Close tabs to the right", "Close other tabs"];

/// What a tab's icon is: the host's OS mark for a session with a saved host whose OS is known,
/// else one for what it holds.
fn tab_icon(app: &NativeApp, tab: &super::Tab) -> Result<&'static super::os::Os, IconId> {
    let host = match (&tab.origin, &app.vault) {
        (Some(OpenTab::Host(id)), Some(vault)) => vault.hosts().iter().find(|host| host.id == *id),
        _ => None,
    };
    if let Some(os) = host.and_then(super::os::of) {
        return Ok(os);
    }
    Err(match tab.kind {
        TabKind::Vault => icons::GRID_VIEW,
        TabKind::Files | TabKind::Sftp => icons::FOLDER,
        TabKind::Terminal => icons::TERMINAL,
        TabKind::NewTab => icons::PLUS,
    })
}

/// Draws tab `index`, taking clicks in `slot` and drawn at `x` (it glides there).
#[allow(clippy::too_many_lines)] // One tab left to right: pill, icon, dot, label, close, menu.
fn tab(ui: &mut Ui<'_>, slot: Rect, x: f32, app: &NativeApp, index: usize) -> TabEvent {
    let tab = &app.tabs[index];
    let id = Id::new(("tab", tab.uid));
    let active = app.active == index;
    // A screen reader hears the dot too: "web-1, New output", and whose tab it is.
    let mut name = match tab.status {
        Some(status) => format!("{}, {}", tab.title, status.describe()),
        None => tab.title.clone(),
    };
    if tab.agent.is_some() {
        name.push_str(", an AI agent's tab");
    }
    let response = ui.zone(id, slot, Role::Tab, &name);
    let t = ui.theme;
    // Still hovered on its close button.
    let hovered = response.hovered || ui.hover_within(slot);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let lit = ui.animate(
        id.with("active"),
        if active { 1.0 } else { 0.0 },
        Motion::BASE,
    );
    let dragged = ui.drag_moved(id).is_some();
    let rect = Rect::new(x, slot.y, slot.w, slot.h);
    // A new tab grows into place.
    let open = ui.entrance(id.with("open"), Motion::POP);
    ui.push_layer(Layer::appear(open, rect.center(), 0.7, 0.0));
    if dragged {
        ui.gfx.shadow(rect, 9.0, 16.0, 5.0, t.shadow.alpha(0.6));
    }
    // Every tab is a soft pill; the one in front is the surface it opens onto.
    let idle = t.hover.alpha(0.35 + 0.45 * hover);
    let fill = idle.mix(t.surface, lit);
    ui.gfx.fill_rect(rect, 9.0, fill);
    if lit > 0.0 {
        ui.gfx
            .stroke_rect(rect, 9.0, 1.0, t.line_strong.alpha(lit * 0.8));
        // Light from above along its top edge.
        let top = Rect::new(rect.x + 8.0, rect.y + 0.5, rect.w - 16.0, 1.0);
        ui.gfx.fill_rect(
            top,
            0.5,
            Color::WHITE.alpha(lit * if t.dark { 0.06 } else { 0.5 }),
        );
    }
    if response.focus_ring {
        ui.focus_ring(rect, 9.0);
    }
    // Squeezed narrower than it wants, a tab trims its padding and keeps room for its close
    // button only while that shows (in front or hovered), so its name keeps a few letters.
    let squeezed = slot.w + 0.5 < tab_width(ui, tab);
    let mut inner = rect.inset(if squeezed { space::S } else { space::M }, 0.0);
    let ink = t.text2.mix(t.text, hover.max(lit));
    let icon = inner.take_left(16.0).centered(16.0, 16.0);
    match tab_icon(app, tab) {
        Ok(os) => super::os::tile(ui, icon, &tab.title, Some(os)),
        Err(glyph) => ui.gfx.icon(glyph, icon, ink.mix(t.accent, lit)),
    }
    if let Some(status) = tab.status {
        // A dot on the icon's corner, ringed in the tab's own color.
        let color = match status {
            TabStatus::Unread => t.accent,
            TabStatus::Input => t.warn,
            TabStatus::Error => t.error,
        };
        let at = Point::new(icon.r() - 1.0, icon.y + 1.0);
        let under = t.chrome.mix(fill, f32::from(fill.a) / 255.0);
        ui.gfx.fill_circle(at, 4.5, under);
        ui.gfx.fill_circle(at, 3.0, color);
    }
    inner.take_left(8.0);
    // An agent's tab says so before its name (ADR-0020 §6).
    if tab.agent.is_some() {
        let chip = ui.chip(Point::new(inner.x, inner.center().y - 10.0), "AI", t.accent);
        inner.take_left(chip + 6.0);
    }
    let mut event = if response.clicked {
        TabEvent::Select
    } else {
        TabEvent::None
    };
    if tab.kind == TabKind::Vault {
        let sync = app.sync_badge();
        if sync != SyncBadge::Local {
            let mark = inner.take_right(CLOUD_W - 4.0).centered(22.0, 22.0);
            if sync_mark(ui, mark, &sync, ink) {
                event = TabEvent::Sync;
            }
        }
    }
    if !tab.fixed() {
        let shown = lit.max(hover);
        let close = (shown > 0.0 || !squeezed).then(|| {
            let close = inner.take_right(18.0).centered(18.0, 18.0);
            inner.take_right(6.0);
            close
        });
        if let Some(close) = close.filter(|_| shown > 0.0) {
            let close_id = id.with("close");
            // In the slot, where it stays while the tab glides.
            let target = Rect::new(close.x - x + slot.x, close.y, close.w, close.h);
            let button = ui.zone(close_id, target, Role::Button, "Close tab");
            let tip = format!("Close tab ({})", super::shortcuts::CLOSE.keys());
            ui.tooltip(close_id, target, &tip);
            let over = ui.ease(
                close_id.with("hover"),
                if button.hovered { 1.0 } else { 0.0 },
            );
            ui.gfx.fill_rect(close, 5.0, t.hover.alpha(over));
            ui.gfx.icon(
                icons::CLOSE,
                close.centered(11.0, 11.0),
                t.text2.mix(t.text, over).alpha(shown),
            );
            if button.clicked {
                event = TabEvent::Close;
            }
        }
        if response.middle_clicked {
            event = TabEvent::Close;
        }
        let menu: Vec<String> = TAB_MENU.iter().map(|item| (*item).to_owned()).collect();
        if let Some(choice) = ui.context_menu(id, &response, &menu) {
            event = TabEvent::Menu(choice);
        }
    }
    ui.gfx
        .text(inner, &tab.title, BODY_MEDIUM, ink, Align::Start);
    ui.pop_layer();
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    event
}

/// Room the sync mark takes in the Vault tab.
const CLOUD_W: f32 = 26.0;

/// What the sync mark says about `sync`: its name and tooltip.
fn sync_words(sync: &SyncBadge) -> (&'static str, String) {
    match sync {
        SyncBadge::Local => (
            "On this device only",
            "The vault is on this device only".into(),
        ),
        SyncBadge::Synced => ("Synced", "Synced with your server: up to date".into()),
        SyncBadge::Pending(count) => (
            "Syncing",
            format!(
                "Syncing: {count} change{} to send",
                if *count == 1 { "" } else { "s" }
            ),
        ),
        SyncBadge::Failed => (
            "Sync failed",
            "Sync failed: it tries again by itself".into(),
        ),
    }
}

/// The vault's one sync mark, in its tab: a quiet cloud while all is synced, a small dot on
/// it that breathes while changes go out, a red one when a round failed. Its tooltip says
/// which; clicking it opens the account in Settings. Returns whether it was clicked.
fn sync_mark(ui: &mut Ui<'_>, rect: Rect, sync: &SyncBadge, ink: Color) -> bool {
    let t = ui.theme;
    let id = Id::new("titlebar.sync");
    let (name, tip) = sync_words(sync);
    let response = ui.zone(id, rect, Role::Button, name);
    ui.tooltip(id, rect, &tip);
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    ui.gfx.fill_rect(rect, 6.0, t.hover.alpha(hover));
    if response.focus_ring {
        ui.focus_ring(rect, 6.0);
    }
    let cloud = rect.centered(16.0, 16.0);
    let color = match sync {
        SyncBadge::Failed => t.error,
        _ => ink.mix(t.text, hover),
    };
    ui.gfx.icon(icons::CLOUD, cloud, color);
    let dot = Point::new(cloud.r() - 1.5, cloud.b() - 3.5);
    match sync {
        SyncBadge::Pending(_) => {
            // Breathing: changes are on their way.
            #[allow(clippy::cast_possible_truncation)] // A phase for drawing.
            let phase = ((ui.time() * 3.0).sin() * 0.5 + 0.5) as f32;
            ui.gfx
                .fill_circle(dot, 3.0, t.accent.alpha(0.45 + 0.55 * phase));
            ui.request_repaint_after(std::time::Duration::from_millis(33));
        }
        SyncBadge::Failed => ui.gfx.fill_circle(dot, 3.0, t.error),
        SyncBadge::Synced | SyncBadge::Local => {}
    }
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.clicked
}
