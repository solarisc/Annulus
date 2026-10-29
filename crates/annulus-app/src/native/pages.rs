//! What the vault's list screens share: the page margins, a header with a title, count,
//! subtitle and actions, list rows, settings cards and the empty state, so every screen
//! looks like one product.

use annulus_config::tokens::space;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::IconId;
use annulus_ui::kit::{Cursor, Id, Response, Role, Ui};
use annulus_ui::widgets::{BODY, BODY_STRONG, CONTROL, HEADING, Kind, SMALL, TITLE, button_width};

/// Side padding of a page.
pub const PAD: f32 = 28.0;
/// Height of a list row.
pub const ROW_H: f32 = 56.0;

/// The page's content area inside `rect`.
#[must_use]
pub fn page(rect: Rect) -> Rect {
    let mut inner = rect.inset(PAD, 0.0);
    inner.take_top(space::XL);
    inner
}

/// The title row and the subtitle under it; returns the space right of the title for
/// actions (as tall as a control) and leaves `rect` below the header.
pub fn header(
    ui: &mut Ui<'_>,
    rect: &mut Rect,
    title: &str,
    count: Option<usize>,
    subtitle: &str,
) -> Rect {
    let t = ui.theme;
    let mut row = rect.take_top(36.0);
    let width = ui.gfx.measure(title, TITLE).width;
    let head = row.take_left(width + 2.0);
    ui.strict_text(head, title, TITLE, t.text, Align::Start);
    row.take_left(space::S + 2.0);
    if let Some(count) = count.filter(|count| *count > 0) {
        let label = count.to_string();
        let w = ui.gfx.measure(&label, SMALL).width + 14.0;
        let badge = Rect::new(row.x, row.center().y - 10.0, w, 20.0);
        ui.gfx.fill_rect(badge, 10.0, t.hover);
        ui.gfx.text(badge, &label, SMALL, t.text2, Align::Center);
    }
    rect.take_top(space::XS);
    ui.gfx
        .text(rect.take_top(20.0), subtitle, BODY, t.text2, Align::Start);
    rect.take_top(space::L);
    row.centered(row.w, CONTROL)
}

/// A button at the right end of `actions`, which shrinks; returns whether it was clicked.
pub fn action(
    ui: &mut Ui<'_>,
    actions: &mut Rect,
    id: &str,
    label: &str,
    icon: Option<IconId>,
    kind: Kind,
) -> bool {
    let width = button_width(ui.gfx, label, icon);
    let rect = actions.take_right(width);
    actions.take_right(space::S);
    ui.button(Id::new(id), rect, label, icon, kind).clicked
}

/// A list row card: hover and selection fills, focus ring, pointer cursor.
pub fn row(ui: &mut Ui<'_>, id: Id, rect: Rect, label: &str, selected: bool) -> Response {
    let response = ui.zone(id, rect, Role::ListItem, label);
    let t = ui.theme;
    // Still hovered on the row's buttons.
    let hovered = ui.hover_within(rect);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let fill = if selected {
        t.selected
    } else {
        t.surface.mix(t.hover, hover * 0.4)
    };
    ui.gfx.fill_rect(rect, 10.0, fill);
    let border = if selected {
        t.accent.alpha(0.6)
    } else {
        t.line.mix(t.line_strong, hover)
    };
    ui.gfx.stroke_rect(rect, 10.0, 1.0, border);
    if response.focus_ring {
        ui.focus_ring(rect, 10.0);
    }
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response
}

/// An icon tile at the left of a row; returns the rest of the row.
pub fn row_icon(ui: &mut Ui<'_>, rect: Rect, icon: IconId) -> Rect {
    let t = ui.theme;
    let mut inner = rect.inset(space::M + 2.0, 0.0);
    let tile = inner.take_left(32.0).centered(32.0, 32.0);
    ui.gfx.fill_rect(tile, 8.0, t.accent.alpha(0.14));
    ui.gfx.icon(icon, tile.centered(18.0, 18.0), t.accent);
    inner.take_left(space::M);
    inner
}

/// Two lines of text, a title and a detail, centered in `rect`.
pub fn two_lines(ui: &mut Ui<'_>, rect: Rect, title: &str, detail: &str, mono_detail: bool) {
    let t = ui.theme;
    let top = Rect::new(rect.x, rect.center().y - 19.0, rect.w, 20.0);
    let bottom = Rect::new(rect.x, rect.center().y + 1.0, rect.w, 18.0);
    ui.gfx.text(top, title, BODY_STRONG, t.text, Align::Start);
    let font = if mono_detail {
        annulus_ui::gfx::Font::mono(11.5)
    } else {
        SMALL
    };
    ui.gfx.text(bottom, detail, font, t.text2, Align::Start);
}

/// A titled card of settings rows; returns the inside, `height` tall, and moves `rect` past it.
pub fn card(ui: &mut Ui<'_>, rect: &mut Rect, title: &str, height: f32) -> Rect {
    let t = ui.theme;
    ui.section_label(rect.take_top(18.0), title);
    rect.take_top(space::S);
    let card = rect.take_top(height);
    ui.gfx.fill_rect(card, 12.0, t.surface);
    ui.gfx.stroke_rect(card, 12.0, 1.0, t.line);
    rect.take_top(space::XL);
    card.inset(space::L, space::S)
}

/// A settings row: a label and its explanation on the left; returns the right side for the
/// control.
pub fn setting(ui: &mut Ui<'_>, row: Rect, title: &str, detail: &str, control_w: f32) -> Rect {
    let mut row = row;
    let control = row.take_right(control_w);
    row.take_right(space::L);
    two_lines(ui, row, title, detail, false);
    control.centered(control.w, CONTROL)
}

/// A hairline between settings rows.
pub fn divider(ui: &mut Ui<'_>, y: f32, rect: Rect) {
    let t = ui.theme;
    ui.gfx
        .fill_rect(Rect::new(rect.x, y, rect.w, 1.0), 0.0, t.line);
}

/// One sentence and an icon in the middle of an empty screen.
pub fn empty(ui: &mut Ui<'_>, rect: Rect, icon: IconId, title: &str, line: &str) {
    let t = ui.theme;
    let block = rect.centered(rect.w.min(460.0), 150.0);
    let mut column = block;
    let badge = column.take_top(64.0).centered(64.0, 64.0);
    ui.gfx.fill_rect(badge, 18.0, t.selected);
    ui.gfx.stroke_rect(badge, 18.0, 1.0, t.accent.alpha(0.35));
    ui.gfx.icon(icon, badge.centered(28.0, 28.0), t.accent);
    column.take_top(space::L);
    ui.strict_text(column.take_top(24.0), title, HEADING, t.text, Align::Center);
    column.take_top(space::XS);
    ui.gfx
        .text(column.take_top(20.0), line, BODY, t.text2, Align::Center);
}
