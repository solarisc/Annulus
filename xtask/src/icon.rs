//! `cargo xtask icon`: draws our mark (the teal tile with a prompt the title bar shows,
//! `titlebar::paint_mark`, ADR-0010) at every Windows icon size and writes:
//!
//! - `crates/annulus-app/assets/annulus.ico`, the icon file;
//! - `crates/annulus-app/assets/annulus.res`, the same images as a compiled Windows resource
//!   (icon group 1), which `build.rs` hands to the MSVC linker so the exe carries it. Writing
//!   the `.res` here keeps the build free of a resource compiler.
//!
//! Both are committed; rerun after changing the mark.

use anyhow::{Context, Result};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::repo;

/// The sizes Windows asks for, from the taskbar's 16 to Explorer's 256.
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// The tile: our dark theme's accent.
const TILE: [u8; 3] = [0x2B, 0xB3, 0xA3];
/// The prompt: our dark theme's window color, as the title bar draws it on the accent.
const INK: [u8; 3] = [0x12, 0x14, 0x16];

pub fn run() -> Result<bool> {
    let images = SIZES
        .iter()
        .map(|&size| Ok((size, mark(size)?)))
        .collect::<Result<Vec<_>>>()?;
    let assets = repo::root()?.join("crates/annulus-app/assets");
    std::fs::create_dir_all(&assets)?;
    std::fs::write(assets.join("annulus.ico"), ico(&images))?;
    std::fs::write(assets.join("annulus.res"), res(&images))?;
    println!("wrote annulus.ico and annulus.res ({} sizes)", images.len());
    Ok(true)
}

/// The mark at `size` pixels square, as a PNG. Its geometry is the title bar's, on a 20-unit
/// tile.
fn mark(size: u32) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(size, size).context("icon size")?;
    #[allow(clippy::cast_precision_loss)] // 16 to 256.
    let scale = size as f32 / 20.0;
    let paint = |[r, g, b]: [u8; 3]| {
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 255);
        paint.anti_alias = true;
        paint
    };
    let tile = rounded_square(20.0 * scale, 6.0 * scale).context("tile path")?;
    pixmap.fill_path(
        &tile,
        &paint(TILE),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let stroke = Stroke {
        width: 1.8 * scale,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let strokes: [&[(f32, f32)]; 2] = [
        &[(5.5, 6.5), (9.0, 10.0), (5.5, 13.5)],
        &[(10.5, 14.0), (14.5, 14.0)],
    ];
    for points in strokes {
        let mut path = PathBuilder::new();
        for (index, (x, y)) in points.iter().enumerate() {
            if index == 0 {
                path.move_to(x * scale, y * scale);
            } else {
                path.line_to(x * scale, y * scale);
            }
        }
        let path = path.finish().context("prompt path")?;
        pixmap.stroke_path(&path, &paint(INK), &stroke, Transform::identity(), None);
    }
    pixmap.encode_png().context("encoding PNG")
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

    #[test]
    fn the_mark_is_a_teal_tile_with_a_dark_prompt_and_clear_corners() {
        let png = mark(64).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        let at = |x: u32, y: u32| {
            let p = pixmap.pixel(x, y).unwrap();
            [p.red(), p.green(), p.blue(), p.alpha()]
        };
        assert_eq!(at(0, 0)[3], 0, "the corner is rounded off");
        assert_eq!(at(56, 32), [TILE[0], TILE[1], TILE[2], 255], "tile");
        // The underline of the prompt runs at y = 14 units, x 10.5 to 14.5.
        assert_eq!(at(40, 45), [INK[0], INK[1], INK[2], 255], "prompt");
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
