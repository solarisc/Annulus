//! `cargo xtask themes <schemes checkout>`: imports the tinted-theming base16/base24 color
//! schemes (MIT) as Annulus's scheme catalog.
//!
//! ```text
//! git clone https://github.com/tinted-theming/schemes <dir>
//! cargo xtask themes <dir>
//! ```
//!
//! The checkout is untrusted data: only its YAML files are read, with a parser for the flat
//! format the schemes use. Every color must be `#rrggbb`, every file name a slug, and names
//! are cleaned and length-limited before they are written as Rust string literals. Both
//! outputs are committed, so builds never need the network:
//!
//! - `crates/annulus-config/src/schemes/catalog.rs`, the compile-time catalog;
//! - `crates/annulus-config/src/schemes/LICENSE.txt`, the MIT license and every scheme's author.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::repo;

/// base24 first: it wins when a scheme exists in both.
const SYSTEMS: [&str; 2] = ["base24", "base16"];
const MAX_NAME: usize = 80;
const MAX_AUTHOR: usize = 200;

struct Imported {
    slug: String,
    name: String,
    author: String,
    base: [[u8; 3]; 24],
}

pub fn run(source: Option<&str>) -> Result<bool> {
    let Some(source) = source else {
        bail!("usage: cargo xtask themes <path to a tinted-theming/schemes checkout>");
    };
    let source = Path::new(source);
    let commit = git_commit(source)?;
    let mut dropped = Vec::new();
    let mut schemes: Vec<Imported> = Vec::new();
    for system in SYSTEMS {
        let mut files: Vec<String> = std::fs::read_dir(source.join(system))
            .with_context(|| format!("reading {system}/"))?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|file| {
                Path::new(file)
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .collect();
        files.sort();
        for file in files {
            let label = format!("{system}/{file}");
            match import(source, system, &file) {
                Ok(scheme) => {
                    let key = name_key(&scheme.name);
                    if let Some(first) = schemes.iter().find(|s| name_key(&s.name) == key) {
                        dropped.push(format!("{label}: same scheme as {}", first.slug));
                    } else {
                        schemes.push(scheme);
                    }
                }
                Err(why) => dropped.push(format!("{label}: {why}")),
            }
        }
    }
    schemes.sort_by_key(|s| s.name.to_lowercase());

    let root = repo::root()?;
    let out = root.join("crates/annulus-config/src/schemes");
    std::fs::write(out.join("catalog.rs"), catalog(&schemes, &commit))?;
    let license = license(source, &schemes, &commit)?;
    std::fs::write(out.join("LICENSE.txt"), &license)?;
    let rbit_license = std::fs::read_to_string(out.join("RBIT-LICENSE.txt"))?;
    std::fs::write(
        root.join("LICENSE-THEMES"),
        format!("{license}\n\nBuilt-in rbit palettes:\n\n{rbit_license}"),
    )?;
    println!(
        "{} schemes from tinted-theming/schemes@{commit}",
        schemes.len()
    );
    for line in &dropped {
        println!("dropped {line}");
    }
    Ok(true)
}

fn git_commit(dir: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("running git")?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{} is not a git checkout", dir.display());
    }
    Ok(sha)
}

fn import(source: &Path, system: &str, file: &str) -> Result<Imported, String> {
    let id = file
        .trim_end_matches(".yaml")
        .trim_end_matches(".yml")
        .to_owned();
    if !is_slug(&id) {
        return Err("file name is not a valid slug".into());
    }
    let text = std::fs::read_to_string(source.join(system).join(file))
        .map_err(|error| error.to_string())?;
    let (fields, palette) = parse(&text);
    let field = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map_or("", |(_, v)| v.as_str())
    };
    if field("system") != system {
        return Err(format!("declares system {:?}", field("system")));
    }
    let count = if system == "base24" { 24 } else { 16 };
    let mut base = [[0; 3]; 24];
    for (index, slot) in base.iter_mut().enumerate().take(count) {
        let key = format!("base{index:02X}");
        let value = palette
            .iter()
            .find(|(k, _)| *k == key)
            .map_or("", |(_, v)| v.as_str());
        *slot = color(value).ok_or_else(|| format!("{key} is not #rrggbb"))?;
    }
    if count == 16 {
        // The base24 fallback for base16 schemes: darker backgrounds are the background,
        // bright colors are the normal ones.
        for (slot, from) in [0x00, 0x00, 0x08, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E]
            .into_iter()
            .enumerate()
        {
            base[0x10 + slot] = base[from];
        }
    }
    let name = clean(field("name"), MAX_NAME);
    if name.is_empty() {
        return Err("no name".into());
    }
    if contrast(base[0], base[5]) < 2.0 {
        return Err("foreground and background are indistinguishable".into());
    }
    let author = match clean(field("author"), MAX_AUTHOR) {
        author if author.is_empty() => "Unknown".to_owned(),
        author => author,
    };
    Ok(Imported {
        slug: format!("{system}-{id}"),
        name,
        author,
        base,
    })
}

type Pairs = Vec<(String, String)>;

/// The flat YAML the schemes use: top-level `key: value` pairs and one nested `palette:`
/// map. Comments, document markers and block-scalar continuation lines are skipped.
fn parse(text: &str) -> (Pairs, Pairs) {
    let (mut fields, mut palette) = (Vec::new(), Vec::new());
    let mut in_palette = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || raw.starts_with("---") {
            continue;
        }
        let indented = raw.starts_with([' ', '\t']);
        if !indented {
            in_palette = false;
        }
        let Some((key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            continue;
        }
        if !indented {
            if key == "palette" {
                in_palette = true;
            } else {
                fields.push((key.to_owned(), scalar(rest)));
            }
        } else if in_palette {
            palette.push((key.to_owned(), scalar(rest)));
        }
    }
    (fields, palette)
}

/// A single-, double- or unquoted value, without a trailing comment.
fn scalar(rest: &str) -> String {
    let value = rest.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote) {
            let mut out = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' if quote == '"' => out.extend(chars.next()),
                    c if c == quote => {
                        if quote == '\'' && chars.as_str().starts_with('\'') {
                            chars.next();
                            out.push('\'');
                        } else {
                            return out;
                        }
                    }
                    c => out.push(c),
                }
            }
            return out;
        }
    }
    value
        .split(" #")
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn is_slug(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Printable text only, whitespace collapsed, bounded length.
fn clean(value: &str, max: usize) -> String {
    let printable: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = printable.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        let mut cut: String = collapsed.chars().take(max - 1).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

/// `author` as the app shows it: email addresses stay in `LICENSE.txt`, out of the app.
fn without_emails(author: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    for word in author.split(' ') {
        let bare = word.trim_matches(|c| matches!(c, '(' | ')' | '<' | '>' | ','));
        let is_email = bare.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
        });
        if !is_email {
            kept.push(word.to_owned());
        } else if word.ends_with(',')
            && let Some(last) = kept.last_mut()
        {
            last.push(',');
        }
    }
    kept.join(" ")
}

fn name_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let linear = |v: u8| {
        let c = f64::from(v) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn catalog(schemes: &[Imported], commit: &str) -> String {
    let mut out = format!(
        "//! The scheme catalog: {} schemes from\n\
         //! [tinted-theming/schemes](https://github.com/tinted-theming/schemes) (MIT) at commit\n\
         //! `{commit}`. Authors and the license: `LICENSE.txt` beside this file.\n\
         //!\n\
         //! Generated by `cargo xtask themes`; do not edit. Sorted by name, in menu order.\n\
         \n\
         use super::Scheme;\n\
         \n\
         /// Every scheme.\n\
         #[rustfmt::skip]\n\
         pub static SCHEMES: &[Scheme] = &[\n",
        schemes.len()
    );
    for scheme in schemes {
        let base = scheme
            .base
            .iter()
            .map(|[r, g, b]| format!("[0x{r:02X}, 0x{g:02X}, 0x{b:02X}]"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "    Scheme {{ name: {:?}, author: {:?}, base: [{base}] }},",
            scheme.name,
            without_emails(&scheme.author)
        );
    }
    out.push_str("];\n");
    out
}

fn license(source: &Path, schemes: &[Imported], commit: &str) -> Result<String> {
    let text = std::fs::read_to_string(source.join("LICENSE")).context("reading LICENSE")?;
    let mut out = format!(
        "Annulus's color schemes are generated from the tinted-theming color schemes:\n\
         https://github.com/tinted-theming/schemes (commit {commit}).\n\n{}\n\n\
         Schemes and their authors:\n\n",
        text.replace("\r\n", "\n").trim()
    );
    for scheme in schemes {
        let _ = writeln!(
            out,
            "  {}: {} by {}",
            scheme.slug, scheme.name, scheme.author
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_lose_their_quotes_escapes_and_trailing_comments() {
        assert_eq!(scalar(r#" "Say \"hi\"" # note"#), r#"Say "hi""#);
        assert_eq!(scalar(" 'it''s' "), "it's");
        assert_eq!(scalar(" #ff5555  # Red"), "#ff5555");
        assert_eq!(scalar(" plain words "), "plain words");
    }

    #[test]
    fn only_the_palette_map_is_read_as_colors() {
        let (fields, palette) = parse(
            "---\n\
             system: \"base16\"\n\
             name: \"Test\"\n\
             # a comment\n\
             description: |\n  \
               base00: not a color\n\
             palette:\n  \
               base00: \"#000000\" # Background\n  \
               base05: '#FFFFFF'\n\
             variant: dark\n  \
               base07: \"#123456\"\n",
        );
        let get =
            |pairs: &Pairs, key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(get(&fields, "name").as_deref(), Some("Test"));
        assert_eq!(get(&fields, "variant").as_deref(), Some("dark"));
        assert_eq!(get(&palette, "base00").as_deref(), Some("#000000"));
        assert_eq!(get(&palette, "base05").as_deref(), Some("#FFFFFF"));
        assert_eq!(palette.len(), 2, "nothing outside palette: counts");
    }

    #[test]
    fn untrusted_names_colors_and_file_names_are_checked() {
        assert!(is_slug("gruvbox-dark-hard"));
        for bad in ["", "Gruvbox", "a--b", "-a", "a-", "a_b", "../x"] {
            assert!(!is_slug(bad), "{bad}");
        }
        assert_eq!(color("#FF5500"), Some([0xFF, 0x55, 0x00]));
        for bad in ["FF5500", "#FF550", "#FF55001", "#GG5500", ""] {
            assert_eq!(color(bad), None, "{bad}");
        }
        assert_eq!(clean("a\u{7}\n  b\tc", 80), "a b c");
        assert_eq!(clean("abcdef", 4), "abc…");
        assert_eq!(
            name_key("Gruvbox dark, hard"),
            name_key("gruvbox-dark-hard")
        );
    }

    #[test]
    fn authors_in_the_catalog_lose_their_email_addresses_but_not_their_handles() {
        assert_eq!(
            without_emails(
                "Dawid Kurek (dawikur@example.com), morhetz (https://github.com/morhetz)"
            ),
            "Dawid Kurek, morhetz (https://github.com/morhetz)"
        );
        assert_eq!(
            without_emails("Wendell, Ryan <ryan@example.com>"),
            "Wendell, Ryan"
        );
        assert_eq!(
            without_emails("Thomas Leon thomas@example.org"),
            "Thomas Leon"
        );
        assert_eq!(
            without_emails("Josh W Lewis (@joshwlewis)"),
            "Josh W Lewis (@joshwlewis)"
        );
        assert_eq!(
            without_emails("@Schnouki, based on X"),
            "@Schnouki, based on X"
        );
    }
}
