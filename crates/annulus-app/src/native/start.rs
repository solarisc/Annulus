//! The New Tab page (F-APP-004, UI-SPEC §1): what + and Ctrl+Shift+T open. A search over the
//! saved hosts takes the keyboard at once; under it this device's recent connections, then
//! saved hosts newest first to fill the page, and quick ways to a local shell, SFTP or a new
//! host. Choosing one turns the tab into that session.

use annulus_config::settings::Recent;
use annulus_config::tokens::space;
use annulus_session::Target;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, FieldStyle, HEADING, KEYCAP_H, Kind, SMALL, TITLE, button_width, keys_width,
};
use annulus_vault::hosts::Host;

use super::os;

/// At most this many recent connections head the list.
pub const RECENT_SHOWN: usize = 6;
/// The widest the page's column gets.
const WIDTH: f32 = 720.0;
/// Height of one row.
const ROW_H: f32 = 52.0;
/// Height of a section's label.
const LABEL_H: f32 = 34.0;

/// What the page asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Connect to a saved host.
    Host(u64),
    /// Connect to a typed `user@host:port`.
    Quick(String),
    /// Open the default local shell.
    Local,
    /// Open the SFTP tab.
    Sftp,
    /// Add a host in the vault.
    NewHost,
    /// Bring hosts in from `~/.ssh/config` (the welcome of an empty vault).
    Import,
    /// Pin saved host `id` to the top of the page, or unpin it; the page stays.
    TogglePin(u64),
}

/// The hosts the page lists.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Pinned hosts that still exist, in the order pinned.
    pub pinned: Vec<u64>,
    /// Recent connections that still name a saved host and are not pinned: id and when, the
    /// latest first.
    pub recent: Vec<(u64, u64)>,
    /// Then saved hosts, the newest first, none of them pinned or among the recent ones.
    pub saved: Vec<u64>,
}

/// What the page lists for `query`: the pinned hosts, up to [`RECENT_SHOWN`] recent
/// connections, then saved hosts newest first. With no query the saved hosts fill what is left
/// of `room` rows, so the page is never half empty; a query lists every host that matches.
#[must_use]
pub fn listing(
    (recent, pinned): (&[Recent], &[u64]),
    hosts: &[Host],
    query: &str,
    room: usize,
) -> Listing {
    let host = |id: u64| hosts.iter().find(|host| host.id == id);
    let pinned: Vec<u64> = pinned
        .iter()
        .copied()
        .filter(|id| host(*id).is_some_and(|host| host.matches(query)))
        .collect();
    let recent: Vec<(u64, u64)> = recent
        .iter()
        .filter(|seen| {
            !pinned.contains(&seen.host) && host(seen.host).is_some_and(|host| host.matches(query))
        })
        .take(RECENT_SHOWN)
        .map(|seen| (seen.host, seen.at))
        .collect();
    let mut saved: Vec<&Host> = hosts
        .iter()
        .filter(|host| {
            host.matches(query)
                && !pinned.contains(&host.id)
                && !recent.iter().any(|(id, _)| *id == host.id)
        })
        .collect();
    // Ids only grow, so the highest is the newest saved on this device.
    saved.sort_by_key(|host| std::cmp::Reverse(host.id));
    let keep = if query.trim().is_empty() {
        room.saturating_sub(recent.len() + pinned.len())
    } else {
        saved.len()
    };
    Listing {
        pinned,
        recent,
        saved: saved.into_iter().take(keep).map(|host| host.id).collect(),
    }
}

/// How long ago `at` was at `now` (both in seconds since the epoch), in a few words.
#[must_use]
pub fn ago(at: u64, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let gone = now.saturating_sub(at);
    match gone {
        0..MINUTE => "just now".to_owned(),
        MINUTE..HOUR => format!("{} min ago", gone / MINUTE),
        HOUR..DAY => format!("{} h ago", gone / HOUR),
        _ if gone < 2 * DAY => "yesterday".to_owned(),
        _ if gone < 14 * DAY => format!("{} days ago", gone / DAY),
        _ if gone < 60 * DAY => format!("{} weeks ago", gone / (7 * DAY)),
        _ => format!("{} months ago", gone / (30 * DAY)),
    }
}

/// One row of the list.
struct Row {
    choice: Choice,
    title: String,
    detail: String,
    os: Option<&'static os::Os>,
    when: Option<String>,
    /// For a saved host: whether it is pinned (its pin shows at all times, lit).
    pinned: Option<bool>,
}

/// The New Tab page's state.
#[derive(Default)]
pub struct StartView {
    /// The search.
    pub query: String,
    /// The row the keyboard is on.
    highlight: usize,
    /// The search was given the keyboard.
    focused: bool,
}

impl StartView {
    /// Draws the page into `rect` for tab `uid`; returns what was chosen. For the SFTP tab
    /// (`files`) it picks a host whose files to show, from every saved host.
    #[allow(clippy::too_many_lines)] // The page top to bottom: search, actions, the list.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        uid: u64,
        (hosts, recent, pinned, now): (&[Host], &[Recent], &[u64], u64),
        files: bool,
    ) -> Option<Choice> {
        let t = ui.theme;
        let base = Id::new(("start", uid));
        let shown = ui.entrance(base.with("enter"), Motion::BASE);
        ui.push_layer(Layer::fade(shown).shifted(0.0, 10.0 * (1.0 - shown)));
        let width = (rect.w - 2.0 * space::XL).clamp(0.0, WIDTH);
        let mut column = Rect::new(rect.x + (rect.w - width) / 2.0, rect.y, width, rect.h);
        column.take_top(space::XL + space::S);
        let mut chosen = None;
        if files {
            ui.strict_text(
                column.take_top(28.0),
                "Browse a host's files",
                TITLE,
                t.text,
                Align::Start,
            );
            ui.gfx.text(
                column.take_top(20.0),
                "Pick a host: its files open beside this computer's, to drag between them.",
                BODY,
                t.text2,
                Align::Start,
            );
            column.take_top(space::L);
        }

        // The search, a size up from a normal field: the page is about it.
        let search_id = base.with("search");
        if !self.focused {
            ui.state.set_focus(Some(search_id), false);
            self.focused = true;
        }
        let search = column.take_top(44.0);
        let field = ui.text_field(
            search_id,
            search,
            &mut self.query,
            FieldStyle {
                placeholder: if files {
                    "Search saved hosts"
                } else {
                    "Search saved hosts, or type user@host"
                },
                icon: Some(icons::SEARCH),
                hint: if files {
                    "Enter opens"
                } else {
                    "Enter connects"
                },
                ..FieldStyle::default()
            },
        );
        if field.changed {
            self.highlight = 0;
        }
        column.take_top(space::M);

        // Quick ways out, as buttons the keyboard reaches after the search.
        if !files {
            let mut actions = column.take_top(32.0);
            for (id, label, icon, choice) in [
                ("local", "Local terminal", icons::TERMINAL, Choice::Local),
                ("sftp", "SFTP", icons::FOLDER, Choice::Sftp),
                ("new", "New host", icons::PLUS, Choice::NewHost),
            ] {
                let w = button_width(&mut ui.gfx, label, Some(icon));
                let slot = actions.take_left(w);
                actions.take_left(space::S);
                if ui
                    .button(base.with(id), slot, label, Some(icon), Kind::Secondary)
                    .clicked
                {
                    chosen = Some(choice);
                }
            }
        }
        column.take_top(space::L);

        // The SFTP tab lists every host; a new tab fills what one page holds.
        let room = if files {
            usize::MAX
        } else {
            room_for(column.h)
        };
        let found = listing((recent, pinned), hosts, &self.query, room);
        let host = |id: u64| hosts.iter().find(|host| host.id == id);
        let row_for = |id: u64, when: Option<String>| {
            host(id).map(|host| Row {
                choice: Choice::Host(id),
                title: host.label.clone(),
                detail: host.target(),
                os: os::of(host),
                when,
                // The SFTP tab's picker pins nothing.
                pinned: (!files).then(|| pinned.contains(&id)),
            })
        };
        let pinned_rows: Vec<Row> = found
            .pinned
            .iter()
            .filter_map(|id| row_for(*id, None))
            .collect();
        let recent_rows: Vec<Row> = found
            .recent
            .iter()
            .filter_map(|(id, at)| row_for(*id, Some(ago(*at, now))))
            .collect();
        let saved_rows: Vec<Row> = found
            .saved
            .iter()
            .filter_map(|id| row_for(*id, None))
            .collect();
        let query = self.query.trim();
        // A typed target no saved host matches: a one-time connection, under its own heading.
        let mut quick_rows = Vec::new();
        if !files
            && pinned_rows.is_empty()
            && recent_rows.is_empty()
            && saved_rows.is_empty()
            && Target::parse(query, "").is_some()
        {
            quick_rows.push(Row {
                choice: Choice::Quick(query.to_owned()),
                title: format!("Connect to {query}"),
                detail: "Not saved: a one-time SSH connection".to_owned(),
                os: None,
                when: None,
                pinned: None,
            });
        }
        let count = pinned_rows.len() + recent_rows.len() + saved_rows.len() + quick_rows.len();
        self.highlight = self.highlight.min(count.saturating_sub(1));
        if ui.key(Key::Down, Modifiers::NONE) && count > 0 {
            self.highlight = (self.highlight + 1).min(count - 1);
        }
        if ui.key(Key::Up, Modifiers::NONE) {
            self.highlight = self.highlight.saturating_sub(1);
        }

        if count == 0 && hosts.is_empty() && !files && query.is_empty() {
            chosen = welcome(ui, base, column).or(chosen);
        } else if count == 0 {
            let line = if hosts.is_empty() {
                "Saved hosts and your recent connections show here."
            } else {
                "No saved host matches. Type user@host to connect to one once."
            };
            empty(ui, column, line);
        } else {
            let sections = [
                ("Pinned", pinned_rows),
                ("Recent connections", recent_rows),
                (
                    if query.is_empty() && !files {
                        "Newest saved hosts"
                    } else {
                        "Saved hosts"
                    },
                    saved_rows,
                ),
                ("Quick connect", quick_rows),
            ];
            #[allow(clippy::cast_precision_loss)] // Rows on one page.
            let content: f32 = sections
                .iter()
                .filter(|(_, rows)| !rows.is_empty())
                .map(|(_, rows)| LABEL_H + rows.len() as f32 * ROW_H + space::M)
                .sum();
            let offset = ui.scroll_view(base.with("scroll"), column, content);
            ui.push_clip(column);
            let mut y = column.y - offset;
            // Where each row is, for the highlight to glide to.
            let mut places = Vec::new();
            let mut index = 0;
            let mut layout = Vec::new();
            for (title, rows) in &sections {
                if rows.is_empty() {
                    continue;
                }
                layout.push((Rect::new(column.x, y, column.w, LABEL_H), *title, rows));
                y += LABEL_H;
                for _ in rows {
                    places.push(Rect::new(column.x, y, column.w - 8.0, ROW_H - 4.0));
                    y += ROW_H;
                }
                y += space::M;
            }
            // The highlight, one pill gliding from row to row under the pointer or the arrows.
            if let Some(target) = places.get(self.highlight) {
                let at = ui.spring(base.with("glide"), target.y);
                let pill = Rect::new(target.x, at, target.w, target.h);
                ui.gfx.fill_rect(pill, 10.0, t.selected);
                ui.gfx.stroke_rect(pill, 10.0, 1.0, t.accent.alpha(0.35));
            }
            for (label, title, rows) in layout {
                // In line with the rows' tiles.
                let line = Rect::new(label.x + space::M, label.y + 8.0, label.w, 18.0);
                ui.section_label(line, &format!("{title} \u{b7} {}", rows.len()));
                for row in rows {
                    let place = places[index];
                    let enter = ui.entrance(
                        base.with(("row", index, &row.title)),
                        Motion::SLOW.staggered(index),
                    );
                    // Grown in place, never slid: a row never covers its neighbour, even when a
                    // search moves it.
                    ui.push_layer(Layer::appear(enter, place.center(), 0.94, 0.0));
                    let highlighted = index == self.highlight;
                    match draw_row(ui, base, place, row, highlighted) {
                        RowEvent::Chosen => chosen = Some(row.choice.clone()),
                        RowEvent::Hovered => self.highlight = index,
                        RowEvent::Pin => {
                            if let Choice::Host(id) = row.choice {
                                chosen = Some(Choice::TogglePin(id));
                            }
                        }
                        RowEvent::None => {}
                    }
                    ui.pop_layer();
                    index += 1;
                }
            }
            ui.pop_clip();
            if field.submitted {
                let all: Vec<&Choice> = sections
                    .iter()
                    .flat_map(|(_, rows)| rows.iter().map(|row| &row.choice))
                    .collect();
                chosen = all.get(self.highlight).map(|choice| (*choice).clone());
            }
        }
        ui.pop_layer();
        chosen
    }
}

/// How many rows the list shows without scrolling in `height`, the sections' labels counted.
fn room_for(height: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small, positive.
    let rows = ((height - 3.0 * (LABEL_H + space::M)) / ROW_H)
        .floor()
        .max(0.0) as usize;
    rows
}

enum RowEvent {
    None,
    Hovered,
    Chosen,
    /// Its pin was clicked.
    Pin,
}

fn draw_row(ui: &mut Ui<'_>, base: Id, rect: Rect, row: &Row, highlighted: bool) -> RowEvent {
    let t = ui.theme;
    let id = base.with(("pick", &row.choice_key()));
    let response = ui.zone(id, rect, Role::ListItem, &row.title);
    if response.focus_ring {
        ui.focus_ring(rect, 10.0);
    }
    let mut inner = rect.inset(space::M, 0.0);
    // A saved host's pin, at its right end: always on a pinned host, else while the row is
    // under the pointer or the keyboard.
    let pin_id = id.with("pin");
    let pin_slot = row
        .pinned
        .map(|_| inner.take_right(28.0).centered(28.0, 28.0));
    let mut pin_clicked = false;
    if let (Some(slot), Some(pinned)) = (pin_slot, row.pinned) {
        inner.take_right(space::S);
        let near = response.hovered || highlighted || ui.hover_within(rect);
        if pinned || near {
            let name = if pinned {
                "Unpin from New Tab"
            } else {
                "Pin to the top of New Tab"
            };
            let pin = ui.zone(pin_id, slot, Role::Button, name);
            ui.tooltip(pin_id, slot, name);
            if pin.hovered {
                ui.gfx.fill_rect(slot, 7.0, t.hover);
                ui.set_cursor(Cursor::Pointer);
            }
            if pin.focus_ring {
                ui.focus_ring(slot, 7.0);
            }
            let ink = if pinned { t.accent } else { t.text2 };
            ui.gfx.icon(icons::PIN, slot.centered(15.0, 15.0), ink);
            pin_clicked = pin.clicked;
        }
    }
    let tile = inner.take_left(32.0).centered(32.0, 32.0);
    if matches!(row.choice, Choice::Quick(_)) {
        ui.gfx.fill_rect(tile, 9.0, t.accent.alpha(0.16));
        ui.gfx
            .icon(icons::TERMINAL, tile.centered(18.0, 18.0), t.accent);
    } else {
        os::tile(ui, tile, &row.title, row.os);
    }
    inner.take_left(space::M);
    // Right side: when it was used, and Enter on the highlighted row. Every row keeps Enter's
    // room, so the times line up in one column.
    let width = keys_width(&mut ui.gfx, "Enter");
    let keys = inner.take_right(width);
    inner.take_right(space::M);
    if highlighted {
        ui.keys(
            Point::new(keys.x, keys.center().y - KEYCAP_H / 2.0),
            "Enter",
        );
    }
    if let Some(when) = &row.when {
        let width = ui.gfx.measure(when, SMALL).width;
        let hint = inner.take_right(width);
        ui.gfx.text(hint, when, SMALL, t.text2, Align::End);
        inner.take_right(space::S);
    }
    let name = Rect::new(inner.x, rect.center().y - 19.0, inner.w, 20.0);
    let detail = Rect::new(inner.x, rect.center().y + 1.0, inner.w, 18.0);
    ui.gfx
        .text(name, &row.title, BODY_STRONG, t.text, Align::Start);
    ui.gfx
        .text(detail, &row.detail, SMALL, t.text2, Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    if pin_clicked {
        RowEvent::Pin
    } else if response.clicked {
        RowEvent::Chosen
    } else if response.hovered {
        RowEvent::Hovered
    } else {
        RowEvent::None
    }
}

impl Row {
    /// What tells rows apart for their zone ids.
    fn choice_key(&self) -> String {
        match &self.choice {
            Choice::Host(id) => format!("host-{id}"),
            Choice::Quick(target) => format!("quick-{target}"),
            other => format!("{other:?}"),
        }
    }
}

/// What a new tab shows while the vault holds no host yet: where to start. Returns what was
/// chosen.
fn welcome(ui: &mut Ui<'_>, base: Id, rect: Rect) -> Option<Choice> {
    let t = ui.theme;
    let mut column = Rect::new(rect.x, rect.y + space::L, rect.w, 220.0);
    let badge = column.take_top(56.0).centered(56.0, 56.0);
    ui.gfx.fill_rect(badge, 16.0, t.accent.alpha(0.14));
    ui.gfx
        .icon(icons::SERVER, badge.centered(24.0, 24.0), t.accent);
    column.take_top(space::L);
    ui.strict_text(
        column.take_top(24.0),
        "Welcome to Annulus",
        HEADING,
        t.text,
        Align::Center,
    );
    column.take_top(space::XS);
    for line in [
        "Bring in the servers you already use from ~/.ssh/config,",
        "add one by hand, or type user@host above to connect once.",
    ] {
        ui.gfx
            .text(column.take_top(20.0), line, BODY, t.text2, Align::Center);
    }
    column.take_top(space::L);
    let import_w = button_width(&mut ui.gfx, "Import ssh_config", Some(icons::DOWNLOAD));
    let new_w = button_width(&mut ui.gfx, "New host", Some(icons::PLUS));
    let mut line = column
        .take_top(32.0)
        .centered(import_w + space::S + new_w, 32.0);
    let import = line.take_left(import_w);
    line.take_left(space::S);
    let mut chosen = None;
    if ui
        .button(
            base.with("welcome.import"),
            import,
            "Import ssh_config",
            Some(icons::DOWNLOAD),
            Kind::Primary,
        )
        .clicked
    {
        chosen = Some(Choice::Import);
    }
    if ui
        .button(
            base.with("welcome.new"),
            line,
            "New host",
            Some(icons::PLUS),
            Kind::Secondary,
        )
        .clicked
    {
        chosen = Some(Choice::NewHost);
    }
    chosen
}

/// What the page shows when it lists nothing.
fn empty(ui: &mut Ui<'_>, rect: Rect, line: &str) {
    let t = ui.theme;
    let block = Rect::new(rect.x, rect.y + space::XL, rect.w, 140.0);
    let mut column = block;
    let badge = column.take_top(56.0).centered(56.0, 56.0);
    ui.gfx.fill_rect(badge, 16.0, t.surface.mix(t.hover, 0.5));
    ui.gfx.stroke_rect(badge, 16.0, 1.0, t.line_strong);
    ui.gfx
        .icon(icons::SERVER, badge.centered(24.0, 24.0), t.text2);
    column.take_top(space::L);
    ui.strict_text(
        column.take_top(24.0),
        "Nothing to show yet",
        HEADING,
        t.text,
        Align::Center,
    );
    column.take_top(space::XS);
    ui.gfx
        .text(column.take_top(20.0), line, BODY, t.text2, Align::Center);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo hosts, from a vault in a folder of `name`'s own (tests run side by side).
    fn hosts(name: &str) -> Vec<Host> {
        let vault = super::super::demo::fixture_vault(name, true);
        vault.hosts().to_vec()
    }

    #[test]
    fn recent_connections_come_first_then_the_newest_saved_hosts_fill_the_room() {
        let hosts = hosts("start-room");
        let id = |label: &str| hosts.iter().find(|host| host.label == label).unwrap().id;
        let recent = [
            Recent {
                host: id("grafana"),
                at: 300,
            },
            // A host deleted since is left out.
            Recent {
                host: 9999,
                at: 200,
            },
            Recent {
                host: id("prod-db"),
                at: 100,
            },
        ];
        let found = listing((&recent, &[]), &hosts, "", 6);
        assert_eq!(found.recent, [(id("grafana"), 300), (id("prod-db"), 100)]);
        assert_eq!(found.saved.len(), 4, "the rest of the room");
        // Newest first, and never one already among the recent.
        assert_eq!(found.saved[0], id("router"));
        assert!(found.saved.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(!found.saved.contains(&id("grafana")));
        // No room: the recent ones still show.
        let tight = listing((&recent, &[]), &hosts, "", 1);
        assert_eq!((tight.recent.len(), tight.saved.len()), (2, 0));
    }

    #[test]
    fn pinned_hosts_head_the_page_in_pin_order_and_are_listed_nowhere_else() {
        let hosts = hosts("start-pinned");
        let id = |label: &str| hosts.iter().find(|host| host.label == label).unwrap().id;
        let recent = [
            Recent {
                host: id("grafana"),
                at: 300,
            },
            Recent {
                host: id("prod-db"),
                at: 100,
            },
        ];
        // Pinned last first; one deleted since is left out.
        let pinned = [id("router"), 9999, id("grafana")];
        let found = listing((&recent, &pinned), &hosts, "", 6);
        assert_eq!(found.pinned, [id("router"), id("grafana")]);
        assert_eq!(
            found.recent,
            [(id("prod-db"), 100)],
            "not again among the recent"
        );
        assert!(!found.saved.contains(&id("router")) && !found.saved.contains(&id("grafana")));
        assert_eq!(
            found.pinned.len() + found.recent.len() + found.saved.len(),
            6,
            "together they fill the room"
        );
        // A search narrows the pinned too.
        let searched = listing((&recent, &pinned), &hosts, "graf", 6);
        assert_eq!(searched.pinned, [id("grafana")]);
        let mut settings = annulus_config::settings::Settings::default();
        settings.toggle_pin(4);
        settings.toggle_pin(9);
        settings.toggle_pin(4);
        assert_eq!(settings.pinned, [9], "a second click unpins");
    }

    #[test]
    fn a_search_lists_every_match_and_at_most_six_recent() {
        let hosts = hosts("start-search");
        let recent: Vec<Recent> = hosts
            .iter()
            .enumerate()
            .map(|(at, host)| Recent {
                host: host.id,
                at: at as u64,
            })
            .collect();
        assert_eq!(
            listing((&recent, &[]), &hosts, "", 40).recent.len(),
            RECENT_SHOWN
        );
        let found = listing((&[], &[]), &hosts, "api", 2);
        assert_eq!(found.saved.len(), 2, "both api hosts, whatever the room");
        let found = listing((&[], &[]), &hosts, "deploy", 1);
        assert!(found.saved.len() > 1);
    }

    #[test]
    fn times_read_as_a_person_would_say_them() {
        let now = 10_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 4 * 60, now), "4 min ago");
        assert_eq!(ago(now - 3 * 3600, now), "3 h ago");
        assert_eq!(ago(now - 30 * 3600, now), "yesterday");
        assert_eq!(ago(now - 5 * 86_400, now), "5 days ago");
        assert_eq!(ago(now - 21 * 86_400, now), "3 weeks ago");
        assert_eq!(ago(now - 95 * 86_400, now), "3 months ago");
        // A clock that went back is "just now", not a panic.
        assert_eq!(ago(now + 50, now), "just now");
    }
}
