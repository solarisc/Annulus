//! The Hosts screen (UI-SPEC §2): a header, a search box that also quick-connects, groups and
//! host cards (or a list), an empty state, and the host editor in a side panel.

use annulus_config::tokens::{metrics, motion, radius, space};
use annulus_session::Target;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Font, Point, Rect, Weight};
use annulus_ui::icons;
use annulus_ui::kit::{Cursor, FOCUS_REACH, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::{Curve, Feel, Layer, Motion};
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, CONTROL, FieldStyle, HEADING, Kind, MENU_BREAK, SCROLL_GUTTER, SEARCH,
    SMALL, SPLIT_MENU_W, Tone, button_width, split_button_width,
};
use annulus_vault::hosts::{DEFAULT_PORT, Host, Secret};
use annulus_vault::vault::Vault;
use zeroize::Zeroizing;

use super::os;
use crate::{connect, groups_view};

/// Width of the editor panel.
const EDITOR_W: f32 = 340.0;
/// The narrowest the host list gets beside the editor; below it the editor covers the list.
const LIST_MIN_W: f32 = 420.0;
/// Height of a chip of the tag filter.
const CHIP_H: f32 = 28.0;
/// The narrowest a host card gets before the grid drops a column.
const CARD_MIN_W: f32 = 224.0;
const CARD_H: f32 = metrics::HOST_CARD_HEIGHT;
const GROUP_H: f32 = 56.0;
const ROW_H: f32 = 44.0;
const GAP: f32 = 8.0;
/// How far a hovered host card rises.
const LIFT: f32 = 1.0;
/// How far a card draws past its place in the list: a hovered one's lift and a focused one's
/// ring. The list's clip leaves this much room, or both are cut off at its edges.
const LIST_REACH: f32 = LIFT + FOCUS_REACH;
/// Side padding of the screen.
const PAD: f32 = 28.0;
/// Width of the grid/list toggle.
const TOGGLE_W: f32 = 72.0;

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
    /// The chips unfolding and folding away.
    tag_fold: super::pages::Fold<()>,
    /// The group and tags listed last frame: changing them fills the list in again.
    was_scope: Option<(Option<String>, Vec<String>)>,
    /// The editor, while open.
    pub editor: Option<Editor>,
    /// A click on a host connects (Settings → Sessions); else it selects.
    pub connect_on_click: bool,
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
    /// Where the editor panel was last drawn, for it to slide out once it closes.
    last_panel: Option<Rect>,
    /// The host editor that has just closed, sliding out. It holds no secrets any more.
    leaving: Option<Editor>,
    /// The group editor that has just closed, sliding out.
    leaving_group: Option<GroupEditor>,
    /// Where each card of the grid was last frame, in the list's own coordinates.
    places: std::collections::HashMap<Id, Rect>,
    /// Whether an editor was open last frame.
    was_open: bool,
    /// How many columns the grid had last frame.
    was_columns: usize,
    /// How many times the grid has changed its columns: each time its cards fill in anew.
    refills: u32,
    /// Where the cards were before the grid changed its columns, and when, while those fade.
    refilled: Option<(std::collections::HashMap<Id, Rect>, f64)>,
    /// What the list showed last frame.
    listed: (Vec<Card>, Vec<GroupCard>),
    /// What the old grid showed, while it fades out.
    left: (Vec<Card>, Vec<GroupCard>),
    /// Until when the grid's cards glide to new places, on the window's clock.
    reflow_until: f64,
    /// Where the editor panel's left edge is this frame, or the sheet's right edge.
    list_edge: f32,
    /// The card that was just connected to, on screen: the host flies from there to its tab.
    pub launched: Option<Rect>,
    /// How long cards wait to fill in when the screen appears, in milliseconds.
    pub enter_after: u32,
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
    notes: String,
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
            notes: host.notes.clone(),
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
        host.notes = annulus_vault::hosts::clean_notes(&self.notes).map_err(|_| {
            format!(
                "Notes hold {} characters at most: shorten them",
                annulus_vault::hosts::NOTES_MAX
            )
        })?;
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
            notes,
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
        // Notes an agent changed meanwhile stay, unless they were edited here too.
        if notes != before.notes {
            host.notes = notes;
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
#[derive(Clone)]
struct Card {
    id: u64,
    label: String,
    detail: String,
    tags: Vec<String>,
    os: Option<&'static os::Os>,
}

/// What the screen shows of one group.
#[derive(Clone)]
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
    /// Opens the `ssh_config` import preview, or says why it cannot (the New Tab page's welcome).
    pub fn start_import(&mut self, vault: &Vault) {
        match crate::import_dialog::ImportPreview::load(vault) {
            Ok(preview) => self.import = Some(preview),
            Err(problem) => self.notice = Some(problem),
        }
    }

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

    /// Opens the editor on saved group `uid`.
    pub fn edit_group(&mut self, vault: &Vault, uid: &str) {
        if let Some(group) = vault.groups().iter().find(|group| group.uid == uid) {
            self.editor = None;
            self.group_editor = Some(GroupEditor::new(group.clone(), false));
        }
    }

    /// Whether saved host `id` is open in the editor.
    #[cfg(test)]
    pub fn editing(&self, id: u64) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.host.id == id)
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
        // The panel travels its whole width in from the sheet's edge, and back out the same way
        // with its form still on it; the list makes room at once and its cards glide there.
        let shown = ui.animate(
            Id::new("editor.shown"),
            if open { 1.0 } else { 0.0 },
            if open {
                Motion::SLOW
            } else {
                Motion::EXIT_SLOW
            },
        );
        // Where the panel's left edge is right now: the list's top row and search follow it.
        self.list_edge = panel
            .or(self.last_panel.filter(|_| shown > 0.0))
            .filter(|panel| panel.x > rect.x + 0.5)
            .map_or(rect.r(), |panel| panel.x + panel.w * (1.0 - shown));
        if !open || beside {
            action = self.listing_area(ui, area, vault).or(action);
        }
        if action == Some(HostsAction::ImportSshConfig) {
            action = None;
            self.start_import(vault);
        }
        if let Some(panel) = panel {
            self.last_panel = Some(panel);
            self.leaving = None;
            self.leaving_group = None;
            ui.push_clip(rect);
            ui.push_layer(Layer::NONE.shifted(panel.w * (1.0 - shown), 0.0));
            panel_backing(ui, rect, panel);
            if self.group_editor.is_some() {
                self.group_panel(ui, panel, vault);
            } else {
                self.editor_panel(ui, panel, vault);
            }
            ui.pop_layer();
            ui.pop_clip();
        } else if let (true, Some(panel)) = (shown > 0.0, self.last_panel) {
            ui.push_clip(rect);
            ui.push_layer(Layer::NONE.shifted(panel.w * (1.0 - shown), 0.0));
            panel_backing(ui, rect, panel);
            self.panel_leaving(ui, panel, vault);
            ui.pop_layer();
            ui.pop_clip();
        } else {
            self.last_panel = None;
            self.leaving = None;
            self.leaving_group = None;
        }
        self.import_card(ui, rect, vault);
        action
    }

    /// The form of the editor that has just closed, drawn on its panel as it slides out.
    fn panel_leaving(&mut self, ui: &mut Ui<'_>, panel: Rect, vault: &mut Vault) {
        ui.push_inert();
        if let Some(leaving) = self.leaving.take() {
            self.editor = Some(leaving);
            self.editor_panel(ui, panel, vault);
            self.leaving = self.editor.take();
        } else if let Some(leaving) = self.leaving_group.take() {
            self.group_editor = Some(leaving);
            self.group_panel(ui, panel, vault);
            self.leaving_group = self.group_editor.take();
        }
        ui.pop_inert();
    }

    /// Where the card `id` of the grid goes this frame: `cell`, or on its way there while the
    /// grid reflows. `top` is where the list's content starts.
    fn place(&mut self, ui: &mut Ui<'_>, id: Id, cell: Rect, top: f32) -> Rect {
        let local = Rect::new(cell.x, cell.y - top, cell.w, cell.h);
        self.places.insert(id, local);
        if ui.time() >= self.reflow_until {
            return cell;
        }
        Rect::new(
            ui.spring_with(id.with("x"), local.x, Feel::Soft),
            ui.spring_with(id.with("y"), local.y, Feel::Soft) + top,
            ui.spring_with(id.with("w"), local.w, Feel::Soft),
            cell.h,
        )
    }

    /// Whether the grid's cards are gliding to new places at `now`.
    #[cfg(test)]
    pub fn reflowing(&self, now: f64) -> bool {
        now < self.reflow_until
    }

    /// Starts the grid's cards moving to where they now belong, when the room they have or how
    /// many there are changed by something done here. In the same columns they glide. Into
    /// other columns their paths would cross, so the old grid fades out and the new one fills
    /// in. A window being resized moves them at once, with the pointer.
    fn reflow(&mut self, ui: &mut Ui<'_>, listed: usize, columns: usize, reordered: bool) {
        /// Longer than any card takes to arrive.
        const GLIDES: f64 = 0.7;
        let open = self.editor.is_some() || self.group_editor.is_some();
        let changed = open != self.was_open || listed != self.places.len() || reordered;
        let new_columns = columns != self.was_columns;
        self.was_open = open;
        self.was_columns = columns;
        let moved = changed && !self.places.is_empty() && !ui.state.reduce_motion;
        if moved && new_columns {
            self.refills = self.refills.wrapping_add(1);
            self.refilled = Some((std::mem::take(&mut self.places), ui.time()));
            self.left = std::mem::take(&mut self.listed);
            self.reflow_until = 0.0;
        } else if moved {
            self.reflow_until = ui.time() + GLIDES;
            for (id, from) in &self.places {
                ui.spring_set(id.with("x"), from.x);
                ui.spring_set(id.with("y"), from.y);
                ui.spring_set(id.with("w"), from.w);
            }
        }
        self.places.clear();
    }

    /// The old grid's fade, 1.0 to 0.0, while it fades out; ends it once it has.
    fn old_grid(&mut self, ui: &mut Ui<'_>) -> Option<f32> {
        let (_, since) = self.refilled.as_ref()?;
        #[allow(clippy::cast_possible_truncation)] // A share for drawing.
        let gone = ((ui.time() - since) * 1000.0 / f64::from(motion::EXIT)) as f32;
        if gone >= 1.0 {
            self.refilled = None;
            return None;
        }
        ui.request_repaint();
        Some(1.0 - Curve::Out.at(gone))
    }

    /// Draws `draw` once more where the old grid had `id`, at `fade`, taking nothing.
    fn in_old_grid(
        &self,
        ui: &mut Ui<'_>,
        (id, fade, top): (Id, f32, f32),
        draw: impl FnOnce(&mut Ui<'_>, Rect),
    ) {
        let Some(from) = self
            .refilled
            .as_ref()
            .and_then(|(places, _)| places.get(&id))
        else {
            return;
        };
        ui.push_inert();
        ui.push_layer(Layer::fade(fade));
        draw(ui, Rect::new(from.x, from.y + top, from.w, from.h));
        ui.pop_layer();
        ui.pop_inert();
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
    #[allow(clippy::too_many_lines)] // The page, top to bottom.
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
        // Another group, or other tags: other hosts, which fill in like a new page.
        let scope = (self.group.clone(), self.tags.clone());
        if self.was_scope.as_ref().is_some_and(|was| *was != scope) && !ui.state.reduce_motion {
            self.refills = self.refills.wrapping_add(1);
            self.refilled = Some((std::mem::take(&mut self.places), ui.time()));
            self.left = std::mem::take(&mut self.listed);
        }
        self.was_scope = Some(scope);
        let mut main = area.inset(PAD, 0.0);
        main.take_top(space::XL);
        // The title and what can be done here on one line, then the search: the hosts start
        // high up the window.
        // How much wider or narrower the list shows right now than it will be once the panel
        // has landed. Its top row and search follow the panel; the cards glide by themselves.
        let extra = self.list_edge - area.r();
        let mut top = main.take_top(36.0);
        super::pages::title(ui, &mut top, "Hosts", Some(cards.len()));
        // Labels go when the row really runs out of room, as the panel's edge reaches it.
        let room = top.w + extra;
        let mut actions = top.centered(top.w, CONTROL);
        actions.w = (actions.w + extra).max(0.0);
        let empty = vault.hosts().is_empty() && vault.groups().is_empty();
        if empty {
            // The page's own buttons say how to start; a local terminal needs no host.
            if super::shortcuts::NEW_HOST.pressed(ui) {
                self.new_host(vault);
            }
            let width = button_width(&mut ui.gfx, "Terminal", Some(icons::TERMINAL));
            if ui
                .button(
                    Id::new("hosts.terminal"),
                    actions.take_right(width),
                    "Terminal",
                    Some(icons::TERMINAL),
                    Kind::Ghost,
                )
                .clicked
            {
                action = Some(HostsAction::LocalTerminal);
            }
        } else if let Some(toolbar) = self.toolbar(ui, actions, room, vault, !tags.is_empty()) {
            action = Some(toolbar);
        }
        main.take_top(space::L);
        let mut search = main.take_top(SEARCH);
        search.w = (search.w + extra).max(0.0);
        if let Some(found) = self.search(ui, search, vault) {
            action = Some(found);
        }
        main.take_top(space::L);
        let mut chips = std::mem::take(&mut self.tag_fold);
        if !self.tag_bar || tags.is_empty() {
            chips.close();
        } else if chips.get().is_none() {
            chips.open(());
        }
        chips.draw(ui, Id::new("hosts.chips"), &mut main, |ui, area, ()| {
            let used = self.tag_chips(ui, *area, &tags);
            area.take_top(used + space::L);
        });
        self.tag_fold = chips;
        if let Some((tag, true)) = self.untag.clone() {
            self.untag = None;
            self.notice = Some(untag_everywhere(vault, &tag));
            self.tags.retain(|chosen| *chosen != tag);
        }
        // What was just done, in a toast that leaves by itself.
        if let Some(notice) = self.notice.take() {
            let tone = if notice.starts_with("Could not") || notice.starts_with("This host") {
                Tone::Problem
            } else {
                Tone::Done
            };
            ui.toast(notice, tone);
        }
        if self.group.is_some() && !self.filtering() {
            let shown = ui.entrance(
                Id::new(("hosts.crumb", self.group.as_deref().unwrap_or_default())),
                Motion::BASE,
            );
            ui.push_layer(Layer::fade(shown));
            if let Some(all) = self.breadcrumb(ui, main.take_top(CONTROL), vault) {
                action = Some(all);
            }
            ui.pop_layer();
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
            1 => match duplicate_host(vault, host) {
                Ok(copy) => format!("Added {}.", copy.label),
                Err(problem) => problem,
            },
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

    /// The search box. It filters as you type; Enter connects to the first card, or to a
    /// typed `user@host:port` when no card matches.
    fn search(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &Vault) -> Option<HostsAction> {
        let field = ui.text_field(
            Id::new("hosts.search"),
            rect,
            &mut self.query,
            FieldStyle {
                placeholder: "Search hosts, or type user@host:port to connect",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        if !field.submitted {
            return None;
        }
        // Matched against the text as it is now: what was typed this frame counts.
        let (cards, _) = self.gather(vault);
        let query = self.query.trim();
        // Enter with nothing typed connects nowhere: the first card is just the first.
        if query.is_empty() {
            return None;
        }
        Some(match cards.first() {
            Some(first) => HostsAction::Connect(first.id),
            None => HostsAction::QuickConnect(query.to_owned()),
        })
    }

    /// The actions beside the title, at the right end of the row: the grid/list toggle, the
    /// Tags button (when any host has a tag), Terminal, then New host with its menu.
    fn toolbar(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        room: f32,
        vault: &mut Vault,
        tagged: bool,
    ) -> Option<HostsAction> {
        let mut row = rect;
        let mut action = None;
        let mut new_w = split_button_width(&mut ui.gfx, "New host", icons::PLUS);
        let mut term_w = button_width(&mut ui.gfx, "Terminal", Some(icons::TERMINAL));
        let mut tags_w = button_width(&mut ui.gfx, "Tags", Some(icons::TAG));
        let tags_room = if tagged { tags_w + space::S } else { 0.0 };
        // Labels go when the row cannot hold them all; the icons and the names stay.
        let compact = new_w + space::S + term_w + space::S + tags_room + space::M + TOGGLE_W > room;
        if compact {
            (new_w, term_w, tags_w) = (CONTROL + SPLIT_MENU_W, CONTROL, CONTROL);
        }
        let label = |text: &'static str| if compact { "" } else { text };

        // Laid out from the right end, drawn from the left: Tab follows the eye.
        let new = row.take_right(new_w);
        row.take_right(space::S);
        let terminal = row.take_right(term_w);
        let tags = tagged.then(|| {
            row.take_right(space::S);
            row.take_right(tags_w)
        });
        row.take_right(space::M);
        let toggle = row.take_right(TOGGLE_W);
        // There are no labels to say what these are in the narrowest case, so the row may be
        // too short for even its icons: then the toggle gives way first.
        let has_toggle = toggle.w >= TOGGLE_W;
        if has_toggle {
            self.view_toggle(ui, toggle);
        }
        if let Some(tags) = tags {
            let tags_id = Id::new("hosts.tags");
            // Open, it is lit like the grid/list toggle's chosen half.
            let lit = ui.animate(
                tags_id.with("lit"),
                if self.tag_bar { 1.0 } else { 0.0 },
                Motion::BASE,
            );
            ui.gfx
                .fill_rect(tags, radius::SMALL + 2.0, ui.theme.selected.alpha(lit));
            if ui
                .button(tags_id, tags, label("Tags"), Some(icons::TAG), Kind::Ghost)
                .clicked
            {
                // Closing the chips stops filtering: a filter is never on out of sight.
                self.tag_bar = !self.tag_bar;
                self.tags.clear();
            }
            ui.set_toggled(tags_id, self.tag_bar);
            if compact {
                ui.set_name(tags_id, "Tags");
                ui.tooltip(tags_id, tags, "Tags");
            }
        }
        // Drawn in the order it reads, left to right.
        let term_id = Id::new("hosts.terminal");
        if ui
            .button(
                term_id,
                terminal,
                label("Terminal"),
                Some(icons::TERMINAL),
                Kind::Ghost,
            )
            .clicked
        {
            action = Some(HostsAction::LocalTerminal);
        }
        if compact {
            ui.set_name(term_id, "Terminal");
            ui.tooltip(term_id, terminal, "Terminal");
        }
        let items = [
            "New group".to_owned(),
            "Import from ssh_config".to_owned(),
            "Copy hosts as ssh_config".to_owned(),
        ];
        let new_host = ui.split_button(
            Id::new("hosts.new"),
            new,
            "New host",
            icons::PLUS,
            "More ways to add hosts",
            &items,
        );
        if new_host.main.clicked || super::shortcuts::NEW_HOST.pressed(ui) {
            self.new_host(vault);
        }
        match new_host.chosen {
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
        action
    }

    /// The grid/list toggle: two icon buttons side by side.
    fn view_toggle(&mut self, ui: &mut Ui<'_>, toggle: Rect) {
        let mut halves = toggle.inset(2.0, 2.0);
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
        let badge = column.take_top(64.0).centered(52.0, 52.0);
        ui.gfx.fill_rect(badge, 14.0, t.hover);
        ui.gfx
            .icon(icons::SERVER, badge.centered(24.0, 24.0), t.text2);
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

    #[allow(clippy::too_many_lines)] // The list top to bottom: groups, heading, hosts.
    fn listing(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        cards: &[Card],
        groups: &[GroupCard],
        vault: &mut Vault,
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
        // The page's title says these are hosts; a heading is for when something else is listed
        // above them, or when they are what a search found.
        let heading = !groups.is_empty() || self.filtering();
        let heading_h = if heading { 28.0 } else { 0.0 };
        let group_block = if groups.is_empty() {
            0.0
        } else {
            28.0 + rows(groups.len(), columns, GROUP_H) + space::XL
        };
        let content_h = group_block + heading_h + host_rows + space::XL;
        let id = Id::new("hosts.scroll");
        let offset = ui.scroll_view(id, rect.wider(SCROLL_GUTTER), content_h);
        // Cards still gliding into a narrower list are not cut off short of the panel, and no
        // card's lift or focus ring is cut off at the list's top or sides.
        let reach = (self.list_edge - rect.r()).max(0.0);
        ui.push_list_clip(
            Rect::new(rect.x, rect.y, rect.w + reach, rect.h),
            offset,
            content_h,
            LIST_REACH,
        );
        let width = rect.w;
        let top = rect.y - offset;
        let mut y = top;
        let mut action = None;
        let reordered = self
            .listed
            .0
            .iter()
            .map(|card| card.id)
            .ne(cards.iter().map(|card| card.id));
        self.reflow(ui, cards.len() + groups.len(), columns, reordered);
        self.listed = (cards.to_vec(), groups.to_vec());
        let old = self.old_grid(ui);
        if let Some(fade) = old {
            for group in &self.left.1 {
                let id = Id::new(("group", group.uid.as_str()));
                self.in_old_grid(ui, (id, fade, top), |ui, rect| {
                    group_card(ui, rect, group);
                });
            }
            for card in &self.left.0 {
                let id = Id::new(("host", card.id));
                self.in_old_grid(ui, (id, fade, top), |ui, rect| {
                    host_card(ui, rect, card, false, menu);
                });
            }
        }
        let refill = self.refills;
        // Two layouts are never on screen together: the new grid waits for the old one to go.
        let wait = if old.is_some() {
            motion::EXIT
        } else {
            self.enter_after
        };
        if !groups.is_empty() {
            let shown = ui.entrance(
                Id::new("hosts.groups.heading").with(("enter", refill)),
                Motion::BASE.delayed(wait),
            );
            ui.push_layer(Layer::fade(shown));
            ui.section_label(Rect::new(rect.x, y, rect.w, 20.0), "Groups");
            ui.pop_layer();
            y += 28.0;
            for (index, group) in groups.iter().enumerate() {
                let cell = grid_cell(rect.x, y, width, columns, index, GROUP_H);
                let id = Id::new(("group", group.uid.as_str()));
                let place = self.place(ui, id, cell, top);
                let shown = ui.entrance(id.with(("enter", refill)), Motion::BASE.delayed(wait));
                ui.push_layer(
                    Layer::fade(shown * in_flight(place, cell)).shifted(0.0, 6.0 * (1.0 - shown)),
                );
                if group_card(ui, place, group) {
                    self.group = Some(group.uid.clone());
                }
                ui.pop_layer();
            }
            y += rows(groups.len(), columns, GROUP_H) + space::XL;
        }
        if heading {
            let label = Rect::new(rect.x, y, rect.w, 20.0);
            let words = if self.filtering() {
                format!("Results \u{b7} {}", cards.len())
            } else {
                "Hosts".to_owned()
            };
            // It moves with the grid, so it arrives with the new one.
            let shown = ui.entrance(
                Id::new("hosts.heading").with(("enter", refill)),
                Motion::BASE.delayed(wait),
            );
            ui.push_layer(Layer::fade(shown));
            ui.section_label(label, &words);
            ui.pop_layer();
            y += 28.0;
        }
        match self.mode {
            Mode::Grid => {
                let homes: Vec<(u64, Rect)> = cards
                    .iter()
                    .enumerate()
                    .map(|(index, card)| {
                        (card.id, grid_cell(rect.x, y, width, columns, index, CARD_H))
                    })
                    .collect();
                let dragging = cards.iter().find_map(|card| {
                    ui.drag_moved(Id::new(("host", card.id)))
                        .map(|point| (card.id, point))
                });
                let released = cards.iter().find_map(|card| {
                    ui.drag_released(Id::new(("host", card.id)))
                        .map(|point| (card.id, point))
                });
                for (index, card) in cards.iter().enumerate() {
                    let id = Id::new(("host", card.id));
                    let home = homes[index].1;
                    let cell = self.place(ui, id, home, top);
                    if dragging.is_some_and(|(moving, _)| moving == card.id) {
                        continue;
                    }
                    let selected = self.selected == Some(card.id);
                    // Cards fill in one after another as the screen appears.
                    let shown = ui.entrance(
                        id.with(("enter", refill)),
                        Motion::SLOW.delayed(wait).staggered(index),
                    );
                    ui.push_layer(
                        Layer::fade(shown * in_flight(cell, home))
                            .scaled(cell.center(), 0.985 + 0.015 * shown)
                            .shifted(0.0, 12.0 * (1.0 - shown)),
                    );
                    let gliding = ui.time() < self.reflow_until;
                    let event = self.clicks(host_card(ui, cell, card, selected && !gliding, menu));
                    if matches!(event, CardEvent::Connect | CardEvent::More(MENU_CONNECT)) {
                        self.launched = Some(ui.gfx.to_screen(cell));
                    }
                    ui.pop_layer();
                    if let Some(found) = self.card_event(vault, card.id, event) {
                        action = Some(found);
                    }
                }
                if let Some((id, pointer)) = dragging
                    && let Some((index, card)) =
                        cards.iter().enumerate().find(|(_, card)| card.id == id)
                {
                    let before = host_drop_before(cards, &homes, id, pointer, Mode::Grid);
                    host_drop_mark(ui, &homes, before, Mode::Grid);
                    let home = homes[index].1;
                    let held = dragged_rect(ui, home, pointer);
                    ui.push_layer(Layer::NONE.shifted(0.0, -2.0));
                    let _ = host_card(ui, held, card, self.selected == Some(id), menu);
                    ui.pop_layer();
                    ui.set_cursor(Cursor::Pointer);
                }
                if let Some((id, pointer)) = released {
                    let before = host_drop_before(cards, &homes, id, pointer, Mode::Grid);
                    match vault.move_host_before(id, before) {
                        Ok(changed) => ui.request_repaint_if(changed),
                        Err(error) => ui.toast(
                            format!("Could not reorder the host: {error}"),
                            Tone::Problem,
                        ),
                    }
                }
            }
            Mode::List => {
                list_header(ui, Rect::new(rect.x, y, width, ROW_H - 12.0));
                y += ROW_H - 12.0;
                let mut row_y = y;
                let homes: Vec<(u64, Rect)> = cards
                    .iter()
                    .map(|card| {
                        let home = Rect::new(rect.x, row_y, width, ROW_H);
                        row_y += ROW_H + GAP;
                        (card.id, home)
                    })
                    .collect();
                let dragging = cards.iter().find_map(|card| {
                    ui.drag_moved(Id::new(("host", card.id)))
                        .map(|point| (card.id, point))
                });
                let released = cards.iter().find_map(|card| {
                    ui.drag_released(Id::new(("host", card.id)))
                        .map(|point| (card.id, point))
                });
                for (index, card) in cards.iter().enumerate() {
                    let id = Id::new(("host", card.id));
                    let home = homes[index].1;
                    let row = self.place(ui, id, home, top);
                    if dragging.is_some_and(|(moving, _)| moving == card.id) {
                        continue;
                    }
                    let shown =
                        ui.entrance(id.with(("row", refill)), Motion::BASE.staggered(index));
                    ui.push_layer(Layer::fade(shown).shifted(0.0, 8.0 * (1.0 - shown)));
                    let event = self.clicks(host_row(
                        ui,
                        row,
                        card,
                        self.selected == Some(card.id),
                        menu,
                    ));
                    ui.pop_layer();
                    if matches!(event, CardEvent::Connect | CardEvent::More(MENU_CONNECT)) {
                        self.launched = Some(ui.gfx.to_screen(row));
                    }
                    if let Some(found) = self.card_event(vault, card.id, event) {
                        action = Some(found);
                    }
                }
                if let Some((id, pointer)) = dragging
                    && let Some((index, card)) =
                        cards.iter().enumerate().find(|(_, card)| card.id == id)
                {
                    let before = host_drop_before(cards, &homes, id, pointer, Mode::List);
                    host_drop_mark(ui, &homes, before, Mode::List);
                    let held = dragged_rect(ui, homes[index].1, pointer);
                    let _ = host_row(ui, held, card, self.selected == Some(id), menu);
                    ui.set_cursor(Cursor::Pointer);
                }
                if let Some((id, pointer)) = released {
                    let before = host_drop_before(cards, &homes, id, pointer, Mode::List);
                    match vault.move_host_before(id, before) {
                        Ok(changed) => ui.request_repaint_if(changed),
                        Err(error) => ui.toast(
                            format!("Could not reorder the host: {error}"),
                            Tone::Problem,
                        ),
                    }
                }
            }
        }
        ui.pop_clip();
        ui.scroll_edges(rect, offset, content_h, ui.theme.bg);
        action
    }

    /// What a click on a host does: it selects, and a double-click connects; with one-click
    /// connecting on (Settings), the click connects and the double-click it may become does
    /// nothing more, so one host never opens twice.
    fn clicks(&self, event: CardEvent) -> CardEvent {
        match (event, self.connect_on_click) {
            (CardEvent::Click, false) => CardEvent::Select,
            (CardEvent::Click, true) | (CardEvent::DoubleClick, false) => CardEvent::Connect,
            (CardEvent::DoubleClick, true) => CardEvent::None,
            (event, _) => event,
        }
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
            // A click and a double-click were made Select or Connect by `clicks` first.
            CardEvent::None | CardEvent::Click | CardEvent::DoubleClick => {}
        }
        None
    }

    #[allow(clippy::too_many_lines)] // One panel, top to bottom: header, form, footer.
    fn editor_panel(&mut self, ui: &mut Ui<'_>, rect: Rect, vault: &mut Vault) {
        let t = ui.theme;
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        // Everything inside the panel takes the pointer: nothing below it is clicked.
        ui.zone(Id::new("editor.backdrop"), rect, Role::Backdrop, "");

        let mut area = rect;
        let mut head = area.take_top(64.0).inset(space::XL, 0.0);
        let close = head.take_right(32.0).centered(32.0, 32.0);
        // A saved host is copied from here as from its menu (F-HOST-007); the copy opens in
        // its place. Drawn before Close, so Tab follows the eye.
        let duplicate = (!editor.is_new).then(|| {
            head.take_right(4.0);
            head.take_right(32.0).centered(32.0, 32.0)
        });
        let duplicate_clicked = duplicate.is_some_and(|slot| {
            ui.icon_button(
                Id::new("editor.duplicate"),
                slot,
                icons::COPY,
                "Duplicate",
                false,
            )
            .clicked
        });
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
        // Saved empty, it takes this device's user (connect::pin_user).
        let user_hint = hint(
            &inherited.username,
            from.username.as_ref(),
            &annulus_session::local_user(),
        );
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
        // What to know about the server, for people and agents alike (F-HOST-011).
        form.area(
            ui,
            "editor.notes",
            "Notes",
            &mut editor.notes,
            "What it is for, what to watch out for",
        );

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
            let delete = Rect::new(footer.x + space::L, cancel.y, delete_w, CONTROL);
            if ui
                .button(
                    Id::new("editor.delete"),
                    delete,
                    "Delete",
                    Some(icons::TRASH),
                    Kind::Quiet,
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
                Kind::Ghost,
            )
            .clicked
        {
            close_editor = true;
        }
        if ui
            .button(Id::new("editor.save"), save, "Save", None, Kind::Primary)
            .clicked
            || super::shortcuts::SAVE.pressed(ui)
        {
            let current = vault.hosts().iter().find(|host| host.id == editor.host.id);
            let edited = editor.merged(current).map(|mut host| {
                crate::connect::pin_user(vault, &mut host, &annulus_session::local_user());
                host
            });
            match edited {
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
        // The copy is made of what the form shows, so nothing typed is lost; the host itself
        // stays as it was saved.
        let mut copied = None;
        if duplicate_clicked && !saved && !close_editor {
            let current = vault.hosts().iter().find(|host| host.id == editor.host.id);
            let unsaved = current.cloned();
            match editor.merged(current) {
                Ok(mut host) => {
                    crate::connect::pin_user(vault, &mut host, &annulus_session::local_user());
                    let changed = unsaved.as_ref() != Some(&host);
                    let original = unsaved.map_or_else(|| host.label.clone(), |saved| saved.label);
                    match duplicate_host(vault, host) {
                        Ok(copy) => copied = Some((copy, changed.then_some(original))),
                        Err(problem) => editor.problem = Some(problem),
                    }
                }
                Err(problem) => editor.problem = Some(problem),
            }
        }
        if saved || close_editor {
            // It slides out with its form, but no secret stays in it for that.
            self.leaving = self.editor.take().map(|mut editor| {
                editor.password = Zeroizing::new(String::new());
                editor.proxy = Zeroizing::new(String::new());
                editor
            });
        }
        if let Some((copy, changed)) = copied {
            self.notice = Some(match changed {
                Some(original) => format!(
                    "Added {} with your changes; {original} stays as it was saved.",
                    copy.label
                ),
                None => format!("Added {}.", copy.label),
            });
            self.edit_host(vault, copy.id);
        }
    }
}

/// A copy of `host` under a new id, named "… copy" (F-HOST-007), saved at once; or why not.
fn duplicate_host(vault: &mut Vault, host: Host) -> Result<Host, String> {
    let copy = Host {
        id: vault.draft_host().id,
        label: format!("{} copy", host.label),
        ..host
    };
    vault
        .upsert_host(copy.clone())
        .map(|()| copy)
        .map_err(|error| format!("Could not duplicate: {error}"))
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
        let content = preview.rows.len() as f32 * 36.0;
        let offset = ui.scroll_view(Id::new("import.scroll"), list, content);
        ui.push_list_clip(list, offset, content, FOCUS_REACH);
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
                    "Imported {} and {}.",
                    super::pages::count(hosts.len(), "host", "hosts"),
                    super::pages::count(rules.len(), "port forward", "port forwards")
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
                let delete = Rect::new(footer.x + space::L, cancel.y, delete_w, CONTROL);
                if ui
                    .button(
                        Id::new("group.delete"),
                        delete,
                        "Delete",
                        Some(icons::TRASH),
                        Kind::Quiet,
                    )
                    .clicked
                {
                    editor.confirm_delete = true;
                }
            }
            if ui
                .button(Id::new("group.cancel"), cancel, "Cancel", None, Kind::Ghost)
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
            self.leaving_group = self.group_editor.take().map(|mut editor| {
                editor.proxy = Zeroizing::new(String::new());
                editor
            });
        }
    }
}

mod view;
use view::{
    CardEvent, Form, MENU_CONNECT, MENU_EDIT, MENU_REST, MENU_SFTP, dragged_rect, grid_cell,
    grid_columns, group_card, hint, host_card, host_drop_before, host_drop_mark, host_menu,
    host_row, in_flight, inherited, list_header, no_results, panel_backing, tag_chip,
    untag_everywhere,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_click_connecting_never_opens_a_host_twice_for_a_double_click() {
        let names = |view: &HostsView| {
            [CardEvent::Click, CardEvent::DoubleClick].map(|event| match view.clicks(event) {
                CardEvent::Select => "select",
                CardEvent::Connect => "connect",
                CardEvent::None => "nothing",
                _ => "other",
            })
        };
        let mut view = HostsView::default();
        assert_eq!(names(&view), ["select", "connect"]);
        view.connect_on_click = true;
        assert_eq!(names(&view), ["connect", "nothing"]);
    }

    /// The Hosts screen with one selected host and no group or search, so its card sits on the
    /// list's top and left edges, drawn until it has settled. From then on the pointer rests on
    /// the card when `hover`, and the card has the keyboard focus with its ring when `focus`.
    /// Returns the canvas's pixels (RGBA, 1000 wide) and where the card is.
    fn edge_card(name: &str, hover: bool, focus: bool) -> (Vec<u8>, Rect) {
        let mut vault = crate::native::demo::fixture_vault(name, false);
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.5".into());
        let id = host.id;
        vault.upsert_host(host).unwrap();
        let mut view = HostsView {
            selected: Some(id),
            ..HostsView::default()
        };
        let mut canvas = annulus_platform::cpu::CpuCanvas::new(
            1000,
            640,
            1.0,
            annulus_platform::cpu::TextEngine::new(),
        )
        .unwrap();
        let theme = annulus_ui::kit::Theme::new(annulus_config::tokens::Appearance::Dark);
        let mut state = annulus_ui::kit::UiState::default();
        let mut card: Option<Rect> = None;
        for frame in 0..50 {
            let events = match card {
                Some(at) if hover => vec![annulus_ui::kit::Event::PointerMoved(at.center())],
                _ => Vec::new(),
            };
            // The page behind the screen, as the window paints it.
            canvas.clear(theme.bg);
            state.begin_frame(&events, f64::from(frame) * 0.1);
            let mut ui = Ui::new(
                &mut canvas,
                theme,
                &mut state,
                Rect::new(0.0, 0.0, 1000.0, 640.0),
            );
            view.draw(
                &mut ui,
                Rect::new(0.0, 0.0, 1000.0, 640.0),
                Some(&mut vault),
            );
            let out = state.end_frame();
            // By three seconds in, the card has come in and settled.
            if frame == 30 {
                let zone = out.zones.iter().find(|zone| zone.label == "web").unwrap();
                card = Some(zone.rect);
                if focus {
                    state.set_focus(Some(zone.id), true);
                }
            }
        }
        (canvas.pixels().to_vec(), card.unwrap())
    }

    /// The pixel at (`x`, `y`) of an `edge_card` canvas.
    fn pixel(pixels: &[u8], x: f32, y: f32) -> [u8; 4] {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On the canvas.
        let at = ((y as usize) * 1000 + x as usize) * 4;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    #[test]
    fn a_hovered_card_on_the_lists_top_edge_keeps_the_top_of_its_outline() {
        let (pixels, card) = edge_card("top-outline", true, false);
        // The hovered card has lifted a pixel, so its outline's top runs just above its place.
        let outline = pixel(&pixels, card.center().x, card.y - 0.5);
        let page = pixel(&pixels, card.center().x, card.y - 12.0);
        assert!(
            outline[1] > page[1].saturating_add(30),
            "the outline's top is cut off: {outline:?} against the page's {page:?}"
        );
    }

    #[test]
    fn a_focused_card_on_the_lists_edges_shows_its_whole_focus_ring() {
        let (pixels, card) = edge_card("edge-ring", false, true);
        let page = pixel(&pixels, card.center().x, card.y - 12.0);
        for (x, y, side) in [
            (card.center().x, card.y - 2.0, "top"),
            (card.x - 2.0, card.center().y, "left"),
        ] {
            let ring = pixel(&pixels, x, y);
            assert!(
                ring[1] > page[1].saturating_add(60),
                "the ring's {side} is cut off: {ring:?} against the page's {page:?}"
            );
        }
    }

    #[test]
    fn the_editor_panel_stays_inside_the_rounded_content_card() {
        let mut vault = crate::native::demo::fixture_vault("editor-corner", false);
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.5".into());
        let id = host.id;
        vault.upsert_host(host).unwrap();
        let mut view = HostsView::default();
        view.edit_host(&vault, id);
        let content = Rect::new(100.0, 40.0, 1000.0, 640.0);
        let mut canvas = annulus_platform::cpu::CpuCanvas::new(
            1120,
            700,
            1.0,
            annulus_platform::cpu::TextEngine::new(),
        )
        .unwrap();
        let theme = annulus_ui::kit::Theme::new(annulus_config::tokens::Appearance::Dark);
        let mut state = annulus_ui::kit::UiState::default();
        // The second frame is long after the panel slid in.
        for time in [0.0, 2.0] {
            canvas.clear(annulus_ui::gfx::Color::rgba(255, 0, 255, 255));
            state.begin_frame(&[], time);
            let mut ui = Ui::new(
                &mut canvas,
                theme,
                &mut state,
                Rect::new(0.0, 0.0, 1120.0, 700.0),
            );
            view.draw(&mut ui, content, Some(&mut vault));
            state.end_frame();
        }
        let pixel = |x: f32, y: f32| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On the canvas.
            let at = ((y as usize) * 1120 + x as usize) * 4;
            canvas.pixels()[at..at + 4].to_vec()
        };
        let untouched = vec![255, 0, 255, 255];
        // Past the card's edges, and outside its rounded corners.
        for (x, y) in [
            (content.r() + 4.0, content.b() - 40.0),
            (content.r() - 40.0, content.b() + 4.0),
            (content.r() - 2.0, content.b() - 2.0),
            (content.r() - 2.0, content.y + 1.0),
        ] {
            assert_eq!(pixel(x, y), untouched, "drawn at ({x}, {y})");
        }
        assert_ne!(
            pixel(content.r() - 40.0, content.b() - 40.0),
            untouched,
            "the panel is there"
        );
    }

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
    fn notes_save_cleaned_and_an_agents_notes_written_meanwhile_stay_unless_edited_here() {
        let vault = crate::native::demo::fixture_vault("notes-editor", false);
        let mut host = vault.draft_host();
        (host.label, host.address) = ("web".into(), "10.0.0.5".into());
        let mut editor = Editor::new(host.clone(), false);
        "deploy".clone_into(&mut editor.username);
        // An agent wrote notes while the editor was open.
        let mut noted = host.clone();
        "nginx front end".clone_into(&mut noted.notes);
        let saved = editor.merged(Some(&noted)).unwrap();
        assert_eq!(saved.notes, "nginx front end", "the agent's notes stay");
        // Notes edited here win, trimmed.
        "  serves the shop  \n".clone_into(&mut editor.notes);
        let saved = editor.merged(Some(&noted)).unwrap();
        assert_eq!(saved.notes, "serves the shop");
        editor.notes = "x".repeat(annulus_vault::hosts::NOTES_MAX + 1);
        let refused = editor.merged(Some(&noted)).unwrap_err();
        assert!(refused.contains("shorten"), "{refused}");
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
    fn a_gliding_card_is_dim_while_far_off_and_whole_when_it_lands() {
        let home = Rect::new(100.0, 100.0, 200.0, 64.0);
        assert!((in_flight(home, home) - 1.0).abs() < f32::EPSILON);
        let near = in_flight(home.offset(4.0, 0.0), home);
        let far = in_flight(home.offset(400.0, 0.0), home);
        assert!(far < near && near < 1.0, "{far} {near}");
        assert!(far > 0.35, "never gone: {far}");
    }

    #[test]
    fn the_grid_drops_columns_as_the_window_narrows() {
        assert_eq!(grid_columns(1200.0), 4);
        assert_eq!(grid_columns(560.0), 2);
        assert_eq!(grid_columns(400.0), 1);
        let cell = grid_cell(0.0, 0.0, 552.0, 2, 3, 72.0);
        assert_eq!(cell, Rect::new(280.0, 80.0, 272.0, 72.0));
    }

    #[test]
    fn a_list_drop_chooses_before_or_after_the_host_under_the_pointer() {
        let cards: Vec<Card> = (1..=3)
            .map(|id| Card {
                id,
                label: format!("host-{id}"),
                detail: String::new(),
                tags: Vec::new(),
                os: None,
            })
            .collect();
        let homes = [
            (1, Rect::new(0.0, 0.0, 300.0, 44.0)),
            (2, Rect::new(0.0, 52.0, 300.0, 44.0)),
            (3, Rect::new(0.0, 104.0, 300.0, 44.0)),
        ];
        assert_eq!(
            host_drop_before(&cards, &homes, 3, homes[0].1.center(), Mode::List),
            Some(1)
        );
        assert_eq!(
            host_drop_before(
                &cards,
                &homes,
                1,
                Point::new(150.0, homes[2].1.b() + 20.0),
                Mode::List,
            ),
            None
        );
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
        assert_eq!(labels, ["tagged", "grouped"]);
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
