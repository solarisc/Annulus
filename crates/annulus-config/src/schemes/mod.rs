//! Community color schemes (F-SET-002, F-TERM-013, ADR-0019): the tinted-theming base16 and
//! base24 schemes, which can recolor the whole window and, chosen separately, the terminal.
//!
//! A scheme is only its 24 base colors. The design tokens and the terminal colors are derived
//! from them here, following the base16 and base24 styling guides, and any color that would
//! fall short of the contrast our own themes meet (WCAG AA, tested over every scheme) is
//! nudged towards white on a dark scheme or black on a light one, just far enough.

mod builtins;
mod catalog;

use std::sync::OnceLock;

pub use builtins::BUILT_INS;
pub use catalog::SCHEMES;

use crate::tokens::{Palette, Rgb};

/// One color scheme.
#[derive(Debug)]
pub struct Scheme {
    /// The name menus show.
    pub name: &'static str,
    /// Who made it, as the scheme states.
    pub author: &'static str,
    /// `base00`–`base17` as `[r, g, b]`. base16 schemes are filled to 24 with the base24
    /// fallback: the darker backgrounds are `base00`, the bright colors the normal ones.
    pub base: [[u8; 3]; 24],
}

/// The colors a terminal draws with, as a scheme gives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColors {
    /// ANSI colors 0–15.
    pub ansi: [Rgb; 16],
    /// Default text.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// The cursor block.
    pub cursor: Rgb,
    /// Behind selected text.
    pub selection: Rgb,
}

const WHITE: Rgb = Rgb::new(0xFF, 0xFF, 0xFF);
const BLACK: Rgb = Rgb::new(0x00, 0x00, 0x00);

/// Text targets a little above WCAG AA (4.5:1): the toolkit draws a few surfaces as blends
/// of these backgrounds.
const TEXT: f64 = 4.6;
/// WCAG AA for graphics: the focus ring, status dots.
const GRAPHIC: f64 = 3.1;

impl Scheme {
    /// The scheme called `name`.
    #[must_use]
    pub fn find(name: &str) -> Option<&'static Scheme> {
        SCHEMES
            .iter()
            .chain(BUILT_INS)
            .find(|scheme| scheme.name == name)
    }

    fn color(&self, index: usize) -> Rgb {
        let [r, g, b] = self.base[index];
        Rgb::new(r, g, b)
    }

    /// Light text on a dark background.
    #[must_use]
    pub fn dark(&self) -> bool {
        self.color(0x00).luminance() < self.color(0x05).luminance()
    }

    /// The design tokens for the window in this scheme, worked out once.
    #[must_use]
    pub fn palette(&'static self) -> Palette {
        static CACHE: OnceLock<Vec<OnceLock<Palette>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| {
            SCHEMES
                .iter()
                .chain(BUILT_INS)
                .map(|_| OnceLock::new())
                .collect()
        });
        match SCHEMES
            .iter()
            .chain(BUILT_INS)
            .position(|scheme| std::ptr::eq(scheme, self))
        {
            Some(index) => *cache[index].get_or_init(|| self.derive_palette()),
            None => self.derive_palette(),
        }
    }

    fn derive_palette(&self) -> Palette {
        let dark = self.dark();
        let extreme = if dark { WHITE } else { BLACK };
        let fg = self.color(0x05);
        let mut surface = self.color(0x00);
        // The window background sits one step deeper than the panels.
        let app = if dark {
            if let Some(deeper) = step_away(surface, BLACK, 1.08) {
                deeper
            } else {
                // base00 is (nearly) black already: lift the panels instead.
                let app = surface;
                surface = step_away(app, fg, 1.08).unwrap_or_else(|| mix(app, fg, 0.08));
                app
            }
        } else {
            step_away(surface, mix(fg, BLACK, 0.5), 1.07)
                .unwrap_or_else(|| mix(surface, BLACK, 0.04))
        };
        let step = |target: f64, fallback: f64| {
            step_away(surface, fg, target).unwrap_or_else(|| mix(surface, fg, fallback))
        };
        // Dark raised panels are lighter; light ones stay the scheme's own background.
        let raised = if dark { step(1.11, 0.06) } else { surface };
        let hover = step(if dark { 1.27 } else { 1.13 }, 0.1);
        let border_subtle = step(if dark { 1.28 } else { 1.26 }, 0.2);
        let raw_accent = self.color(0x0D);
        let selected = mix(surface, raw_accent, if dark { 0.22 } else { 0.16 });

        let all = [app, surface, raised, hover, selected];
        let panels = [app, surface, raised];
        let strong = |color: Rgb| {
            ensure(
                ensure(color, &panels, TEXT, extreme),
                &all,
                GRAPHIC,
                extreme,
            )
        };
        Palette {
            bg_app: app,
            bg_surface: surface,
            bg_raised: raised,
            bg_hover: hover,
            bg_selected: selected,
            border_subtle,
            text_primary: ensure(fg, &all, TEXT, extreme),
            text_secondary: ensure(self.color(0x04), &all, TEXT, extreme),
            accent: strong(raw_accent),
            status_warn: ensure(self.color(0x0A), &all, GRAPHIC, extreme),
            status_error: strong(self.color(0x08)),
            status_ok: ensure(self.color(0x0B), &all, GRAPHIC, extreme),
        }
    }

    /// The terminal colors in this scheme: the ANSI table of the base24 styling guide.
    #[must_use]
    pub fn terminal(&self) -> TerminalColors {
        let order = [
            0x00, 0x08, 0x0B, 0x0A, 0x0D, 0x0E, 0x0C, 0x05, 0x03, 0x12, 0x14, 0x13, 0x16, 0x17,
            0x15, 0x07,
        ];
        let background = self.color(0x00);
        let extreme = if self.dark() { WHITE } else { BLACK };
        let foreground = ensure(self.color(0x05), &[background], TEXT, extreme);
        TerminalColors {
            ansi: order.map(|index| self.color(index)),
            foreground,
            background,
            cursor: foreground,
            selection: self.color(0x02),
        }
    }
}

/// sRGB to the Oklab color space, where mixing looks even.
fn oklab(color: Rgb) -> [f64; 3] {
    let linear = |value: u8| {
        let c = f64::from(value) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (red, green, blue) = (linear(color.r), linear(color.g), linear(color.b));
    let long = (0.412_221_470_8 * red + 0.536_332_536_3 * green + 0.051_445_992_9 * blue).cbrt();
    let medium = (0.211_903_498_2 * red + 0.680_699_545_1 * green + 0.107_396_956_6 * blue).cbrt();
    let short = (0.088_302_461_9 * red + 0.281_718_837_6 * green + 0.629_978_700_5 * blue).cbrt();
    [
        0.210_454_255_3 * long + 0.793_617_785 * medium - 0.004_072_046_8 * short,
        1.977_998_495_1 * long - 2.428_592_205 * medium + 0.450_593_709_9 * short,
        0.025_904_037_1 * long + 0.782_771_766_2 * medium - 0.808_675_766 * short,
    ]
}

/// Oklab back to sRGB, clamped to the gamut.
fn from_oklab([lightness, green_red, blue_yellow]: [f64; 3]) -> Rgb {
    let cone = |a: f64, b: f64| (lightness + a * green_red + b * blue_yellow).powi(3);
    let long = cone(0.396_337_777_4, 0.215_803_757_3);
    let medium = cone(-0.105_561_345_8, -0.063_854_172_8);
    let short = cone(-0.089_484_177_5, -1.291_485_548);
    let channel = |linear: f64| {
        let linear = linear.clamp(0.0, 1.0);
        let gamma = if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 to 255.
        let byte = (gamma.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    };
    Rgb::new(
        channel(4.076_741_662_1 * long - 3.307_711_591_3 * medium + 0.230_969_929_2 * short),
        channel(-1.268_438_004_6 * long + 2.609_757_401_1 * medium - 0.341_319_396_5 * short),
        channel(-0.004_196_086_3 * long - 0.703_418_614_7 * medium + 1.707_614_701 * short),
    )
}

/// `amount` of the way from `a` to `b`, in Oklab.
fn mix(a: Rgb, b: Rgb, amount: f64) -> Rgb {
    let (x, y) = (oklab(a), oklab(b));
    from_oklab([0, 1, 2].map(|i| x[i] + (y[i] - x[i]) * amount))
}

/// The smallest step from `from` towards `toward` that reaches `target` contrast with `from`.
fn step_away(from: Rgb, toward: Rgb, target: f64) -> Option<Rgb> {
    if from.contrast(toward) < target {
        return None;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..24 {
        let mid = f64::midpoint(lo, hi);
        if from.contrast(mix(from, toward, mid)) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(mix(from, toward, hi))
}

/// `color`, nudged towards `extreme` until it reaches `target` contrast on every background.
fn ensure(color: Rgb, backgrounds: &[Rgb], target: f64, extreme: Rgb) -> Rgb {
    let passes = |c: Rgb| backgrounds.iter().all(|bg| c.contrast(*bg) >= target);
    (0..=50)
        .map(|step| mix(color, extreme, f64::from(step) / 50.0))
        .find(|candidate| passes(*candidate))
        .unwrap_or(extreme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_holds_every_scheme_once_in_menu_order() {
        assert!(SCHEMES.len() > 450, "{} schemes", SCHEMES.len());
        for pair in SCHEMES.windows(2) {
            assert!(
                pair[0].name.to_lowercase() <= pair[1].name.to_lowercase(),
                "{} before {}",
                pair[0].name,
                pair[1].name
            );
            assert_ne!(pair[0].name, pair[1].name);
        }
        assert_eq!(
            Scheme::find("Dracula").map(|s| s.author),
            Some("clach04 (https://github.com/clach04)")
        );
        assert!(Scheme::find("No Such Scheme").is_none());
    }

    #[test]
    fn the_six_built_in_palettes_resolve_and_have_readable_palettes() {
        assert_eq!(BUILT_INS.len(), 6);
        for scheme in BUILT_INS {
            assert!(std::ptr::eq(Scheme::find(scheme.name).unwrap(), scheme));
            let palette = scheme.palette();
            for background in palette.backgrounds() {
                assert!(
                    palette.text_primary.contrast(background) >= 4.5,
                    "{}",
                    scheme.name
                );
                assert!(
                    palette.text_secondary.contrast(background) >= 4.5,
                    "{}",
                    scheme.name
                );
            }
        }
        assert!(Scheme::find("rbit Dark Colorblind").unwrap().dark());
        assert!(!Scheme::find("rbit Light Colorblind").unwrap().dark());
    }

    #[test]
    fn base24_dracula_maps_to_the_dracula_ansi_colors() {
        let dracula = Scheme::find("Dracula").unwrap().terminal();
        let hex = |rgb: u32| {
            let [_, r, g, b] = rgb.to_be_bytes();
            Rgb::new(r, g, b)
        };
        // The styling guide makes ANSI black the background itself.
        let expected = [
            0x0028_2A36,
            0x00FF_5555,
            0x0050_FA7B,
            0x00F1_FA8C,
            0x00BD_93F9,
            0x00FF_79C6,
            0x008B_E9FD,
            0x00F8_F8F2,
            0x0062_72A4,
            0x00FF_6E6E,
            0x0069_FF94,
            0x00FF_FFA5,
            0x00D6_ACFF,
            0x00FF_92DF,
            0x00A4_FFFF,
            0x00FF_FFFF,
        ]
        .map(hex);
        assert_eq!(dracula.ansi, expected);
        assert_eq!(dracula.background, hex(0x0028_2A36));
        assert_eq!(dracula.foreground, hex(0x00F8_F8F2));
        assert_eq!(dracula.selection, hex(0x0044_475A));
    }

    #[test]
    fn base16_schemes_reuse_the_normal_colors_as_bright() {
        let scheme = SCHEMES
            .iter()
            .find(|s| s.name == "3024")
            .expect("base16 3024");
        let colors = scheme.terminal();
        for (normal, bright) in [(1, 9), (2, 10), (3, 11), (4, 12), (5, 13), (6, 14)] {
            assert_eq!(colors.ansi[normal], colors.ansi[bright], "{normal}");
        }
        assert_eq!(colors.ansi[0], colors.background);
    }

    #[test]
    fn every_scheme_keeps_terminal_text_readable() {
        for scheme in SCHEMES {
            let colors = scheme.terminal();
            let ratio = colors.foreground.contrast(colors.background);
            assert!(ratio >= 4.5, "{}: {ratio:.2}", scheme.name);
        }
    }

    #[test]
    fn a_step_reaches_its_contrast_and_no_further() {
        let surface = Rgb::new(0x28, 0x2A, 0x36);
        let deeper = step_away(surface, BLACK, 1.08).unwrap();
        let ratio = surface.contrast(deeper);
        assert!((1.08..1.12).contains(&ratio), "{ratio:.3}");
        assert!(deeper.luminance() < surface.luminance());
        // Black has no deeper step to give.
        assert_eq!(step_away(BLACK, BLACK, 1.08), None);
        // A dark scheme's window sits below its panels, and its hover above them.
        let dracula = Scheme::find("Dracula").unwrap().palette();
        assert!(dracula.bg_app.luminance() < dracula.bg_surface.luminance());
        assert!(dracula.bg_hover.luminance() > dracula.bg_surface.luminance());
        assert_eq!(dracula.bg_surface, surface);
    }

    #[test]
    fn nudging_moves_a_color_only_as_far_as_it_must() {
        let bg = Rgb::new(0x20, 0x20, 0x20);
        let readable = Rgb::new(0xE0, 0xE0, 0xE0);
        assert_eq!(ensure(readable, &[bg], 4.5, WHITE), readable);
        let dim = Rgb::new(0x40, 0x40, 0x40);
        let nudged = ensure(dim, &[bg], 4.5, WHITE);
        assert!(nudged.contrast(bg) >= 4.5);
        assert!(
            nudged.luminance() < 0.5,
            "{nudged:?} went further than needed"
        );
    }
}
