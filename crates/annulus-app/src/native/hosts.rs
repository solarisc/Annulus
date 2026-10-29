//! The Hosts screen (UI-SPEC §2): a header, a search box that also quick-connects, groups and
//! host cards (or a list), an empty state, and the host editor in a side panel.

use annulus_config::tokens::space;
use annulus_session::Target;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Font, Rect, Weight};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::{Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, TITLE, button_width,
};
use annulus_vault::hosts::{DEFAULT_PORT, Host, Secret};
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::os;
use crate::{connect, groups_view};

/// Width of the editor panel.
const EDITOR_W: f32 = 392.0;
/// The narrowest the host list gets beside the editor; below it the editor covers the list.
const LIST_MIN_W: f32 = 420.0;
/// Height of a chip of the tag filter.
const CHIP_H: f32 = 28.0;
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
    /// Connect to every host directly in the group with this uid, one tab each (F-HOST-008).
    ConnectAll(String),
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
    /// The tags a listed host must all have (F-HOST-004); while any are chosen, matching
    /// hosts from every group are listed.
    pub tags: Vec<String>,
    /// The tag filter's chips are shown.
    tag_bar: bool,
    /// The editor, while open.
    pub editor: Option<Editor>,
    /// The group editor, while open.
    pub group_editor: Option<GroupEditor>,
    /// The `ssh_config` import preview, while open.
    import: Option<crate::import_dialog::ImportPreview>,
    /// The result of the last import, export or menu choice, until the next.
    notice: Option<String>,
    /// A host's More menu choice this frame: the host and the item.
    picked: Option<(u64, usize)>,
    /// Deleting a tag from every host: the tag, and whether the user said yes.
    untag: Option<(String, bool)>,
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
    /// The icon picked by hand (an OS key), or `None` to show what the server says.
    icon: Option<String>,
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
            icon: host.os_by_hand.then(|| host.os.clone()),
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
        host.port = port(&self.port)?;
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
        match &self.icon {
            Some(os) => {
                os.clone_into(&mut host.os);
                host.os_by_hand = true;
            }
            // Back to automatic: the next connection says what it is.
            None if self.host.os_by_hand => {
                host.os.clear();
                host.os_by_hand = false;
            }
            None => {}
        }
        Ok(host)
    }

    /// The host to save: as edited, over what the vault holds now. A field edited here wins;
    /// every other field keeps what changed meanwhile elsewhere (a move from the card's menu,
    /// a sync), instead of the copy taken when the editor opened. A host deleted meanwhile is
    /// not brought back.
    fn merged(&self, current: Option<&Host>) -> Result<Host, String> {
        let edited = self.build()?;
        if self.is_new {
            return Ok(edited);
        }
        let Some(current) = current else {
            return Err("This host was deleted while you edited it, so it was not saved.".into());
        };
        let before = &self.host;
        let mut host = current.clone();
        // Every field by name, so a new one cannot be forgotten here.
        let Host {
            id: _,
            label,
            address,
            port,
            username,
            tags,
            password,
            key_id,
            jump_id,
            proxy,
            no_reconnect,
            group,
            env,
            startup,
            term,
            os,
            os_by_hand,
        } = edited;
        // The icon goes as one: what the server said, or what was picked.
        if (os.as_str(), os_by_hand) != (before.os.as_str(), before.os_by_hand) {
            host.os = os;
            host.os_by_hand = os_by_hand;
        }
        if label != before.label {
            host.label = label;
        }
        if address != before.address {
            host.address = address;
        }
        if port != before.port {
            host.port = port;
        }
        if username != before.username {
            host.username = username;
        }
        if tags != before.tags {
            host.tags = tags;
        }
        if password != before.password {
            host.password = password;
        }
        if key_id != before.key_id {
            host.key_id = key_id;
        }
        if jump_id != before.jump_id {
            host.jump_id = jump_id;
        }
        if proxy != before.proxy {
            host.proxy = proxy;
        }
        if no_reconnect != before.no_reconnect {
            host.no_reconnect = no_reconnect;
        }
        if group != before.group {
            host.group = group;
        }
        if env != before.env {
            host.env = env;
        }
        if startup != before.startup {
            host.startup = startup;
        }
        if term != before.term {
            host.term = term;
        }
        Ok(host)
    }
}

/// A port typed in an editor: empty for the default, else 1 to 65535.
fn port(text: &str) -> Result<Option<u16>, String> {
    match text.trim() {
        "" => Ok(None),
        port => port
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .map(Some)
            .ok_or_else(|| "The port is a number from 1 to 65535".to_owned()),
    }
}

/// What the screen shows of one host.
struct Card {
    id: u64,
    label: String,
    detail: String,
    tags: Vec<String>,
    os: Option<&'static os::Os>,
}

/// What the screen shows of one group.
struct GroupCard {
    uid: String,
    name: String,
    holds: String,
}

/// Every tag in use and how many hosts have it, by name.
fn all_tags(hosts: &[Host]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for tag in hosts.iter().flat_map(|host| &host.tags) {
        *counts.entry(tag.clone()).or_default() += 1;
    }
    counts.into_iter().collect()
}

/// Whether `host` has every tag in `filter`.
fn has_tags(host: &Host, filter: &[String]) -> bool {
    filter.iter().all(|tag| host.tags.contains(tag))
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
        let open = self.editor.is_some() || self.group_editor.is_some();
        // Beside the list while the list keeps room for its header and cards; over it in a
        // narrow window, where both side by side would be too cramped to read.
        let beside = area.w - EDITOR_W >= LIST_MIN_W;
        let panel = open.then(|| {
            if beside {
                area.take_right(EDITOR_W.min(area.w * 0.6))
            } else {
                area
            }
        });
        let vault = vault?;
        if !open || beside {
            action = self.listing_area(ui, area, vault).or(action);
        }
        if action == Some(HostsAction::ImportSshConfig) {
            action = None;
            match crate::import_dialog::ImportPreview::load(vault) {
                Ok(preview) => self.import = Some(preview),
                Err(problem) => self.notice = Some(problem),
            }
        }
        if let Some(panel) = panel {
            if self.group_editor.is_some() {
                self.group_panel(ui, panel, vault);
            } else {
                self.editor_panel(ui, panel, vault);
            }
        }
        self.import_card(ui, rect, vault);
        action
    }

    /// Whether a search or the tag filter narrows the listing to matching hosts.
    fn filtering(&self) -> bool {
        !self.query.trim().is_empty() || !self.tags.is_empty()
    }

    /// What the search box holds as a new host, when it is an SSH target (`user@host:port`)
    /// that no saved host has the label of (F-APP-006).
    fn unsaved_target(&self, vault: &Vault, query: &str) -> Option<Host> {
        if !query.contains(['@', ':', '.'])
            || query.contains(char::is_whitespace)
            || vault.hosts().iter().any(|host| host.label == query)
        {
            return None;
        }
        let target = Target::parse(query, "")?;
        let mut host = vault.draft_host();
        host.label.clone_from(&target.host);
        host.address = target.host;
        host.port = (target.port != DEFAULT_PORT).then_some(target.port);
        host.username = target.user;
        host.group.clone_from(&self.group);
        Some(host)
    }

    /// Everything left of the editor: header, toolbar, and the list or an empty state.
    fn listing_area(
        &mut self,
        ui: &mut Ui<'_>,
        area: Rect,
        vault: &mut Vault,
    ) -> Option<HostsAction> {
        let mut action = None;
        let tags = all_tags(vault.hosts());
        // A tag no host has any more cannot filter.
        self.tags
            .retain(|chosen| tags.iter().any(|(tag, _)| tag == chosen));
        let (cards, groups) = self.gather(vault);
        let mut main = area.inset(PAD, 0.0);
        main.take_top(space::XL);
        self.header(ui, main.take_top(36.0), cards.len(), !tags.is_empty());
        main.take_top(space::L);
        if let Some(toolbar) = self.toolbar(ui, main.take_top(CONTROL), vault) {
            action = Some(toolbar);
        }
        main.take_top(space::L);
        if self.tag_bar && !tags.is_empty() {
            let used = self.tag_chips(ui, main, &tags);
            main.take_top(used + space::L);
        }
        if let Some((tag, true)) = self.untag.clone() {
            self.untag = None;
            self.notice = Some(untag_everywhere(vault, &tag));
            self.tags.retain(|chosen| *chosen != tag);
        }
        if let Some(notice) = &self.notice {
            let t = ui.theme;
            ui.gfx
                .text(main.take_top(20.0), notice, SMALL, t.text2, Align::Start);
            main.take_top(space::S);
        }
        if self.group.is_some() && !self.filtering() {
            if let Some(all) = self.breadcrumb(ui, main.take_top(CONTROL), vault) {
                action = Some(all);
            }
            main.take_top(space::M);
        }
        if cards.is_empty() && groups.is_empty() && (self.group.is_none() || self.filtering()) {
            if self.filtering() {
                let typed = self.unsaved_target(vault, self.query.trim());
                if no_results(ui, main, self.query.trim(), typed.is_some())
                    && let Some(host) = typed
                {
                    self.group_editor = None;
                    self.editor = Some(Editor::new(host, true));
                    self.query.clear();
                }
            } else if let Some(empty) = self.empty_state(ui, main, vault) {
                action = Some(empty);
            }
            return action;
        }
        let moves = groups_view::choices(vault.groups(), None);
        if let Some(listed) = self.listing(ui, main, &cards, &groups, vault, &host_menu(&moves)) {
            action = Some(listed);
        }
        if let Some((id, choice)) = self.picked.take() {
            self.host_menu_action(ui, vault, id, choice, &moves);
        }
        action
    }

    /// Acts on a host's More menu choice: see [`host_menu`].
    fn host_menu_action(
        &mut self,
        ui: &mut Ui<'_>,
        vault: &mut Vault,
        id: u64,
        choice: usize,
        moves: &[(String, String)],
    ) {
        let Some(host) = vault.hosts().iter().find(|host| host.id == id).cloned() else {
            return;
        };
        let Some(choice) = choice.checked_sub(MENU_REST) else {
            return;
        };
        self.notice = Some(match choice {
            0 => {
                ui.copy(connect::ssh_command(vault, &host));
                format!("Copied the ssh command for {}.", host.label)
            }
            1 => {
                let label = format!("{} copy", host.label);
                let copy = Host {
                    id: vault.draft_host().id,
                    label: label.clone(),
                    ..host
                };
                match vault.upsert_host(copy) {
                    Ok(()) => format!("Added {label}."),
                    Err(error) => format!("Could not duplicate: {error}"),
                }
            }
            _ => {
                let label = host.label.clone();
                let group = choice
                    .checked_sub(3)
                    .and_then(|at| moves.get(at))
                    .map(|(uid, path)| (uid.clone(), path.clone()));
                let regrouped = Host {
                    group: group.as_ref().map(|(uid, _)| uid.clone()),
                    ..host
                };
                let place = group.map_or_else(|| "the top level".to_owned(), |(_, path)| path);
                match vault.upsert_host(regrouped) {
                    Ok(()) => format!("Moved {label} to {place}."),
                    Err(error) => format!("Could not move: {error}"),
                }
            }
        });
    }

    /// The hosts and groups to list: those of the group shown, or the hosts of every group
    /// that match the search and have every chosen tag.
    fn gather(&self, vault: &Vault) -> (Vec<Card>, Vec<GroupCard>) {
        let query = self.query.trim().to_lowercase();
        let searching = !query.is_empty();
        let filtering = self.filtering();
        let cards = vault
            .hosts()
            .iter()
            .filter(|host| filtering || host.group == self.group)
            .filter(|host| has_tags(host, &self.tags))
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
                    os: os::of(host),
                }
            })
            .collect();
        let groups = if filtering {
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

    /// The title, the host count, the Tags button (when any host has a tag) and the grid/list
    /// toggle.
    fn header(&mut self, ui: &mut Ui<'_>, rect: Rect, count: usize, tagged: bool) {
        let t = ui.theme;
        let mut row = rect;
        // Laid out from the right, drawn from the left: Tab follows the drawing order.
        let toggle = row.take_right(72.0);
        let tags = tagged.then(|| {
            row.take_right(space::S);
            let width = button_width(&mut ui.gfx, "Tags", Some(icons::TAG));
            row.take_right(width).centered(width, CONTROL)
        });
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
        let kind = if self.tag_bar {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
        if let Some(tags) = tags
            && ui
                .button(Id::new("hosts.tags"), tags, "Tags", Some(icons::TAG), kind)
                .clicked
        {
            // Closing the chips stops filtering: a filter is never on out of sight.
            self.tag_bar = !self.tag_bar;
            self.tags.clear();
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

    fn toolbar(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) -> Option<HostsAction> {
        let mut row = rect;
        let mut action = None;
        let new_w = button_width(&mut ui.gfx, "New host", Some(icons::PLUS));
        let term_w = button_width(&mut ui.gfx, "Terminal", Some(icons::TERMINAL));
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
            // Matched against the text as it is now: what was typed this frame counts.
            let (cards, _) = self.gather(vault);
            let query = self.query.trim();
            // Enter with nothing typed connects nowhere: the first card is just the first.
            if !query.is_empty() {
                action = Some(match cards.first() {
                    Some(first) => HostsAction::Connect(first.id),
                    None => HostsAction::QuickConnect(query.to_owned()),
                });
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
        let items = vec![
            "New group".to_owned(),
            "Import from ssh_config".to_owned(),
            "Copy hosts as ssh_config".to_owned(),
        ];
        let chosen = ui.menu_button(
            Id::new("hosts.new.more"),
            more,
            "",
            Some(icons::CHEVRON_DOWN),
            Kind::Primary,
            &items,
        );
        ui.set_name(Id::new("hosts.new.more"), "More ways to add hosts");
        ui.tooltip(
            Id::new("hosts.new.more"),
            more,
            "New group, import or export",
        );
        match chosen {
            Some(0) => self.new_group(vault),
            Some(1) => action = Some(HostsAction::ImportSshConfig),
            Some(_) => {
                let resolved: Vec<Host> = vault
                    .hosts()
                    .iter()
                    .map(|host| vault.resolve(host).0)
                    .collect();
                ui.copy(annulus_import::ssh_config::export(&resolved));
                self.notice = Some("Copied the hosts as an OpenSSH config (no passwords).".into());
            }
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

    /// The tag filter (F-HOST-004): a chip per tag in use, `(name, hosts)`, that chooses or
    /// drops it, wrapping onto more lines as needed, and Clear while any is chosen. Returns
    /// the height taken from the top of `rect`.
    fn tag_chips(&mut self, ui: &mut Ui<'_>, rect: Rect, tags: &[(String, usize)]) -> f32 {
        let (mut x, mut y) = (rect.x, rect.y);
        // The next chip's place, on a new line when the one it is on is full.
        let mut place = |width: f32| {
            let width = width.min(rect.w);
            if x > rect.x && x + width > rect.r() {
                (x, y) = (rect.x, y + CHIP_H + space::S);
            }
            let slot = Rect::new(x, y, width, CHIP_H);
            x += width + space::S;
            slot
        };
        let mut flipped = None;
        for (tag, count) in tags {
            let label = format!("{tag} ({count})");
            let chip = place(ui.gfx.measure(&label, SMALL).width + 2.0 * space::M);
            let on = self.tags.contains(tag);
            if tag_chip(ui, Id::new(("hosts.tag", tag.as_str())), chip, &label, on) {
                flipped = Some(tag.clone());
            }
        }
        if !self.tags.is_empty() {
            let clear = place(button_width(&mut ui.gfx, "Clear", None));
            if ui
                .button(
                    Id::new("hosts.tags.clear"),
                    clear,
                    "Clear",
                    None,
                    Kind::Ghost,
                )
                .clicked
            {
                self.tags.clear();
            }
        }
        // One tag chosen: it can be taken off every host, after a yes.
        if let [only] = self.tags.as_slice() {
            let only = only.clone();
            let count = tags
                .iter()
                .find(|(tag, _)| *tag == only)
                .map_or(0, |(_, count)| *count);
            match &self.untag {
                Some((tag, false)) if *tag == only => {
                    let label = match count {
                        1 => format!("Delete \u{201c}{only}\u{201d} from 1 host"),
                        n => format!("Delete \u{201c}{only}\u{201d} from {n} hosts"),
                    };
                    let cancel = place(button_width(&mut ui.gfx, "Cancel", None));
                    let yes = place(button_width(&mut ui.gfx, &label, None));
                    if ui
                        .button(
                            Id::new("hosts.untag.cancel"),
                            cancel,
                            "Cancel",
                            None,
                            Kind::Ghost,
                        )
                        .clicked
                        || ui.key(Key::Escape, Modifiers::NONE)
                    {
                        self.untag = None;
                    }
                    if ui
                        .button(Id::new("hosts.untag.yes"), yes, &label, None, Kind::Danger)
                        .clicked
                    {
                        self.untag = Some((only, true));
                    }
                }
                Some((_, true)) => {}
                _ => {
                    let ask = place(button_width(&mut ui.gfx, "Delete tag\u{2026}", None));
                    if ui
                        .button(
                            Id::new("hosts.untag"),
                            ask,
                            "Delete tag\u{2026}",
                            None,
                            Kind::Ghost,
                        )
                        .clicked
                    {
                        self.untag = Some((only, false));
                    }
                }
            }
        } else if self.untag.as_ref().is_some_and(|(_, yes)| !yes) {
            self.untag = None;
        }
        if let Some(tag) = flipped {
            if self.tags.contains(&tag) {
                self.tags.retain(|chosen| *chosen != tag);
            } else {
                self.tags.push(tag);
            }
        }
        y + CHIP_H - rect.y
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
        let new_w = button_width(&mut ui.gfx, "New host", Some(icons::PLUS));
        let import_w = button_width(&mut ui.gfx, "Import ssh_config", Some(icons::DOWNLOAD));
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
        menu: &[String],
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
        let heading = if self.filtering() {
            format!("Results \u{b7} {}", cards.len())
        } else {
            format!("Hosts \u{b7} {}", cards.len())
        };
        ui.section_label(label, &heading);
        y += 28.0;
        match self.mode {
            Mode::Grid => {
                for (index, card) in cards.iter().enumerate() {
                    let cell = grid_cell(rect.x, y, rect.w - 8.0, columns, index, CARD_H);
                    // Cards fill in one after another as the screen appears.
                    let shown = ui.entrance(
                        Id::new(("host", card.id)).with("enter"),
                        Motion::SLOW.staggered(index),
                    );
                    ui.push_layer(Layer::fade(shown).shifted(0.0, 8.0 * (1.0 - shown)));
                    let event = host_card(ui, cell, card, self.selected == Some(card.id), menu);
                    ui.pop_layer();
                    if let Some(found) = self.card_event(vault, card.id, event) {
                        action = Some(found);
                    }
                }
            }
            Mode::List => {
                list_header(ui, Rect::new(rect.x, y, rect.w - 8.0, ROW_H - 12.0));
                y += ROW_H - 12.0;
                for card in cards {
                    let row = Rect::new(rect.x, y, rect.w - 8.0, ROW_H);
                    let event = host_row(ui, row, card, self.selected == Some(card.id), menu);
                    if let Some(found) = self.card_event(vault, card.id, event) {
                        action = Some(found);
                    }
                    y += ROW_H + GAP;
                }
            }
        }
        ui.pop_clip();
        action
    }

    /// Acts on what a host's card or row did; returns a tab to open.
    fn card_event(&mut self, vault: &Vault, id: u64, event: CardEvent) -> Option<HostsAction> {
        match event {
            CardEvent::Select => self.selected = Some(id),
            CardEvent::Connect | CardEvent::More(MENU_CONNECT) => {
                return Some(HostsAction::Connect(id));
            }
            CardEvent::More(MENU_SFTP) => return Some(HostsAction::Sftp(id)),
            CardEvent::Edit | CardEvent::More(MENU_EDIT) => self.edit_host(vault, id),
            CardEvent::More(choice) => self.picked = Some((id, choice)),
            CardEvent::None => {}
        }
        None
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
        let (inherited, from) = inherited(vault, &editor.host, editor.group.clone());
        let user_hint = hint(&inherited.username, from.username.as_ref(), "root");
        let port = inherited
            .port
            .map(|port| port.to_string())
            .unwrap_or_default();
        let port_hint = hint(&port, from.port.as_ref(), "22");
        let startup_hint = hint(&inherited.startup, from.startup.as_ref(), "tmux attach");
        // A proxy can hold a password: say where it comes from, never what it is.
        let proxy_hint = from.proxy.as_ref().map_or_else(
            || "socks5://host:1080 or http://host:3128".to_owned(),
            |group| format!("From {group}"),
        );
        let (left, right) = form.split(96.0);
        let mut port_form = Form { w: left, ..form };
        port_form.text(
            ui,
            "editor.port",
            "Port",
            &mut editor.port,
            &port_hint,
            None,
        );
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
        // The card's icon: what the server said it runs, or one picked here (F-HOST-009).
        let detected = os::of(&editor.host).filter(|_| !editor.host.os_by_hand);
        let mut icon_items = vec![detected.map_or_else(
            || "Automatic, from the server".to_owned(),
            |os| format!("Automatic ({})", os.name),
        )];
        icon_items.extend(os::ALL.iter().map(|os| os.name.to_owned()));
        let icon_at = editor
            .icon
            .as_deref()
            .and_then(|key| os::ALL.iter().position(|os| os.key == key))
            .map_or(0, |at| at + 1);
        if let Some(chosen) = form.select(ui, "editor.icon", "Icon", &icon_items, Some(icon_at)) {
            editor.icon = chosen.checked_sub(1).map(|at| os::ALL[at].key.to_owned());
        }

        form.gap(space::S);
        form.section(ui, "Credentials");
        form.text(
            ui,
            "editor.user",
            "Username",
            &mut editor.username,
            &user_hint,
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
        let inherited_key = inherited
            .key_id
            .and_then(|id| keys.iter().find(|(known, _)| *known == id))
            .zip(from.key_id.as_ref());
        let mut key_items = vec![inherited_key.map_or_else(
            || "Default keys (~/.ssh)".to_owned(),
            |((_, key), group)| format!("{key} \u{b7} from {group}"),
        )];
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
            let inherited_jump = inherited
                .jump_id
                .and_then(|id| hosts.iter().find(|(known, _)| *known == id))
                .zip(from.jump_id.as_ref());
            let mut jump_items = vec![inherited_jump.map_or_else(
                || "None (direct)".to_owned(),
                |((_, jump), group)| format!("{jump} \u{b7} from {group}"),
            )];
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
            form.secret(ui, "editor.proxy", "Proxy", &mut editor.proxy, &proxy_hint);
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
                &startup_hint,
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
        let save_w = button_width(&mut ui.gfx, "Save", None).max(88.0);
        let save = buttons.take_right(save_w);
        buttons.take_right(space::S);
        let cancel_w = button_width(&mut ui.gfx, "Cancel", None);
        let cancel = buttons.take_right(cancel_w);
        let mut saved = false;
        if !editor.is_new {
            let delete_w = button_width(&mut ui.gfx, "Delete", Some(icons::TRASH));
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
            let current = vault.hosts().iter().find(|host| host.id == editor.host.id);
            match editor.merged(current) {
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
        defaults.port = port(&self.port)?;
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

    /// The `ssh_config` import, over the screen.
    fn import_card(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) {
        let Some(preview) = self.import.as_mut() else {
            return;
        };
        let t = ui.theme;
        ui.gfx.fill_rect(rect, 0.0, t.scrim);
        ui.zone(Id::new("import.backdrop"), rect, Role::Backdrop, "");
        #[allow(clippy::cast_precision_loss)] // A config holds dozens of hosts.
        let list_h = (preview.rows.len() as f32 * 36.0).min(320.0);
        let card = rect.centered(rect.w.min(560.0), list_h + 170.0);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        ui.strict_text(
            column.take_top(24.0),
            "Import from OpenSSH config",
            HEADING,
            t.text,
            Align::Start,
        );
        ui.gfx.text(
            column.take_top(20.0),
            "Choose the hosts to add. ProxyJump becomes a jump host when that host comes too.",
            SMALL,
            t.text2,
            Align::Start,
        );
        column.take_top(space::M);
        let list = column.take_top(list_h);
        #[allow(clippy::cast_precision_loss)] // As above.
        let offset = ui.scroll_view(
            Id::new("import.scroll"),
            list,
            preview.rows.len() as f32 * 36.0,
        );
        ui.push_clip(list);
        for (index, (entry, selected)) in preview.rows.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * 36.0;
            let mut row = Rect::new(list.x, y, list.w - 10.0, 32.0);
            let mut detail = format!("{}:{}", entry.address, entry.port);
            if let Some(user) = &entry.user {
                detail = format!("{user}@{detail}");
            }
            if let Some(jump) = &entry.proxy_jump {
                detail.push_str(" via ");
                detail.push_str(jump);
            }
            if preview.existing.contains(&entry.alias.to_lowercase()) {
                detail.push_str(" \u{b7} already saved");
            }
            let text = row.take_right(row.w * 0.55);
            ui.checkbox(Id::new(("import", index)), row, selected, &entry.alias);
            ui.gfx.text(text, &detail, SMALL, t.text2, Align::Start);
        }
        ui.pop_clip();
        column.take_top(space::L);
        let mut buttons = column.take_top(CONTROL);
        let chosen: Vec<&annulus_import::ssh_config::Found> = preview
            .rows
            .iter()
            .filter(|(_, on)| *on)
            .map(|(entry, _)| entry)
            .collect();
        let label = format!("Import {}", chosen.len());
        let import = buttons.take_right(button_width(&mut ui.gfx, &label, None).max(100.0));
        buttons.take_right(space::S);
        let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
        let go = ui
            .button(Id::new("import.go"), import, &label, None, Kind::Primary)
            .clicked
            && !chosen.is_empty();
        let mut close = ui
            .button(
                Id::new("import.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Ghost,
            )
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE);
        if go {
            let (hosts, rules) = crate::import_dialog::entities(&chosen);
            self.notice = Some(match vault.adopt(&hosts, &rules, &[], &[], &[]) {
                Ok(()) => format!(
                    "Imported {} host(s) and {} forward(s).",
                    hosts.len(),
                    rules.len()
                ),
                Err(error) => format!("Nothing was imported: {error}"),
            });
            close = true;
        }
        if close {
            self.import = None;
        }
    }

    /// The path back up from the group shown (All hosts, then each parent), its edit button,
    /// and Connect all at the right while hosts are directly in it.
    fn breadcrumb(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &Vault) -> Option<HostsAction> {
        let t = ui.theme;
        let mut chain: Vec<(String, String)> =
            annulus_vault::groups::chain(vault.groups(), self.group.as_deref())
                .iter()
                .map(|group| (group.uid.clone(), group.name.clone()))
                .collect();
        chain.reverse();
        let mut row = rect;
        // Laid out first, drawn last: Tab follows the drawing order, left to right.
        let connect_all = vault
            .hosts()
            .iter()
            .any(|host| host.group.is_some() && host.group == self.group)
            .then(|| row.take_right(button_width(&mut ui.gfx, "Connect all", Some(icons::PLAY))));
        let mut go = None;
        let all_w = button_width(&mut ui.gfx, "All hosts", None);
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
            let width = button_width(&mut ui.gfx, name, None);
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
        let mut action = None;
        if let (Some(slot), Some(uid)) = (connect_all, self.group.clone())
            && ui
                .button(
                    Id::new("crumb.connect-all"),
                    slot,
                    "Connect all",
                    Some(icons::PLAY),
                    Kind::Secondary,
                )
                .clicked
        {
            action = Some(HostsAction::ConnectAll(uid));
        }
        if let Some(target) = go {
            self.group = target;
        }
        action
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
            let up_w = button_width(&mut ui.gfx, "Move its hosts up", None);
            let all_w = button_width(&mut ui.gfx, "Delete its hosts too", None);
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
            let save = buttons.take_right(button_width(&mut ui.gfx, "Save", None).max(88.0));
            buttons.take_right(space::S);
            let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
            if !editor.is_new {
                let delete_w = button_width(&mut ui.gfx, "Delete", Some(icons::TRASH));
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
        // A screen reader says "Username", not the example in the empty field.
        ui.set_name(Id::new(id), label);
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

#[derive(Clone, Copy)]
enum CardEvent {
    None,
    Select,
    Connect,
    Edit,
    /// An item of the host's context menu.
    More(usize),
}

/// The first items of a host's context menu, which the listing handles itself; the rest go to
/// [`HostsView::host_menu_action`].
const MENU_CONNECT: usize = 0;
const MENU_SFTP: usize = 1;
const MENU_EDIT: usize = 2;
/// Where the items [`HostsView::host_menu_action`] handles start.
const MENU_REST: usize = 3;

/// A host's context menu (right click, or Shift+F10): connect, open its files, edit, copy its
/// ssh command, duplicate it, or move it to the top level or a group of `moves`
/// (`(uid, path)`).
fn host_menu(moves: &[(String, String)]) -> Vec<String> {
    let mut items = vec![
        "Connect".to_owned(),
        "Open SFTP".to_owned(),
        "Edit".to_owned(),
        "Copy ssh command".to_owned(),
        "Duplicate".to_owned(),
        "Move to the top level".to_owned(),
    ];
    items.extend(moves.iter().map(|(_, path)| format!("Move to {path}")));
    items
}

/// Takes `tag` off every host that has it (F-HOST-004); the message to show.
fn untag_everywhere(vault: &mut Vault, tag: &str) -> String {
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
fn inherited(
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
fn hint(value: &str, group: Option<&String>, example: &str) -> String {
    match group {
        Some(group) if !value.is_empty() => format!("{value} \u{b7} from {group}"),
        _ => example.to_owned(),
    }
}

/// The Edit button of a host card or row, faded in by `shown` (hovered, selected or focused);
/// the rest of what can be done with a host is in its context menu. Returns whether it was
/// clicked.
fn edit_button(ui: &mut Ui<'_>, id: Id, rect: Rect, shown: f32) -> bool {
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

fn host_card(
    ui: &mut Ui<'_>,
    rect: Rect,
    card: &Card,
    selected: bool,
    menu: &[String],
) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    // The card stays lit while its context menu is open over it.
    let hovered = ui.hover_within(rect) || ui.menu_open(id);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let press = ui.ease(id.with("press"), if response.pressed { 1.0 } else { 0.0 });
    let chosen = ui.animate(
        id.with("selected"),
        if selected { 1.0 } else { 0.0 },
        Motion::BASE,
    );
    // A hovered card lifts a little on a soft shadow; pressing sets it down again.
    let lift = hover * (1.0 - press);
    let shape = rect.offset(0.0, -1.5 * lift);
    if lift > 0.0 {
        ui.gfx
            .shadow(shape, 12.0, 18.0, 6.0, t.shadow.alpha(lift * 0.5));
    }
    let fill = t
        .surface
        .mix(t.hover, hover * 0.3)
        .mix(t.selected, chosen * 0.6);
    ui.gfx.fill_rect(shape, 12.0, fill);
    let border = t.line.mix(t.line_strong, hover).mix(t.accent, chosen);
    ui.gfx.stroke_rect(shape, 12.0, 1.0 + 0.5 * chosen, border);
    if response.focus_ring {
        ui.focus_ring(shape, 12.0);
    }
    let mut inner = shape.inset(space::M + 2.0, 0.0);
    let tile = inner.take_left(40.0).centered(40.0, 40.0);
    os::tile(ui, tile, &card.label, card.os);
    inner.take_left(space::M);
    // Kept for the Edit button whether it shows or not, so the text never moves under it.
    let edit = inner.take_right(28.0).centered(28.0, 28.0);
    inner.take_right(space::S);
    let mut event = if response.double_clicked {
        CardEvent::Connect
    } else if response.clicked {
        CardEvent::Select
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
    let name_line = Rect::new(inner.x, shape.y + 15.0, inner.w, 20.0);
    let mut detail = Rect::new(inner.x, shape.y + 37.0, inner.w, 20.0);
    ui.gfx
        .text(name_line, &card.label, BODY_STRONG, t.text, Align::Start);
    // The tags share the second line with the address, which keeps at least half of it.
    let (count, width) = tags_fit(ui, &card.tags, detail.w * 0.5);
    if width > 0.0 {
        let tags = detail.take_right(width);
        draw_tags(ui, tags, &card.tags, count);
        detail.take_right(space::S);
    }
    ui.gfx
        .text(detail, &card.detail, SMALL, t.text2, Align::Start);
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

fn tag_width(ui: &mut Ui<'_>, label: &str) -> f32 {
    ui.gfx.measure(label, TAG).width + 14.0
}

/// How many of `tags` fit in `max` with a "+n" chip for the rest, and the width they take;
/// nothing when not even that fits.
fn tags_fit(ui: &mut Ui<'_>, tags: &[String], max: f32) -> (usize, f32) {
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
fn draw_tags(ui: &mut Ui<'_>, rect: Rect, tags: &[String], count: usize) {
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
fn tag_chip(ui: &mut Ui<'_>, id: Id, rect: Rect, label: &str, on: bool) -> bool {
    let response = ui.zone(id, rect, Role::CheckBox, label);
    ui.set_toggled(id, on);
    let t = ui.theme;
    let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
    let round = rect.h / 2.0;
    let (fill, border, ink) = if on {
        (t.selected, t.accent, t.accent)
    } else {
        (
            t.surface.mix(t.hover, hover),
            t.line.mix(t.line_strong, hover),
            t.text2.mix(t.text, hover),
        )
    };
    ui.gfx.fill_rect(rect, round, fill);
    ui.gfx.stroke_rect(rect, round, 1.0, border);
    if response.focus_ring {
        ui.focus_ring(rect, round);
    }
    ui.gfx.text(rect, label, SMALL, ink, Align::Center);
    if response.hovered {
        ui.set_cursor(Cursor::Pointer);
    }
    response.clicked
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

fn host_row(
    ui: &mut Ui<'_>,
    rect: Rect,
    card: &Card,
    selected: bool,
    menu: &[String],
) -> CardEvent {
    let id = Id::new(("host", card.id));
    let response = ui.zone(id, rect, Role::ListItem, &card.label);
    let t = ui.theme;
    let hovered = ui.hover_within(rect) || ui.menu_open(id);
    let hover = ui.ease(id.with("hover"), if hovered { 1.0 } else { 0.0 });
    let chosen = ui.animate(
        id.with("selected"),
        if selected { 1.0 } else { 0.0 },
        Motion::BASE,
    );
    let fill = t.hover.alpha(hover * 0.7).mix(t.selected, chosen);
    ui.gfx.fill_rect(rect, 8.0, fill);
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
        .text(name, &card.label, BODY_STRONG, t.text, Align::Start);
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

/// What a search or tag filter that matches no host shows, with Save as host when `query`
/// is a target that can be saved (F-APP-006). Returns whether that was clicked.
fn no_results(ui: &mut Ui<'_>, rect: Rect, query: &str, can_save: bool) -> bool {
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
        BODY_STRONG,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_field_shows_what_the_host_takes_from_its_group() {
        let mut vault = crate::native::demo::fixture_vault("inherited-hints", false);
        let mut group = vault.draft_group(None).unwrap();
        group.name = "Production".into();
        group.defaults.username = "deploy".into();
        group.defaults.port = Some(2222);
        let uid = group.uid.clone();
        vault.upsert_group(group).unwrap();
        let host = vault.draft_host();
        let (values, from) = inherited(&vault, &host, Some(uid));
        assert_eq!(
            hint(&values.username, from.username.as_ref(), "root"),
            "deploy \u{b7} from Production"
        );
        let port = values.port.map(|port| port.to_string()).unwrap_or_default();
        assert_eq!(
            hint(&port, from.port.as_ref(), "22"),
            "2222 \u{b7} from Production"
        );
        let (top, from) = inherited(&vault, &host, None);
        assert_eq!(
            hint(&top.username, from.username.as_ref(), "root"),
            "root",
            "no group"
        );
    }

    #[test]
    fn saving_an_editor_keeps_what_changed_meanwhile_and_applies_what_was_edited() {
        let vault = crate::native::demo::fixture_vault("stale-editor", false);
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.5".into());
        let mut editor = Editor::new(host.clone(), false);
        "deploy".clone_into(&mut editor.username);
        // Moved from the card's menu while the editor was open.
        let mut moved = host.clone();
        moved.group = Some("group-1".into());
        let saved = editor.merged(Some(&moved)).unwrap();
        assert_eq!(saved.group.as_deref(), Some("group-1"), "the move stays");
        assert_eq!(saved.username, "deploy", "the edit is saved");
        // The group edited here wins over the move.
        editor.group = Some("group-2".into());
        let saved = editor.merged(Some(&moved)).unwrap();
        assert_eq!(saved.group.as_deref(), Some("group-2"));
    }

    #[test]
    fn a_host_deleted_while_it_was_edited_is_not_brought_back() {
        let vault = crate::native::demo::fixture_vault("deleted-editor", false);
        let mut host = vault.draft_host();
        host.address = "10.0.0.5".into();
        assert!(Editor::new(host.clone(), false).merged(None).is_err());
        assert!(
            Editor::new(host, true).merged(None).is_ok(),
            "a new host is new"
        );
    }

    #[test]
    fn a_port_is_empty_or_from_1_to_65535() {
        assert_eq!(port(""), Ok(None));
        assert_eq!(port(" 2222 "), Ok(Some(2222)));
        for wrong in ["0", "65536", "-1", "ssh"] {
            assert!(port(wrong).is_err(), "{wrong}");
        }
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

    #[test]
    fn tags_are_counted_and_a_filter_needs_every_chosen_tag() {
        let mut vault = super::super::demo::fixture_vault("tag-filter", false);
        let Ok(mut group) = vault.draft_group(None) else {
            unreachable!("a draft group in an empty vault")
        };
        group.name = "Lab".into();
        assert!(vault.upsert_group(group.clone()).is_ok());
        for (label, tags, group) in [
            ("tagged", &["prod", "db"][..], None),
            ("web", &["prod"][..], None),
            ("grouped", &["db", "prod"][..], Some(group.uid.clone())),
        ] {
            let mut host = vault.draft_host();
            (host.label, host.address, host.group) = (label.into(), format!("{label}.lan"), group);
            host.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
            assert!(vault.upsert_host(host).is_ok());
        }
        assert_eq!(
            all_tags(vault.hosts()),
            [("db".to_owned(), 2), ("prod".to_owned(), 3)]
        );
        let host = |label: &str| vault.hosts().iter().find(|host| host.label == label);
        let both = ["prod".to_owned(), "db".to_owned()];
        assert!(host("tagged").is_some_and(|host| has_tags(host, &both)));
        assert!(
            host("web").is_some_and(|host| !has_tags(host, &both)),
            "all, not any"
        );
        assert!(host("web").is_some_and(|host| has_tags(host, &[])));
        // The listing with the filter on: every group's hosts with both tags, and no groups.
        let view = HostsView {
            tags: both.to_vec(),
            ..HostsView::default()
        };
        let (cards, groups) = view.gather(&vault);
        let labels: Vec<&str> = cards.iter().map(|card| card.label.as_str()).collect();
        assert_eq!(labels, ["grouped", "tagged"]);
        assert!(groups.is_empty());
    }

    #[test]
    fn only_a_target_no_host_is_labelled_with_becomes_a_draft_in_the_group_shown() {
        let mut vault = super::super::demo::fixture_vault("unsaved-target", false);
        let mut saved = vault.draft_host();
        (saved.label, saved.address) = ("db.example.com".into(), "db.example.com".into());
        assert!(vault.upsert_host(saved).is_ok());
        let view = HostsView {
            group: Some("lab".into()),
            ..HostsView::default()
        };
        let draft = view.unsaved_target(&vault, "deploy@10.0.9.9:22");
        assert!(draft.is_some_and(|host| (
            host.label.as_str(),
            host.username.as_str(),
            host.port,
            host.group.as_deref()
        ) == ("10.0.9.9", "deploy", None, Some("lab"))));
        let plain = view.unsaved_target(&vault, "cache.lan");
        assert!(plain.is_some_and(|host| (host.port, host.username.is_empty()) == (None, true)));
        assert!(
            view.unsaved_target(&vault, "db.example.com").is_none(),
            "saved"
        );
        assert!(view.unsaved_target(&vault, "a b.lan").is_none(), "words");
        assert!(view.unsaved_target(&vault, "web").is_none(), "a search");
    }

    #[test]
    fn a_host_can_turn_off_reconnecting() {
        let mut vault = super::super::demo::fixture_vault("no-reconnect", false);
        let mut host = vault.draft_host();
        host.address = "10.0.4.12".into();
        assert!(vault.upsert_host(host.clone()).is_ok());
        let files = annulus_session::SshFiles {
            known_hosts: std::env::temp_dir().join("annulus-no-reconnect-known_hosts"),
            key_dir: None,
        };
        let reconnects = |vault: &Vault| {
            vault.hosts().iter().all(|host| {
                matches!(
                    connect::host(vault, &files, host),
                    annulus_session::Program::Ssh {
                        reconnect: true,
                        ..
                    }
                )
            })
        };
        assert!(reconnects(&vault), "on by default");
        let mut editor = Editor::new(host, false);
        assert!(editor.reconnect, "the switch shows it on");
        editor.reconnect = false;
        let Ok(edited) = editor.build() else {
            unreachable!("a host with an address builds")
        };
        assert!(vault.upsert_host(edited.clone()).is_ok());
        assert!(!reconnects(&vault));
        assert!(
            !Editor::new(edited, false).reconnect,
            "and shows it off again"
        );
    }
}
