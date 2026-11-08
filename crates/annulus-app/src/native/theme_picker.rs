//! The theme browser: searchable families, independent variant filters and a live preview.

mod catalog;
mod preview;
use catalog::{Entry, Filter, catalog};

use annulus_config::{settings::Settings, tokens::ThemeMode};
use annulus_ui::gfx::{Align, Canvas as _, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::widgets::{BODY, BODY_MEDIUM, CONTROL, FieldStyle, HEADING, Kind, SMALL};

use super::{colors, pages};

const ROW: f32 = 44.0;
const GROUP: f32 = 28.0;

/// The modal's temporary state. Preferences are written only after Apply.
#[derive(Default)]
pub struct ThemePicker {
    query: String,
    filter: Filter,
    selected: usize,
    opened: bool,
    previous_focus: Option<Id>,
    preview: colors::ThemeFade,
    terminal_fade: preview::TerminalFade,
    terminal: Option<annulus_term::Emulator>,
    reveal: bool,
    pointer: Option<annulus_ui::gfx::Point>,
    focus: Option<Id>,
    hovered: Option<(usize, f64)>,
    recent: Vec<String>,
    closing: Option<(Outcome, f64)>,
}

/// How the browser closed.
#[derive(Clone, Copy)]
pub enum Outcome {
    /// Keep the saved preference.
    Cancel,
    /// Save this index in `colors::app_schemes`.
    Apply(usize),
}

impl ThemePicker {
    /// Opens at the saved scheme and the current mode's variant filter.
    pub fn new(settings: &Settings) -> Self {
        Self {
            selected: colors::app_schemes()
                .iter()
                .position(|name| {
                    name.as_str()
                        == settings
                            .app_scheme
                            .as_deref()
                            .unwrap_or(match settings.theme {
                                ThemeMode::Light => colors::OWN_LIGHT,
                                ThemeMode::Dark => colors::OWN_DARK,
                                ThemeMode::System => colors::OWN_COLORS,
                            })
                })
                .unwrap_or(0),
            filter: match settings.theme {
                ThemeMode::System => Filter::All,
                ThemeMode::Light => Filter::Light,
                ThemeMode::Dark => Filter::Dark,
            },
            recent: settings.recent_themes.iter().take(6).cloned().collect(),
            reveal: true,
            ..Self::default()
        }
    }

    fn results(&self) -> Vec<&'static Entry> {
        let query = self.query.to_lowercase();
        let mut results: Vec<_> = catalog()
            .iter()
            .filter(|entry| {
                let variant = match entry.dark {
                    Some(true) => "dark",
                    Some(false) => "light",
                    None => "system light dark",
                };
                (entry.dark.is_none()
                    || self.filter == Filter::All
                    || entry.dark == Some(self.filter == Filter::Dark))
                    && query
                        .split_whitespace()
                        .all(|word| entry.search.contains(word) || variant.contains(word))
            })
            .collect();
        results.sort_by_key(|entry| {
            self.recent
                .iter()
                .position(|name| name == &colors::app_schemes()[entry.index])
                .unwrap_or(usize::MAX)
        });
        results
    }

    fn group<'a>(&self, entry: &'a Entry) -> &'a str {
        if self
            .recent
            .iter()
            .any(|name| name == &colors::app_schemes()[entry.index])
        {
            "Recent"
        } else {
            &entry.family
        }
    }

    /// The opener to focus after the background becomes interactive again.
    pub fn return_focus(&self) -> Option<Id> {
        self.previous_focus
    }

    /// Draws over the inert window; the caller restores `return_focus` on the next frame.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        settings: &Settings,
        system: annulus_ui::kit::Theme,
    ) -> Option<Outcome> {
        if !self.opened {
            self.previous_focus = ui.state.focus();
            ui.state.set_focus(Some(Id::new("theme.search")), false);
            self.opened = true;
        }
        if let Some((outcome, since)) = self.closing {
            let share = ((ui.time() - since) / 0.09).clamp(0.0, 1.0);
            if ui.state.reduce_motion || share >= 1.0 {
                return Some(outcome);
            }
            #[allow(clippy::cast_possible_truncation)] // A normalized animation share.
            ui.push_layer(annulus_ui::motion::Layer::fade(1.0 - share as f32).grouped(ui.viewport));
            ui.push_inert();
            ui.request_repaint();
        }
        let window = ui.viewport;
        let card = window.centered((window.w - 40.0).min(1040.0), (window.h - 48.0).min(650.0));
        let outside = ui.zone(Id::new("theme.backdrop"), window, Role::Backdrop, "");
        let cancel = outside.clicked || ui.key(Key::Escape, Modifiers::NONE);
        pages::card_in(ui, Id::new("theme.picker"), window, card);
        ui.raised_surface(card, 14.0);
        ui.zone(
            Id::new("theme.card"),
            card,
            Role::Backdrop,
            "Choose a theme",
        );
        let outcome = self.contents(ui, card.inset(20.0, 20.0), settings, system);
        pages::card_done(ui);
        let outcome = if cancel {
            Some(Outcome::Cancel)
        } else {
            outcome
        };
        if self.closing.is_some() {
            ui.pop_inert();
            ui.pop_layer();
        } else if let Some(outcome) = outcome {
            self.closing = Some((outcome, ui.time()));
            ui.request_repaint();
        }
        None
    }

    #[allow(clippy::too_many_lines)] // The modal's header, filters, columns and footer.
    fn contents(
        &mut self,
        ui: &mut Ui<'_>,
        mut area: Rect,
        settings: &Settings,
        system: annulus_ui::kit::Theme,
    ) -> Option<Outcome> {
        let t = ui.theme;
        let mut head = area.take_top(32.0);
        let close = head.take_right(72.0);
        ui.strict_text(head, "Choose a theme", HEADING, t.text, Align::Start);
        let cancel = ui
            .button(Id::new("theme.close"), close, "Close", None, Kind::Ghost)
            .clicked;
        area.take_top(12.0);
        let field = ui.text_field(
            Id::new("theme.search"),
            area.take_top(CONTROL),
            &mut self.query,
            FieldStyle {
                placeholder: "Search themes, families or authors",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        let changed = field.changed;
        let submitted = field.submitted;
        area.take_top(10.0);
        let mut bar = area.take_top(30.0);
        let mut refilter = changed;
        for (filter, label) in [
            (Filter::All, "All"),
            (Filter::Dark, "Dark"),
            (Filter::Light, "Light"),
        ] {
            let id = Id::new(("theme.filter", label));
            let kind = if self.filter == filter {
                Kind::Primary
            } else {
                Kind::Ghost
            };
            if ui
                .button(id, bar.take_left(62.0), label, None, kind)
                .clicked
            {
                self.filter = filter;
                ui.state.set_focus(Some(Id::new("theme.search")), false);
                refilter = true;
            }
            ui.set_toggled(id, self.filter == filter);
            bar.take_left(4.0);
        }
        let results = self.results();
        ui.gfx.text(
            bar,
            &format!("{} themes", results.len()),
            SMALL,
            t.text2,
            Align::End,
        );
        area.take_top(12.0);
        let footer = area.take_bottom(32.0);
        area.take_bottom(12.0);
        let mut outcome = cancel.then_some(Outcome::Cancel);
        let mut moved = std::mem::take(&mut self.reveal);
        if !results.iter().any(|entry| entry.index == self.selected) {
            self.selected = results.first().map_or(0, |entry| entry.index);
        }
        let at = results
            .iter()
            .position(|entry| entry.index == self.selected)
            .unwrap_or(0);
        let next = if ui.key(Key::Down, Modifiers::NONE) {
            Some((at + 1).min(results.len().saturating_sub(1)))
        } else if ui.key(Key::Up, Modifiers::NONE) {
            Some(at.saturating_sub(1))
        } else {
            None
        };
        if let Some(entry) = next.and_then(|next| results.get(next)) {
            self.selected = entry.index;
            self.hovered = None;
            moved = true;
        }
        let mut list = area;
        let preview = list.take_right((list.w * 0.43).min(410.0));
        list.take_right(16.0);
        let nav = (list.w > 430.0).then(|| {
            let nav = list.take_left(150.0);
            list.take_left(12.0);
            nav
        });
        let mut y = 0.0;
        let mut group = "";
        let mut positions = Vec::with_capacity(results.len());
        let mut groups = Vec::new();
        for entry in &results {
            if self.group(entry) != group {
                group = self.group(entry);
                groups.push((group, y));
                y += GROUP;
            }
            positions.push(y);
            y += ROW;
        }
        let scroll = Id::new("theme.list.scroll");
        if refilter {
            ui.set_scroll_offset(scroll, 0.0);
        }
        if let Some(nav) = nav {
            Self::navigation(ui, nav, &groups, scroll);
        }
        if moved
            && let Some(at) = results
                .iter()
                .position(|entry| entry.index == self.selected)
        {
            let current = ui.scroll_offset(scroll);
            if positions[at] < current {
                ui.set_scroll_offset(scroll, positions[at]);
            } else if positions[at] + ROW > current + list.h {
                ui.set_scroll_offset(scroll, positions[at] + ROW - list.h);
            }
        }
        let offset = ui.scroll_view(scroll, list, y.max(list.h));
        ui.push_list_clip(list, offset, y, 4.0);
        for (group, y) in groups {
            let line = Rect::new(list.x + 8.0, list.y + y - offset, list.w - 16.0, GROUP);
            if line.b() > list.y && line.y < list.b() {
                ui.gfx.text(line, group, SMALL, t.text2, Align::Start);
            }
        }
        for (entry, y) in results.iter().zip(positions) {
            let row = Rect::new(list.x, list.y + y - offset, list.w - 10.0, ROW - 2.0);
            if row.b() <= list.y || row.y >= list.b() {
                continue;
            }
            if self.row(ui, row, entry, settings) {
                outcome = Some(Outcome::Apply(entry.index));
            }
        }
        if results.is_empty() {
            ui.gfx.text(
                list,
                "No themes match your search.",
                BODY,
                t.text2,
                Align::Center,
            );
        }
        ui.pop_clip();
        self.preview(ui, preview, settings, system);
        let mut foot = footer;
        if !results.is_empty()
            && (ui
                .button(
                    Id::new("theme.apply"),
                    foot.take_right(110.0),
                    "Apply theme",
                    None,
                    Kind::Primary,
                )
                .clicked
                || submitted
                || ui.key(Key::Enter, Modifiers::NONE))
        {
            outcome = Some(Outcome::Apply(self.selected));
        }
        ui.gfx.text(
            foot,
            "↑ ↓ Preview  ·  Enter Apply  ·  Esc Cancel",
            SMALL,
            t.text2,
            Align::Start,
        );
        self.pointer = ui.pointer();
        self.focus = ui.state.focus();
        outcome
    }

    fn navigation(ui: &mut Ui<'_>, rect: Rect, groups: &[(&str, f32)], scroll: Id) {
        #[allow(clippy::cast_precision_loss)] // A bounded theme catalog.
        let height = groups.len() as f32 * 30.0;
        let offset = ui.scroll_view(Id::new("theme.families.scroll"), rect, height.max(rect.h));
        ui.push_list_clip(rect, offset, height, 4.0);
        let mut y = rect.y - offset;
        let current = ui.scroll_offset(scroll);
        let active = groups
            .iter()
            .rposition(|(_, top)| *top <= current + GROUP)
            .unwrap_or(0);
        for (index, (group, top)) in groups.iter().enumerate() {
            let row = Rect::new(rect.x, y, rect.w - 10.0, 28.0);
            y += 30.0;
            if row.b() <= rect.y || row.y >= rect.b() {
                continue;
            }
            let id = Id::new(("theme.family", group));
            if ui
                .button(
                    id,
                    row,
                    group,
                    None,
                    if active == index {
                        Kind::Secondary
                    } else {
                        Kind::Ghost
                    },
                )
                .clicked
            {
                ui.set_scroll_offset(scroll, *top);
            }
            ui.set_toggled(id, active == index);
        }
        ui.pop_clip();
    }

    fn row(&mut self, ui: &mut Ui<'_>, row: Rect, entry: &Entry, settings: &Settings) -> bool {
        let t = ui.theme;
        let name = &colors::app_schemes()[entry.index];
        let id = Id::new(("theme.option", entry.index));
        let response = ui.zone(id, row, Role::ListItem, name);
        ui.set_toggled(id, entry.index == self.selected);
        if response.hovered {
            ui.set_cursor(Cursor::Pointer);
            if self.pointer != ui.pointer() {
                self.hovered = Some((entry.index, ui.time()));
            }
            if let Some((index, since)) = self.hovered.filter(|(index, _)| *index == entry.index) {
                let left = 0.18 - (ui.time() - since);
                if left <= 0.0 {
                    self.selected = index;
                } else {
                    ui.request_repaint_after(std::time::Duration::from_secs_f64(left));
                }
            }
        }
        if response.focused && self.focus != Some(id) {
            self.selected = entry.index;
        }
        if response.clicked {
            self.selected = entry.index;
        }
        if self.selected == entry.index {
            ui.gfx.fill_rect(row, 7.0, t.selected);
        }
        if response.focus_ring {
            ui.focus_ring(row, 7.0);
        }
        let mut line = row.inset(8.0, 0.0);
        let chip = line.take_left(46.0).centered(42.0, 28.0);
        let swatch = &colors::app_swatches()[entry.index];
        ui.gfx.fill_rect(chip, 5.0, swatch[0]);
        ui.gfx
            .text(chip, "Aa", BODY_MEDIUM, swatch[1], Align::Center);
        line.take_left(8.0);
        let current = settings
            .app_scheme
            .as_deref()
            .unwrap_or(match settings.theme {
                ThemeMode::Light => colors::OWN_LIGHT,
                ThemeMode::Dark => colors::OWN_DARK,
                ThemeMode::System => colors::OWN_COLORS,
            })
            == name;
        let variant = if current {
            "Saved"
        } else {
            match entry.dark {
                Some(true) => "Dark",
                Some(false) => "Light",
                None => "Auto",
            }
        };
        ui.gfx
            .text(line.take_right(34.0), variant, SMALL, t.text2, Align::End);
        ui.gfx.text(line, name, BODY, t.text, Align::Start);
        response.clicked
    }
}

#[cfg(test)]
mod tests;
