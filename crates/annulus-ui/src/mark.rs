//! Our mark (ADR-0010): a ring gate. A metal ring holds nine chevrons around a pool of the
//! accent, and where there are pixels enough to show them, the ring has rims, a track and
//! glyphs running round it. One list of shapes on a 20-unit grid draws it everywhere: the
//! title bar, the lock screen, the film's cards, and the app icon and the README's logo
//! (`cargo xtask icon`), so none of them can drift from the others.

use crate::gfx::{Canvas, Color, Point, Rect};
use crate::kit::Theme;

/// The grid the mark is drawn on, in units a side.
pub const GRID: f32 = 20.0;
/// How many chevrons the ring holds.
pub const CHEVRONS: u16 = 9;

/// From this many pixels across, the ring shows its rims and track and the pool glows.
const RIMMED: f32 = 40.0;
/// From this many pixels across, glyphs run round the track.
const GLYPHED: f32 = 64.0;

/// The middle of the grid.
const CENTER: Point = Point::new(GRID / 2.0, GRID / 2.0);
/// The ring: the radius of its middle, and its width.
const RING: (f32, f32) = (7.9, 3.0);
/// The track inside the ring, which the glyphs run along.
const TRACK: (f32, f32) = (7.75, 1.5);
/// The chevrons: how far in their tips reach, how far out their arms, how far apart the arms
/// spread, and their stroke.
const CHEVRON: (f32, f32, f32, f32) = (7.0, 9.0, 1.25, 1.15);
/// Where in each gap between two chevrons the glyphs sit, in degrees past the first one.
const GLYPHS: [f32; 3] = [13.0, 20.0, 27.0];
/// The stroke of a glyph.
const GLYPH: f32 = 0.28;
/// The pool's glow: faint discs one over another, so it brightens toward the middle in small
/// steps.
const GLOW: [f32; 5] = [5.6, 4.6, 3.6, 2.6, 1.6];

/// What a shape is colored as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The ring in one piece, where it is too small for rims and a track.
    Band,
    /// The ring's rims, and the glyphs on its track.
    Rim,
    /// The track inside the ring.
    Track,
    /// The ring's outlines.
    Edge,
    /// The chevrons.
    Chevron,
    /// The pool inside the ring.
    Horizon,
    /// The pool's glow, layered brighter toward its middle.
    Glow,
}

/// One shape of the mark, in grid units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A filled circle.
    Disc {
        /// Its middle.
        center: Point,
        /// Its radius.
        radius: f32,
    },
    /// A stroked circle.
    Ring {
        /// Its middle.
        center: Point,
        /// The radius of the stroke's middle.
        radius: f32,
        /// The stroke's width.
        width: f32,
    },
    /// A straight stroke with round caps.
    Line {
        /// Where it starts.
        from: Point,
        /// Where it ends.
        to: Point,
        /// The stroke's width.
        width: f32,
    },
    /// A chevron: two strokes meeting at `tip`, with round caps and a round join.
    Vee {
        /// The end of one arm.
        left: Point,
        /// Where the arms meet.
        tip: Point,
        /// The end of the other arm.
        right: Point,
        /// The stroke's width.
        width: f32,
    },
}

impl Shape {
    /// How far from the grid's middle the shape reaches, strokes included.
    #[must_use]
    pub fn reach(&self) -> f32 {
        let from_center = |p: Point| (p.x - CENTER.x).hypot(p.y - CENTER.y);
        match *self {
            Shape::Disc { center, radius } => from_center(center) + radius,
            Shape::Ring {
                center,
                radius,
                width,
            } => from_center(center) + radius + width / 2.0,
            Shape::Line { from, to, width } => from_center(from).max(from_center(to)) + width / 2.0,
            Shape::Vee {
                left,
                tip,
                right,
                width,
            } => {
                from_center(left)
                    .max(from_center(tip))
                    .max(from_center(right))
                    + width / 2.0
            }
        }
    }
}

/// The point `radius` out from the middle at `degrees` (clockwise from three o'clock), moved
/// `across` at right angles to that.
fn polar(radius: f32, degrees: f32, across: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Point::new(
        CENTER.x + radius * cos - across * sin,
        CENTER.y + radius * sin + across * cos,
    )
}

fn ring((radius, width): (f32, f32)) -> Shape {
    Shape::Ring {
        center: CENTER,
        radius,
        width,
    }
}

fn disc(radius: f32) -> Shape {
    Shape::Disc {
        center: CENTER,
        radius,
    }
}

/// The angle of chevron `index`, the first straight up.
fn chevron_angle(index: u16) -> f32 {
    -90.0 + 360.0 / f32::from(CHEVRONS) * f32::from(index)
}

/// The mark drawn `pixels` across, back to front. Its finer parts come only where there are
/// pixels to show them; small, it is a ring, its chevrons and the pool.
#[must_use]
pub fn shapes(pixels: f32) -> Vec<(Shape, Tone)> {
    let (ring_r, ring_w) = RING;
    let mut out = vec![(disc(ring_r - ring_w / 2.0 + 0.3), Tone::Horizon)];
    if pixels >= RIMMED {
        out.extend(GLOW.map(|radius| (disc(radius), Tone::Glow)));
        out.push((ring(RING), Tone::Rim));
        out.push((ring(TRACK), Tone::Track));
        out.push((ring((ring_r - ring_w / 2.0, 0.3)), Tone::Edge));
    } else {
        out.push((ring(RING), Tone::Band));
    }
    if pixels >= GLYPHED {
        let (track, _) = TRACK;
        for index in 0..CHEVRONS {
            for (place, offset) in (index..).zip(GLYPHS) {
                let degrees = chevron_angle(index) + offset;
                // Three kinds of glyph, so neighbors differ: across the track, a dot, along it.
                let glyph = match place % 3 {
                    0 => Shape::Line {
                        from: polar(track - 0.4, degrees, 0.0),
                        to: polar(track + 0.4, degrees, 0.0),
                        width: GLYPH,
                    },
                    1 => Shape::Disc {
                        center: polar(track, degrees, 0.0),
                        radius: 0.24,
                    },
                    _ => Shape::Line {
                        from: polar(track, degrees, -0.28),
                        to: polar(track, degrees, 0.28),
                        width: GLYPH,
                    },
                };
                out.push((glyph, Tone::Rim));
            }
        }
    }
    out.push((ring((ring_r + ring_w / 2.0, 0.35)), Tone::Edge));
    let (tip, arms, spread, width) = CHEVRON;
    for index in 0..CHEVRONS {
        let degrees = chevron_angle(index);
        out.push((
            Shape::Vee {
                left: polar(arms, degrees, -spread),
                tip: polar(tip, degrees, 0.0),
                right: polar(arms, degrees, spread),
                width,
            },
            Tone::Chevron,
        ));
    }
    out
}

/// The colors the mark is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    /// [`Tone::Band`].
    pub band: Color,
    /// [`Tone::Rim`].
    pub rim: Color,
    /// [`Tone::Track`].
    pub track: Color,
    /// [`Tone::Edge`].
    pub edge: Color,
    /// [`Tone::Chevron`].
    pub chevron: Color,
    /// [`Tone::Horizon`].
    pub horizon: Color,
    /// [`Tone::Glow`].
    pub glow: Color,
}

impl Colors {
    /// The mark in `theme`'s colors: a ring of its secondary text on its title bar, chevrons of
    /// its warning color, and a pool of its accent.
    #[must_use]
    pub fn for_theme(theme: &Theme) -> Self {
        let rim = theme.text2;
        Self {
            band: rim.mix(theme.chrome, 0.3),
            rim,
            track: rim.mix(theme.chrome, 0.75),
            edge: theme.chrome,
            chevron: theme.warn,
            horizon: theme.accent,
            glow: theme.accent.mix(Color::WHITE, 0.45).alpha(0.17),
        }
    }

    /// The color of `tone`.
    #[must_use]
    pub fn of(&self, tone: Tone) -> Color {
        match tone {
            Tone::Band => self.band,
            Tone::Rim => self.rim,
            Tone::Track => self.track,
            Tone::Edge => self.edge,
            Tone::Chevron => self.chevron,
            Tone::Horizon => self.horizon,
            Tone::Glow => self.glow,
        }
    }
}

/// Draws the mark into the square in the middle of `rect`, with as much detail as its pixels
/// on this screen can show.
pub fn paint<C: Canvas + ?Sized>(canvas: &mut C, rect: Rect, colors: &Colors) {
    let side = rect.w.min(rect.h);
    let unit = side / GRID;
    let origin = Point::new(
        rect.x + (rect.w - side) / 2.0,
        rect.y + (rect.h - side) / 2.0,
    );
    let at = |p: Point| Point::new(origin.x + p.x * unit, origin.y + p.y * unit);
    for (shape, tone) in shapes(side * canvas.scale()) {
        let color = colors.of(tone);
        match shape {
            Shape::Disc { center, radius } => canvas.fill_circle(at(center), radius * unit, color),
            Shape::Ring {
                center,
                radius,
                width,
            } => canvas.stroke_circle(at(center), radius * unit, width * unit, color),
            Shape::Line { from, to, width } => {
                canvas.polyline(&[at(from), at(to)], false, width * unit, color);
            }
            Shape::Vee {
                left,
                tip,
                right,
                width,
            } => canvas.polyline(&[at(left), at(tip), at(right)], false, width * unit, color),
        }
    }
}

#[cfg(test)]
mod tests {
    use annulus_config::tokens::{Appearance, Rgb};

    use super::*;

    fn tips(pixels: f32) -> Vec<Point> {
        shapes(pixels)
            .into_iter()
            .filter_map(|(shape, _)| match shape {
                Shape::Vee { tip, .. } => Some(tip),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nine_chevrons_ring_the_mark_evenly_with_one_straight_up() {
        for pixels in [16.0, 256.0] {
            let tips = tips(pixels);
            assert_eq!(tips.len(), 9);
            let angles: Vec<f32> = tips
                .iter()
                .map(|tip| (tip.y - CENTER.y).atan2(tip.x - CENTER.x).to_degrees())
                .collect();
            assert!(
                (angles[0] + 90.0).abs() < 1e-3,
                "the first points down from the top"
            );
            for pair in angles.windows(2) {
                let step = (pair[1] - pair[0]).rem_euclid(360.0);
                assert!((step - 40.0).abs() < 1e-3, "{angles:?}");
            }
        }
    }

    #[test]
    fn nothing_reaches_past_the_grid_at_any_size() {
        // Room for the edge pixel's smoothing, so an icon is never cut off at its side.
        for pixels in [16.0, 20.0, 40.0, 64.0, 256.0] {
            for (shape, _) in shapes(pixels) {
                assert!(
                    shape.reach() <= GRID / 2.0 - 0.3,
                    "{shape:?} at {pixels} px"
                );
            }
        }
    }

    #[test]
    fn fine_parts_come_only_with_the_pixels_to_show_them() {
        let tones = |pixels: f32| -> Vec<Tone> {
            shapes(pixels).into_iter().map(|(_, tone)| tone).collect()
        };
        let small = tones(20.0);
        assert!(small.contains(&Tone::Band));
        assert!(!small.contains(&Tone::Track) && !small.contains(&Tone::Glow));
        assert_eq!(small.len(), 3 + 9, "pool, ring, outline and the chevrons");
        let large = tones(256.0);
        assert!(!large.contains(&Tone::Band));
        let glyphs = shapes(256.0)
            .into_iter()
            .filter(|(shape, tone)| *tone == Tone::Rim && !matches!(shape, Shape::Ring { .. }))
            .count();
        assert_eq!(glyphs, 27, "three in each gap between chevrons");
        assert!(
            shapes(48.0).len() < shapes(64.0).len(),
            "no glyphs below 64 px"
        );
    }

    #[test]
    fn the_ring_its_chevrons_and_pool_stand_out_from_the_title_bar_in_both_themes() {
        let rgb = |c: Color| Rgb::new(c.r, c.g, c.b);
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::new(appearance);
            let colors = Colors::for_theme(&theme);
            let bar = rgb(theme.chrome);
            for (name, color) in [
                ("band", colors.band),
                ("rim", colors.rim),
                ("chevron", colors.chevron),
                ("horizon", colors.horizon),
            ] {
                let contrast = rgb(color).contrast(bar);
                // WCAG's bar for graphics (1.4.11).
                assert!(contrast >= 3.0, "{name} in {appearance:?}: {contrast:.2}");
            }
        }
    }
}
