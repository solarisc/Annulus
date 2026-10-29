//! The Hosts screen (UI-SPEC §2): a header, a search box that also quick-connects, groups and
//! host cards (or a list), an empty state, and the host editor in a side panel.

use std::hash::{DefaultHasher, Hash, Hasher};

use annulus_config::tokens::{TILES, space};
use annulus_ui::gfx::{Align, Color, Point, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, TITLE, button_width,
};
use annulus_vault::hosts::{Host, Secret};
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use crate::groups_view;

/// Width of the editor panel.
const EDITOR_W: f32 = 392.0;
/// The narrowest a host card gets before the grid drops a column.
const CARD_MIN_W: f32 = 264.0;
const CARD_H: f32 = 72.0;
const GROUP_H: f32 = 60.0;
const ROW_H: f32 = 44.0;
const GAP: f32 = 12.0;
/// Side padding of the screen.
const PAD: f32 = 28.0;

/// How hosts are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Cards in a grid.
    #[default]
    Grid,
    /// One row per host, in columns.
    List,
}

/// What the screen asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostsAction {
    /// Connect to a saved host.
    Connect(u64),
    /// Connect to a typed `user@host:port`.
    QuickConnect(String),
    /// Open a local terminal.
    LocalTerminal,
    /// Browse a saved host's files.
    Sftp(u64),
    /// Import hosts from `~/.ssh/config`.
    ImportSshConfig,
}

/// The Hosts screen's state.
#[derive(Default)]
pub struct HostsView {
    /// The search box.
    pub query: String,
    /// Grid or list.
    pub mode: Mode,
    /// The selected host.
    pub selected: Option<u64>,
    /// The group being shown (`None` is the top level).
    pub group: Option<String>,
    /// The editor, while open.
    pub editor: Option<Editor>,
    /// The group editor, while open.
    pub group_editor: Option<GroupEditor>,
}

/// The host editor's draft.
pub struct Editor {
    host: Host,
    is_new: bool,
    label: String,
    address: String,
    port: String,
    group: Option<String>,
    tags: String,
    username: String,
    password: Zeroizing<String>,
    key: Option<u64>,
    jump: Option<u64>,
    proxy: Zeroizing<String>,
    env: String,
    startup: String,
    term: String,
    reconnect: bool,
    /// The Advanced section is open.
    pub advanced: bool,
    problem: Option<String>,
    content_h: f32,
}

impl Editor {
    /// An editor over `host`.
    #[must_use]
    pub fn new(host: Host, is_new: bool) -> Self {
        Self {
            label: host.label.clone(),
            address: host.address.clone(),
            port: host.port.map(|port| port.to_string()).unwrap_or_default(),
            group: host.group.clone(),
            tags: host.tags.join(", "),
            username: host.username.clone(),
            password: Zeroizing::new(String::new()),
            key: host.key_id,
            jump: host.jump_id,
            proxy: Zeroizing::new(
                host.proxy
                    .as_ref()
                    .map(|proxy| proxy.expose().to_owned())
                    .unwrap_or_default(),
            ),
            env: groups_view::format_env(&host.env),
            startup: host.startup.clone(),
            term: host.term.clone(),
            reconnect: !host.no_reconnect,
            advanced: false,
            problem: None,
            content_h: 0.0,
            host,
            is_new,
        }
    }

    /// The host as edited, or why it cannot be saved.
    fn build(&self) -> Result<Host, String> {
        let mut host = self.host.clone();
        self.label.trim().clone_into(&mut host.label);
        self.address.trim().clone_into(&mut host.address);
        if host.address.is_empty() {
            return Err("Enter an address".into());
        }
        if host.label.is_empty() {
            host.label.clone_from(&host.address);
        }
        host.port = match self.port.trim() {
            "" => None,
            port => Some(
                port.parse()
                    .map_err(|_| "The port is a number from 1 to 65535".to_owned())?,
            ),
        };
        host.group.clone_from(&self.group);
        host.tags = self
            .tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect();
        self.username.trim().clone_into(&mut host.username);
        if !self.password.is_empty() {
            host.password = Some(Secret::new(self.password.as_str()));
        }
        host.key_id = self.key;
        host.jump_id = self.jump;
        host.proxy = (!self.proxy.trim().is_empty()).then(|| Secret::new(self.proxy.trim()));
        host.env = groups_view::parse_env(&self.env)?;
        self.startup.trim().clone_into(&mut host.startup);
        self.term.trim().clone_into(&mut host.term);
        host.no_reconnect = !self.reconnect;
        Ok(host)
    }
}

/// What the screen shows of one host.
struct Card {
    id: u64,
    label: String,
    detail: String,
    tags: Vec<String>,
}

/// What the screen shows of one group.
struct GroupCard {
    uid: String,
    name: String,
    holds: String,
}

impl HostsView {
    /// Opens the editor on a new host.
    pub fn new_host(&mut self, vault: &Vault) {
        let mut host = vault.draft_host();
        host.group.clone_from(&self.group);
        self.group_editor = None;
        self.editor = Some(Editor::new(host, true));
    }

    /// Opens the editor on saved host `id`.
    pub fn edit_host(&mut self, vault: &Vault, id: u64) {
        if let Some(host) = vault.hosts().iter().find(|host| host.id == id) {
            self.selected = Some(id);
            self.group_editor = None;
            self.editor = Some(Editor::new(host.clone(), false));
        }
    }

    /// Draws the screen into `rect`.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: Option<&mut Vault>,
    ) -> Option<HostsAction> {
        let mut area = rect;
        let mut action = None;
        let panel = (self.editor.is_some() || self.group_editor.is_some())
            .then(|| area.take_right(EDITOR_W.min(area.w * 0.6)));
        let vault = vault?;
        action = self.listing_area(ui, area, vault).or(action);
        if let Some(panel) = panel {
            if self.group_editor.is_some() {
                self.group_panel(ui, panel, vault);
            } else {
                self.editor_panel(ui, panel, vault);
            }
        }
        action
    }

    /// Everything left of the editor: header, toolbar, and the list or an empty state.
    fn listing_area(
        &mut self,
        ui: &mut Ui<'_>,
        area: Rect,
        vault: &mut Vault,
    ) -> Option<HostsAction> {
        let mut action = None;
        let (cards, groups) = self.gather(vault);
        let mut main = area.inset(PAD, 0.0);
        main.take_top(space::XL);
        self.header(ui, main.take_top(36.0), cards.len());
        main.take_top(space::L);
        if let Some(toolbar) = self.toolbar(ui, main.take_top(CONTROL), vault, &cards) {
            action = Some(toolbar);
        }
        main.take_top(space::L);
        if self.group.is_some() && self.query.trim().is_empty() {
            self.breadcrumb(ui, main.take_top(CONTROL), vault);
            main.take_top(space::M);
        }
        if cards.is_empty() && groups.is_empty() && self.group.is_none() {
            if self.query.trim().is_empty() {
                if let Some(empty) = self.empty_state(ui, main, vault) {
                    action = Some(empty);
                }
            } else {
                no_results(ui, main, &self.query);
            }
            return action;
        }
        if let Some(listed) = self.listing(ui, main, &cards, &groups, vault) {
            action = Some(listed);
        }
        action
    }

    fn gather(&self, vault: &Vault) -> (Vec<Card>, Vec<GroupCard>) {
        let query = self.query.trim().to_lowercase();
        let searching = !query.is_empty();
        let cards = vault
            .hosts()
            .iter()
            .filter(|host| searching || host.group == self.group)
            .filter(|host| {
                !searching
                    || host.label.to_lowercase().contains(&query)
                    || host.address.to_lowercase().contains(&query)
                    || host.username.to_lowercase().contains(&query)
                    || host
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query))
            })
            .map(|host| {
                let (resolved, _) = vault.resolve(host);
                Card {
                    id: host.id,
                    label: host.label.clone(),
                    detail: resolved.target(),
                    tags: host.tags.clone(),
                }
            })
            .collect();
        let groups = if searching {
            Vec::new()
        } else {
            vault
                .groups()
                .iter()
                .filter(|group| group.parent == self.group)
                .map(|group| {
                    let hosts = vault
                        .hosts()
                        .iter()
                        .filter(|host| host.group.as_deref() == Some(group.uid.as_str()))
                        .count();
                    let subgroups = vault
                        .groups()
                        .iter()
                        .filter(|child| child.parent.as_deref() == Some(group.uid.as_str()))
                        .count();
                    GroupCard {
                        uid: group.uid.clone(),
                        name: group.name.clone(),
                        holds: groups_view::holds(hosts, subgroups),
                    }
                })
                .collect()
        };
        (cards, groups)
    }

    fn header(&mut self, ui: &mut Ui<'_>, rect: Rect, count: usize) {
        let t = ui.theme;
        let mut row = rect;
        let toggle = row.take_right(72.0);
        let title = ui.gfx.measure("Hosts", TITLE).width;
        let head = row.take_left(title + 2.0);
        ui.strict_text(head, "Hosts", TITLE, t.text, Align::Start);
        row.take_left(space::S + 2.0);
        if count > 0 {
            let label = count.to_string();
            let width = ui.gfx.measure(&label, SMALL).width + 14.0;
            let badge = Rect::new(row.x, row.center().y - 10.0, width, 20.0);
            ui.gfx.fill_rect(badge, 10.0, t.hover);
            ui.gfx.text(badge, &label, SMALL, t.text2, Align::Center);
        }
        let segment = toggle.centered(72.0, 32.0);
        ui.gfx.fill_rect(segment, 8.0, t.surface);
        ui.gfx.stroke_rect(segment, 8.0, 1.0, t.line);
        let mut halves = segment.inset(2.0, 2.0);
        let grid = halves.take_left(34.0);
        let list = halves;
        if ui
            .icon_button(
                Id::new("hosts.grid"),
                grid,
                icons::GRID_VIEW,
                "Grid view",
                self.mode == Mode::Grid,
            )
            .clicked
        {
            self.mode = Mode::Grid;
        }
        if ui
            .icon_button(
                Id::new("hosts.list"),
                list,
                icons::LIST,
                "List view",
                self.mode == Mode::List,
            )
            .clicked
        {
            self.mode = Mode::List;
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        vault: &mut Vault,
        cards: &[Card],
    ) -> Option<HostsAction> {
        let mut row = rect;
        let mut action = None;
        let new_w = button_width(ui.gfx, "New host", Some(icons::PLUS));
        let term_w = button_width(ui.gfx, "Terminal", Some(icons::TERMINAL));
        // Labels go when the search box would get too narrow; icons stay.
        let compact = row.w - (new_w + CONTROL + term_w + 3.0 * space::S) < 240.0;
        let (new_label, term_label) = if compact {
            ("", "")
        } else {
            ("New host", "Terminal")
        };
        let term_w = if compact { CONTROL } else { term_w };
        let new_w = if compact { CONTROL } else { new_w };

        // Laid out from the right, drawn from the left: Tab follows the drawing order.
        let terminal = row.take_right(term_w);
        row.take_right(space::S);
        let more = row.take_right(CONTROL);
        row.take_right(2.0);
        let new = row.take_right(new_w);
        row.take_right(space::M);
        let search = row;
        let field = ui.text_field(
            Id::new("hosts.search"),
            search,
            &mut self.query,
            FieldStyle {
                placeholder: "Search hosts, or type user@host:port to connect",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        if field.submitted {
            let query = self.query.trim();
            if let Some(first) = cards.first() {
                action = Some(HostsAction::Connect(first.id));
            } else if !query.is_empty() {
                action = Some(HostsAction::QuickConnect(query.to_owned()));
            }
        }
        if ui
            .button(
                Id::new("hosts.new"),
                new,
                new_label,
                Some(icons::PLUS),
                Kind::Primary,
            )
            .clicked
            || ui.key(Key::Char('n'), Modifiers::CTRL_SHIFT)
        {
            self.new_host(vault);
        }
        let items = vec!["New group".to_owned(), "Import from ssh_config".to_owned()];
        match ui.menu_button(
            Id::new("hosts.new.more"),
            more,
            "",
            Some(icons::CHEVRON_DOWN),
            Kind::Primary,
            &items,
        ) {
            Some(0) => self.new_group(vault),
            Some(_) => action = Some(HostsAction::ImportSshConfig),
            None => {}
        }
        if ui
            .button(
                Id::new("hosts.terminal"),
                terminal,
                term_label,
                Some(icons::TERMINAL),
                Kind::Secondary,
            )
            .clicked
        {
            action = Some(HostsAction::LocalTerminal);
        }
        action
    }

    fn empty_state(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &Vault) -> Option<HostsAction> {
        let t = ui.theme;
        let block = rect.centered(rect.w.min(460.0), 220.0);
        let mut column = block;
        let badge = column.take_top(64.0).centered(64.0, 64.0);
        ui.gfx.fill_rect(badge, 18.0, t.selected);
        ui.gfx.stroke_rect(badge, 18.0, 1.0, t.accent.alpha(0.35));
        ui.gfx
            .icon(icons::SERVER, badge.centered(28.0, 28.0), t.accent);
        column.take_top(space::L);
        ui.strict_text(
            column.take_top(24.0),
            "No hosts yet",
            HEADING,
            t.text,
            Align::Center,
        );
        column.take_top(space::XS);
        ui.gfx.text(
            column.take_top(20.0),
            "Add a server you connect to often,",
            BODY,
            t.text2,
            Align::Center,
        );
        ui.gfx.text(
            column.take_top(20.0),
            "or type user@host:port above to connect right away.",
            BODY,
            t.text2,
            Align::Center,
        );
        column.take_top(space::L);
        let buttons = column.take_top(CONTROL);
        let new_w = button_width(ui.gfx, "New host", Some(icons::PLUS));
        let import_w = button_width(ui.gfx, "Import ssh_config", Some(icons::DOWNLOAD));
        let total = new_w + space::S + import_w;
        let mut line = buttons.centered(total, CONTROL);
        let new = line.take_left(new_w);
        line.take_left(space::S);
        let mut action = None;
        if ui
            .button(
                Id::new("hosts.empty.new"),
                new,
                "New host",
                Some(icons::PLUS),
                Kind::Primary,
            )
            .clicked
        {
            self.new_host(vault);
        }
        if ui
            .button(
                Id::new("hosts.empty.import"),
                line,
                "Import ssh_config",
                Some(icons::DOWNLOAD),
                Kind::Secondary,
            )
            .clicked
        {
            action = Some(HostsAction::ImportSshConfig);
        }
        action
    }

    fn listing(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        cards: &[Card],
        groups: &[GroupCard],
        vault: &Vault,
    ) -> Option<HostsAction> {
        let columns = grid_columns(rect.w);
        #[allow(clippy::cast_precision_loss)] // Column and row counts are small.
        let rows = |count: usize, per_row: usize, height: f32| -> f32 {
            let lines = count.div_ceil(per_row.max(1));
            lines as f32 * height + lines.saturating_sub(1) as f32 * GAP
        };
        let host_rows = match self.mode {
            Mode::Grid => rows(cards.len(), columns, CARD_H),
            Mode::List => rows(cards.len(), 1, ROW_H) + ROW_H,
        };
        let group_block = if groups.is_empty() {
            0.0
        } else {
            28.0 + rows(groups.len(), columns, GROUP_H) + space::XL
        };
        let content_h = group_block + 28.0 + host_rows + space::XL;
        let id = Id::new("hosts.scroll");
        let offset = ui.scroll_view(id, rect, content_h);
        ui.push_clip(rect);
        let mut y = rect.y - offset;
        let mut action = None;
        if !groups.is_empty() {
            let label = Rect::new(rect.x, y, rect.w, 20.0);
            ui.section_label(label, &format!("Groups \u{b7} {}", groups.len()));
            y += 28.0;
            for (index, group) in groups.iter().enumerate() {
                let cell = grid_cell(rect.x, y, rect.w - 8.0, columns, index, GROUP_H);
                if group_card(ui, cell, group) {
                    self.group = Some(group.uid.clone());
                }
            }
            y += rows(groups.len(), columns, GROUP_H) + space::XL;
        }
        let label = Rect::new(rect.x, y, rect.w, 20.0);
        let heading = if self.query.trim().is_empty() {
            format!("Hosts \u{b7} {}", cards.len())
        } else {
            format!("Results \u{b7} {}", cards.len())
        };
        ui.section_label(label, &heading);
        y += 28.0;
        match self.mode {
            Mode::Grid => {
                for (index, card) in cards.iter().enumerate() {
                    let cell = grid_cell(rect.x, y, rect.w - 8.0, columns, index, CARD_H);
                    match host_card(ui, cell, card, self.selected == Some(card.id)) {
                        CardEvent::Select => self.selected = Some(card.id),
                        CardEvent::Connect => action = Some(HostsAction::Connect(card.id)),
                        CardEvent::Edit => self.edit_host(vault, card.id),
                        CardEvent::Files => action = Some(HostsAction::Sftp(card.id)),
                        CardEvent::None => {}
                    }
                }
            }
            Mode::List => {
                list_header(ui, Rect::new(rect.x, y, rect.w - 8.0, ROW_H - 12.0));
                y += ROW_H - 12.0;
                for card in cards {
                    let row = Rect::new(rect.x, y, rect.w - 8.0, ROW_H);
                    match host_row(ui, row, card, self.selected == Some(card.id)) {
                        CardEvent::Select => self.selected = Some(card.id),
                        CardEvent::Connect => action = Some(HostsAction::Connect(card.id)),
                        CardEvent::Edit => self.edit_host(vault, card.id),
                        CardEvent::Files => action = Some(HostsAction::Sftp(card.id)),
                        CardEvent::None => {}
                    }
                    y += ROW_H + GAP;
                }
            }
        }
        ui.pop_clip();
        action
    }

    #[allow(clippy::too_many_lines)] // One panel, top to bottom: header, form, footer.
    fn editor_panel(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) {
        let t = ui.theme;
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        ui.gfx.shadow(rect, 0.0, 18.0, 0.0, t.shadow);
        let shape = Rect::new(rect.x, rect.y, rect.w + 24.0, rect.h + 24.0);
        ui.gfx.fill_rect(shape, 0.0, t.raised);
        ui.gfx
            .fill_rect(Rect::new(rect.x, rect.y, 1.0, rect.h), 0.0, t.line);
        // Everything inside the panel takes the pointer: nothing below it is clicked.
        ui.zone(Id::new("editor.backdrop"), rect, Role::Backdrop, "");

        let mut area = rect;
        let mut head = area.take_top(64.0).inset(space::XL, 0.0);
        let close = head.take_right(32.0).centered(32.0, 32.0);
        let mut close_editor = ui
            .icon_button(Id::new("editor.close"), close, icons::CLOSE, "Close", false)
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE);
        let title = if editor.is_new {
            "New host"
        } else {
            "Edit host"
        };
        let title_rect = Rect::new(head.x, head.y + 14.0, head.w, 22.0);
        ui.strict_text(title_rect, title, HEADING, t.text, Align::Start);
        let subtitle = if editor.is_new {
            "Saved in your vault, encrypted".to_owned()
        } else {
            editor.host.target()
        };
        let sub_rect = Rect::new(head.x, head.y + 36.0, head.w, 18.0);
        ui.gfx
            .text(sub_rect, &subtitle, SMALL, t.text2, Align::Start);
        ui.gfx
            .fill_rect(Rect::new(area.x, area.y, area.w, 1.0), 0.0, t.line);

        let footer = area.take_bottom(68.0);
        ui.gfx
            .fill_rect(Rect::new(footer.x, footer.y, footer.w, 1.0), 0.0, t.line);

        let body = area;
        let offset = ui.scroll_view(Id::new("editor.scroll"), body, editor.content_h);
        ui.push_clip(body);
        let mut form = Form {
            x: body.x + space::XL,
            y: body.y + space::XL - offset,
            w: body.w - 2.0 * space::XL,
        };
        let groups = groups_view::choices(vault.groups(), None);
        let mut group_items = vec!["Top level".to_owned()];
        group_items.extend(groups.iter().map(|(_, path)| path.clone()));
        let group_index = editor
            .group
            .as_ref()
            .and_then(|uid| groups.iter().position(|(known, _)| known == uid))
            .map_or(0, |at| at + 1);
        let hosts: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .filter(|host| host.id != editor.host.id)
            .map(|host| (host.id, host.label.clone()))
            .collect();
        let keys: Vec<(u64, String)> = vault
            .ssh_keys()
            .iter()
            .map(|key| (key.id, key.label.clone()))
            .collect();

        form.section(ui, "General");
        form.text(
            ui,
            "editor.label",
            "Label",
            &mut editor.label,
            "prod-db",
            None,
        );
        let address_error = editor
            .problem
            .as_deref()
            .filter(|problem| problem.contains("address"));
        form.text(
            ui,
            "editor.address",
            "Address",
            &mut editor.address,
            "10.0.4.12 or db.example.com",
            address_error,
        );
        let (left, right) = form.split(96.0);
        let mut port_form = Form { w: left, ..form };
        port_form.text(ui, "editor.port", "Port", &mut editor.port, "22", None);
        let mut group_form = Form {
            x: form.x + left + space::M,
            w: right,
            ..form
        };
        if let Some(chosen) =
            group_form.select(ui, "editor.group", "Group", &group_items, Some(group_index))
        {
            editor.group = chosen
                .checked_sub(1)
                .and_then(|at| groups.get(at))
                .map(|(uid, _)| uid.clone());
        }
        form.y = port_form.y.max(group_form.y);
        form.text(
            ui,
            "editor.tags",
            "Tags",
            &mut editor.tags,
            "postgres, prod",
            None,
        );

        form.gap(space::S);
        form.section(ui, "Credentials");
        form.text(
            ui,
            "editor.user",
            "Username",
            &mut editor.username,
            "root",
            None,
        );
        let password_hint = if editor.host.password.is_some() {
            "Saved \u{2014} type to replace"
        } else {
            "Optional"
        };
        form.secret(
            ui,
            "editor.password",
            "Password",
            &mut editor.password,
            password_hint,
        );
        let mut key_items = vec!["Default keys (~/.ssh)".to_owned()];
        key_items.extend(keys.iter().map(|(_, label)| label.clone()));
        let key_index = editor
            .key
            .and_then(|id| keys.iter().position(|(known, _)| *known == id))
            .map_or(0, |at| at + 1);
        if let Some(chosen) = form.select(ui, "editor.key", "Key", &key_items, Some(key_index)) {
            editor.key = chosen
                .checked_sub(1)
                .and_then(|at| keys.get(at))
                .map(|(id, _)| *id);
        }

        form.gap(space::S);
        if form.disclosure(ui, "editor.advanced", "Advanced", editor.advanced) {
            editor.advanced = !editor.advanced;
        }
        if editor.advanced {
            let mut jump_items = vec!["None (direct)".to_owned()];
            jump_items.extend(hosts.iter().map(|(_, label)| label.clone()));
            let jump_index = editor
                .jump
                .and_then(|id| hosts.iter().position(|(known, _)| *known == id))
                .map_or(0, |at| at + 1);
            if let Some(chosen) = form.select(
                ui,
                "editor.jump",
                "Jump host",
                &jump_items,
                Some(jump_index),
            ) {
                editor.jump = chosen
                    .checked_sub(1)
                    .and_then(|at| hosts.get(at))
                    .map(|(id, _)| *id);
            }
            form.secret(
                ui,
                "editor.proxy",
                "Proxy",
                &mut editor.proxy,
                "socks5://host:1080 or http://host:3128",
            );
            form.text(
                ui,
                "editor.env",
                "Environment",
                &mut editor.env,
                "LANG=en_US.UTF-8",
                None,
            );
            form.text(
                ui,
                "editor.startup",
                "Startup command",
                &mut editor.startup,
                "tmux attach",
                None,
            );
            form.text(
                ui,
                "editor.term",
                "Terminal type",
                &mut editor.term,
                "xterm-256color",
                None,
            );
            form.check(
                ui,
                "editor.reconnect",
                "Reconnect when the connection drops",
                &mut editor.reconnect,
            );
        }
        if let Some(problem) = editor.problem.as_deref().filter(|p| !p.contains("address")) {
            let line = Rect::new(form.x, form.y, form.w, 20.0);
            ui.gfx.text(line, problem, SMALL, t.error, Align::Start);
            form.y += 28.0;
        }
        editor.content_h = form.y + offset - body.y + space::L;
        ui.pop_clip();

        let mut buttons = footer.inset(space::XL, (footer.h - CONTROL) / 2.0);
        let save_w = button_width(ui.gfx, "Save", None).max(88.0);
        let save = buttons.take_right(save_w);
        buttons.take_right(space::S);
        let cancel_w = button_width(ui.gfx, "Cancel", None);
        let cancel = buttons.take_right(cancel_w);
        let mut saved = false;
        if !editor.is_new {
            let delete_w = button_width(ui.gfx, "Delete", Some(icons::TRASH));
            let delete = Rect::new(footer.x + space::XL, cancel.y, delete_w, CONTROL);
            if ui
                .button(
                    Id::new("editor.delete"),
                    delete,
                    "Delete",
                    Some(icons::TRASH),
                    Kind::Danger,
                )
                .clicked
                && vault.remove_host(editor.host.id).is_ok()
            {
                saved = true;
            }
        }
        if ui
            .button(
                Id::new("editor.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Secondary,
            )
            .clicked
        {
            close_editor = true;
        }
        if ui
            .button(Id::new("editor.save"), save, "Save", None, Kind::Primary)
            .clicked
            || ui.key(Key::Char('s'), Modifiers::CTRL)
        {
            match editor.build() {
                Ok(host) => match vault.upsert_host(host.clone()) {
                    Ok(()) => {
                        self.selected = Some(host.id);
                        saved = true;
                    }
                    Err(error) => editor.problem = Some(error.to_string()),
                },
                Err(problem) => editor.problem = Some(problem),
            }
        }
        if saved || close_editor {
            self.editor = None;
        }
    }
}

/// The group editor's draft (F-HOST-003): name, parent and what its hosts inherit.
pub struct GroupEditor {
    group: annulus_vault::groups::Group,
    is_new: bool,
    name: String,
    username: String,
    port: String,
    proxy: Zeroizing<String>,
    env: String,
    startup: String,
    term: String,
    problem: Option<String>,
    confirm_delete: bool,
    content_h: f32,
}

impl GroupEditor {
    /// An editor over `group`.
    #[must_use]
    pub fn new(group: annulus_vault::groups::Group, is_new: bool) -> Self {
        let defaults = &group.defaults;
        Self {
            name: group.name.clone(),
            username: defaults.username.clone(),
            port: defaults
                .port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            proxy: Zeroizing::new(
                defaults
                    .proxy
                    .as_ref()
                    .map(|proxy| proxy.expose().to_owned())
                    .unwrap_or_default(),
            ),
            env: groups_view::format_env(&defaults.env),
            startup: defaults.startup.clone(),
            term: defaults.term.clone(),
            problem: None,
            confirm_delete: false,
            content_h: 0.0,
            group,
            is_new,
        }
    }

    /// The group as edited, or why it cannot be saved.
    fn build(&self) -> Result<annulus_vault::groups::Group, String> {
        let mut group = self.group.clone();
        self.name.trim().clone_into(&mut group.name);
        if group.name.is_empty() {
            return Err("Give the group a name".into());
        }
        let defaults = &mut group.defaults;
        self.username.trim().clone_into(&mut defaults.username);
        defaults.port = match self.port.trim() {
            "" => None,
            port => Some(
                port.parse()
                    .map_err(|_| "The port is a number from 1 to 65535".to_owned())?,
            ),
        };
        defaults.proxy = (!self.proxy.trim().is_empty()).then(|| Secret::new(self.proxy.trim()));
        defaults.env = groups_view::parse_env(&self.env)?;
        self.startup.trim().clone_into(&mut defaults.startup);
        self.term.trim().clone_into(&mut defaults.term);
        Ok(group)
    }
}

impl HostsView {
    /// Opens the group editor on a new group inside the one shown.
    pub fn new_group(&mut self, vault: &Vault) {
        if let Ok(group) = vault.draft_group(self.group.clone()) {
            self.editor = None;
            self.group_editor = Some(GroupEditor::new(group, true));
        }
    }

    /// The path back up from the group shown: All hosts, then each parent.
    fn breadcrumb(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &Vault) {
        let t = ui.theme;
        let mut chain: Vec<(String, String)> =
            annulus_vault::groups::chain(vault.groups(), self.group.as_deref())
                .iter()
                .map(|group| (group.uid.clone(), group.name.clone()))
                .collect();
        chain.reverse();
        let mut row = rect;
        let mut go = None;
        let all_w = button_width(ui.gfx, "All hosts", None);
        if ui
            .button(
                Id::new("crumb.all"),
                row.take_left(all_w),
                "All hosts",
                None,
                Kind::Ghost,
            )
            .clicked
        {
            go = Some(None);
        }
        for (index, (uid, name)) in chain.iter().enumerate() {
            let chevron = row.take_left(16.0).centered(12.0, 12.0);
            ui.gfx.icon(icons::CHEVRON_RIGHT, chevron, t.text3);
            let width = button_width(ui.gfx, name, None);
            let slot = row.take_left(width);
            let last = index + 1 == chain.len();
            let kind = if last { Kind::Secondary } else { Kind::Ghost };
            if ui
                .button(Id::new(("crumb", uid.as_str())), slot, name, None, kind)
                .clicked
                && !last
            {
                go = Some(Some(uid.clone()));
            }
        }
        if let Some(group) = self.group.clone() {
            row.take_left(space::S);
            let edit = row.take_left(30.0).centered(30.0, 30.0);
            if ui
                .icon_button(
                    Id::new("crumb.edit"),
                    edit,
                    icons::EDIT,
                    "Edit this group",
                    false,
                )
                .clicked
                && let Some(found) = vault.groups().iter().find(|known| known.uid == group)
            {
                self.editor = None;
                self.group_editor = Some(GroupEditor::new(found.clone(), false));
            }
        }
        if let Some(target) = go {
            self.group = target;
        }
    }

    #[allow(clippy::too_many_lines)] // One panel, top to bottom: header, form, footer.
    fn group_panel(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) {
        let t = ui.theme;
        let Some(editor) = self.group_editor.as_mut() else {
            return;
        };
        ui.gfx.shadow(rect, 0.0, 18.0, 0.0, t.shadow);
        let shape = Rect::new(rect.x, rect.y, rect.w + 24.0, rect.h + 24.0);
        ui.gfx.fill_rect(shape, 0.0, t.raised);
        ui.gfx
            .fill_rect(Rect::new(rect.x, rect.y, 1.0, rect.h), 0.0, t.line);
        ui.zone(Id::new("group.backdrop"), rect, Role::Backdrop, "");
        let mut area = rect;
        let mut head = area.take_top(64.0).inset(space::XL, 0.0);
        let close = head.take_right(32.0).centered(32.0, 32.0);
        let mut close_editor = ui
            .icon_button(Id::new("group.close"), close, icons::CLOSE, "Close", false)
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE);
        let title = if editor.is_new {
            "New group"
        } else {
            "Edit group"
        };
        ui.strict_text(
            Rect::new(head.x, head.y + 14.0, head.w, 22.0),
            title,
            HEADING,
            t.text,
            Align::Start,
        );
        ui.gfx.text(
            Rect::new(head.x, head.y + 36.0, head.w, 18.0),
            "Hosts inside inherit what you set here",
            SMALL,
            t.text2,
            Align::Start,
        );
        ui.gfx
            .fill_rect(Rect::new(area.x, area.y, area.w, 1.0), 0.0, t.line);
        let footer = area.take_bottom(68.0);
        ui.gfx
            .fill_rect(Rect::new(footer.x, footer.y, footer.w, 1.0), 0.0, t.line);
        let body = area;
        let offset = ui.scroll_view(Id::new("group.scroll"), body, editor.content_h);
        ui.push_clip(body);
        let mut form = Form {
            x: body.x + space::XL,
            y: body.y + space::XL - offset,
            w: body.w - 2.0 * space::XL,
        };
        let choices = groups_view::choices(vault.groups(), Some(editor.group.uid.as_str()));
        let mut parents = vec!["Top level".to_owned()];
        parents.extend(choices.iter().map(|(_, path)| path.clone()));
        let parent_at = editor
            .group
            .parent
            .as_ref()
            .and_then(|uid| choices.iter().position(|(known, _)| known == uid))
            .map_or(0, |at| at + 1);
        let hosts: Vec<(u64, String)> = vault
            .hosts()
            .iter()
            .map(|h| (h.id, h.label.clone()))
            .collect();
        let keys: Vec<(u64, String)> = vault
            .ssh_keys()
            .iter()
            .map(|k| (k.id, k.label.clone()))
            .collect();

        form.section(ui, "Group");
        let name_error = editor.problem.as_deref().filter(|p| p.contains("name"));
        form.text(
            ui,
            "group.name",
            "Name",
            &mut editor.name,
            "Production",
            name_error,
        );
        if let Some(chosen) = form.select(ui, "group.parent", "Inside", &parents, Some(parent_at)) {
            editor.group.parent = chosen
                .checked_sub(1)
                .and_then(|at| choices.get(at))
                .map(|(uid, _)| uid.clone());
        }
        form.gap(space::S);
        form.section(ui, "Hosts inside inherit");
        form.text(
            ui,
            "group.user",
            "Username",
            &mut editor.username,
            "deploy",
            None,
        );
        form.text(ui, "group.port", "Port", &mut editor.port, "22", None);
        let mut key_items = vec!["From the parent group".to_owned()];
        key_items.extend(keys.iter().map(|(_, label)| label.clone()));
        let key_at = editor
            .group
            .defaults
            .key_id
            .and_then(|id| keys.iter().position(|(k, _)| *k == id))
            .map_or(0, |at| at + 1);
        if let Some(chosen) = form.select(ui, "group.key", "Key", &key_items, Some(key_at)) {
            editor.group.defaults.key_id = chosen
                .checked_sub(1)
                .and_then(|at| keys.get(at))
                .map(|(id, _)| *id);
        }
        let mut jump_items = vec!["From the parent group".to_owned()];
        jump_items.extend(hosts.iter().map(|(_, label)| label.clone()));
        let jump_at = editor
            .group
            .defaults
            .jump_id
            .and_then(|id| hosts.iter().position(|(h, _)| *h == id))
            .map_or(0, |at| at + 1);
        if let Some(chosen) = form.select(ui, "group.jump", "Jump host", &jump_items, Some(jump_at))
        {
            editor.group.defaults.jump_id = chosen
                .checked_sub(1)
                .and_then(|at| hosts.get(at))
                .map(|(id, _)| *id);
        }
        form.secret(
            ui,
            "group.proxy",
            "Proxy",
            &mut editor.proxy,
            "socks5://host:1080",
        );
        form.text(
            ui,
            "group.env",
            "Environment",
            &mut editor.env,
            "LANG=en_US.UTF-8",
            None,
        );
        form.text(
            ui,
            "group.startup",
            "Startup command",
            &mut editor.startup,
            "tmux attach",
            None,
        );
        form.text(
            ui,
            "group.term",
            "Terminal type",
            &mut editor.term,
            "xterm-256color",
            None,
        );
        if let Some(problem) = editor.problem.as_deref().filter(|p| !p.contains("name")) {
            ui.gfx.text(
                Rect::new(form.x, form.y, form.w, 20.0),
                problem,
                SMALL,
                t.error,
                Align::Start,
            );
            form.y += 28.0;
        }
        editor.content_h = form.y + offset - body.y + space::L;
        ui.pop_clip();

        let mut buttons = footer.inset(space::XL, (footer.h - CONTROL) / 2.0);
        let mut done = false;
        if editor.confirm_delete {
            let up_w = button_width(ui.gfx, "Move its hosts up", None);
            let all_w = button_width(ui.gfx, "Delete its hosts too", None);
            let up = buttons.take_left(up_w);
            buttons.take_left(space::S);
            let all = buttons.take_left(all_w);
            let how = if ui
                .button(
                    Id::new("group.delete.up"),
                    up,
                    "Move its hosts up",
                    None,
                    Kind::Secondary,
                )
                .clicked
            {
                Some(groups_view::Deletion::MoveHostsUp)
            } else if ui
                .button(
                    Id::new("group.delete.all"),
                    all,
                    "Delete its hosts too",
                    None,
                    Kind::Danger,
                )
                .clicked
            {
                Some(groups_view::Deletion::WithHosts)
            } else {
                None
            };
            if let Some(how) = how {
                match groups_view::delete_group(vault, &editor.group, how) {
                    Ok(()) => {
                        if self.group.as_deref() == Some(editor.group.uid.as_str()) {
                            self.group.clone_from(&editor.group.parent);
                        }
                        done = true;
                    }
                    Err(problem) => editor.problem = Some(problem),
                }
            }
        } else {
            let save = buttons.take_right(button_width(ui.gfx, "Save", None).max(88.0));
            buttons.take_right(space::S);
            let cancel = buttons.take_right(button_width(ui.gfx, "Cancel", None));
            if !editor.is_new {
                let delete_w = button_width(ui.gfx, "Delete", Some(icons::TRASH));
                let delete = Rect::new(footer.x + space::XL, cancel.y, delete_w, CONTROL);
                if ui
                    .button(
                        Id::new("group.delete"),
                        delete,
                        "Delete",
                        Some(icons::TRASH),
                        Kind::Danger,
                    )
                    .clicked
                {
                    editor.confirm_delete = true;
                }
            }
            if ui
                .button(
                    Id::new("group.cancel"),
                    cancel,
                    "Cancel",
                    None,
                    Kind::Secondary,
                )
                .clicked
            {
                close_editor = true;
            }
            if ui
                .button(Id::new("group.save"), save, "Save", None, Kind::Primary)
                .clicked
            {
                match editor.build() {
                    Ok(group) => match vault.upsert_group(group) {
                        Ok(()) => done = true,
                        Err(error) => editor.problem = Some(error.to_string()),
                    },
                    Err(problem) => editor.problem = Some(problem),
                }
            }
        }
        if done || close_editor {
            self.group_editor = None;
        }
    }
}

/// A column of labelled fields in the editor, laid out top to bottom.
#[derive(Clone, Copy)]
struct Form {
    x: f32,
    y: f32,
    w: f32,
}

impl Form {
    fn gap(&mut self, h: f32) {
        self.y += h;
    }

    fn split(&self, left: f32) -> (f32, f32) {
        (left, self.w - left - space::M)
    }

    fn section(&mut self, ui: &mut Ui<'_>, title: &str) {
        ui.section_label(Rect::new(self.x, self.y, self.w, 16.0), title);
        self.y += 16.0 + space::M;
    }

    fn label(&mut self, ui: &mut Ui<'_>, label: &str) {
        let t = ui.theme;
        ui.strict_text(
            Rect::new(self.x, self.y, self.w, 18.0),
            label,
            BODY_MEDIUM,
            t.text2.mix(t.text, 0.3),
            Align::Start,
        );
        self.y += 18.0 + 6.0;
    }

    fn text(
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
        self.y += CONTROL;
        if let Some(error) = error {
            let t = ui.theme;
            let line = Rect::new(self.x, self.y + 4.0, self.w, 16.0);
            ui.gfx.text(line, error, SMALL, t.error, Align::Start);
            self.y += 20.0;
        }
        self.y += space::L;
    }

    fn secret(
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
        self.y += CONTROL + space::L;
    }

    fn select(
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
        self.y += CONTROL + space::L;
        chosen
    }

    fn check(&mut self, ui: &mut Ui<'_>, id: &str, label: &str, value: &mut bool) {
        let row = Rect::new(self.x, self.y, self.w, 24.0);
        ui.checkbox(Id::new(id), row, value, label);
        self.y += 24.0 + space::L;
    }

    /// A clickable section title with a chevron; returns whether it was clicked.
    fn disclosure(&mut self, ui: &mut Ui<'_>, id: &str, title: &str, open: bool) -> bool {
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

/// How many card columns fit in `width`.
fn grid_columns(width: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small, positive.
    let columns = ((width + GAP) / (CARD_MIN_W + GAP)).floor().max(1.0) as usize;
    columns.min(4)
}

#[allow(clippy::cast_precision_loss)] // Column and row counts are small.
fn grid_cell(x: f32, y: f32, width: f32, columns: usize, index: usize, height: f32) -> Rect {
    let card_w = (width - GAP * (columns as f32 - 1.0)) / columns as f32;
    let (row, column) = (index / columns, index % columns);
    Rect::new(
        x + column as f32 * (card_w + GAP),
        y + row as f32 * (height + GAP),
        card_w,
        height,
    )
}

enum CardEvent {
    None,
    Select,
    Connect,
    Files,
    Edit,
}

/// A host's tile color, stable for its label.
fn tile_color(label: &str) -> Color {
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    #[allow(clippy::cast_possible_truncation)] // Only the low bits pick a color.
    let index = (hasher.finish() % TILES.len() as u64) as usize;
    TILES[index].into()
}

/// Up to two letters for a host's tile: the first letters of its first two words, or its
/// first two letters.
fn initials(label: &str) -> String {
    let words: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

fn host_tile(ui: &mut Ui<'_>, rect: Rect, label: &str) {
    let color = tile_color(label);
    ui.gfx.fill_rect(rect, rect.w * 0.26, color);
    let font = if rect.w >= 36.0 { BODY_STRONG } else { SMALL };
    ui.gfx
        .text(rect, &initials(label), font, Color::WHITE, Align::Center);
}

fn host_card(ui: &mut Ui<'_>, rect: Rect, card: &Card, selected: bool) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    if hover > 0.0 {
        ui.gfx
            .shadow(rect, 10.0, 12.0, 4.0, t.shadow.alpha(hover * 0.6));
    }
    ui.gfx
        .fill_rect(rect, 10.0, t.surface.mix(t.hover, hover * 0.35));
    let border = if selected {
        t.accent
    } else {
        t.line.mix(t.line_strong, hover)
    };
    ui.gfx
        .stroke_rect(rect, 10.0, if selected { 1.5 } else { 1.0 }, border);
    if response.focus_ring {
        ui.focus_ring(rect, 10.0);
    }
    let mut inner = rect.inset(space::M + 2.0, 0.0);
    let tile = inner.take_left(40.0).centered(40.0, 40.0);
    host_tile(ui, tile, &card.label);
    inner.take_left(space::M);
    let mut event = if response.double_clicked {
        CardEvent::Connect
    } else if response.clicked {
        CardEvent::Select
    } else {
        CardEvent::None
    };
    if response.hovered || hover > 0.0 || selected {
        let actions = inner.take_right(98.0);
        let mut slots = actions;
        let connect = slots.take_right(30.0).centered(30.0, 30.0);
        slots.take_right(4.0);
        let files = slots.take_right(30.0).centered(30.0, 30.0);
        slots.take_right(4.0);
        let edit = slots.take_right(30.0).centered(30.0, 30.0);
        if ui
            .icon_button(id.with("edit"), edit, icons::EDIT, "Edit", false)
            .clicked
        {
            event = CardEvent::Edit;
        }
        if ui
            .icon_button(id.with("files"), files, icons::FOLDER, "Open SFTP", false)
            .clicked
        {
            event = CardEvent::Files;
        }
        if ui
            .icon_button(id.with("connect"), connect, icons::PLAY, "Connect", false)
            .clicked
        {
            event = CardEvent::Connect;
        }
    } else if !card.tags.is_empty() {
        let width = tags_width(ui, &card.tags, inner.w * 0.4);
        let tags = inner.take_right(width);
        draw_tags(ui, tags, &card.tags);
        inner.take_right(space::S);
    }
    let name_line = Rect::new(inner.x, rect.y + 16.0, inner.w, 20.0);
    let detail = Rect::new(inner.x, rect.y + 37.0, inner.w, 18.0);
    ui.gfx
        .text(name_line, &card.label, BODY_STRONG, t.text, Align::Start);
    ui.gfx
        .text(detail, &card.detail, SMALL, t.text2, Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    event
}

/// The width the first tags take, up to `max`.
fn tags_width(ui: &mut Ui<'_>, tags: &[String], max: f32) -> f32 {
    let mut width = 0.0;
    for tag in tags.iter().take(2) {
        let chip = ui.gfx.measure(tag, SMALL).width + 2.0 * space::S + space::XS;
        if width + chip > max {
            break;
        }
        width += chip;
    }
    width
}

fn draw_tags(ui: &mut Ui<'_>, rect: Rect, tags: &[String]) {
    let t = ui.theme;
    let mut x = rect.x;
    for tag in tags.iter().take(2) {
        let width = ui.gfx.measure(tag, SMALL).width + 2.0 * space::S;
        if x + width > rect.r() + 0.5 {
            break;
        }
        ui.chip(Point::new(x, rect.center().y - 10.0), tag, t.text2);
        x += width + space::XS;
    }
}

fn list_header(ui: &mut Ui<'_>, rect: Rect) {
    let t = ui.theme;
    let [name, address, tags] = list_columns(rect);
    for (cell, title) in [(name, "Name"), (address, "Address"), (tags, "Tags")] {
        ui.gfx
            .text(cell, title, SMALL, t.text3.mix(t.text2, 0.5), Align::Start);
    }
    ui.gfx
        .fill_rect(Rect::new(rect.x, rect.b() - 1.0, rect.w, 1.0), 0.0, t.line);
}

fn list_columns(rect: Rect) -> [Rect; 3] {
    let mut row = rect.inset(space::M, 0.0);
    let name = row.take_left(row.w * 0.36);
    let address = row.take_left(row.w * 0.55);
    [name, address, row]
}

fn host_row(ui: &mut Ui<'_>, rect: Rect, card: &Card, selected: bool) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let fill = if selected {
        t.selected
    } else {
        t.hover.alpha(hover * 0.7)
    };
    ui.gfx.fill_rect(rect, 8.0, fill);
    if response.focus_ring {
        ui.focus_ring(rect, 8.0);
    }
    let [mut name, address, tags] = list_columns(rect);
    let tile = name.take_left(28.0).centered(28.0, 28.0);
    host_tile(ui, tile, &card.label);
    name.take_left(space::M);
    ui.gfx
        .text(name, &card.label, BODY_STRONG, t.text, Align::Start);
    ui.gfx
        .text(address, &card.detail, BODY, t.text2, Align::Start);
    draw_tags(ui, tags, &card.tags);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    if response.double_clicked {
        CardEvent::Connect
    } else if response.clicked {
        CardEvent::Select
    } else {
        CardEvent::None
    }
}

fn group_card(ui: &mut Ui<'_>, rect: Rect, group: &GroupCard) -> bool {
    let id = Id::new(("group", group.uid.as_str()));
    let response = ui.zone(id, rect, Role::ListItem, &group.name);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    ui.gfx
        .fill_rect(rect, 10.0, t.surface.mix(t.hover, hover * 0.35));
    ui.gfx
        .stroke_rect(rect, 10.0, 1.0, t.line.mix(t.line_strong, hover));
    if response.focus_ring {
        ui.focus_ring(rect, 10.0);
    }
    let mut inner = rect.inset(space::M + 2.0, 0.0);
    let tile = inner.take_left(32.0).centered(32.0, 32.0);
    ui.gfx.fill_rect(tile, 8.0, t.accent.alpha(0.14));
    ui.gfx
        .icon(icons::FOLDER, tile.centered(18.0, 18.0), t.accent);
    inner.take_left(space::M);
    let chevron = inner.take_right(16.0).centered(14.0, 14.0);
    ui.gfx
        .icon(icons::CHEVRON_RIGHT, chevron, t.text3.mix(t.text2, hover));
    let name_line = Rect::new(inner.x, rect.y + 11.0, inner.w, 20.0);
    let detail = Rect::new(inner.x, rect.y + 31.0, inner.w, 18.0);
    ui.gfx
        .text(name_line, &group.name, BODY_STRONG, t.text, Align::Start);
    ui.gfx
        .text(detail, &group.holds, SMALL, t.text2, Align::Start);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.double_clicked || response.clicked
}

fn no_results(ui: &mut Ui<'_>, rect: Rect, query: &str) {
    let t = ui.theme;
    let block = rect.centered(rect.w.min(460.0), 120.0);
    let mut column = block;
    let icon = column.take_top(32.0).centered(28.0, 28.0);
    ui.gfx.icon(icons::SEARCH, icon, t.text3);
    column.take_top(space::M);
    ui.gfx.text(
        column.take_top(22.0),
        &format!("No hosts match \u{201c}{query}\u{201d}"),
        BODY_STRONG,
        t.text,
        Align::Center,
    );
    ui.gfx.text(
        column.take_top(20.0),
        "Press Enter to connect to it as typed.",
        BODY,
        t.text2,
        Align::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_two_words_or_two_letters() {
        assert_eq!(initials("prod-db"), "PD");
        assert_eq!(initials("web"), "WE");
        assert_eq!(initials("cache 1 eu"), "C1");
        assert_eq!(initials("--"), "?");
    }

    #[test]
    fn the_grid_drops_columns_as_the_window_narrows() {
        assert_eq!(grid_columns(1200.0), 4);
        assert_eq!(grid_columns(560.0), 2);
        assert_eq!(grid_columns(400.0), 1);
        let cell = grid_cell(0.0, 0.0, 552.0, 2, 3, 72.0);
        assert_eq!(cell, Rect::new(282.0, 84.0, 270.0, 72.0));
    }

    #[test]
    fn the_editor_builds_a_host_and_refuses_a_bad_port_or_no_address() {
        let vault = super::super::demo::fixture_vault("editor-build", false);
        let mut editor = Editor::new(vault.draft_host(), true);
        editor.address = " db.example.com ".into();
        editor.port = "2222".into();
        editor.tags = "prod, , db".into();
        let Ok(host) = editor.build() else {
            unreachable!("a valid draft builds")
        };
        assert_eq!(host.label, "db.example.com");
        assert_eq!(host.port, Some(2222));
        assert_eq!(host.tags, ["prod", "db"]);
        editor.port = "ssh".into();
        assert!(editor.build().is_err());
        editor.port.clear();
        editor.address.clear();
        assert_eq!(editor.build().err().as_deref(), Some("Enter an address"));
    }
}
