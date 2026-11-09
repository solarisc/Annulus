//! The Snippets screen: saved commands, editing, deletion, and runs on several hosts.

use annulus_config::tokens::space;
use annulus_session::{Program, SshFiles};
use annulus_ui::gfx::{Align, Canvas as _, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{FOCUS_REACH, Id, Key, Modifiers, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL, button_width};
use annulus_vault::snippets::Snippet;
use annulus_vault::vault::Vault;

use super::super::pages::{self, Fold, ListMotion, ROW_H, RowLook};
use super::{GAP, LINE, SCRIPT_H, ScreenAction, confirm_delete, row_buttons, rows};
use crate::{connect, snippets_screen};

/// The Snippets screen.
#[derive(Default)]
pub struct SnippetsView {
    draft: Fold<(Snippet, String, String)>,
    /// The snippet whose Delete asks once more.
    confirm: Option<u64>,
    /// Running a snippet on hosts: the snippet, and the hosts ticked.
    targets: Fold<(u64, Vec<u64>)>,
    pub(super) rows: ListMotion,
}

impl SnippetsView {
    /// Opens saved snippet `id` in the edit card.
    pub fn edit_snippet(&mut self, vault: &Vault, id: u64) {
        if let Some(snippet) = vault.snippets().iter().find(|snippet| snippet.id == id) {
            self.draft.open((
                snippet.clone(),
                snippet.label.clone(),
                snippet.script.clone(),
            ));
        }
    }

    /// Whether saved snippet `id` is open in the edit card.
    #[cfg(test)]
    pub fn editing(&self, id: u64) -> bool {
        self.draft
            .get()
            .is_some_and(|(snippet, _, _)| snippet.id == id)
    }

    /// The hosts to run a snippet on (F-SNIP-004): tick up to [`snippets_screen::MAX_TARGETS`],
    /// then each gets a terminal that runs it after the host's own startup command. Returns
    /// whether the card closes, and what to run.
    #[allow(clippy::too_many_lines)] // The list, then its buttons.
    fn targets_card(
        (snippet_id, ticked): &mut (u64, Vec<u64>),
        ui: &mut Ui<'_>,
        area: &mut Rect,
        vault: &Vault,
        files: &SshFiles,
    ) -> (bool, Option<ScreenAction>) {
        let Some(snippet) = vault
            .snippets()
            .iter()
            .find(|s| s.id == *snippet_id)
            .cloned()
        else {
            return (true, None);
        };
        let hosts: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .map(|host| (host.id, host.label.clone()))
            .collect();
        #[allow(clippy::cast_precision_loss)] // Lists are short.
        let list_h = (hosts.len() as f32 * LINE).min(5.0 * LINE);
        let mut card = pages::card(
            ui,
            area,
            &format!("Run {} on hosts", snippet.label),
            list_h + CONTROL + 4.0 * space::S,
        );
        card.take_top(space::S);
        let list = card.take_top(list_h);
        #[allow(clippy::cast_precision_loss)] // As above.
        let content = hosts.len() as f32 * LINE;
        let offset = ui.scroll_view(Id::new("snippet.targets"), list, content);
        ui.push_list_clip(list, offset, content, FOCUS_REACH);
        let full = ticked.len() >= snippets_screen::MAX_TARGETS;
        for (index, (id, label)) in hosts.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * LINE;
            if y + LINE < list.y || y > list.b() {
                continue;
            }
            let mut on = ticked.contains(id);
            let before = on;
            ui.checkbox(
                Id::new(("snippet.target", *id)),
                Rect::new(list.x, y, list.w - 10.0, LINE - 2.0),
                &mut on,
                label,
            );
            if on && !before && !full {
                ticked.push(*id);
            } else if !on && before {
                ticked.retain(|known| known != id);
            }
        }
        ui.pop_clip();
        card.take_top(space::S);
        let mut line = card.take_top(CONTROL);
        let go_label = match ticked.len() {
            1 => "Run on 1 host".to_owned(),
            n => format!("Run on {n} hosts"),
        };
        let go = line.take_right(button_width(&mut ui.gfx, &go_label, Some(icons::PLAY)));
        line.take_right(space::S);
        let cancel = line.take_right(button_width(&mut ui.gfx, "Cancel", None));
        line.take_right(space::M);
        let t = ui.theme;
        if full {
            ui.gfx.text(
                line,
                &format!("At most {} at once.", snippets_screen::MAX_TARGETS),
                SMALL,
                t.text2,
                Align::Start,
            );
        }
        let cancelled = ui
            .button(
                Id::new("snippet.targets.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Ghost,
            )
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE);
        let run = ui
            .button(
                Id::new("snippet.targets.go"),
                go,
                &go_label,
                Some(icons::PLAY),
                Kind::Primary,
            )
            .clicked
            && !ticked.is_empty();
        if cancelled {
            return (true, None);
        }
        if !run {
            return (false, None);
        }
        let tabs: snippets_screen::Panes = ticked
            .iter()
            .filter_map(|id| vault.hosts().iter().find(|host| host.id == *id))
            .map(|host| {
                (
                    host.label.clone(),
                    snippets_screen::with_script(
                        connect::host(vault, files, host),
                        &snippet.script,
                    ),
                )
            })
            .collect();
        let title = match tabs.len() {
            1 => snippet.label.clone(),
            n => format!("{} on {n} hosts", snippet.label),
        };
        (true, Some(ScreenAction::Terminals(title, tabs)))
    }

    /// Draws the screen; running a snippet opens a terminal, or one per host.
    #[allow(clippy::too_many_lines)] // Header, editor and list, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        files: &SshFiles,
    ) -> Option<ScreenAction> {
        let mut area = pages::page(rect);
        let count = vault.snippets().len();
        let mut actions = pages::header(
            ui,
            &mut area,
            "Snippets",
            Some(count),
            "Commands you run often, on a local shell or your hosts.",
        );
        if pages::action(
            ui,
            &mut actions,
            "snippets.new",
            "New snippet",
            Some(icons::PLUS),
            Kind::Primary,
        ) {
            self.draft
                .open((vault.draft_snippet(), String::new(), String::new()));
        }
        let mut close_draft = false;
        self.draft.draw(
            ui,
            Id::new("snippets.draft"),
            &mut area,
            |ui, area, draft| {
                let (snippet, label, script) = draft;
                let title = if vault.snippets().iter().any(|saved| saved.id == snippet.id) {
                    "Edit snippet"
                } else {
                    "New snippet"
                };
                let mut card =
                    pages::card(ui, area, title, 2.0 * CONTROL + SCRIPT_H + 4.0 * space::M);
                card.take_top(space::S);
                ui.text_field(
                    Id::new("snippet.label"),
                    card.take_top(CONTROL),
                    label,
                    FieldStyle {
                        placeholder: "Label, e.g. Restart nginx",
                        ..FieldStyle::default()
                    },
                );
                card.take_top(space::S);
                ui.text_area(
                    Id::new("snippet.script"),
                    card.take_top(SCRIPT_H),
                    script,
                    FieldStyle {
                        placeholder: "sudo systemctl restart nginx",
                        mono: true,
                        ..FieldStyle::default()
                    },
                );
                card.take_top(space::S);
                let mut line = card.take_top(CONTROL);
                let save = line.take_right(88.0);
                line.take_right(space::S);
                let cancel = line.take_right(80.0);
                let cancelled = ui
                    .button(
                        Id::new("snippet.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE);
                let saved = ui
                    .button(Id::new("snippet.save"), save, "Save", None, Kind::Primary)
                    .clicked;
                if saved && !script.trim().is_empty() {
                    let mut snippet = snippet.clone();
                    snippet.label = if label.trim().is_empty() {
                        script.trim().to_owned()
                    } else {
                        label.trim().to_owned()
                    };
                    script.trim().clone_into(&mut snippet.script);
                    let _ = vault.upsert_snippet(snippet);
                    close_draft = true;
                } else if cancelled {
                    close_draft = true;
                }
            },
        );
        if close_draft {
            self.draft.close();
        }
        let mut action = None;
        let mut close_targets = false;
        self.targets.draw(
            ui,
            Id::new("snippets.targets"),
            &mut area,
            |ui, area, targets| {
                (close_targets, action) = Self::targets_card(targets, ui, area, vault, files);
            },
        );
        if close_targets {
            self.targets.close();
        }
        if count == 0 {
            self.rows.empty(ui, area, ROW_H + GAP);
            pages::empty(
                ui,
                area,
                icons::CODE,
                "No snippets yet",
                "Save a command once, run it anywhere.",
            );
            return action;
        }
        let list: Vec<Snippet> = vault.snippets().to_vec();
        let mut remove = None;
        let mut pick = None;
        let looks: Vec<RowLook> = list
            .iter()
            .map(|snippet| RowLook {
                key: snippet.id,
                icon: icons::CODE,
                title: snippet.label.clone(),
                detail: snippet.script.lines().next().unwrap_or_default().to_owned(),
                mono: true,
            })
            .collect();
        let confirm = self.confirm;
        let mut confirm_next = confirm;
        rows(
            ui,
            "snippets.list",
            area,
            &looks,
            &mut self.rows,
            |ui, index, row| {
                let snippet = &list[index];
                let zone = Id::new(("snippet", snippet.id));
                pages::row(ui, zone, row, &snippet.label, false);
                let (clicked, remainder) = if confirm == Some(snippet.id) {
                    let (answer, remainder) = confirm_delete(ui, row, zone, "Delete snippet");
                    match answer {
                        Some(true) => remove = Some(snippet.id),
                        Some(false) => confirm_next = None,
                        None => {}
                    }
                    (None, remainder)
                } else {
                    row_buttons(
                        ui,
                        row,
                        zone,
                        &[
                            (icons::TRASH, "Delete"),
                            (icons::SERVER, "Run on hosts"),
                            (icons::PLAY, "Run in a local terminal"),
                        ],
                    )
                };
                let inner = pages::row_icon(ui, remainder, icons::CODE);
                let first = snippet.script.lines().next().unwrap_or_default();
                pages::two_lines(ui, inner, &snippet.label, first, true);
                match clicked {
                    Some(1) => pick = Some(snippet.id),
                    Some(2) => {
                        action = Some(ScreenAction::Terminal(
                            snippet.label.clone(),
                            Box::new(snippets_screen::with_script(
                                Program::DefaultShell,
                                &snippet.script,
                            )),
                        ));
                    }
                    Some(0) => confirm_next = Some(snippet.id),
                    _ => {}
                }
            },
        );
        self.confirm = confirm_next;
        if let Some(id) = pick {
            self.targets.open((id, Vec::new()));
        }
        if let Some(id) = remove {
            self.confirm = None;
            let _ = vault.remove_snippet(id);
        }
        action
    }
}

/// The snippet panel beside a terminal (Ctrl+Shift+.): search, then Paste types a snippet
/// into the focused pane and Run types it with Enter. Returns the bytes to type. `rect` is the
/// inside of the panel's card.
pub fn snippet_panel(
    ui: &mut Ui<'_>,
    rect: Rect,
    search: &mut String,
    snippets: &[Snippet],
) -> Option<Vec<u8>> {
    let t = ui.theme;
    let mut column = rect.inset(space::M, space::M);
    ui.section_label(column.take_top(18.0), "Snippets");
    column.take_top(space::S);
    ui.text_field(
        Id::new("panel.search"),
        column.take_top(CONTROL),
        search,
        FieldStyle {
            placeholder: "Search snippets",
            icon: Some(icons::SEARCH),
            ..FieldStyle::default()
        },
    );
    column.take_top(space::S);
    let query = search.trim().to_lowercase();
    let shown: Vec<&Snippet> = snippets
        .iter()
        .filter(|s| {
            query.is_empty()
                || s.label.to_lowercase().contains(&query)
                || s.script.to_lowercase().contains(&query)
        })
        .collect();
    if shown.is_empty() {
        ui.gfx.text(
            column.take_top(20.0),
            "No snippets match.",
            SMALL,
            t.text2,
            Align::Start,
        );
        return None;
    }
    let mut out = None;
    for snippet in shown {
        if column.h < 52.0 {
            break;
        }
        let card = column.take_top(52.0);
        column.take_top(space::XS);
        let id = Id::new(("panel.snippet", snippet.id));
        pages::row(ui, id, card, &snippet.label, false);
        let mut inner = card.inset(space::M, 0.0);
        let run = inner.take_right(28.0).centered(28.0, 28.0);
        inner.take_right(2.0);
        let paste = inner.take_right(28.0).centered(28.0, 28.0);
        let first = snippet.script.lines().next().unwrap_or_default();
        pages::two_lines(ui, inner, &snippet.label, first, true);
        if ui
            .icon_button(
                id.with("paste"),
                paste,
                icons::CODE,
                "Type into the terminal",
                false,
            )
            .clicked
        {
            out = Some(snippet.script.clone().into_bytes());
        }
        if ui
            .icon_button(
                id.with("run"),
                run,
                icons::PLAY,
                "Run in the terminal",
                false,
            )
            .clicked
        {
            let mut text = snippet.script.replace('\n', "\r");
            text.push('\r');
            out = Some(text.into_bytes());
        }
    }
    out
}
