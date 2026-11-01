//! The port forwarding wizard (F-FWD-006): each kind of rule drawn as a small diagram a
//! packet travels across, then a rule made in three steps: what it does, through which
//! server and ports, and a last look. With no rules saved it is the Port Forwarding screen
//! itself, so a first rule is also a lesson in how tunnels work.

use annulus_config::tokens::space;
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Font, Point, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::{Cursor, Id, Key, Modifiers, Role, Ui};
use annulus_ui::motion::Curve;
use annulus_ui::widgets::{
    BODY, BODY_MEDIUM, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, button_width,
};
use annulus_vault::forwards::{ForwardKind, PortForward};

use crate::forwards_screen::Draft;

/// How long a packet takes across a diagram, in seconds. Short, so a diagram left alone soon
/// stops asking for frames (PERFORMANCE.md P-06).
const PLAY: f64 = 1.4;

/// The kinds in the order the first step offers them, each with its card's title and two
/// lines on what it does.
const KINDS: [(ForwardKind, &str, [&str; 2]); 3] = [
    (
        ForwardKind::Local,
        "Reach a service behind a server",
        [
            "A port on this computer leads to",
            "a host only the server can reach.",
        ],
    ),
    (
        ForwardKind::Remote,
        "Share a service from this computer",
        [
            "A port on the server leads back",
            "to a service running here.",
        ],
    ),
    (
        ForwardKind::Dynamic,
        "Browse through a server",
        [
            "Apps set to this SOCKS proxy go",
            "out to the web from the server.",
        ],
    ),
];

/// The steps, by name.
const STEPS: [&str; 3] = ["What it does", "Server and ports", "Review"];

/// Height of a diagram in the first step's cards, and of the big one in the other steps.
const SMALL_DIAGRAM: f32 = 124.0;
const BIG_DIAGRAM: f32 = 150.0;

/// A diagram's packet: it crosses once when the diagram first shows and again whenever the
/// pointer comes onto it; otherwise the diagram rests, its arrows showing the way.
#[derive(Clone, Copy, Default)]
struct Play {
    since: Option<f64>,
    hovered: bool,
}

impl Play {
    /// How far the packet is at this frame (`None` at rest); asks for frames while it moves.
    fn at(&mut self, ui: &mut Ui<'_>, hovered: bool) -> Option<f32> {
        let now = ui.time();
        if self.since.is_none() || (hovered && !self.hovered) {
            self.since = Some(now);
        }
        self.hovered = hovered;
        let share = (now - self.since?) / PLAY;
        if ui.state.reduce_motion || share >= 1.0 {
            return None;
        }
        ui.request_repaint();
        #[allow(clippy::cast_possible_truncation)] // A share between 0 and 1.
        Some(share.max(0.0) as f32)
    }
}

/// What a diagram names under its three ends (this computer, the server, the far side):
/// a name and an address each.
pub(crate) type Ends = [(String, String); 3];

/// The ends of `rule` through the server called `server`, with its ports as typed so far
/// (`ports`: listening, then destination). Blanks read as what still has to be filled in.
pub(crate) fn ends(rule: &PortForward, ports: (&str, &str), server: &str) -> Ends {
    let or = |text: &str, blank: &str| {
        let text = text.trim();
        if text.is_empty() {
            blank.to_owned()
        } else {
            text.to_owned()
        }
    };
    let listen = format!("{}:{}", here(&rule.bind_address), or(ports.0, "port"));
    let destination = format!(
        "{}:{}",
        or(&rule.destination_host, "host"),
        or(ports.1, "port")
    );
    let server = or(server, "Your server");
    match rule.kind {
        ForwardKind::Local => [
            ("This computer".into(), listen),
            (server, "SSH".into()),
            ("Destination".into(), destination),
        ],
        ForwardKind::Remote => [
            ("This computer".into(), destination),
            (server, format!("port {}", or(ports.0, "?"))),
            ("Its users".into(), "any client".into()),
        ],
        ForwardKind::Dynamic => [
            ("This computer".into(), listen),
            (server, "SSH".into()),
            ("Any site".into(), "via the server".into()),
        ],
    }
}

/// `address` as people say it: loopback is `localhost`.
fn here(address: &str) -> &str {
    let address = address.trim();
    if address
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        "localhost"
    } else {
        address
    }
}

/// One sentence on what `rule` through `server` does, with its ports as typed.
pub(crate) fn sentence(rule: &PortForward, ports: (&str, &str), server: &str) -> String {
    let [(_, here_end), (server_name, server_end), (_, far_end)] = ends(rule, ports, server);
    match rule.kind {
        ForwardKind::Local => format!(
            "Apps on this computer open {here_end} and reach {far_end} as if from {server_name}."
        ),
        ForwardKind::Remote => format!(
            "Whoever opens {server_end} on {server_name} reaches {here_end} on this computer."
        ),
        ForwardKind::Dynamic => {
            format!("Apps set to use the SOCKS proxy {here_end} reach the web from {server_name}.")
        }
    }
}

/// A name for a new rule until the user gives one.
fn default_label(rule: &PortForward, ports: (&str, &str), server: &str) -> String {
    match rule.kind {
        ForwardKind::Local => format!(
            "{}:{} via {server}",
            rule.destination_host.trim(),
            ports.1.trim()
        ),
        ForwardKind::Remote => format!("{server}:{} to this computer", ports.0.trim()),
        ForwardKind::Dynamic => format!("SOCKS via {server}"),
    }
}

/// The point `share` of the way along `points`, measured by length.
fn along(points: &[Point], share: f32) -> Point {
    let lengths: Vec<f32> = points
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
        .collect();
    let mut left = lengths.iter().sum::<f32>() * share.clamp(0.0, 1.0);
    for (pair, length) in points.windows(2).zip(&lengths) {
        if left <= *length && *length > 0.0 {
            let k = left / length;
            return Point::new(
                pair[0].x + (pair[1].x - pair[0].x) * k,
                pair[0].y + (pair[1].y - pair[0].y) * k,
            );
        }
        left -= length;
    }
    points.last().copied().unwrap_or(Point::new(0.0, 0.0))
}

/// A small arrowhead at `at` pointing from `from` to `to`.
fn chevron(ui: &mut Ui<'_>, at: Point, (from, to): (Point, Point)) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy).max(0.001);
    let (ux, uy) = (dx / length, dy / length);
    let color = ui.theme.text3;
    ui.gfx.polyline(
        &[
            Point::new(at.x - 3.0 * ux - 4.0 * uy, at.y - 3.0 * uy + 4.0 * ux),
            Point::new(at.x + 2.5 * ux, at.y + 2.5 * uy),
            Point::new(at.x - 3.0 * ux + 4.0 * uy, at.y - 3.0 * uy - 4.0 * ux),
        ],
        false,
        1.5,
        color,
    );
}

/// Draws how a rule of `kind` carries traffic into `rect`: this computer, the server and the
/// far side named by `ends`, the encrypted tunnel between the first two, arrows for the way
/// traffic goes and, at `packet` (0.0 to 1.0), a packet on its way.
#[allow(clippy::too_many_lines)] // Tunnel, lines, ends, packets and labels, back to front.
fn diagram(ui: &mut Ui<'_>, rect: Rect, kind: ForwardKind, ends: &Ends, packet: Option<f32>) {
    let t = ui.theme;
    let size = (rect.h * 0.34).clamp(28.0, 46.0);
    let column = rect.w / 3.0;
    let y = rect.y + rect.h * 0.36;
    let here = Point::new(rect.x + column / 2.0, y);
    let server = Point::new(rect.center().x, y);
    let far_x = rect.r() - column / 2.0;
    // SOCKS reaches many sites; the other kinds one end.
    let small = size * 0.5;
    let far: Vec<Point> = if kind == ForwardKind::Dynamic {
        [-1.2, 0.0, 1.2]
            .iter()
            .map(|step| Point::new(far_x, y + step * small))
            .collect()
    } else {
        vec![Point::new(far_x, y)]
    };
    let far_size = if far.len() > 1 { small } else { size };
    // The tunnel: the SSH connection, encrypted, between this computer and the server.
    let tunnel = Rect::new(
        here.x + size / 2.0 + 6.0,
        y - 9.0,
        (server.x - here.x - size - 12.0).max(0.0),
        18.0,
    );
    ui.gfx.fill_rect(tunnel, 9.0, t.accent.alpha(0.1));
    ui.gfx.stroke_rect(tunnel, 9.0, 1.0, t.accent.alpha(0.45));
    // From the server to the far side, in the clear.
    let mut lines = Vec::new();
    for end in &far {
        let from = Point::new(server.x + size / 2.0 + 6.0, y);
        let to = Point::new(end.x - far_size / 2.0 - 6.0, end.y);
        ui.gfx.polyline(&[from, to], false, 1.5, t.line_strong);
        lines.push((from, to));
    }
    // Arrows the way traffic goes: toward the far side, except for a remote rule.
    let forward = kind != ForwardKind::Remote;
    let way = |a: Point, b: Point| if forward { (a, b) } else { (b, a) };
    let tunnel_ends = (Point::new(tunnel.x, y), Point::new(tunnel.r(), y));
    for share in [0.25, 0.75] {
        let at = along(&[tunnel_ends.0, tunnel_ends.1], share);
        chevron(ui, at, way(tunnel_ends.0, tunnel_ends.1));
    }
    for (from, to) in &lines {
        chevron(ui, along(&[*from, *to], 0.5), way(*from, *to));
    }
    let lock = tunnel.centered(22.0, 16.0);
    ui.gfx.fill_rect(lock, 8.0, t.surface);
    ui.gfx.stroke_rect(lock, 8.0, 1.0, t.accent.alpha(0.45));
    ui.gfx
        .icon(icons::LOCK, lock.centered(11.0, 11.0), t.accent);
    // Where the packets are: one along each path, a SOCKS proxy's a little apart.
    let paths: Vec<Vec<Point>> = far
        .iter()
        .map(|end| {
            if forward {
                vec![here, server, *end]
            } else {
                vec![*end, server, here]
            }
        })
        .collect();
    #[allow(clippy::cast_precision_loss)] // One to three paths.
    let spread = (paths.len() - 1) as f32 * 0.12;
    let packets: Vec<(Point, f32)> = packet
        .map(|share| {
            paths
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    #[allow(clippy::cast_precision_loss)] // As above.
                    let own = ((share - index as f32 * 0.12) / (1.0 - spread)).clamp(0.0, 1.0);
                    let eased = Curve::InOut.at(own);
                    let shown = (own * 8.0).min((1.0 - own) * 8.0).clamp(0.0, 1.0);
                    (along(path, eased), shown)
                })
                .collect()
        })
        .unwrap_or_default();
    // The ends, lit while a packet passes through.
    let far_icon = match kind {
        ForwardKind::Local => icons::SERVER,
        ForwardKind::Remote => icons::USER,
        ForwardKind::Dynamic => icons::CLOUD,
    };
    let mut tiles: Vec<(Point, f32, IconId)> =
        vec![(here, size, icons::DEVICE), (server, size, icons::TERMINAL)];
    tiles.extend(far.iter().map(|end| (*end, far_size, far_icon)));
    for (center, side, icon) in tiles {
        let tile = Rect::new(center.x - side / 2.0, center.y - side / 2.0, side, side);
        let lit = packets
            .iter()
            .map(|(at, shown)| {
                let near = (at.x - center.x).hypot(at.y - center.y) < side * 0.6;
                if near { *shown } else { 0.0 }
            })
            .fold(0.0, f32::max);
        ui.gfx
            .fill_rect(tile, side * 0.24, t.raised.mix(t.accent, 0.12 * lit));
        ui.gfx
            .stroke_rect(tile, side * 0.24, 1.0, t.line_strong.mix(t.accent, lit));
        ui.gfx.icon(
            icon,
            tile.centered(side * 0.5, side * 0.5),
            t.text2.mix(t.accent, lit),
        );
    }
    for (at, shown) in &packets {
        if *shown > 0.0 {
            ui.gfx.fill_circle(*at, 8.0, t.accent.alpha(0.22 * shown));
            ui.gfx.fill_circle(*at, 4.0, t.accent.alpha(*shown));
        }
    }
    // Names and addresses under the ends.
    // Addresses are a size smaller in a card's narrow columns, so neighbours never touch.
    let mono = Font::mono(if column < 100.0 { 10.0 } else { 11.0 });
    let far_bottom = far.last().map_or(y, |end| end.y) + far_size / 2.0;
    // A label may run a little past its column into the gaps beside it (a narrow card).
    let label_w = column * 1.3;
    for (index, (name, address)) in ends.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // Three columns.
        let center = rect.x + (index as f32 + 0.5) * column;
        let x = (center - label_w / 2.0).clamp(rect.x - 8.0, rect.r() + 8.0 - label_w);
        let top = 6.0
            + if index == 2 {
                far_bottom
            } else {
                y + size / 2.0
            };
        let name_rect = Rect::new(x, top, label_w, 18.0);
        ui.gfx.text(name_rect, name, SMALL, t.text, Align::Center);
        let address_rect = Rect::new(x, top + 17.0, label_w, 16.0);
        ui.gfx
            .text(address_rect, address, mono, t.text2, Align::Center);
    }
}

/// What the wizard came to this frame.
pub(crate) enum Finish {
    /// Closed without a rule.
    Cancel,
    /// The rule to save, and whether to start it now.
    Save(PortForward, bool),
}

/// Making a rule in three steps.
pub(crate) struct Wizard {
    step: usize,
    draft: Draft,
    /// Why the step cannot go on, when it cannot.
    pub(crate) problem: Option<String>,
    /// The big diagram's packet, and each of the first step's cards'.
    play: Play,
    cards: [Play; 3],
    /// How tall the step drew last frame, for its scroll.
    content_h: f32,
    /// The name was typed: it no longer follows the server and ports.
    named: bool,
}

impl Wizard {
    /// A wizard for `rule`, a fresh draft from the vault.
    pub(crate) fn new(rule: PortForward) -> Self {
        Self {
            step: 0,
            draft: Draft::new(rule, true),
            problem: None,
            play: Play::default(),
            cards: [Play::default(); 3],
            content_h: 0.0,
            named: false,
        }
    }

    /// Draws the wizard into `rect`. `hosts` are the servers a rule can run through; with
    /// `cancellable`, a Cancel button closes it (there are rules to go back to).
    pub(crate) fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        hosts: &[(u64, String)],
        cancellable: bool,
    ) -> Option<Finish> {
        if !hosts.iter().any(|(id, _)| *id == self.draft.rule.host_id)
            && let Some((first, _)) = hosts.first()
        {
            self.draft.rule.host_id = *first;
        }
        let mut area = rect;
        let mut top = area.take_top(CONTROL);
        let mut finish = None;
        if cancellable {
            let width = button_width(&mut ui.gfx, "Cancel", None);
            let cancel = top.take_right(width);
            top.take_right(space::M);
            self.steps(ui, top);
            if ui
                .button(
                    Id::new("wizard.cancel"),
                    cancel,
                    "Cancel",
                    None,
                    Kind::Ghost,
                )
                .clicked
                || ui.key(Key::Escape, Modifiers::NONE)
            {
                finish = Some(Finish::Cancel);
            }
        } else {
            self.steps(ui, top);
        }
        area.take_top(space::L);
        let offset = ui.scroll_view(Id::new("wizard.scroll"), area, self.content_h);
        ui.push_clip(area);
        let mut page = Rect::new(area.x, area.y - offset, area.w - 12.0, f32::MAX / 4.0);
        let start = page.y;
        let server = hosts
            .iter()
            .find(|(id, _)| *id == self.draft.rule.host_id)
            .map_or_else(String::new, |(_, name)| name.clone());
        let done = match self.step {
            0 => {
                self.choose_kind(ui, &mut page, !hosts.is_empty());
                None
            }
            1 => {
                self.details(ui, &mut page, hosts, &server);
                None
            }
            _ => self.review(ui, &mut page, &server),
        };
        self.content_h = page.y - start + space::L;
        ui.pop_clip();
        finish.or(done)
    }

    /// The steps across the top, the one shown marked.
    fn steps(&self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let mut row = rect;
        for (index, name) in STEPS.iter().enumerate() {
            let reached = index <= self.step;
            let dot = row.take_left(22.0).centered(22.0, 22.0);
            let fill = if reached { t.accent } else { t.hover };
            ui.gfx.fill_rect(dot, 11.0, fill);
            let ink = if reached { t.on_accent } else { t.text2 };
            ui.gfx
                .text(dot, &(index + 1).to_string(), SMALL, ink, Align::Center);
            row.take_left(space::S);
            let font = if index == self.step {
                BODY_MEDIUM
            } else {
                BODY
            };
            let width = ui.gfx.measure(name, font).width + 2.0;
            let label = row.take_left(width.min(row.w.max(0.0)));
            let color = if index == self.step { t.text } else { t.text2 };
            ui.gfx.text(label, name, font, color, Align::Start);
            if index + 1 < STEPS.len() && row.w > 40.0 {
                row.take_left(space::M);
                let line = row.take_left(28.0);
                let color = if index < self.step {
                    t.accent
                } else {
                    t.line_strong
                };
                ui.gfx.fill_rect(
                    Rect::new(line.x, line.center().y - 0.5, line.w, 1.0),
                    0.0,
                    color,
                );
                row.take_left(space::M);
            }
        }
    }

    /// A heading and the line under it; leaves `page` below them.
    fn heading(ui: &mut Ui<'_>, page: &mut Rect, title: &str, line: &str) {
        let t = ui.theme;
        ui.gfx
            .text(page.take_top(26.0), title, HEADING, t.text, Align::Start);
        ui.gfx
            .text(page.take_top(20.0), line, BODY, t.text2, Align::Start);
        page.take_top(space::L);
    }

    /// Step 1: a card per kind, each with its diagram; a click picks it.
    fn choose_kind(&mut self, ui: &mut Ui<'_>, page: &mut Rect, can_pick: bool) {
        let t = ui.theme;
        Self::heading(
            ui,
            page,
            "What should the rule do?",
            "Each card shows how its traffic travels. Point at one to watch it again.",
        );
        // Side by side when there is room, else one under another.
        let stacked = page.w < 680.0;
        // Stacked, the cards are short, so two fit a small window.
        let gap = if stacked { space::S } else { space::L };
        let card_h = if stacked { 110.0 } else { SMALL_DIAGRAM + 96.0 };
        let card_w = if stacked {
            page.w
        } else {
            (page.w - 2.0 * gap) / 3.0
        };
        let row = page.take_top(if stacked {
            3.0 * card_h + 2.0 * gap
        } else {
            card_h
        });
        for (index, (kind, title, lines)) in KINDS.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // Three cards.
            let at = index as f32;
            let card = if stacked {
                Rect::new(row.x, row.y + at * (card_h + gap), card_w, card_h)
            } else {
                Rect::new(row.x + at * (card_w + gap), row.y, card_w, card_h)
            };
            let id = Id::new(("wizard.kind", index));
            let response = ui.zone(id, card, Role::Button, title);
            let hover = ui.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
            ui.gfx
                .fill_rect(card, 12.0, t.surface.mix(t.hover, 0.4 * hover));
            ui.gfx
                .stroke_rect(card, 12.0, 1.0, t.line.mix(t.accent, 0.6 * hover));
            if response.focus_ring {
                ui.focus_ring(card, 12.0);
            }
            if response.hovered {
                ui.set_cursor(Cursor::Pointer);
            }
            let mut inside = card.inset(space::M, space::M);
            let (picture, mut words) = if stacked {
                let picture = inside.take_left(inside.w.min(300.0));
                inside.take_left(space::M);
                (
                    picture.centered(picture.w, SMALL_DIAGRAM.min(picture.h)),
                    inside,
                )
            } else {
                let picture = inside.take_top(SMALL_DIAGRAM);
                inside.take_top(space::S);
                (picture, inside)
            };
            let packet = self.cards[index].at(ui, response.hovered || response.focused);
            let sample = sample(*kind);
            diagram(ui, picture, *kind, &sample, packet);
            if stacked {
                words = words.centered(words.w, 20.0 + 4.0 + 2.0 * 18.0);
            }
            ui.gfx.text(
                words.take_top(20.0),
                title,
                BODY_STRONG,
                t.text,
                Align::Start,
            );
            words.take_top(4.0);
            for line in lines {
                ui.gfx
                    .text(words.take_top(18.0), line, SMALL, t.text2, Align::Start);
            }
            if response.clicked && can_pick {
                self.draft.rule.kind = *kind;
                if *kind == ForwardKind::Remote && self.draft.rule.destination_host.is_empty() {
                    "localhost".clone_into(&mut self.draft.rule.destination_host);
                }
                (self.step, self.play, self.problem) = (1, Play::default(), None);
            }
        }
        page.take_top(space::L);
        if !can_pick {
            ui.gfx.text(
                page.take_top(20.0),
                "Save a host first: a rule runs through one of your hosts.",
                BODY,
                t.warn,
                Align::Start,
            );
        }
    }

    /// The big diagram of the rule so far; leaves `page` below it.
    fn big_diagram(&mut self, ui: &mut Ui<'_>, page: &mut Rect, server: &str) {
        let picture = page.take_top(BIG_DIAGRAM);
        let rect = picture.centered(picture.w.min(620.0), picture.h);
        let hovered = ui.hover_within(rect);
        let packet = self.play.at(ui, hovered);
        let ends = ends(&self.draft.rule, self.ports(), server);
        diagram(ui, rect, self.draft.rule.kind, &ends, packet);
        page.take_top(space::L);
    }

    fn ports(&self) -> (&str, &str) {
        (&self.draft.bind_port, &self.draft.destination_port)
    }

    /// Step 2: the server, the port to listen on and, but for SOCKS, where traffic goes.
    #[allow(clippy::too_many_lines)] // The fields of three kinds, then the buttons.
    fn details(&mut self, ui: &mut Ui<'_>, page: &mut Rect, hosts: &[(u64, String)], server: &str) {
        let t = ui.theme;
        Self::heading(
            ui,
            page,
            "Through which server, and which ports?",
            "The diagram follows what you type.",
        );
        self.big_diagram(ui, page, server);
        let (listen, destination) = match self.draft.rule.kind {
            ForwardKind::Local => (
                "Port on this computer",
                Some("Destination, as the server sees it"),
            ),
            ForwardKind::Remote => ("Port on the server", Some("Service on this computer")),
            ForwardKind::Dynamic => ("Proxy port on this computer", None),
        };
        // The server and the port to listen on.
        let mut labels = page.take_top(18.0);
        let mut fields = page.take_top(CONTROL);
        let server_w = 260.0_f32.min(fields.w * 0.5);
        let pick = fields.take_left(server_w);
        let pick_label = labels.take_left(server_w);
        fields.take_left(space::L);
        labels.take_left(space::L);
        ui.section_label(pick_label, "Server");
        let names: Vec<String> = hosts.iter().map(|(_, name)| name.clone()).collect();
        let at = hosts
            .iter()
            .position(|(id, _)| *id == self.draft.rule.host_id);
        if let Some(chosen) = ui.select(Id::new("wizard.host"), pick, &names, at, "Choose a server")
        {
            self.draft.rule.host_id = hosts[chosen].0;
        }
        let port = fields.take_left(140.0_f32.min(fields.w));
        ui.section_label(labels.take_left(port.w.max(0.0) + 120.0), listen);
        ui.text_field(
            Id::new("wizard.listen"),
            port,
            &mut self.draft.bind_port,
            FieldStyle {
                placeholder: "e.g. 8080",
                ..FieldStyle::default()
            },
        );
        ui.set_name(Id::new("wizard.listen"), listen);
        page.take_top(space::L);
        if let Some(destination) = destination {
            ui.section_label(page.take_top(18.0), destination);
            let mut line = page.take_top(CONTROL);
            let port = line.take_right(140.0_f32.min(line.w * 0.4));
            line.take_right(space::M);
            ui.text_field(
                Id::new("wizard.dest"),
                line,
                &mut self.draft.rule.destination_host,
                FieldStyle {
                    placeholder: "Host, e.g. db.internal or localhost",
                    ..FieldStyle::default()
                },
            );
            ui.set_name(Id::new("wizard.dest"), &format!("{destination}: host"));
            ui.text_field(
                Id::new("wizard.dport"),
                port,
                &mut self.draft.destination_port,
                FieldStyle {
                    placeholder: "e.g. 5432",
                    ..FieldStyle::default()
                },
            );
            ui.set_name(Id::new("wizard.dport"), &format!("{destination}: port"));
            page.take_top(space::L);
        }
        if let Some(problem) = &self.problem {
            ui.gfx
                .text(page.take_top(20.0), problem, SMALL, t.error, Align::Start);
            page.take_top(space::S);
        }
        let mut buttons = page.take_top(CONTROL);
        let back = buttons.take_left(button_width(&mut ui.gfx, "Back", None));
        let next = buttons.take_right(button_width(&mut ui.gfx, "Next", None).max(96.0));
        if ui
            .button(Id::new("wizard.back"), back, "Back", None, Kind::Secondary)
            .clicked
        {
            (self.step, self.problem) = (0, None);
            return;
        }
        if ui
            .button(Id::new("wizard.next"), next, "Next", None, Kind::Primary)
            .clicked
        {
            if hosts.is_empty() {
                self.problem =
                    Some("Save a host first: a rule runs through one of your hosts.".into());
                return;
            }
            // A name comes later; one is given meanwhile, so only these fields are checked. Until
            // one is typed, it follows the server and ports each time.
            if !self.named || self.draft.rule.label.trim().is_empty() {
                self.draft.rule.label = default_label(&self.draft.rule, self.ports(), server);
                self.named = false;
            }
            match self.draft.finish() {
                Ok(_) => (self.step, self.play, self.problem) = (2, Play::default(), None),
                Err(problem) => self.problem = Some(problem),
            }
        }
    }

    /// Step 3: the rule in one sentence, its name and when it starts; Save or Save and start.
    fn review(&mut self, ui: &mut Ui<'_>, page: &mut Rect, server: &str) -> Option<Finish> {
        let t = ui.theme;
        Self::heading(
            ui,
            page,
            "Ready to save",
            &sentence(&self.draft.rule, self.ports(), server),
        );
        self.big_diagram(ui, page, server);
        ui.section_label(page.take_top(18.0), "Name");
        let name = page.take_top(CONTROL);
        if ui
            .text_field(
                Id::new("wizard.label"),
                Rect::new(name.x, name.y, name.w.min(420.0), CONTROL),
                &mut self.draft.rule.label,
                FieldStyle {
                    placeholder: "Name",
                    ..FieldStyle::default()
                },
            )
            .changed
        {
            self.named = true;
        }
        ui.set_name(Id::new("wizard.label"), "Name");
        page.take_top(space::M);
        let mut ticks = page.take_top(CONTROL);
        let first = ticks.take_left((ticks.w / 2.0).min(300.0));
        ui.checkbox(
            Id::new("wizard.auto"),
            first,
            &mut self.draft.rule.auto_start,
            "Start when the vault unlocks",
        );
        ui.checkbox(
            Id::new("wizard.with"),
            ticks,
            &mut self.draft.rule.with_host,
            "Start with the host's terminal",
        );
        page.take_top(space::L);
        if let Some(problem) = &self.problem {
            ui.gfx
                .text(page.take_top(20.0), problem, SMALL, t.error, Align::Start);
            page.take_top(space::S);
        }
        let mut buttons = page.take_top(CONTROL);
        let back = buttons.take_left(button_width(&mut ui.gfx, "Back", None));
        let go = buttons.take_right(button_width(&mut ui.gfx, "Save and start", None));
        buttons.take_right(space::S);
        let save = buttons.take_right(button_width(&mut ui.gfx, "Save", None).max(80.0));
        if ui
            .button(Id::new("wizard.back"), back, "Back", None, Kind::Secondary)
            .clicked
        {
            (self.step, self.play, self.problem) = (1, Play::default(), None);
            return None;
        }
        let saved = ui
            .button(Id::new("wizard.save"), save, "Save", None, Kind::Secondary)
            .clicked;
        let started = ui
            .button(
                Id::new("wizard.start"),
                go,
                "Save and start",
                None,
                Kind::Primary,
            )
            .clicked;
        if !(saved || started) {
            return None;
        }
        match self.draft.finish() {
            Ok(rule) => Some(Finish::Save(rule, started)),
            Err(problem) => {
                self.problem = Some(problem);
                None
            }
        }
    }
}

/// What the first step's card of `kind` shows: a typical rule of that kind.
fn sample(kind: ForwardKind) -> Ends {
    let mut rule = PortForward::new(0, 0);
    rule.kind = kind;
    let ports = match kind {
        ForwardKind::Local => {
            "db".clone_into(&mut rule.destination_host);
            ("8080", "5432")
        }
        ForwardKind::Remote => {
            "localhost".clone_into(&mut rule.destination_host);
            ("9000", "3000")
        }
        ForwardKind::Dynamic => ("1080", ""),
    };
    ends(&rule, ports, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_names_its_ends_and_says_what_it_does_with_what_was_typed() {
        let mut rule = PortForward::new(1, 1);
        rule.destination_host = "db.internal".into();
        assert_eq!(
            ends(&rule, ("15432", "5432"), "web-1"),
            [
                ("This computer".to_owned(), "localhost:15432".to_owned()),
                ("web-1".to_owned(), "SSH".to_owned()),
                ("Destination".to_owned(), "db.internal:5432".to_owned()),
            ]
        );
        assert_eq!(
            sentence(&rule, ("15432", "5432"), "web-1"),
            "Apps on this computer open localhost:15432 and reach db.internal:5432 as if from \
             web-1."
        );
        // What is still blank says so.
        assert_eq!(ends(&rule, ("", ""), "")[0].1, "localhost:port");
        assert_eq!(ends(&rule, ("", ""), "")[1].0, "Your server");
        rule.kind = ForwardKind::Remote;
        rule.destination_host = "localhost".into();
        assert_eq!(
            sentence(&rule, ("9000", "3000"), "web-1"),
            "Whoever opens port 9000 on web-1 reaches localhost:3000 on this computer."
        );
        rule.kind = ForwardKind::Dynamic;
        assert_eq!(
            sentence(&rule, ("1080", ""), "web-1"),
            "Apps set to use the SOCKS proxy localhost:1080 reach the web from web-1."
        );
    }

    #[test]
    fn a_packet_moves_along_its_path_by_length() {
        let path = [
            Point::new(0.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(30.0, 10.0),
        ];
        assert_eq!(along(&path, 0.0), Point::new(0.0, 0.0));
        assert_eq!(along(&path, 0.75), Point::new(30.0, 0.0));
        assert_eq!(along(&path, 1.0), Point::new(30.0, 10.0));
        assert_eq!(
            along(&path, 2.0),
            Point::new(30.0, 10.0),
            "never past the end"
        );
    }
}
