//! `cargo xtask icon`: draws our mark (the ring gate of `annulus_ui::mark`, which the title bar
//! shows, ADR-0010) in the dark theme's colors at every Windows and macOS icon size and writes:
//!
//! - `crates/annulus-app/assets/annulus.ico`, the icon file: the ring alone, on nothing;
//! - `crates/annulus-app/assets/annulus.res`, the same images as a compiled Windows resource
//!   (icon group 1), which `build.rs` hands to the MSVC linker so the exe carries it. Writing
//!   the `.res` here keeps the build free of a resource compiler;
//! - `crates/annulus-app/assets/annulus.icns`, the macOS icon `cargo xtask mac-app` puts in
//!   `Annulus.app`: the ring on a dark tile on macOS's icon grid, a margin around it as around
//!   every other app's in the Dock;
//! - `docs/media/logo.svg`, the README's logo: the ring as vectors.
//!
//! All four are committed, and a test fails while one is not what the mark draws now: rerun
//! this after changing the mark.

use std::fmt::Write as _;
use std::path::PathBuf;

use annulus_config::tokens::Appearance;
use annulus_ui::gfx::{Color, Point};
use annulus_ui::kit::Theme;
use annulus_ui::mark::{self, Colors, Shape};
use anyhow::{Context, Result};
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    SpreadMode, Stroke, Transform,
};

use crate::repo;

/// The sizes Windows asks for, from the taskbar's 16 to Explorer's 256.
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// The images of a macOS icon: each one's type in the `.icns` file and its size in pixels
/// (the `@2x` ones are the types from `ic11` on), as `iconutil` writes them.
const MAC_ICONS: [(&[u8; 4], u32); 10] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
];
/// The share of a macOS icon its tile takes: 824 of 1024 pixels on Apple's grid.
const MAC_TILE: f32 = 824.0 / 1024.0;
/// The tile's corner radius, as a share of its side: Apple's 185 of 824.
const MAC_CORNER: f32 = 185.0 / 824.0;
/// The share of the tile the ring takes.
const MAC_RING: f32 = 0.8;
/// How many pixels the README's logo is drawn for: enough for every detail of the mark.
const LOGO_PIXELS: f32 = 256.0;

/// Where each file goes, from the repository root.
const ICO: &str = "crates/annulus-app/assets/annulus.ico";
const RES: &str = "crates/annulus-app/assets/annulus.res";
const ICNS: &str = "crates/annulus-app/assets/annulus.icns";
const LOGO: &str = "docs/media/logo.svg";

pub fn run() -> Result<bool> {
    let root = repo::root()?;
    let files = files()?;
    for (path, bytes) in &files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    println!(
        "wrote annulus.ico and annulus.res ({} sizes), annulus.icns ({} images) and logo.svg",
        SIZES.len(),
        MAC_ICONS.len()
    );
    Ok(true)
}

/// Every file this writes, by its path from the repository root, and what goes in it.
fn files() -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let images = SIZES
        .iter()
        .map(|&size| Ok((size, icon(size, false)?)))
        .collect::<Result<Vec<_>>>()?;
    let mac = MAC_ICONS
        .iter()
        .map(|&(kind, size)| Ok((*kind, icon(size, true)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![
        (ICO.into(), ico(&images)),
        (RES.into(), res(&images)),
        (ICNS.into(), icns(&mac)),
        (LOGO.into(), svg(&colors()).into_bytes()),
    ])
}

/// The theme the icon is drawn in: the dark one, whose light ring with its dark outline
/// reads on a light taskbar or Dock as well as on a dark one.
fn theme() -> Theme {
    Theme::new(Appearance::Dark)
}

fn colors() -> Colors {
    Colors::for_theme(&theme())
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// The icon `pixels` square, as a PNG: on macOS on its tile, elsewhere the ring alone.
fn icon(pixels: u32, macos: bool) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(pixels, pixels).context("icon size")?;
    #[allow(clippy::cast_precision_loss)] // 16 to 1024.
    let canvas = pixels as f32;
    let (mut at, mut side) = (0.0, canvas);
    if macos {
        let tile = canvas * MAC_TILE;
        let margin = (canvas - tile) / 2.0;
        let path = rounded_square(tile, tile * MAC_CORNER).context("tile path")?;
        // Lit from above: the raised surface's color at the top, the sheet's at the bottom,
        // so the tile stays in sight on a dark Dock.
        let stop = |at: f32, c: Color| {
            GradientStop::new(at, tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a))
        };
        let shade = LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, 0.0),
            tiny_skia::Point::from_xy(0.0, tile),
            vec![stop(0.0, theme().raised), stop(1.0, theme().bg)],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .context("tile shade")?;
        let mut fill = paint(theme().bg);
        fill.shader = shade;
        pixmap.fill_path(
            &path,
            &fill,
            FillRule::Winding,
            Transform::from_translate(margin, margin),
            None,
        );
        side = tile * MAC_RING;
        at = (canvas - side) / 2.0;
    }
    draw(&mut pixmap, at, side, &colors())?;
    pixmap.encode_png().context("encoding PNG")
}

/// Draws the mark `side` pixels square with its top-left corner at (`at`, `at`).
fn draw(pixmap: &mut Pixmap, at: f32, side: f32, colors: &Colors) -> Result<()> {
    let unit = side / mark::GRID;
    let place = |p: Point| (at + p.x * unit, at + p.y * unit);
    let stroke = |width: f32| Stroke {
        width: width * unit,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let line = |points: &[Point]| {
        let mut path = PathBuilder::new();
        for (index, &point) in points.iter().enumerate() {
            let (x, y) = place(point);
            if index == 0 {
                path.move_to(x, y);
            } else {
                path.line_to(x, y);
            }
        }
        path.finish()
    };
    let none = Transform::identity();
    for (shape, tone) in mark::shapes(side) {
        let paint = paint(colors.of(tone));
        match shape {
            Shape::Disc { center, radius } => {
                let (x, y) = place(center);
                let path = PathBuilder::from_circle(x, y, radius * unit).context("disc")?;
                pixmap.fill_path(&path, &paint, FillRule::Winding, none, None);
            }
            Shape::Ring {
                center,
                radius,
                width,
            } => {
                let (x, y) = place(center);
                let path = PathBuilder::from_circle(x, y, radius * unit).context("ring")?;
                pixmap.stroke_path(&path, &paint, &stroke(width), none, None);
            }
            Shape::Line { from, to, width } => {
                let path = line(&[from, to]).context("line")?;
                pixmap.stroke_path(&path, &paint, &stroke(width), none, None);
            }
            Shape::Vee {
                left,
                tip,
                right,
                width,
            } => {
                let path = line(&[left, tip, right]).context("chevron")?;
                pixmap.stroke_path(&path, &paint, &stroke(width), none, None);
            }
        }
    }
    Ok(())
}

/// A number for the SVG: three decimals at most, no trailing zeros.
fn num(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// `color` as an SVG paint, its alpha as an opacity attribute named `what`.
fn svg_paint(what: &str, color: Color) -> String {
    let mut out = format!("{what}=\"#{:02X}{:02X}{:02X}\"", color.r, color.g, color.b);
    if color.a < 255 {
        let _ = write!(
            out,
            " {what}-opacity=\"{}\"",
            num(f32::from(color.a) / 255.0)
        );
    }
    out
}

/// The mark as an SVG on its 20-unit grid.
fn svg(colors: &Colors) -> String {
    let mut out = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\" role=\"img\" aria-label=\"Annulus\">\n",
    );
    let points = |points: &[Point]| {
        points
            .iter()
            .map(|p| format!("{},{}", num(p.x), num(p.y)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let round = "fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\"";
    for (shape, tone) in mark::shapes(LOGO_PIXELS) {
        let color = colors.of(tone);
        let line = match shape {
            Shape::Disc { center, radius } => format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {}/>",
                num(center.x),
                num(center.y),
                num(radius),
                svg_paint("fill", color)
            ),
            Shape::Ring {
                center,
                radius,
                width,
            } => format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" {} stroke-width=\"{}\"/>",
                num(center.x),
                num(center.y),
                num(radius),
                svg_paint("stroke", color),
                num(width)
            ),
            Shape::Line { from, to, width } => format!(
                "<polyline points=\"{}\" {round} {} stroke-width=\"{}\"/>",
                points(&[from, to]),
                svg_paint("stroke", color),
                num(width)
            ),
            Shape::Vee {
                left,
                tip,
                right,
                width,
            } => format!(
                "<polyline points=\"{}\" {round} {} stroke-width=\"{}\"/>",
                points(&[left, tip, right]),
                svg_paint("stroke", color),
                num(width)
            ),
        };
        let _ = writeln!(out, "  {line}");
    }
    out.push_str("</svg>\n");
    out
}

/// A square of side `side` with corners of radius `r`.
fn rounded_square(side: f32, r: f32) -> Option<tiny_skia::Path> {
    // Distance of the cubic control points that best approximates a quarter circle.
    let k = r * 0.552_284_8;
    let s = side;
    let mut path = PathBuilder::new();
    path.move_to(r, 0.0);
    path.line_to(s - r, 0.0);
    path.cubic_to(s - r + k, 0.0, s, r - k, s, r);
    path.line_to(s, s - r);
    path.cubic_to(s, s - r + k, s - r + k, s, s - r, s);
    path.line_to(r, s);
    path.cubic_to(r - k, s, 0.0, s - r + k, 0.0, s - r);
    path.line_to(0.0, r);
    path.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    path.close();
    path.finish()
}

/// The size byte of an icon directory entry: 0 stands for 256.
fn dimension(size: u32) -> u8 {
    u8::try_from(size).unwrap_or(0)
}

fn len32(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap_or(u32::MAX)
}

/// An `.ico` file holding each image as a PNG (Windows Vista and later read these).
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(images.len()).unwrap_or(u16::MAX);
    let mut out = Vec::new();
    out.extend(0_u16.to_le_bytes());
    out.extend(1_u16.to_le_bytes()); // An icon, not a cursor.
    out.extend(count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for (size, png) in images {
        out.extend([dimension(*size), dimension(*size), 0, 0]);
        out.extend(1_u16.to_le_bytes()); // Planes.
        out.extend(32_u16.to_le_bytes()); // Bits per pixel.
        out.extend(len32(png).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += len32(png);
    }
    for (_, png) in images {
        out.extend(png);
    }
    out
}

/// A macOS `.icns` file: the magic and the whole length, then each image as its type, its
/// length (with this 8-byte header) and its PNG. Lengths are big-endian.
fn icns(images: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (kind, png) in images {
        body.extend(kind);
        body.extend((len32(png) + 8).to_be_bytes());
        body.extend(png);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend(b"icns");
    out.extend((len32(&body) + 8).to_be_bytes());
    out.extend(body);
    out
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// A compiled resource file (`.res`): each image as an `RT_ICON` numbered from 1, and icon
/// group 1 listing them, which is the icon Explorer shows and `LoadIconW(.., 1)` loads.
fn res(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    // A .res file starts with an empty entry that marks it as 32-bit.
    let mut out = entry(0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend(0_u16.to_le_bytes());
    group.extend(1_u16.to_le_bytes());
    group.extend(
        u16::try_from(images.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    for (id, (size, png)) in (1_u16..).zip(images) {
        out.extend(entry(RT_ICON, id, 0x1010, png));
        group.extend([dimension(*size), dimension(*size), 0, 0]);
        group.extend(1_u16.to_le_bytes());
        group.extend(32_u16.to_le_bytes());
        group.extend(len32(png).to_le_bytes());
        group.extend(id.to_le_bytes());
    }
    out.extend(entry(RT_GROUP_ICON, 1, 0x1030, &group));
    out
}

/// One resource entry with a numeric type and name, language neutral, padded to 4 bytes.
fn entry(kind: u16, name: u16, flags: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(len32(data).to_le_bytes());
    out.extend(32_u32.to_le_bytes()); // This header's size.
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(name.to_le_bytes());
    out.extend(0_u32.to_le_bytes()); // Data version.
    out.extend(flags.to_le_bytes());
    out.extend(0_u16.to_le_bytes()); // Language: neutral.
    out.extend(0_u32.to_le_bytes()); // Version.
    out.extend(0_u32.to_le_bytes()); // Characteristics.
    out.extend(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    /// The color at (`x`, `y`) of a PNG, alpha last.
    fn pixels(png: &[u8]) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
        let pixmap = Pixmap::decode_png(png).unwrap();
        move |x, y| {
            let p = pixmap.pixel(x, y).unwrap().demultiply();
            [p.red(), p.green(), p.blue(), p.alpha()]
        }
    }

    fn opaque(color: Color) -> [u8; 4] {
        [color.r, color.g, color.b, 255]
    }

    fn near(got: [u8; 4], want: [u8; 4]) -> bool {
        got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2)
    }

    #[test]
    fn the_icon_is_the_ring_alone_with_its_pool_rim_and_top_chevron() {
        // 256 pixels: 12.8 to a unit of the mark's grid, its middle at 128.
        let at = pixels(&icon(256, false).unwrap());
        let colors = colors();
        for (x, y) in [(0, 0), (20, 20), (255, 255), (236, 20)] {
            assert_eq!(at(x, y)[3], 0, "clear outside the ring at ({x}, {y})");
        }
        // The middle of the pool, under every layer of its glow.
        let glows = mark::shapes(256.0)
            .iter()
            .filter(|(_, tone)| *tone == mark::Tone::Glow)
            .count();
        let pool = (0..glows).fold(colors.horizon, |under, _| colors.glow.over(under));
        assert!(near(at(128, 128), opaque(pool)), "pool {:?}", at(128, 128));
        // The rim, 8.85 units out halfway between two chevrons, outside the track and inside
        // the outline.
        assert_eq!(at(239, 147), opaque(colors.rim), "rim");
        // The top chevron, just inside its tip 3 units from the top.
        assert_eq!(at(128, 42), opaque(colors.chevron), "chevron");
    }

    #[test]
    fn on_macos_the_tile_sits_centered_with_a_clear_margin_around_it() {
        let at = pixels(&icon(1024, true).unwrap());
        // 100 pixels of margin on each side of an 824-pixel tile.
        for (x, y) in [(512, 99), (99, 512), (512, 925), (925, 512)] {
            assert_eq!(at(x, y)[3], 0, "margin at ({x}, {y})");
        }
        for (x, y) in [(512, 101), (101, 512), (512, 922), (922, 512)] {
            assert_eq!(at(x, y)[3], 255, "tile at ({x}, {y})");
        }
    }

    #[test]
    fn the_committed_icons_and_logo_are_the_mark_as_drawn_now() {
        let root = repo::root().unwrap();
        for (path, bytes) in files().unwrap() {
            let committed = std::fs::read(root.join(&path)).unwrap();
            assert!(
                committed == bytes,
                "{} is not the mark as it is drawn now: run `cargo xtask icon`",
                path.display()
            );
        }
    }

    #[test]
    fn the_logo_draws_every_shape_of_the_mark_on_its_grid() {
        let logo = svg(&colors());
        assert!(logo.starts_with("<svg ") && logo.contains("viewBox=\"0 0 20 20\""));
        let drawn = logo.matches("<circle").count() + logo.matches("<polyline").count();
        assert_eq!(drawn, mark::shapes(LOGO_PIXELS).len());
        let glow = colors().glow;
        assert!(logo.contains(&format!(
            "fill-opacity=\"{}\"",
            num(f32::from(glow.a) / 255.0)
        )));
    }

    #[test]
    fn the_macos_icon_file_holds_each_image_under_its_type() {
        let images = vec![(*b"icp4", vec![1, 2, 3]), (*b"ic10", vec![4, 5])];
        let file = icns(&images);
        let be =
            |at: usize| u32::from_be_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]]);
        assert_eq!(&file[..4], b"icns");
        assert_eq!(be(4) as usize, file.len());
        assert_eq!((&file[8..12], be(12)), (&b"icp4"[..], 11));
        assert_eq!(&file[16..19], &[1, 2, 3]);
        assert_eq!((&file[19..23], be(23)), (&b"ic10"[..], 10));
        assert_eq!(&file[27..], &[4, 5]);
    }

    #[test]
    fn the_icon_file_lists_each_size_and_points_at_its_png() {
        let images = vec![(16, vec![1, 2, 3]), (256, vec![4, 5])];
        let file = ico(&images);
        assert_eq!((u16_at(&file, 2), u16_at(&file, 4)), (1, 2));
        assert_eq!((file[6], file[22]), (16, 0), "256 is written as 0");
        let second = u32_at(&file, 22 + 12) as usize;
        assert_eq!(&file[second..], &[4, 5]);
        assert_eq!(u32_at(&file, 6 + 8), 3, "first image's length");
    }

    #[test]
    fn the_resource_holds_numbered_icons_then_group_one() {
        let images = vec![(16, vec![9; 5]), (32, vec![7; 8])];
        let file = res(&images);
        // The leading empty entry, then each entry padded to 4 bytes.
        assert_eq!((u32_at(&file, 0), u32_at(&file, 4)), (0, 32));
        let mut at = 32;
        let mut seen = Vec::new();
        let mut group = 0;
        while at < file.len() {
            let size = u32_at(&file, at) as usize;
            let kind = u16_at(&file, at + 10);
            let name = u16_at(&file, at + 14);
            seen.push((kind, name, size));
            group = at + 32;
            at += 32 + size.next_multiple_of(4);
        }
        assert_eq!(at, file.len());
        assert_eq!(
            seen,
            vec![
                (RT_ICON, 1, 5),
                (RT_ICON, 2, 8),
                (RT_GROUP_ICON, 1, 6 + 2 * 14)
            ]
        );
        // The group's second entry is 32 pixels and names icon 2.
        assert_eq!(file[group + 6 + 14], 32);
        assert_eq!(u16_at(&file, group + 6 + 2 * 14 - 2), 2);
    }
}
