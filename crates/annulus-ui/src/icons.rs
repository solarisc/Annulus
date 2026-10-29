//! Icon set v1: our own line icons (UI-SPEC §8, ADR-0010), drawn on a 16x16 grid and
//! stroked 1.5 units wide with round caps and joins. Renderers scale them into the icon's
//! rectangle, so they stay sharp at any size and DPI.

/// Identifies one of our icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconId(pub u16);

/// One drawing step, in 16x16 grid units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    /// Start a new stroke at a point.
    Move(f32, f32),
    /// Straight line to a point.
    Line(f32, f32),
    /// Closes the current stroke back to its start.
    Close,
    /// A full circle: center x, center y, radius.
    Circle(f32, f32, f32),
}

/// An icon's geometry.
#[derive(Debug)]
pub struct Icon {
    /// A stable name, for the gallery and tests.
    pub name: &'static str,
    /// The strokes.
    pub segments: &'static [Segment],
}

use Segment::{Circle, Close, Line, Move};

macro_rules! icons {
    ($($(#[$doc:meta])* $id:ident = $index:literal, $name:literal, [$($segment:expr),* $(,)?];)*) => {
        $(
            $(#[$doc])*
            pub const $id: IconId = IconId($index);
        )*

        /// Every icon in id order.
        pub const ALL: &[(IconId, Icon)] = &[
            $(($id, Icon { name: $name, segments: &[$($segment),*] }),)*
        ];
    };
}

icons! {
    /// Drop-down and split-button menus.
    CHEVRON_DOWN = 1, "chevron-down", [Move(4.0, 6.0), Line(8.0, 10.0), Line(12.0, 6.0)];
    /// Checked check boxes.
    CHECK = 2, "check", [Move(3.5, 8.5), Line(6.5, 11.5), Line(12.5, 4.5)];
    /// Search fields.
    SEARCH = 3, "search", [Circle(7.0, 7.0, 4.25), Move(10.25, 10.25), Line(13.5, 13.5)];
    /// Closing tabs, dialogs and banners.
    CLOSE = 4, "close", [Move(4.0, 4.0), Line(12.0, 12.0), Move(12.0, 4.0), Line(4.0, 12.0)];
    /// New host, new tab.
    PLUS = 5, "plus", [Move(8.0, 3.0), Line(8.0, 13.0), Move(3.0, 8.0), Line(13.0, 8.0)];
    /// Expanding tree rows.
    CHEVRON_RIGHT = 6, "chevron-right", [Move(6.0, 4.0), Line(10.0, 8.0), Line(6.0, 12.0)];
    /// Local shells and terminals.
    TERMINAL = 7, "terminal", [
        Move(2.0, 3.0), Line(14.0, 3.0), Line(14.0, 13.0), Line(2.0, 13.0), Close,
        Move(4.5, 6.0), Line(7.0, 8.0), Line(4.5, 10.0),
        Move(8.5, 10.5), Line(11.5, 10.5),
    ];
    /// Hosts.
    SERVER = 8, "server", [
        Move(2.5, 2.5), Line(13.5, 2.5), Line(13.5, 7.0), Line(2.5, 7.0), Close,
        Move(2.5, 9.0), Line(13.5, 9.0), Line(13.5, 13.5), Line(2.5, 13.5), Close,
        Move(5.0, 4.75), Line(5.5, 4.75), Move(5.0, 11.25), Line(5.5, 11.25),
    ];
    /// Keys and identities.
    KEY = 9, "key", [
        Circle(5.0, 11.0, 2.75),
        Move(7.0, 9.0), Line(13.0, 3.0),
        Move(11.0, 5.0), Line(12.75, 6.75),
        Move(9.5, 6.5), Line(11.0, 8.0),
    ];
    /// Groups and SFTP folders.
    FOLDER = 10, "folder", [
        Move(2.0, 4.0), Line(6.0, 4.0), Line(7.5, 5.5), Line(14.0, 5.5),
        Line(14.0, 12.5), Line(2.0, 12.5), Close,
    ];
    /// Copy to clipboard.
    COPY = 11, "copy", [
        Move(5.5, 5.5), Line(13.0, 5.5), Line(13.0, 13.0), Line(5.5, 13.0), Close,
        Move(3.0, 10.5), Line(3.0, 3.0), Line(10.5, 3.0),
    ];
    /// Overflow menus.
    MORE = 12, "more", [
        Circle(3.5, 8.0, 0.75), Circle(8.0, 8.0, 0.75), Circle(12.5, 8.0, 0.75),
    ];
    /// The app lock and locked vaults.
    LOCK = 13, "lock", [
        Move(3.5, 7.5), Line(12.5, 7.5), Line(12.5, 14.0), Line(3.5, 14.0), Close,
        Move(5.5, 7.5), Line(5.5, 5.0), Line(6.5, 3.0), Line(9.5, 3.0), Line(10.5, 5.0), Line(10.5, 7.5),
    ];
    /// Settings.
    SETTINGS = 14, "settings", [
        Circle(8.0, 8.0, 2.25),
        Move(8.0, 1.5), Line(8.0, 3.5), Move(8.0, 12.5), Line(8.0, 14.5),
        Move(1.5, 8.0), Line(3.5, 8.0), Move(12.5, 8.0), Line(14.5, 8.0),
        Move(3.4, 3.4), Line(4.8, 4.8), Move(11.2, 11.2), Line(12.6, 12.6),
        Move(12.6, 3.4), Line(11.2, 4.8), Move(4.8, 11.2), Line(3.4, 12.6),
    ];
}

/// The geometry of `id`, or `None` for an unknown id.
#[must_use]
pub fn icon(id: IconId) -> Option<&'static Icon> {
    ALL.iter()
        .find(|(known, _)| *known == id)
        .map(|(_, icon)| icon)
}

/// Draws `id` scaled into `rect` with `color`, as 1.5-unit round-capped strokes. Unknown
/// icons draw nothing.
pub fn paint(painter: &egui::Painter, id: IconId, rect: egui::Rect, color: egui::Color32) {
    let Some(icon) = icon(id) else { return };
    let unit = rect.width().min(rect.height()) / GRID;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x * unit, y * unit);
    let stroke = egui::Stroke::new(STROKE * unit, color);
    let mut line: Vec<egui::Pos2> = Vec::new();
    let mut start = None;
    let flush = |line: &mut Vec<egui::Pos2>| {
        if line.len() > 1 {
            painter.add(egui::Shape::line(std::mem::take(line), stroke));
        }
        line.clear();
    };
    for segment in icon.segments {
        match *segment {
            Segment::Move(x, y) => {
                flush(&mut line);
                line.push(at(x, y));
                start = Some(at(x, y));
            }
            Segment::Line(x, y) => line.push(at(x, y)),
            Segment::Close => {
                if let Some(start) = start {
                    line.push(start);
                }
            }
            Segment::Circle(x, y, r) => {
                flush(&mut line);
                painter.circle_stroke(at(x, y), r * unit, stroke);
            }
        }
    }
    flush(&mut line);
}

/// The size of the design grid.
pub const GRID: f32 = 16.0;

/// The stroke width on the design grid.
pub const STROKE: f32 = 1.5;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_and_names_are_unique_and_resolve() {
        let ids: HashSet<_> = ALL.iter().map(|(id, _)| *id).collect();
        let names: HashSet<_> = ALL.iter().map(|(_, icon)| icon.name).collect();
        assert_eq!(ids.len(), ALL.len());
        assert_eq!(names.len(), ALL.len());
        for (id, _) in ALL {
            assert!(icon(*id).is_some());
        }
        assert!(icon(IconId(u16::MAX)).is_none());
    }

    #[test]
    fn every_icon_stays_inside_the_grid_with_room_for_the_stroke() {
        let margin = STROKE / 2.0;
        let inside = |v: f32| (margin..=GRID - margin).contains(&v);
        for (_, icon) in ALL {
            assert!(
                matches!(icon.segments.first(), Some(Move(..) | Circle(..))),
                "{}",
                icon.name
            );
            for segment in icon.segments {
                let fits = match *segment {
                    Move(x, y) | Line(x, y) => inside(x) && inside(y),
                    Circle(x, y, r) => {
                        inside(x - r) && inside(x + r) && inside(y - r) && inside(y + r)
                    }
                    Close => true,
                };
                assert!(fits, "{}: {segment:?}", icon.name);
            }
        }
    }
}
