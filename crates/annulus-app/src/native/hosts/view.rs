//! Host editor forms, cards, rows, tags, and drag drawing.

use super::*;

/// A column of labelled fields in the editor, laid out top to bottom.
#[derive(Clone, Copy)]
pub(super) struct Form {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
}

impl Form {
    pub(super) fn gap(&mut self, h: f32) {
        self.y += h;
    }

    pub(super) fn split(&self, left: f32) -> (f32, f32) {
        (left, self.w - left - space::M)
    }

    pub(super) fn section(&mut self, ui: &mut Ui<'_>, title: &str) {
        let t = ui.theme;
        ui.strict_text(
            Rect::new(self.x, self.y, self.w, 18.0),
            title,
            BODY_MEDIUM,
            t.text,
            Align::Start,
        );
        self.y += 18.0 + space::M;
    }

    pub(super) fn label(&mut self, ui: &mut Ui<'_>, label: &str) {
        let t = ui.theme;
        ui.strict_text(
            Rect::new(self.x, self.y, self.w, 16.0),
            label,
            SMALL,
            t.text2,
            Align::Start,
        );
        self.y += 16.0 + 5.0;
    }

    pub(super) fn text(
        &mut self,
        ui: &mut Ui<'_>,
        id: &str,
        label: &str,
        buffer: &mut String,
        placeholder: &str,
        error: Option<&str>,
    ) {
        self.label(ui, label);
        let field = Rect::new(self.x, self.y, self.w, CONTROL);
        ui.text_field(
            Id::new(id),
            field,
            buffer,
            FieldStyle {
                placeholder,
                error: error.is_some(),
                ..FieldStyle::default()
            },
        );
        // A screen reader says "Username", not the example in the empty field.
        ui.set_name(Id::new(id), label);
        self.y += CONTROL;
        if let Some(error) = error {
            let t = ui.theme;
            let line = Rect::new(self.x, self.y + 4.0, self.w, 16.0);
            ui.gfx.text(line, error, SMALL, t.error, Align::Start);
            self.y += 20.0;
        }
        self.y += space::M + 2.0;
    }

    /// A field of several lines (notes) under its label, its lines wrapped at its width.
    pub(super) fn area(
        &mut self,
        ui: &mut Ui<'_>,
        id: &str,
        label: &str,
        buffer: &mut String,
        placeholder: &str,
    ) {
        /// Room for about five lines.
        const AREA_H: f32 = 108.0;
        self.label(ui, label);
        let field = Rect::new(self.x, self.y, self.w, AREA_H);
        ui.text_area(
            Id::new(id),
            field,
            buffer,
            FieldStyle {
                placeholder,
                wrap: true,
                ..FieldStyle::default()
            },
        );
        ui.set_name(Id::new(id), label);
        self.y += AREA_H + space::M + 2.0;
    }

    pub(super) fn secret(
        &mut self,
        ui: &mut Ui<'_>,
        id: &str,
        label: &str,
        buffer: &mut Zeroizing<String>,
        placeholder: &str,
    ) {
        self.label(ui, label);
        let field = Rect::new(self.x, self.y, self.w, CONTROL);
        ui.text_field(
            Id::new(id),
            field,
            buffer,
            FieldStyle {
                placeholder,
                ..FieldStyle::default()
            },
        );
        self.y += CONTROL + space::M + 2.0;
    }

    pub(super) fn select(
        &mut self,
        ui: &mut Ui<'_>,
        id: &str,
        label: &str,
        items: &[String],
        selected: Option<usize>,
    ) -> Option<usize> {
        self.label(ui, label);
        let field = Rect::new(self.x, self.y, self.w, CONTROL);
        let chosen = ui.select(Id::new(id), field, items, selected, "");
        self.y += CONTROL + space::M + 2.0;
        chosen
    }

    pub(super) fn check(&mut self, ui: &mut Ui<'_>, id: &str, label: &str, value: &mut bool) {
        let row = Rect::new(self.x, self.y, self.w, 24.0);
        ui.checkbox(Id::new(id), row, value, label);
        self.y += 24.0 + space::L;
    }

    /// A clickable section title with a chevron; returns whether it was clicked.
    pub(super) fn disclosure(
        &mut self,
        ui: &mut Ui<'_>,
        id: &str,
        title: &str,
        open: bool,
    ) -> bool {
        let t = ui.theme;
        let row = Rect::new(self.x - 6.0, self.y - 6.0, self.w + 12.0, 28.0);
        let response = ui.zone(Id::new(id), row, Role::Button, title);
        if response.hovered {
            ui.gfx.fill_rect(row, 6.0, t.hover);
            ui.set_cursor(Cursor::Pointer);
        }
        if response.focus_ring {
            ui.focus_ring(row, 6.0);
        }
        let mut inner = row.inset(6.0, 0.0);
        let chevron = inner.take_right(16.0).centered(14.0, 14.0);
        ui.gfx.icon(
            if open {
                icons::CHEVRON_DOWN
            } else {
                icons::CHEVRON_RIGHT
            },
            chevron,
            t.text2,
        );
        ui.section_label(inner, title);
        self.y += 28.0 + space::S;
        response.clicked
    }
}

/// How solid a card gliding at `place` is on its way to `home`: dim while far off, so cards
/// passing over each other read as a move and not as a pile, and whole when it lands.
pub(super) fn in_flight(place: Rect, home: Rect) -> f32 {
    let away = (place.x - home.x).hypot(place.y - home.y);
    1.0 - 0.6 * (away / (away + 40.0))
}

/// How many card columns fit in `width`.
pub(super) fn grid_columns(width: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small, positive.
    let columns = ((width + GAP) / (CARD_MIN_W + GAP)).floor().max(1.0) as usize;
    columns.min(4)
}

#[allow(clippy::cast_precision_loss)] // Column and row counts are small.
pub(super) fn grid_cell(
    x: f32,
    y: f32,
    width: f32,
    columns: usize,
    index: usize,
    height: f32,
) -> Rect {
    let card_w = (width - GAP * (columns as f32 - 1.0)) / columns as f32;
    let (row, column) = (index / columns, index % columns);
    Rect::new(
        x + column as f32 * (card_w + GAP),
        y + row as f32 * (height + GAP),
        card_w,
        height,
    )
}

/// Keeps the same point of a host under the pointer while it is dragged.
pub(super) fn dragged_rect(ui: &Ui<'_>, home: Rect, pointer: Point) -> Rect {
    let pressed = ui.pressed_at().unwrap_or_else(|| home.center());
    let grab_x = (pressed.x - home.x).clamp(0.0, home.w);
    let grab_y = (pressed.y - home.y).clamp(0.0, home.h);
    Rect::new(pointer.x - grab_x, pointer.y - grab_y, home.w, home.h)
}

/// The host that a dragged host goes before. `None` is the end of this group.
pub(super) fn host_drop_before(
    cards: &[Card],
    homes: &[(u64, Rect)],
    moving: u64,
    pointer: Point,
    mode: Mode,
) -> Option<u64> {
    let remaining: Vec<(u64, Rect)> = homes
        .iter()
        .copied()
        .filter(|(id, _)| *id != moving)
        .collect();
    let (target, (_, rect)) = remaining
        .iter()
        .enumerate()
        .min_by(|(_, (_, a)), (_, (_, b))| {
            let distance = |rect: &Rect| {
                let center = rect.center();
                (center.x - pointer.x).powi(2) + (center.y - pointer.y).powi(2)
            };
            distance(a).total_cmp(&distance(b))
        })?;
    let after = match mode {
        Mode::Grid if (pointer.y - rect.center().y).abs() < rect.h / 2.0 + GAP => {
            pointer.x > rect.center().x
        }
        Mode::Grid | Mode::List => pointer.y > rect.center().y,
    };
    let before = remaining
        .get(target + usize::from(after))
        .map(|(id, _)| *id);
    before.filter(|id| cards.iter().any(|card| card.id == *id))
}

/// The clear insertion line under a dragged host.
pub(super) fn host_drop_mark(
    ui: &mut Ui<'_>,
    homes: &[(u64, Rect)],
    before: Option<u64>,
    mode: Mode,
) {
    let Some((_, edge)) = before
        .and_then(|id| homes.iter().find(|(host, _)| *host == id))
        .or_else(|| homes.last())
    else {
        return;
    };
    let color = ui.theme.accent;
    let mark = match (mode, before.is_some()) {
        (Mode::List, true) => Rect::new(edge.x, edge.y - 2.0, edge.w, 3.0),
        (Mode::List, false) => Rect::new(edge.x, edge.b() + 2.0, edge.w, 3.0),
        (Mode::Grid, true) => Rect::new(edge.x - 3.0, edge.y + 4.0, 3.0, edge.h - 8.0),
        (Mode::Grid, false) => Rect::new(edge.r() + 1.0, edge.y + 4.0, 3.0, edge.h - 8.0),
    };
    ui.gfx.fill_rect(mark, 1.5, color);
}

#[derive(Clone, Copy)]
pub(super) enum CardEvent {
    None,
    /// A click (or Enter on the card); what it does is [`HostsView::clicks`]'s to say.
    Click,
    /// A double-click, likewise.
    DoubleClick,
    Select,
    Connect,
    Edit,
    /// An item of the host's context menu.
    More(usize),
}

/// The first items of a host's context menu, which the listing handles itself; the rest go to
/// [`HostsView::host_menu_action`].
pub(super) const MENU_CONNECT: usize = 0;
pub(super) const MENU_SFTP: usize = 1;
pub(super) const MENU_EDIT: usize = 2;
/// Where the items [`HostsView::host_menu_action`] handles start.
pub(super) const MENU_REST: usize = 3;

/// A host's context menu (right click, or Shift+F10): connect, open its files, edit, copy its
/// ssh command, duplicate it, or move it to the top level or a group of `moves`
/// (`(uid, path)`).
pub(super) fn host_menu(moves: &[(String, String)]) -> Vec<String> {
    let mut items = vec![
        "Connect".to_owned(),
        "Open SFTP".to_owned(),
        format!("{MENU_BREAK}Edit"),
        "Copy ssh command".to_owned(),
        "Duplicate".to_owned(),
        format!("{MENU_BREAK}Move to the top level"),
    ];
    items.extend(moves.iter().map(|(_, path)| format!("Move to {path}")));
    items
}

/// Takes `tag` off every host that has it (F-HOST-004); the message to show.
pub(super) fn untag_everywhere(vault: &mut Vault, tag: &str) -> String {
    let tagged: Vec<Host> = vault
        .hosts()
        .iter()
        .filter(|host| host.tags.iter().any(|known| known == tag))
        .cloned()
        .collect();
    let mut done = 0;
    for mut host in tagged {
        host.tags.retain(|known| known != tag);
        if vault.upsert_host(host).is_ok() {
            done += 1;
        }
    }
    match done {
        1 => format!("Deleted the tag \u{201c}{tag}\u{201d} from 1 host."),
        n => format!("Deleted the tag \u{201c}{tag}\u{201d} from {n} hosts."),
    }
}

/// What `host` would take from `group` (and the groups above it) for every field it leaves
/// empty, and from which group each comes (F-HOST-003).
pub(super) fn inherited(
    vault: &Vault,
    host: &Host,
    group: Option<String>,
) -> (Host, annulus_vault::groups::Sources) {
    let probe = Host {
        group,
        username: String::new(),
        port: None,
        key_id: None,
        jump_id: None,
        proxy: None,
        startup: String::new(),
        ..host.clone()
    };
    vault.resolve(&probe)
}

/// The text of an empty field: `value` and the group it comes from, or `example` when the
/// field inherits nothing.
pub(super) fn hint(value: &str, group: Option<&String>, example: &str) -> String {
    match group {
        Some(group) if !value.is_empty() => format!("{value} \u{b7} from {group}"),
        _ => example.to_owned(),
    }
}

/// The Edit button of a host card or row, faded in by `shown` (hovered, selected or focused);
/// the rest of what can be done with a host is in its context menu. Returns whether it was
/// clicked.
pub(super) fn edit_button(ui: &mut Ui<'_>, id: Id, rect: Rect, shown: f32) -> bool {
    if shown <= 0.0 {
        return false;
    }
    ui.push_layer(Layer::fade(shown));
    let clicked = ui
        .icon_button(id.with("edit"), rect, icons::EDIT, "Edit", false)
        .clicked;
    ui.pop_layer();
    clicked
}

pub(super) fn host_card(
    ui: &mut Ui<'_>,
    rect: Rect,
    card: &Card,
    selected: bool,
    menu: &[String],
) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    // On the card itself or on its button (whose last place may be a frame behind while the
    // card moves in), and lit while its context menu is open over it.
    let hovered = response.hovered || ui.hover_within(rect) || ui.menu_open(id);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let press = ui.ease(id.with("press"), if response.pressed { 1.0 } else { 0.0 });
    let chosen = ui.animate(
        id.with("selected"),
        if selected { 1.0 } else { 0.0 },
        Motion::BASE,
    );
    // A hovered card lifts a little on a soft shadow; pressing sets it down again and it gives
    // a little, like a key.
    let lift = hover * (1.0 - press);
    let shape = rect.offset(0.0, -LIFT * lift);
    ui.push_layer(Layer::NONE.scaled(shape.center(), 1.0 - 0.012 * press));
    let round = radius::CARD;
    if lift > 0.0 {
        ui.gfx
            .shadow(shape, round, 22.0, 8.0, t.shadow.alpha(lift * 0.6));
    }
    ui.card_surface(shape, round);
    ui.gfx.fill_rect(shape, round, t.hover.alpha(hover));
    ui.gfx.fill_rect(shape, round, t.selected.alpha(chosen));
    if chosen > 0.0 {
        ui.gfx
            .stroke_rect(shape, round, 1.0, t.accent.alpha(0.6 * chosen));
    }
    if response.focus_ring {
        ui.focus_ring(shape, round);
    }
    let mut inner = shape.inset(space::M, 0.0);
    let tile = inner.take_left(36.0).centered(36.0, 36.0);
    os::tile(ui, tile, &card.label, card.os);
    inner.take_left(space::M);
    // Kept for the Edit button whether it shows or not, so the text never moves under it.
    let edit = inner.take_right(28.0).centered(28.0, 28.0);
    inner.take_right(space::S);
    let mut event = if response.double_clicked {
        CardEvent::DoubleClick
    } else if response.clicked {
        CardEvent::Click
    } else {
        CardEvent::None
    };
    let focused = if response.focused { 1.0 } else { 0.0 };
    if edit_button(ui, id, edit, hover.max(chosen).max(focused)) {
        event = CardEvent::Edit;
    }
    if let Some(choice) = ui.context_menu(id, &response, menu) {
        event = CardEvent::More(choice);
    }
    let name_line = Rect::new(inner.x, shape.y + 12.0, inner.w, 20.0);
    let mut detail = Rect::new(inner.x, shape.y + 32.0, inner.w, 20.0);
    ui.gfx
        .text(name_line, &card.label, BODY_MEDIUM, t.text, Align::Start);
    // The tags share the second line with the address: the address takes what it needs (up
    // to 70 % of the line), the tags the rest, with a "+n" for those that do not fit.
    let address = ui.gfx.measure(&card.detail, SMALL).width + space::S;
    let room = (detail.w - address).max(detail.w * 0.3);
    let (count, width) = tags_fit(ui, &card.tags, room);
    if width > 0.0 {
        // Right after the address, so a column of cards reads as one rhythm.
        let at = (detail.x + address).min(detail.r() - width);
        let tags = Rect::new(at, detail.y, width, detail.h);
        draw_tags(ui, tags, &card.tags, count);
        detail.w = at - space::S - detail.x;
    }
    ui.gfx
        .text(detail, &card.detail, SMALL, t.text2, Align::Start);
    ui.pop_layer();
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    event
}

/// Tags on cards and rows.
const TAG: Font = Font::ui(11.0).weight(Weight::Medium);
/// Height of a tag chip.
const TAG_H: f32 = 18.0;
/// Space between tag chips.
const TAG_GAP: f32 = 4.0;

pub(super) fn tag_width(ui: &mut Ui<'_>, label: &str) -> f32 {
    ui.gfx.measure(label, TAG).width + 14.0
}

/// How many of `tags` fit in `max` with a "+n" chip for the rest, and the width they take;
/// nothing when not even that fits.
pub(super) fn tags_fit(ui: &mut Ui<'_>, tags: &[String], max: f32) -> (usize, f32) {
    let mut width = 0.0;
    let mut count = 0;
    for (index, tag) in tags.iter().enumerate() {
        let gap = if index > 0 { TAG_GAP } else { 0.0 };
        let chip = tag_width(ui, tag) + gap;
        let rest = tags.len() - index - 1;
        let more = if rest > 0 {
            tag_width(ui, &format!("+{rest}")) + TAG_GAP
        } else {
            0.0
        };
        if width + chip + more > max {
            break;
        }
        width += chip;
        count += 1;
    }
    let hidden = tags.len() - count;
    if hidden > 0 {
        let gap = if count > 0 { TAG_GAP } else { 0.0 };
        let more = tag_width(ui, &format!("+{hidden}")) + gap;
        if width + more > max {
            return (0, 0.0);
        }
        width += more;
    }
    (count, width)
}

/// Draws the first `count` of `tags` as colored chips in `rect`, then a "+n" chip for the
/// rest.
pub(super) fn draw_tags(ui: &mut Ui<'_>, rect: Rect, tags: &[String], count: usize) {
    let t = ui.theme;
    let mut x = rect.x;
    let y = rect.center().y - TAG_H / 2.0;
    let hidden = tags.len() - count.min(tags.len());
    let more = format!("+{hidden}");
    let chips = tags
        .iter()
        .take(count)
        .map(|tag| (tag.as_str(), Some(os::hue(tag))))
        .chain((hidden > 0).then_some((more.as_str(), None)));
    for (label, hue) in chips {
        let width = tag_width(ui, label);
        let chip = Rect::new(x, y, width, TAG_H);
        let (fill, ink) = match hue {
            Some(hue) => (
                hue.alpha(if t.dark { 0.22 } else { 0.12 }),
                hue.mix(t.text, if t.dark { 0.55 } else { 0.3 }),
            ),
            None => (t.hover, t.text2),
        };
        ui.gfx.fill_rect(chip, TAG_H / 2.0, fill);
        ui.gfx.text(chip, label, TAG, ink, Align::Center);
        x += width + TAG_GAP;
    }
}

/// A chip of the tag filter, accent-colored while chosen; a check box to assistive
/// technology. Returns whether it was clicked (or pressed with Space or Enter).
pub(super) fn tag_chip(ui: &mut Ui<'_>, id: Id, rect: Rect, label: &str, on: bool) -> bool {
    let response = ui.zone(id, rect, Role::CheckBox, label);
    ui.set_toggled(id, on);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let round = rect.h / 2.0;
    let lit = ui.animate(id.with("on"), if on { 1.0 } else { 0.0 }, Motion::FAST);
    let ink = t.text2.mix(t.text, hover.max(lit));
    ui.gfx.fill_rect(
        rect,
        round,
        t.hover.alpha(hover).over(t.surface).alpha(1.0 - lit),
    );
    ui.gfx.fill_rect(rect, round, t.selected.alpha(lit));
    ui.gfx
        .stroke_rect(rect, round, 1.0, t.accent.alpha(0.6 * lit));
    if response.focus_ring {
        ui.focus_ring(rect, round);
    }
    ui.gfx.text(rect, label, SMALL, ink, Align::Center);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.clicked
}

pub(super) fn list_header(ui: &mut Ui<'_>, rect: Rect) {
    let t = ui.theme;
    let [name, address, tags] = list_columns(rect);
    for (cell, title) in [(name, "Name"), (address, "Address"), (tags, "Tags")] {
        ui.gfx
            .text(cell, title, SMALL, t.text3.mix(t.text2, 0.5), Align::Start);
    }
    ui.gfx
        .fill_rect(Rect::new(rect.x, rect.b() - 1.0, rect.w, 1.0), 0.0, t.line);
}

pub(super) fn list_columns(rect: Rect) -> [Rect; 3] {
    let mut row = rect.inset(space::M, 0.0);
    let name = row.take_left(row.w * 0.36);
    let address = row.take_left(row.w * 0.55);
    [name, address, row]
}

pub(super) fn host_row(
    ui: &mut Ui<'_>,
    rect: Rect,
    card: &Card,
    selected: bool,
    menu: &[String],
) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    let hovered = response.hovered || ui.hover_within(rect) || ui.menu_open(id);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let chosen = ui.animate(
        id.with("selected"),
        if selected { 1.0 } else { 0.0 },
        Motion::BASE,
    );
    ui.gfx.fill_rect(rect, 8.0, t.hover.alpha(hover));
    ui.gfx.fill_rect(rect, 8.0, t.selected.alpha(chosen));
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let [mut name, address, mut tags] = list_columns(rect);
    let edit = tags.take_right(28.0).centered(28.0, 28.0);
    tags.take_right(space::S);
    let focused = if response.focused { 1.0 } else { 0.0 };
    let edited = edit_button(ui, id, edit, hover.max(chosen).max(focused));
    let picked = ui.context_menu(id, &response, menu);
    let tile = name.take_left(28.0).centered(28.0, 28.0);
    os::tile(ui, tile, &card.label, card.os);
    name.take_left(space::M);
    ui.gfx
        .text(name, &card.label, BODY_MEDIUM, t.text, Align::Start);
    ui.gfx
        .text(address, &card.detail, BODY, t.text2, Align::Start);
    let (count, width) = tags_fit(ui, &card.tags, tags.w);
    draw_tags(
        ui,
        Rect::new(tags.x, tags.y, width, tags.h),
        &card.tags,
        count,
    );
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    if let Some(choice) = picked {
        CardEvent::More(choice)
    } else if edited {
        CardEvent::Edit
    } else if response.double_clicked {
        CardEvent::DoubleClick
    } else if response.clicked {
        CardEvent::Click
    } else {
        CardEvent::None
    }
}

pub(super) fn group_card(ui: &mut Ui<'_>, rect: Rect, group: &GroupCard) -> bool {
    let id = Id::new(("group", group.uid.as_str()));
    let response = ui.zone(id, rect, Role::ListItem, &group.name);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    ui.card_surface(rect, radius::CARD);
    ui.gfx.fill_rect(rect, radius::CARD, t.hover.alpha(hover));
    if response.focus_ring {
        ui.focus_ring(rect, radius::CARD);
    }
    let mut inner = rect.inset(space::M, 0.0);
    let tile = inner.take_left(32.0).centered(32.0, 32.0);
    ui.gfx.fill_rect(tile, 8.0, t.hover);
    ui.gfx
        .icon(icons::FOLDER, tile.centered(17.0, 17.0), t.text2);
    inner.take_left(space::M);
    let chevron = inner.take_right(16.0).centered(14.0, 14.0);
    ui.gfx
        .icon(icons::CHEVRON_RIGHT, chevron, t.text3.mix(t.text2, hover));
    let name_line = Rect::new(inner.x, rect.y + 9.0, inner.w, 20.0);
    let detail = Rect::new(inner.x, rect.y + 29.0, inner.w, 18.0);
    ui.gfx
        .text(name_line, &group.name, BODY_MEDIUM, t.text, Align::Start);
    ui.gfx
        .text(detail, &group.holds, SMALL, t.text2, Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.double_clicked || response.clicked
}

/// What a search or tag filter that matches no host shows, with Save as host when `query`
/// is a target that can be saved (F-APP-006). Returns whether that was clicked.
pub(super) fn no_results(ui: &mut Ui<'_>, rect: Rect, query: &str, can_save: bool) -> bool {
    let t = ui.theme;
    let height = if can_save {
        120.0 + space::L + CONTROL
    } else {
        120.0
    };
    let block = rect.centered(rect.w.min(460.0), height);
    let mut column = block;
    let icon = column.take_top(32.0).centered(28.0, 28.0);
    ui.gfx.icon(icons::SEARCH, icon, t.text3);
    column.take_top(space::M);
    let (title, detail) = if query.is_empty() {
        (
            "No host has every chosen tag".to_owned(),
            "Choose fewer tags to see more hosts.",
        )
    } else {
        (
            format!("No hosts match \u{201c}{query}\u{201d}"),
            "Press Enter to connect to it as typed.",
        )
    };
    ui.gfx.text(
        column.take_top(22.0),
        &title,
        BODY_MEDIUM,
        t.text,
        Align::Center,
    );
    ui.gfx
        .text(column.take_top(20.0), detail, BODY, t.text2, Align::Center);
    if !can_save {
        return false;
    }
    column.take_top(space::L);
    let width = button_width(&mut ui.gfx, "Save as host", Some(icons::PLUS));
    let save = column.take_top(CONTROL).centered(width, CONTROL);
    ui.button(
        Id::new("hosts.save-target"),
        save,
        "Save as host",
        Some(icons::PLUS),
        Kind::Secondary,
    )
    .clicked
}

/// The surface of an editor panel inside the content card `content`: the card's own rounded
/// corners and outline where they meet, square and shadowed where it lies over the list.
pub(super) fn panel_backing(ui: &mut Ui<'_>, content: Rect, panel: Rect) {
    let t = ui.theme;
    let radius = crate::native::PANEL_RADIUS;
    // Beside the list, or over all of it in a narrow window.
    let beside = panel.x > content.x + 0.5;
    if beside {
        // A soft edge of shadow on the list.
        ui.gfx.fill_gradient(
            Rect::new(panel.x - 22.0, panel.y, 22.0, panel.h),
            0.0,
            t.shadow.alpha(0.0),
            t.shadow.alpha(if t.dark { 0.55 } else { 0.4 }),
            true,
        );
    }
    ui.gfx.fill_rect(panel, radius, t.surface);
    if beside {
        ui.gfx.fill_rect(
            Rect::new(panel.x, panel.y, 2.0 * radius, panel.h),
            0.0,
            t.surface,
        );
        // The card's outline on the right, which the surface covers.
        ui.push_clip(Rect::new(
            panel.x + radius,
            panel.y - 1.0,
            panel.w - radius + 1.0,
            panel.h + 2.0,
        ));
        ui.gfx.stroke_rect(panel, radius, 1.0, t.line);
        ui.pop_clip();
        ui.gfx
            .fill_rect(Rect::new(panel.x, panel.y, 1.0, panel.h), 0.0, t.line);
    } else {
        ui.gfx.stroke_rect(panel, radius, 1.0, t.line);
    }
}
