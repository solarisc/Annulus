//! `cargo xtask icon`: draws our mark (`annulus_ui::mark`, which the title bar shows: the
//! owner's design, on its own tile in its own colors) at every Windows and macOS icon size and
//! writes:
//!
//! - `crates/annulus-app/assets/annulus.ico`, the icon file: the tile edge to edge;
//! - `crates/annulus-app/assets/annulus.res`, the same images as a compiled Windows resource
//!   (icon group 1), which `build.rs` hands to the MSVC linker so the exe carries it. Writing
//!   the `.res` here keeps the build free of a resource compiler;
//! - `crates/annulus-app/assets/annulus.icns`, the macOS icon `cargo xtask mac-app` puts in
//!   `Annulus.app`: the tile on macOS's icon grid, a margin around it as around every other
//!   app's in the Dock;
//! - `docs/media/logo.svg`, the README's logo: the mark as vectors.
//!
//! All four are committed, and a test fails while one is not what the mark draws now: rerun
//! this after changing the mark.

use std::fmt::Write as _;
use std::path::PathBuf;

use annulus_ui::gfx::Color;
use annulus_ui::mark::{self, Shape};
use anyhow::{Context, Result};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

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
        .map(|&size| Ok((size, icon(size, 1.0)?)))
        .collect::<Result<Vec<_>>>()?;
    let mac = MAC_ICONS
        .iter()
        .map(|&(kind, size)| Ok((*kind, icon(size, MAC_TILE)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![
        (ICO.into(), ico(&images)),
        (RES.into(), res(&images)),
        (ICNS.into(), icns(&mac)),
        (LOGO.into(), svg().into_bytes()),
    ])
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    paint
}

/// The icon `pixels` square, as a PNG: the mark `share` of that wide, in the middle.
fn icon(pixels: u32, share: f32) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(pixels, pixels).context("icon size")?;
    #[allow(clippy::cast_precision_loss)] // 16 to 1024.
    let canvas = pixels as f32;
    let side = canvas * share;
    draw(&mut pixmap, (canvas - side) / 2.0, side)?;
    pixmap.encode_png().context("encoding PNG")
}

/// Draws the mark `side` pixels square with its top-left corner at (`at`, `at`).
fn draw(pixmap: &mut Pixmap, at: f32, side: f32) -> Result<()> {
    let unit = side / mark::GRID;
    let place = Transform::from_translate(at, at);
    for (shape, tone) in mark::shapes() {
        let paint = paint(mark::color(tone));
        match shape {
            Shape::Tile { radius } => {
                let path = rounded_square(side, radius * unit).context("tile")?;
                pixmap.fill_path(&path, &paint, FillRule::Winding, place, None);
            }
            Shape::Outline { radius, width } => {
                let half = width * unit / 2.0;
                let path =
                    rounded_square(side - 2.0 * half, radius * unit - half).context("outline")?;
                let stroke = Stroke {
                    width: width * unit,
                    ..Stroke::default()
                };
                let inset = Transform::from_translate(at + half, at + half);
                pixmap.stroke_path(&path, &paint, &stroke, inset, None);
            }
            Shape::Polygon(points) => {
                let mut path = PathBuilder::new();
                for (index, p) in points.iter().enumerate() {
                    if index == 0 {
                        path.move_to(p.x * unit, p.y * unit);
                    } else {
                        path.line_to(p.x * unit, p.y * unit);
                    }
                }
                path.close();
                let path = path.finish().context("polygon")?;
                pixmap.fill_path(&path, &paint, FillRule::Winding, place, None);
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

/// `color` as an SVG color.
fn hex(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

/// The mark as an SVG on its 20-unit grid.
fn svg() -> String {
    let mut out = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\" role=\"img\" aria-label=\"Annulus\">\n",
    );
    for (shape, tone) in mark::shapes() {
        let color = hex(mark::color(tone));
        let line = match shape {
            Shape::Tile { radius } => format!(
                "<rect width=\"20\" height=\"20\" rx=\"{}\" fill=\"{color}\"/>",
                num(radius)
            ),
            Shape::Outline { radius, width } => {
                let half = width / 2.0;
                format!(
                    "<rect x=\"{0}\" y=\"{0}\" width=\"{1}\" height=\"{1}\" rx=\"{2}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{3}\"/>",
                    num(half),
                    num(mark::GRID - width),
                    num(radius - half),
                    num(width)
                )
            }
            Shape::Polygon(points) => {
                let points: Vec<String> = points
                    .iter()
                    .map(|p| format!("{},{}", num(p.x), num(p.y)))
                    .collect();
                format!(
                    "<polygon points=\"{}\" fill=\"{color}\"/>",
                    points.join(" ")
                )
            }
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

    fn opaque(tone: mark::Tone) -> [u8; 4] {
        let color = mark::color(tone);
        [color.r, color.g, color.b, 255]
    }

    #[test]
    fn the_icon_is_the_tile_with_its_ring_chevron_letter_and_spark() {
        // 256 pixels: 12.8 to a unit of the mark's grid.
        let at = pixels(&icon(256, 1.0).unwrap());
        assert_eq!(at(0, 0)[3], 0, "the corner is rounded off");
        assert_eq!(at(20, 128), opaque(mark::Tone::Tile), "tile");
        assert_eq!(
            at(191, 93),
            opaque(mark::Tone::Ring),
            "a segment of the ring"
        );
        assert_eq!(at(128, 55), opaque(mark::Tone::Light), "the top chevron");
        assert_eq!(
            at(87, 179),
            opaque(mark::Tone::Letter),
            "a leg of the letter"
        );
        assert_eq!(at(128, 161), opaque(mark::Tone::Light), "the spark");
        assert_eq!(at(128, 200), opaque(mark::Tone::Tile), "between the legs");
    }

    #[test]
    fn on_macos_the_tile_sits_centered_with_a_clear_margin_around_it() {
        let at = pixels(&icon(1024, MAC_TILE).unwrap());
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
        let logo = svg();
        assert!(logo.starts_with("<svg ") && logo.contains("viewBox=\"0 0 20 20\""));
        let drawn = logo.matches("<rect").count() + logo.matches("<polygon").count();
        assert_eq!(drawn, mark::shapes().len());
        assert!(logo.contains(&hex(mark::color(mark::Tone::Light))));
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
