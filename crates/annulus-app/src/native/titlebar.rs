//! The title bar is the tab strip (UI-SPEC §1): our mark, the tabs, new-tab buttons, the sync
//! badge and the window buttons. Empty space drags the window; the window host finds it, and
//! the buttons, by their zone ids ([`DRAG`], [`MINIMIZE`], [`MAXIMIZE`], [`CLOSE`]).

use annulus_config::tokens::space;
use annulus_ui::gfx::{Align, Color, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Role, Ui};
use annulus_ui::widgets::{BODY_MEDIUM, SMALL};

use super::{NativeApp, SyncBadge, TabKind, WindowAction};
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
    // Reserved now, drawn after the tabs: Tab follows the drawing order.
    let sync = app.sync_badge();
    let badge_w = badge_width(ui, &sync);
    let badge = Rect::new(row.r() - badge_w, row.center().y - 12.0, badge_w, 24.0);
    row.take_right(badge_w + space::S);

    row.take_left(space::M + 2.0);
    let mark = row.take_left(20.0).centered(20.0, 20.0);
    paint_mark(ui, mark);
    row.take_left(space::M);

    let mut close_tab = None;
    for index in 0..app.tabs.len() {
        let width = tab_width(ui, &app.tabs[index].title, app.tabs[index].kind);
        let available = row.w - 2.0 * 32.0;
        if available < 60.0 {
            break;
        }
        let slot = row.take_left(width.min(available));
        let rect = slot.centered(slot.w, TAB_H);
        match tab(ui, rect, app, index) {
            TabEvent::Select => app.active = index,
            TabEvent::Close => close_tab = Some(index),
            TabEvent::None => {}
        }
        row.take_left(2.0);
    }
    row.take_left(space::XS);
    let plus = row.take_left(30.0).centered(30.0, 30.0);
    let new_tab = ui
        .icon_button(
            Id::new("tab.new"),
            plus,
            icons::PLUS,
            "New terminal tab",
            false,
        )
        .clicked;
    ui.tooltip(Id::new("tab.new"), plus, "New terminal tab (Ctrl+Shift+T)");
    if new_tab {
        app.open_local();
    }
    sync_badge(ui, badge, &sync);
    if let Some(index) = close_tab
        && index > 0
    {
        app.close_tab(index);
    }
    action
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

/// The width a tab wants.
fn tab_width(ui: &mut Ui<'_>, title: &str, kind: TabKind) -> f32 {
    let text = ui.gfx.measure(title, BODY_MEDIUM).width;
    match kind {
        TabKind::Vault => (text + 16.0 + 8.0 + 2.0 * space::M).ceil(),
        TabKind::Terminal | TabKind::Sftp => {
            (text + 8.0 + 8.0 + 16.0 + 6.0 + 2.0 * space::M).clamp(132.0, 220.0)
        }
    }
}

enum TabEvent {
    None,
    Select,
    Close,
}

fn tab(ui: &mut Ui<'_>, rect: Rect, app: &NativeApp, index: usize) -> TabEvent {
    let tab = &app.tabs[index];
    let id = Id::new(("tab", tab.uid));
    let active = app.active == index;
    // A screen reader hears the dot too: "web-1, New output".
    let name = match tab.status {
        Some(status) => format!("{}, {}", tab.title, status.describe()),
        None => tab.title.clone(),
    };
    let response = ui.zone(id, rect, Role::Tab, &name);
    let t = ui.theme;
    // Still hovered on its close button.
    let hovered = ui.hover_within(rect);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let fill = if active {
        t.surface
    } else {
        t.hover.alpha(hover * 0.7)
    };
    ui.gfx.fill_rect(rect, 8.0, fill);
    if active {
        ui.gfx.stroke_rect(rect, 8.0, 1.0, t.line);
    }
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let mut inner = rect.inset(space::M, 0.0);
    let ink = if active {
        t.text
    } else {
        t.text2.mix(t.text, hover)
    };
    match tab.kind {
        TabKind::Vault => {
            let icon = inner.take_left(16.0).centered(16.0, 16.0);
            ui.gfx
                .icon(icons::GRID_VIEW, icon, if active { t.accent } else { ink });
            inner.take_left(8.0);
        }
        TabKind::Terminal | TabKind::Sftp => {
            let dot = inner.take_left(8.0);
            let color = match tab.status {
                Some(TabStatus::Unread) => t.accent,
                Some(TabStatus::Input) => t.warn,
                Some(TabStatus::Error) => t.error,
                None => t.ok.alpha(0.85),
            };
            ui.gfx.fill_circle(dot.center(), 3.5, color);
            inner.take_left(8.0);
        }
    }
    let mut event = if response.clicked {
        TabEvent::Select
    } else {
        TabEvent::None
    };
    if tab.kind != TabKind::Vault {
        let close = inner.take_right(18.0).centered(18.0, 18.0);
        if active || hovered || hover > 0.0 {
            let close_id = id.with("close");
            let x = ui.zone(close_id, close, Role::Button, "Close tab");
            ui.tooltip(close_id, close, "Close tab (Ctrl+Shift+W)");
            if x.hovered {
                ui.gfx.fill_rect(close, 4.0, t.hover);
            }
            ui.gfx.icon(
                icons::CLOSE,
                close.centered(12.0, 12.0),
                t.text2.alpha(if active { 1.0 } else { hover }),
            );
            if x.clicked {
                event = TabEvent::Close;
            }
        }
        if response.middle_clicked {
            event = TabEvent::Close;
        }
        inner.take_right(6.0);
    }
    ui.gfx
        .text(inner, &tab.title, BODY_MEDIUM, ink, Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    event
}

/// The sync badge's words and color; `None` for a local vault, which shows none.
fn badge_label(ui: &Ui<'_>, sync: &SyncBadge) -> Option<(String, Color)> {
    let t = ui.theme;
    match sync {
        SyncBadge::Local => None,
        SyncBadge::Synced => Some(("Synced".to_owned(), t.ok)),
        SyncBadge::Pending(count) => Some((format!("Syncing {count}"), t.warn)),
        SyncBadge::Failed => Some(("Sync failed".to_owned(), t.error)),
    }
}

/// The width the sync badge takes.
fn badge_width(ui: &mut Ui<'_>, sync: &SyncBadge) -> f32 {
    badge_label(ui, sync).map_or(0.0, |(label, _)| {
        ui.gfx.measure(&label, SMALL).width + 8.0 + 6.0 + 2.0 * space::S + 4.0
    })
}

/// Draws the sync badge into `rect`.
fn sync_badge(ui: &mut Ui<'_>, rect: Rect, sync: &SyncBadge) {
    let t = ui.theme;
    let Some((label, color)) = badge_label(ui, sync) else {
        return;
    };
    let id = Id::new("titlebar.sync");
    let response = ui.zone(id, rect, Role::Button, &label);
    ui.gfx.fill_rect(
        rect,
        12.0,
        color.alpha(if response.hovered { 0.2 } else { 0.12 }),
    );
    let mut inner = rect.inset(space::S + 2.0, 0.0);
    let dot = inner.take_left(8.0);
    ui.gfx.fill_circle(dot.center(), 3.5, color);
    inner.take_left(6.0);
    ui.gfx.text(inner, &label, SMALL, t.text, Align::Start);
}
