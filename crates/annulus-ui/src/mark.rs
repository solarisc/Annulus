//! Our mark (UI-SPEC §8): a dark tile holding a ring of segments that is
//! open at the bottom, three chevrons on it pointing in, a letter A without its bar whose legs
//! run out through the opening, and a four-pointed spark under its peak. One list of shapes on
//! a 20-unit grid draws it everywhere: the title bar, the lock screen, the film's cards, and the
//! app icon and the README's logo (`cargo xtask icon`), so none of them can drift from the
//! others. It brings its own tile, so it keeps its own colors (`tokens::MARK`) in every theme.

use annulus_config::tokens::MARK;

use crate::gfx::{Canvas, Color, Point, Rect};

/// The grid the mark is drawn on, in units a side.
pub const GRID: f32 = 20.0;

/// The tile's corner radius.
const CORNER: f32 = 5.2;
/// The width of the tile's outline, inside its edge.
const OUTLINE: f32 = 0.17;
/// The middle of the ring, a little below the tile's.
const CENTER: Point = Point::new(10.0, 10.32);
/// The ring's inner and outer radius.
const RING: (f32, f32) = (5.3, 6.3);
/// The width of the cuts between the ring's segments, and between a segment and a chevron
/// or a leg of the letter.
const CUT: f32 = 0.36;
const CHEVRON_CUT: f32 = 0.5;
const LEG_CUT: f32 = 0.35;
/// The chevrons: how far out their tips and bases are, and half their bases' width.
const CHEVRON: (f32, f32, f32) = (4.92, 6.52, 0.68);
/// Where the side chevrons sit: this many degrees above three and nine o'clock.
const SIDE_CHEVRON: f32 = 9.0;
/// The cuts between segments on the right half, in degrees clockwise from three o'clock.
const CUTS: [f32; 2] = [-48.7, 29.7];
/// The letter: the corners of its outline, clockwise from the top of its peak.
const LETTER: [Point; 7] = [
    Point::new(9.51, 7.2),
    Point::new(10.49, 7.2),
    Point::new(14.88, 16.03),
    Point::new(13.49, 16.03),
    Point::new(10.0, 8.86),
    Point::new(6.51, 16.03),
    Point::new(5.12, 16.03),
];
/// The spark: its middle, how far its points reach across and up, and how deep its sides
/// curve in (the power of the curve between two points; 1 would be straight).
const SPARK: (Point, f32, f32, f32) = (Point::new(10.0, 12.59), 0.98, 1.02, 0.556);

/// What a shape is colored as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The tile.
    Tile,
    /// The ring's segments and the tile's outline.
    Ring,
    /// The letter.
    Letter,
    /// The chevrons and the spark.
    Light,
}

/// One shape of the mark, in grid units.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// The tile: a square over the whole grid, its corners rounded by `radius`.
    Tile {
        /// The corners' radius.
        radius: f32,
    },
    /// The tile's outline, `width` wide inside its edge.
    Outline {
        /// The corners' radius at the tile's edge.
        radius: f32,
        /// The outline's width.
        width: f32,
    },
    /// A filled polygon.
    Polygon(Vec<Point>),
}

/// The point `radius` out from the ring's middle at `degrees`, clockwise from three o'clock.
fn polar(radius: f32, degrees: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Point::new(CENTER.x + radius * cos, CENTER.y + radius * sin)
}

fn angle_of(p: Point) -> f32 {
    (p.y - CENTER.y).atan2(p.x - CENTER.x).to_degrees()
}

/// The line through `a` and `b` moved `by` to the side away from `from`.
fn moved(a: Point, b: Point, by: f32, from: Point) -> (Point, Point) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx.hypot(dy);
    let (mut nx, mut ny) = (-dy / length, dx / length);
    if (from.x - a.x) * nx + (from.y - a.y) * ny > 0.0 {
        (nx, ny) = (-nx, -ny);
    }
    (
        Point::new(a.x + nx * by, a.y + ny * by),
        Point::new(b.x + nx * by, b.y + ny * by),
    )
}

/// Where the line crosses the circle of `radius` round the ring's middle, as an angle: the
/// crossing nearest `near` degrees.
fn crossing((a, b): (Point, Point), radius: f32, near: f32) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (fx, fy) = (a.x - CENTER.x, a.y - CENTER.y);
    let qa = dx * dx + dy * dy;
    let qb = 2.0 * (fx * dx + fy * dy);
    let qc = fx * fx + fy * fy - radius * radius;
    let root = (qb * qb - 4.0 * qa * qc).max(0.0).sqrt();
    [(-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa)]
        .map(|t| angle_of(Point::new(a.x + t * dx, a.y + t * dy)))
        .into_iter()
        .min_by(|x, y| {
            let off = |angle: f32| (angle - near + 540.0).rem_euclid(360.0) - 180.0;
            off(*x).abs().total_cmp(&off(*y).abs())
        })
        .unwrap_or(near)
}

/// Points along the circle of `radius` from one angle to another, about every 3 degrees.
fn arc(radius: f32, from: f32, to: f32) -> impl Iterator<Item = Point> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Under a turn.
    let steps = ((to - from).abs() / 3.0).ceil().max(1.0) as u16;
    (0..=steps).map(move |step| {
        polar(
            radius,
            from + (to - from) * f32::from(step) / f32::from(steps),
        )
    })
}

/// The chevron at `degrees`: a triangle pointing in, its tip first, then its base's corner a
/// step clockwise and the one a step back.
fn chevron(degrees: f32) -> [Point; 3] {
    let (tip, base, half) = CHEVRON;
    let (sin, cos) = degrees.to_radians().sin_cos();
    let across = Point::new(-sin * half, cos * half);
    let middle = polar(base, degrees);
    [
        polar(tip, degrees),
        Point::new(middle.x + across.x, middle.y + across.y),
        Point::new(middle.x - across.x, middle.y - across.y),
    ]
}

/// The ring's segments on the right half, clockwise from the top; the left half mirrors them.
fn right_segments() -> Vec<Vec<Point>> {
    let (inner, outer) = RING;
    let [top_tip, top_right, _] = chevron(-90.0);
    let [side_tip, side_down, side_up] = chevron(-SIDE_CHEVRON);
    let away =
        |[tip, a, b]: [Point; 3]| Point::new((tip.x + a.x + b.x) / 3.0, (tip.y + a.y + b.y) / 3.0);
    let radial = |degrees: f32, side: f32| {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let shift = Point::new(-sin * CUT / 2.0 * side, cos * CUT / 2.0 * side);
        let a = Point::new(CENTER.x + shift.x, CENTER.y + shift.y);
        (a, Point::new(a.x + cos, a.y + sin))
    };
    let top = moved(top_tip, top_right, CHEVRON_CUT, away(chevron(-90.0)));
    let side_upper = moved(side_tip, side_up, CHEVRON_CUT, away(chevron(-SIDE_CHEVRON)));
    let side_lower = moved(
        side_tip,
        side_down,
        CHEVRON_CUT,
        away(chevron(-SIDE_CHEVRON)),
    );
    let leg = moved(LETTER[1], LETTER[2], LEG_CUT, CENTER);
    let [first, second] = CUTS;
    // Each segment runs from one cut line to the next, near the angles given.
    let bounds = [
        (top, -80.0, radial(first, -1.0), first),
        (radial(first, 1.0), first, side_upper, -16.0),
        (side_lower, 0.0, radial(second, -1.0), second),
        (radial(second, 1.0), second, leg, 43.0),
    ];
    bounds
        .into_iter()
        .map(|(start, near_start, end, near_end)| {
            let (out_from, out_to) = (
                crossing(start, outer, near_start),
                crossing(end, outer, near_end),
            );
            let (in_from, in_to) = (
                crossing(start, inner, near_start),
                crossing(end, inner, near_end),
            );
            arc(outer, out_from, out_to)
                .chain(arc(inner, in_to, in_from))
                .collect()
        })
        .collect()
}

fn mirrored(points: &[Point]) -> Vec<Point> {
    points
        .iter()
        .rev()
        .map(|p| Point::new(2.0 * CENTER.x - p.x, p.y))
        .collect()
}

/// The spark: four points joined by curves bowed in toward its middle.
fn spark() -> Vec<Point> {
    let (middle, across, up, power) = SPARK;
    let quarter = 16_u16;
    (0..4 * quarter)
        .map(|step| {
            let turn = std::f32::consts::TAU * f32::from(step) / f32::from(4 * quarter);
            let (sin, cos) = turn.sin_cos();
            // A superellipse: |x|^p + |y|^p = 1, its sides bowed in for a power under 1.
            let bow = |v: f32| v.signum() * v.abs().powf(2.0 / power);
            Point::new(middle.x + across * bow(cos), middle.y + up * bow(sin))
        })
        .collect()
}

/// The mark, back to front.
#[must_use]
pub fn shapes() -> Vec<(Shape, Tone)> {
    let mut out = vec![
        (Shape::Tile { radius: CORNER }, Tone::Tile),
        (
            Shape::Outline {
                radius: CORNER,
                width: OUTLINE,
            },
            Tone::Ring,
        ),
    ];
    for segment in right_segments() {
        out.push((Shape::Polygon(mirrored(&segment)), Tone::Ring));
        out.push((Shape::Polygon(segment), Tone::Ring));
    }
    for degrees in [-90.0, -SIDE_CHEVRON, SIDE_CHEVRON - 180.0] {
        out.push((Shape::Polygon(chevron(degrees).to_vec()), Tone::Light));
    }
    out.push((Shape::Polygon(LETTER.to_vec()), Tone::Letter));
    out.push((Shape::Polygon(spark()), Tone::Light));
    out
}

/// The color of `tone`.
#[must_use]
pub fn color(tone: Tone) -> Color {
    Color::from(match tone {
        Tone::Tile => MARK.tile,
        Tone::Ring => MARK.ring,
        Tone::Letter => MARK.letter,
        Tone::Light => MARK.light,
    })
}

/// Draws the mark into the square in the middle of `rect`.
pub fn paint<C: Canvas + ?Sized>(canvas: &mut C, rect: Rect) {
    let side = rect.w.min(rect.h);
    let unit = side / GRID;
    let origin = Point::new(
        rect.x + (rect.w - side) / 2.0,
        rect.y + (rect.h - side) / 2.0,
    );
    let tile = Rect::new(origin.x, origin.y, side, side);
    let at = |p: &Point| Point::new(origin.x + p.x * unit, origin.y + p.y * unit);
    for (shape, tone) in shapes() {
        let color = color(tone);
        match shape {
            Shape::Tile { radius } => canvas.fill_rect(tile, radius * unit, color),
            Shape::Outline { radius, width } => {
                let half = width * unit / 2.0;
                canvas.stroke_rect(
                    tile.inset(half, half),
                    radius * unit - half,
                    width * unit,
                    color,
                );
            }
            Shape::Polygon(points) => {
                let points: Vec<Point> = points.iter().map(at).collect();
                canvas.fill_polygon(&points, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use annulus_config::tokens::Rgb;

    use super::*;

    fn polygons(tone: Tone) -> Vec<Vec<Point>> {
        shapes()
            .into_iter()
            .filter(|(_, t)| *t == tone)
            .filter_map(|(shape, _)| match shape {
                Shape::Polygon(points) => Some(points),
                _ => None,
            })
            .collect()
    }

    fn distance(p: Point) -> f32 {
        (p.x - CENTER.x).hypot(p.y - CENTER.y)
    }

    #[test]
    fn three_chevrons_point_in_at_the_top_and_either_side() {
        let chevrons: Vec<Vec<Point>> = polygons(Tone::Light)
            .into_iter()
            .filter(|points| points.len() == 3)
            .collect();
        assert_eq!(chevrons.len(), 3);
        for points in &chevrons {
            let tip = points[0];
            assert!(
                points[1..]
                    .iter()
                    .all(|base| distance(*base) > distance(tip))
            );
        }
        let tips: Vec<Point> = chevrons.iter().map(|points| points[0]).collect();
        assert!(
            (tips[0].x - 10.0).abs() < 1e-4 && tips[0].y < CENTER.y,
            "the top"
        );
        assert!(
            (tips[1].x + tips[2].x - 20.0).abs() < 1e-4,
            "the sides mirror"
        );
        assert!((tips[1].y - tips[2].y).abs() < 1e-4 && tips[1].y < CENTER.y);
    }

    #[test]
    fn nothing_reaches_past_the_tile() {
        for (shape, _) in shapes() {
            if let Shape::Polygon(points) = shape {
                for p in points {
                    assert!(
                        (0.5..=19.5).contains(&p.x) && (0.5..=19.5).contains(&p.y),
                        "{p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_ring_is_open_at_the_bottom_where_the_legs_run_out() {
        let segments = polygons(Tone::Ring);
        assert_eq!(segments.len(), 8);
        for points in &segments {
            for p in points {
                let below = angle_of(*p);
                assert!(!(50.0..130.0).contains(&below), "{p:?} at {below}");
                assert!((RING.0 - 0.01..=RING.1 + 0.01).contains(&distance(*p)));
            }
        }
        let lowest_ring = segments.iter().flatten().map(|p| p.y).fold(0.0, f32::max);
        let feet = LETTER.iter().map(|p| p.y).fold(0.0, f32::max);
        assert!(feet > lowest_ring + 1.0, "the legs run out past the ring");
    }

    #[test]
    fn segments_keep_their_cuts_apart() {
        let segments = polygons(Tone::Ring);
        let near = |a: &Vec<Point>, b: &Vec<Point>| {
            a.iter()
                .flat_map(|p| b.iter().map(move |q| (p.x - q.x).hypot(p.y - q.y)))
                .fold(f32::MAX, f32::min)
        };
        for (index, a) in segments.iter().enumerate() {
            for b in &segments[index + 1..] {
                assert!(near(a, b) >= CUT * 0.9, "{}", near(a, b));
            }
        }
    }

    #[test]
    fn the_letter_and_the_light_stand_out_from_the_tile() {
        let rgb = |c: Color| Rgb::new(c.r, c.g, c.b);
        let tile = rgb(color(Tone::Tile));
        for tone in [Tone::Letter, Tone::Light] {
            let contrast = rgb(color(tone)).contrast(tile);
            // WCAG's bar for graphics (1.4.11), with room to spare.
            assert!(contrast >= 7.0, "{tone:?}: {contrast:.2}");
        }
    }
}
