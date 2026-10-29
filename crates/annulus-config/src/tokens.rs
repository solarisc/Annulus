//! Design tokens: our own visual identity (UI-SPEC §8, ADR-0010). Colors, radii, spacing,
//! type sizes and control metrics, for the light and dark themes.

/// An opaque sRGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// The color with these red, green and blue values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// WCAG 2.x relative luminance, 0.0 (black) to 1.0 (white).
    #[must_use]
    pub fn luminance(self) -> f64 {
        let channel = |value: u8| {
            let c = f64::from(value) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// WCAG 2.x contrast ratio with `other`, from 1.0 to 21.0.
    #[must_use]
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

/// Which theme the user picked in Settings (F-SET-002).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Always light.
    Light,
    /// Always dark.
    Dark,
    /// Follow the operating system, switching live when it does.
    #[default]
    System,
}

/// A resolved theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    /// Light theme.
    Light,
    /// Dark theme.
    Dark,
}

impl ThemeMode {
    /// The theme to draw, given what the OS currently uses.
    #[must_use]
    pub fn resolve(self, system: Appearance) -> Appearance {
        match self {
            ThemeMode::Light => Appearance::Light,
            ThemeMode::Dark => Appearance::Dark,
            ThemeMode::System => system,
        }
    }
}

/// The color tokens of one theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Window background.
    pub bg_app: Rgb,
    /// Panels and cards.
    pub bg_surface: Rgb,
    /// Popovers, menus and dialogs.
    pub bg_raised: Rgb,
    /// Hovered rows and controls.
    pub bg_hover: Rgb,
    /// Selected rows.
    pub bg_selected: Rgb,
    /// Dividers and control outlines.
    pub border_subtle: Rgb,
    /// Main text.
    pub text_primary: Rgb,
    /// Secondary text: hints, metadata.
    pub text_secondary: Rgb,
    /// Accent: primary buttons, links, the focus ring.
    pub accent: Rgb,
    /// Warnings.
    pub status_warn: Rgb,
    /// Errors, also used as text.
    pub status_error: Rgb,
    /// Success and "connected".
    pub status_ok: Rgb,
}

impl Palette {
    /// The palette for `appearance`.
    #[must_use]
    pub const fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => LIGHT,
            Appearance::Dark => DARK,
        }
    }

    /// Every background a token can sit on.
    #[must_use]
    pub const fn backgrounds(&self) -> [Rgb; 5] {
        [
            self.bg_app,
            self.bg_surface,
            self.bg_raised,
            self.bg_hover,
            self.bg_selected,
        ]
    }
}

/// The dark theme.
pub const DARK: Palette = Palette {
    bg_app: Rgb::new(0x12, 0x14, 0x16),
    bg_surface: Rgb::new(0x1A, 0x1D, 0x20),
    bg_raised: Rgb::new(0x23, 0x27, 0x2B),
    bg_hover: Rgb::new(0x2B, 0x30, 0x35),
    bg_selected: Rgb::new(0x13, 0x3A, 0x37),
    border_subtle: Rgb::new(0x2C, 0x31, 0x36),
    text_primary: Rgb::new(0xE8, 0xEA, 0xED),
    text_secondary: Rgb::new(0xA9, 0xB0, 0xB7),
    accent: Rgb::new(0x2B, 0xB3, 0xA3),
    status_warn: Rgb::new(0xE0, 0xA4, 0x3A),
    status_error: Rgb::new(0xE8, 0x64, 0x5D),
    status_ok: Rgb::new(0x4C, 0xC2, 0x7A),
};

/// The light theme. Raised surfaces are white and set apart by a shadow.
pub const LIGHT: Palette = Palette {
    bg_app: Rgb::new(0xF6, 0xF7, 0xF8),
    bg_surface: Rgb::new(0xFF, 0xFF, 0xFF),
    bg_raised: Rgb::new(0xFF, 0xFF, 0xFF),
    bg_hover: Rgb::new(0xEE, 0xF0, 0xF2),
    bg_selected: Rgb::new(0xD9, 0xF2, 0xEE),
    border_subtle: Rgb::new(0xE2, 0xE5, 0xE8),
    text_primary: Rgb::new(0x15, 0x18, 0x1B),
    text_secondary: Rgb::new(0x4D, 0x56, 0x5E),
    accent: Rgb::new(0x11, 0x7C, 0x70),
    status_warn: Rgb::new(0xB9, 0x7B, 0x10),
    status_error: Rgb::new(0xC9, 0x3A, 0x33),
    status_ok: Rgb::new(0x24, 0x93, 0x4F),
};

/// Host tile colors: each host gets one from its label, so a list is easy to scan. Even in
/// lightness, so no host looks louder than another, and dark enough that white initials
/// meet the text contrast of WCAG AA (4.5:1, tested) in both themes.
pub const TILES: [Rgb; 8] = [
    Rgb::new(0x17, 0x7A, 0x70),
    Rgb::new(0x35, 0x63, 0xC9),
    Rgb::new(0x5B, 0x55, 0xD0),
    Rgb::new(0x87, 0x46, 0xBE),
    Rgb::new(0xBD, 0x3E, 0x68),
    Rgb::new(0xB0, 0x50, 0x1C),
    Rgb::new(0x2C, 0x7A, 0x45),
    Rgb::new(0x4F, 0x5D, 0x70),
];

/// Corner radii in DIPs.
pub mod radius {
    /// Inputs and chips.
    pub const SMALL: f32 = 4.0;
    /// Cards and buttons.
    pub const MEDIUM: f32 = 8.0;
    /// Dialogs.
    pub const LARGE: f32 = 12.0;
}

/// The 4-DIP spacing scale.
pub mod space {
    /// 4 DIPs.
    pub const XS: f32 = 4.0;
    /// 8 DIPs.
    pub const S: f32 = 8.0;
    /// 12 DIPs.
    pub const M: f32 = 12.0;
    /// 16 DIPs.
    pub const L: f32 = 16.0;
    /// 24 DIPs.
    pub const XL: f32 = 24.0;
    /// 32 DIPs.
    pub const XXL: f32 = 32.0;
}

/// Type sizes in DIPs, for the system UI font.
pub mod text {
    /// Body text.
    pub const BODY: f32 = 13.0;
    /// Secondary text.
    pub const SECONDARY: f32 = 12.0;
    /// Section headings.
    pub const HEADING: f32 = 15.0;
    /// Page headings.
    pub const TITLE: f32 = 18.0;
}

/// Control and list metrics in DIPs.
pub mod metrics {
    /// Height of buttons, fields and selects.
    pub const CONTROL_HEIGHT: f32 = 32.0;
    /// Height of a host card in grid view.
    pub const HOST_CARD_HEIGHT: f32 = 72.0;
    /// Height of a row in list view.
    pub const LIST_ROW_HEIGHT: f32 = 36.0;
    /// Width of the focus ring, drawn outside the control with this offset.
    pub const FOCUS_RING_WIDTH: f32 = 2.0;
}

/// Animation durations in milliseconds (UI-SPEC §8 Motion). Nothing animates under reduced
/// motion.
pub mod motion {
    /// Hover, press and focus feedback, small fades.
    pub const FAST: u32 = 120;
    /// Panels, menus, page and tab changes.
    pub const BASE: u32 = 200;
    /// Large surfaces: dialogs, the New Tab page, a list filling in.
    pub const SLOW: u32 = 280;
    /// Each further row of a list that fills in starts this much later.
    pub const STAGGER: u32 = 24;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG AA for normal text.
    const TEXT_AA: f64 = 4.5;
    /// WCAG AA for UI components and graphics (focus ring, status dots), 1.4.11.
    const NON_TEXT_AA: f64 = 3.0;

    fn failures(palette: &Palette) -> Vec<String> {
        let mut failures = Vec::new();
        let mut require = |name: &str, color: Rgb, backgrounds: &[Rgb], minimum: f64| {
            for background in backgrounds {
                let ratio = color.contrast(*background);
                if ratio < minimum {
                    failures.push(format!("{name} on {background:?}: {ratio:.2} < {minimum}"));
                }
            }
        };
        let all = palette.backgrounds();
        let panels = [palette.bg_app, palette.bg_surface, palette.bg_raised];
        require("text_primary", palette.text_primary, &all, TEXT_AA);
        require("text_secondary", palette.text_secondary, &all, TEXT_AA);
        // Links and error messages are text on panels; the ring and dots are graphics.
        require("accent", palette.accent, &panels, TEXT_AA);
        require("status_error", palette.status_error, &panels, TEXT_AA);
        for (name, color) in [
            ("accent", palette.accent),
            ("status_warn", palette.status_warn),
            ("status_error", palette.status_error),
            ("status_ok", palette.status_ok),
        ] {
            require(name, color, &all, NON_TEXT_AA);
        }
        failures
    }

    #[test]
    fn both_themes_meet_wcag_aa() {
        assert_eq!(failures(&DARK), Vec::<String>::new(), "dark");
        assert_eq!(failures(&LIGHT), Vec::<String>::new(), "light");
    }

    #[test]
    fn white_initials_are_readable_on_every_host_tile() {
        let white = Rgb::new(0xFF, 0xFF, 0xFF);
        for tile in TILES {
            let ratio = white.contrast(tile);
            assert!(ratio >= TEXT_AA, "{tile:?}: {ratio:.2}");
        }
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let black = Rgb::new(0x00, 0x00, 0x00);
        let white = Rgb::new(0xFF, 0xFF, 0xFF);
        assert!((black.contrast(white) - 21.0).abs() < 1e-9);
        assert!((white.contrast(white) - 1.0).abs() < 1e-9);
        // #767676 on white is the classic smallest gray that passes AA (4.54:1).
        assert!((Rgb::new(0x76, 0x76, 0x76).contrast(white) - 4.54).abs() < 0.01);
    }

    #[test]
    fn system_mode_follows_the_os_and_fixed_modes_ignore_it() {
        assert_eq!(
            ThemeMode::System.resolve(Appearance::Dark),
            Appearance::Dark
        );
        assert_eq!(
            ThemeMode::System.resolve(Appearance::Light),
            Appearance::Light
        );
        assert_eq!(
            ThemeMode::Light.resolve(Appearance::Dark),
            Appearance::Light
        );
        assert_eq!(ThemeMode::Dark.resolve(Appearance::Light), Appearance::Dark);
    }
}
