//! What the vault's list screens share: the page margins, a header with a title, count,
//! subtitle and actions, list rows, settings cards and the empty state, so every screen
//! looks like one product.

use annulus_config::tokens::space;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Rect};
use annulus_ui::icons::IconId;
use annulus_ui::kit::{Cursor, Id, Response, Role, Ui};
use annulus_ui::motion::{Curve, Feel, Layer, Motion};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, CONTROL, HEADING, Kind, SMALL, TITLE, button_width};

/// Side padding of a page.
pub const PAD: f32 = 28.0;
/// Height of a list row.
pub const ROW_H: f32 = 56.0;

/// `n` of something in words: "no hosts", "1 host", "3 hosts".
#[must_use]
pub fn count(n: usize, one: &str, many: &str) -> String {
    match n {
        0 => format!("no {many}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// The page's content area inside `rect`.
#[must_use]
pub fn page(rect: Rect) -> Rect {
    let mut inner = rect.inset(PAD, 0.0);
    inner.take_top(space::XL);
    inner
}

/// How many things a page's title stands for, set quietly beside it.
const TITLE_COUNT: annulus_ui::gfx::Font = annulus_ui::gfx::Font::ui(15.0);

/// A page's title with the number of things under it set quietly beside it, taken from the
/// left of `row`.
pub fn title(ui: &mut Ui<'_>, row: &mut Rect, title: &str, count: Option<usize>) {
    let t = ui.theme;
    let width = ui.gfx.measure(title, TITLE).width;
    let head = row.take_left(width + 2.0);
    ui.strict_text(head, title, TITLE, t.text, Align::Start);
    row.take_left(space::S);
    if let Some(count) = count.filter(|count| *count > 0) {
        let label = count.to_string();
        let w = ui.gfx.measure(&label, TITLE_COUNT).width + 2.0;
        // On the title's baseline, near enough for the eye.
        let beside = row.take_left(w);
        let beside = Rect::new(beside.x, beside.y + 3.0, beside.w, beside.h);
        ui.gfx
            .text(beside, &label, TITLE_COUNT, t.text3, Align::Start);
        row.take_left(space::S);
    }
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
    self::title(ui, &mut row, title, count);
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
    let width = button_width(&mut ui.gfx, label, icon);
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
    ui.card_surface(rect, 10.0);
    ui.gfx.fill_rect(rect, 10.0, t.hover.alpha(hover));
    if selected {
        ui.gfx.fill_rect(rect, 10.0, t.selected);
        ui.gfx.stroke_rect(rect, 10.0, 1.0, t.accent.alpha(0.55));
    }
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
    // Rows hand over what is left inside their own padding.
    let mut inner = rect;
    let tile = inner.take_left(32.0).centered(32.0, 32.0);
    ui.gfx.fill_rect(tile, 8.0, t.hover);
    ui.gfx.icon(icon, tile.centered(17.0, 17.0), t.text2);
    inner.take_left(space::M);
    inner
}

/// Two lines of text, a title and a detail, centered in `rect`.
pub fn two_lines(ui: &mut Ui<'_>, rect: Rect, title: &str, detail: &str, mono_detail: bool) {
    let t = ui.theme;
    let top = Rect::new(rect.x, rect.center().y - 19.0, rect.w, 20.0);
    let bottom = Rect::new(rect.x, rect.center().y + 1.0, rect.w, 18.0);
    ui.gfx.text(top, title, BODY_MEDIUM, t.text, Align::Start);
    let (font, ink) = if mono_detail {
        (annulus_ui::gfx::Font::mono(11.5), t.text3)
    } else {
        (SMALL, t.text2)
    };
    ui.gfx.text(bottom, detail, font, ink, Align::Start);
}

/// A titled card of settings rows; returns the inside, `height` tall, and moves `rect` past it.
pub fn card(ui: &mut Ui<'_>, rect: &mut Rect, title: &str, height: f32) -> Rect {
    ui.section_label(rect.take_top(18.0), title);
    rect.take_top(space::S);
    let card = rect.take_top(height);
    ui.card_surface(card, 12.0);
    rect.take_top(space::XL);
    card.inset(space::L, space::S)
}

/// A card on a page that unfolds into place and folds away again, with the page under it
/// gliding to make room. Holds what the card shows: what is open, or for a moment after it
/// closes what was, drawn once more to go (and wiped of anything secret first).
pub struct Fold<T> {
    now: Option<T>,
    going: Option<T>,
    /// How tall it drew last, all open.
    natural: f32,
    /// Counts what replaced what was open, which fades in where it stands.
    swaps: u32,
}

impl<T> Default for Fold<T> {
    fn default() -> Self {
        Self {
            now: None,
            going: None,
            natural: 0.0,
            swaps: 0,
        }
    }
}

impl<T> Fold<T> {
    /// Unfolds with `value`; if one is open, `value` takes its place.
    pub fn open(&mut self, value: T) {
        if self.now.is_some() {
            self.swaps += 1;
        }
        self.now = Some(value);
        self.going = None;
    }

    /// Folds away what is open.
    pub fn close(&mut self) {
        self.close_with(|_| {});
    }

    /// Folds away what is open, after `forget` takes from it what must not stay (a secret, a
    /// running task): it is drawn a little longer, inert.
    pub fn close_with(&mut self, forget: impl FnOnce(&mut T)) {
        if let Some(mut value) = self.now.take() {
            forget(&mut value);
            self.going = Some(value);
        }
        self.swaps = 0;
    }

    /// What is open.
    #[must_use]
    pub fn get(&self) -> Option<&T> {
        self.now.as_ref()
    }

    /// What is open.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.now.as_mut()
    }

    /// Draws it into the top of `area` with `each`, taking from `area` as much as has
    /// unfolded so far. Call it every frame the page is drawn, open or not, so that it knows
    /// when it opens. `each` takes from the top of the rect it is given.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        id: Id,
        area: &mut Rect,
        each: impl FnOnce(&mut Ui<'_>, &mut Rect, &mut T),
    ) {
        let open = self.now.is_some();
        let motion = if open {
            Motion::SLOW
        } else {
            Motion::EXIT_SLOW
        };
        let shown = ui.presence(id, open, motion);
        if !open && shown <= 0.0 {
            self.going = None;
            return;
        }
        let Some(value) = self.now.as_mut().or(self.going.as_mut()) else {
            return;
        };
        let tall = ui.spring_with(id.with("tall"), self.natural, Feel::Soft);
        let swapped = ui.entrance_if(
            id.with(("swap", self.swaps)),
            Motion::BASE,
            open && self.swaps > 0,
        );
        // At rest it is an ordinary card, with room for its focus rings.
        if open && shown >= 1.0 && swapped >= 1.0 && (tall - self.natural).abs() < 0.5 {
            let before = area.h;
            each(ui, area, value);
            self.natural = before - area.h;
            return;
        }
        let mut inside = *area;
        let height = tall.min(area.h) * shown;
        let window = Rect::new(area.x - space::S, area.y, area.w + 2.0 * space::S, height);
        ui.push_clip(window);
        // It comes down out of the page as it opens; what it shows is ahead of its height.
        let alpha = (shown * 1.6).min(1.0) * swapped;
        ui.push_layer(
            Layer::fade(alpha)
                .shifted(0.0, -12.0 * (1.0 - shown))
                .grouped(window),
        );
        if !open {
            ui.push_inert();
        }
        each(ui, &mut inside, value);
        if !open {
            ui.pop_inert();
        }
        ui.pop_layer();
        ui.pop_clip();
        let natural = area.h - inside.h;
        if self.natural <= 0.0 {
            ui.spring_set(id.with("tall"), natural);
        }
        self.natural = natural;
        area.take_top(height);
    }
}

/// What a row of a list shows, kept so that it can still show it on its way out.
#[derive(Clone)]
pub struct RowLook {
    /// Which row it is, for as long as the list has it.
    pub key: u64,
    /// The tile at its left.
    pub icon: IconId,
    /// Its first line.
    pub title: String,
    /// Its second line.
    pub detail: String,
    /// The second line is a key, a command or the like.
    pub mono: bool,
}

/// How long a row takes to leave a list, in seconds.
const ROW_LEAVES: f64 = 0.22;

/// A list's rows from frame to frame: which came and which went.
#[derive(Default)]
pub struct ListMotion {
    last: Vec<RowLook>,
    /// Rows on their way out: each with its place in the list and since when.
    leaving: Vec<(RowLook, usize, f64)>,
    /// The frame that last drew the list.
    drawn: Option<f64>,
    /// When its rows last changed, for the layout checks.
    #[cfg(test)]
    changed_at: Option<f64>,
}

impl ListMotion {
    /// Whether its rows are gliding to new places at `now`, passing over each other for a
    /// moment on the way.
    #[cfg(test)]
    pub fn gliding(&self, now: f64) -> bool {
        /// Longer than any row takes to arrive.
        const GLIDES: f64 = 0.7;
        self.changed_at.is_some_and(|at| now < at + GLIDES)
    }

    /// Notes this frame's rows; returns whether the list was on screen in the frame before.
    /// If it was not, nothing arrives or leaves: its rows came with their page.
    pub fn begin(&mut self, ui: &Ui<'_>, looks: &[RowLook]) -> bool {
        let on_screen = self
            .drawn
            .is_some_and(|drawn| drawn + 1e-9 >= ui.frame_before());
        if !on_screen || ui.state.reduce_motion {
            self.leaving.clear();
        } else {
            for (place, look) in self.last.iter().enumerate() {
                if looks.iter().all(|row| row.key != look.key) {
                    self.leaving.push((look.clone(), place, ui.time()));
                }
            }
        }
        #[cfg(test)]
        if on_screen
            && self
                .last
                .iter()
                .map(|row| row.key)
                .ne(looks.iter().map(|row| row.key))
        {
            self.changed_at = Some(ui.time());
        }
        self.last = looks.to_vec();
        self.drawn = Some(ui.time());
        on_screen
    }

    /// Draws the rows on their way out in the list at `rect`, scrolled by `offset`, a row
    /// every `step`.
    pub fn draw_leaving(&mut self, ui: &mut Ui<'_>, rect: Rect, offset: f32, step: f32) {
        let now = ui.time();
        self.leaving
            .retain(|(_, _, since)| now - *since < ROW_LEAVES);
        if self.leaving.is_empty() {
            return;
        }
        ui.push_inert();
        for (look, place, since) in &self.leaving {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = ((now - *since) / ROW_LEAVES) as f32;
            let left = 1.0 - Curve::Exit.at(share);
            #[allow(clippy::cast_precision_loss)] // Lists are short.
            let y = rect.y - offset + *place as f32 * step;
            let row = Rect::new(rect.x, y, rect.w, ROW_H);
            ui.push_layer(
                Layer::fade(left)
                    .scaled(row.center(), 0.97 + 0.03 * left)
                    .grouped(row.outset(2.0)),
            );
            ui.card_surface(row, 10.0);
            let inner = row_icon(ui, row.inset(space::M, 0.0), look.icon);
            two_lines(ui, inner, &look.title, &look.detail, look.mono);
            ui.pop_layer();
        }
        ui.pop_inert();
        ui.request_repaint();
    }

    /// The list has no rows left: only those on their way out, over its empty state.
    pub fn empty(&mut self, ui: &mut Ui<'_>, rect: Rect, step: f32) {
        self.begin(ui, &[]);
        self.draw_leaving(ui, rect, 0.0, step);
    }
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

/// A card coming in over `area`: the scrim fades in and the card at `card` grows into place,
/// passing its size a touch. Draw the card after this, then call [`card_done`].
pub fn card_in(ui: &mut Ui<'_>, id: Id, area: Rect, card: Rect) {
    let t = ui.theme;
    let open = ui.entrance(id.with("card.open"), Motion::POP);
    ui.gfx
        .fill_rect(area, 0.0, t.scrim.alpha(open.clamp(0.0, 1.0)));
    ui.push_layer(Layer::appear(open, card.center(), 0.97, 10.0).grouped(card.outset(48.0)));
}

/// Ends what [`card_in`] began.
pub fn card_done(ui: &mut Ui<'_>) {
    ui.pop_layer();
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
    let shown = ui.entrance(Id::new(("empty", title)), Motion::BASE);
    ui.push_layer(Layer::appear(shown, block.center(), 0.98, 8.0).grouped(block.outset(8.0)));
    let mut column = block;
    let badge = column.take_top(64.0).centered(52.0, 52.0);
    ui.gfx.fill_rect(badge, 14.0, t.hover);
    ui.gfx.icon(icon, badge.centered(24.0, 24.0), t.text2);
    column.take_top(space::L);
    ui.strict_text(column.take_top(24.0), title, HEADING, t.text, Align::Center);
    column.take_top(space::XS);
    ui.gfx
        .text(column.take_top(20.0), line, BODY, t.text2, Align::Center);
    ui.pop_layer();
}
