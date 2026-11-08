//! Which colors the window and the terminal draw with (F-SET-002, F-TERM-013): our own
//! themes, or a community color scheme, chosen separately for each.

use std::sync::OnceLock;

use annulus_config::schemes::{BUILT_INS, SCHEMES, Scheme};
use annulus_config::settings::Settings;
use annulus_config::tokens::{Rgb, ThemeMode};
use annulus_term::Palette;
use annulus_term::alacritty_terminal::vte::ansi::Rgb as TermRgb;
use annulus_ui::gfx::Color;
use annulus_ui::kit::Theme;
use annulus_ui::motion::Motion;

/// The first entry of the window's scheme menu: no scheme, our own theme.
pub const OWN_COLORS: &str = "Annulus";
/// Our own fixed light and dark choices, alongside the system-following default.
pub const OWN_LIGHT: &str = "Annulus Light";
/// Our own fixed dark choice.
pub const OWN_DARK: &str = "Annulus Dark";

/// The window's colors on their way to new ones: a scheme previewed or chosen, or the OS
/// turning dark, blends in over [`Motion::BASE`] instead of flashing.
#[derive(Default)]
pub struct ThemeFade {
    /// Where the colors come from, where they go, and since when.
    way: Option<(Theme, Theme, f64)>,
}

impl ThemeFade {
    /// The colors to draw with at `now` on the way to `target`, and whether they are still
    /// moving. With `instant` (reduced motion) a change shows at once.
    pub fn step(&mut self, target: Theme, now: f64, instant: bool) -> (Theme, bool) {
        if instant {
            self.way = Some((target, target, now));
            return (target, false);
        }
        let shown = self.at(now);
        if self.way.is_none_or(|(_, to, _)| to != target) {
            let from = match shown {
                Some((theme, _)) if !instant => theme,
                _ => target,
            };
            self.way = Some((from, target, now));
        }
        self.at(now).unwrap_or((target, false))
    }

    fn at(&self, now: f64) -> Option<(Theme, bool)> {
        let (from, to, start) = self.way?;
        let share = (now - start) / (f64::from(Motion::BASE.ms) / 1000.0);
        if share >= 1.0 || from == to {
            return Some((to, false));
        }
        #[allow(clippy::cast_possible_truncation)] // A share between 0 and 1.
        let eased = Motion::BASE.curve.at(share.max(0.0) as f32);
        Some((from.mix(&to, eased), true))
    }
}

/// The window's colors: the chosen scheme, else our own theme in `settings.theme`, where
/// `system` is ours as the OS currently wants it.
#[must_use]
pub fn app_theme(settings: &Settings, system: Theme) -> Theme {
    scheme_theme(settings.app_scheme.as_deref(), settings, system)
}

/// [`app_theme`] while the window's scheme menu has the pointer on entry `hovered` of
/// [`app_schemes`]: the window shows that one until the menu closes.
#[must_use]
pub fn previewed_theme(settings: &Settings, hovered: Option<usize>, system: Theme) -> Theme {
    match hovered.and_then(|index| app_schemes().get(index)) {
        Some(name) => scheme_theme(Some(name), settings, system),
        None => app_theme(settings, system),
    }
}

fn scheme_theme(scheme: Option<&str>, settings: &Settings, system: Theme) -> Theme {
    match scheme {
        Some(OWN_COLORS) => return system,
        Some(OWN_LIGHT) => return Theme::new(annulus_config::tokens::Appearance::Light),
        Some(OWN_DARK) => return Theme::new(annulus_config::tokens::Appearance::Dark),
        _ => {}
    }
    if let Some(scheme) = scheme.and_then(Scheme::find) {
        return Theme::from_palette(&scheme.palette(), scheme.dark());
    }
    match settings.theme {
        ThemeMode::Light => Theme::new(annulus_config::tokens::Appearance::Light),
        ThemeMode::Dark => Theme::new(annulus_config::tokens::Appearance::Dark),
        ThemeMode::System => system,
    }
}

/// The window's scheme menu: our own colors, then every scheme.
pub fn app_schemes() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        std::iter::once(OWN_COLORS.to_owned())
            .chain(
                SCHEMES
                    .iter()
                    .chain(BUILT_INS)
                    .map(|scheme| scheme.name.to_owned()),
            )
            .chain([OWN_LIGHT.to_owned(), OWN_DARK.to_owned()])
            .collect()
    })
}

/// Applies a browser choice and remembers only applied themes.
pub fn apply(settings: &mut Settings, index: usize) {
    let Some(name) = app_schemes().get(index) else {
        return;
    };
    match name.as_str() {
        OWN_COLORS => {
            settings.app_scheme = None;
            settings.theme = ThemeMode::System;
        }
        OWN_LIGHT => {
            settings.app_scheme = None;
            settings.theme = ThemeMode::Light;
        }
        OWN_DARK => {
            settings.app_scheme = None;
            settings.theme = ThemeMode::Dark;
        }
        _ => {
            settings.app_scheme = Some(name.clone());
            if let Some(scheme) = Scheme::find(name) {
                settings.theme = if scheme.dark() {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                };
            }
        }
    }
    settings.recent_themes.retain(|known| known != name);
    settings.recent_themes.insert(0, name.clone());
    settings.recent_themes.truncate(6);
}

/// The window's scheme once the mode becomes `mode` (ADR-0026): a scheme of the other kind
/// goes, so the window turns light or dark as the mode says; under System any stays.
#[must_use]
pub fn scheme_kept(scheme: Option<String>, mode: ThemeMode) -> Option<String> {
    let dark = scheme.as_deref().and_then(Scheme::find).map(Scheme::dark);
    match (mode, dark) {
        (ThemeMode::Light, Some(true)) | (ThemeMode::Dark, Some(false)) => None,
        _ => scheme,
    }
}

/// What each entry of [`app_schemes`] looks like in small: the window's background, then its
/// text, accent, success and error colors.
pub fn app_swatches() -> &'static [Vec<Color>] {
    static SWATCHES: OnceLock<Vec<Vec<Color>>> = OnceLock::new();
    SWATCHES.get_or_init(|| {
        let own = Theme::new(annulus_config::tokens::Appearance::Dark);
        std::iter::once(vec![own.chrome, own.text, own.accent, own.ok, own.error])
            // The scheme's own colors, as the window takes them (`Scheme::palette`), without
            // deriving every palette just to open the menu.
            .chain(SCHEMES.iter().chain(BUILT_INS).map(|scheme| {
                [0x00, 0x05, 0x0D, 0x0B, 0x08]
                    .map(|slot: usize| {
                        let [r, g, b] = scheme.base[slot];
                        Color::from(Rgb::new(r, g, b))
                    })
                    .to_vec()
            }))
            .chain([annulus_config::tokens::Appearance::Light, annulus_config::tokens::Appearance::Dark].map(|appearance| {
                let own = Theme::new(appearance);
                vec![own.chrome, own.text, own.accent, own.ok, own.error]
            }))
            .collect()
    })
}

/// What each entry of [`terminal_themes`] looks like in small: the background, then the text,
/// red, green and blue.
pub fn terminal_swatches() -> &'static [Vec<Color>] {
    static SWATCHES: OnceLock<Vec<Vec<Color>>> = OnceLock::new();
    SWATCHES.get_or_init(|| {
        terminal_themes()
            .iter()
            .map(|name| {
                let palette = terminal_palette(name);
                let color = |c: TermRgb| Color::rgba(c.r, c.g, c.b, 255);
                vec![
                    color(palette.background),
                    color(palette.foreground),
                    color(palette.ansi[1]),
                    color(palette.ansi[2]),
                    color(palette.ansi[4]),
                ]
            })
            .collect()
    })
}

/// The terminal theme menu: our built-ins, then every scheme they do not already name.
pub fn terminal_themes() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let builtins = annulus_term::themes::THEMES;
        builtins
            .iter()
            .map(|name| (*name).to_owned())
            .chain(
                SCHEMES
                    .iter()
                    .chain(BUILT_INS)
                    .filter(|scheme| !builtins.contains(&scheme.name))
                    .map(|scheme| scheme.name.to_owned()),
            )
            .collect()
    })
}

/// The terminal colors called `name`: a built-in, a scheme, or our default for a name this
/// version does not know.
#[must_use]
pub fn terminal_palette(name: &str) -> Palette {
    annulus_term::themes::theme(name)
        .or_else(|| {
            Scheme::find(name).map(|scheme| {
                let colors = scheme.terminal();
                let term = |c: Rgb| TermRgb {
                    r: c.r,
                    g: c.g,
                    b: c.b,
                };
                Palette {
                    ansi: colors.ansi.map(term),
                    foreground: term(colors.foreground),
                    background: term(colors.background),
                    cursor: term(colors.cursor),
                    selection: term(colors.selection),
                }
            })
        })
        .unwrap_or_else(Palette::annulus_dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use annulus_config::tokens::Appearance;

    #[test]
    fn a_scheme_recolors_the_window_and_none_keeps_our_own_theme() {
        let system = Theme::new(Appearance::Light);
        let mut settings = Settings {
            theme: ThemeMode::Dark,
            ..Settings::default()
        };
        assert_eq!(app_theme(&settings, system), Theme::new(Appearance::Dark));
        settings.theme = ThemeMode::System;
        assert_eq!(app_theme(&settings, system), system);

        // A scheme is light or dark by itself, whatever the mode says.
        settings.app_scheme = Some("Solarized Light".into());
        let solarized = app_theme(&settings, system);
        assert!(!solarized.dark);
        assert_eq!(
            solarized.chrome,
            annulus_ui::gfx::Color::from(Scheme::find("Solarized Light").unwrap().palette().bg_app)
        );
        settings.app_scheme = Some("Dracula".into());
        assert!(app_theme(&settings, system).dark);
        // A scheme this version does not know falls back to our own.
        settings.app_scheme = Some("Gone".into());
        assert_eq!(app_theme(&settings, system), system);
    }

    #[test]
    fn the_hovered_scheme_shows_until_the_menu_lets_go_and_each_entry_has_a_swatch() {
        let system = Theme::new(Appearance::Light);
        let settings = Settings {
            theme: ThemeMode::Dark,
            app_scheme: Some("Solarized Light".into()),
            ..Settings::default()
        };
        let names = app_schemes();
        let dracula = names.iter().position(|name| name == "Dracula").unwrap();
        let shown = previewed_theme(&settings, Some(dracula), system);
        assert!(shown.dark);
        assert_eq!(
            shown,
            app_theme(
                &Settings {
                    app_scheme: Some("Dracula".into()),
                    ..settings.clone()
                },
                system
            )
        );
        // The system-following default previews the system, independently of the saved mode.
        assert_eq!(previewed_theme(&settings, Some(0), system), system);
        assert_eq!(
            previewed_theme(&settings, None, system),
            app_theme(&settings, system)
        );

        assert_eq!(app_swatches().len(), names.len());
        assert_eq!(terminal_swatches().len(), terminal_themes().len());
        let ayu = terminal_themes()
            .iter()
            .position(|name| name == "Ayu Dark")
            .unwrap();
        let background = terminal_palette("Ayu Dark").background;
        assert_eq!(
            terminal_swatches()[ayu][0],
            Color::rgba(background.r, background.g, background.b, 255)
        );
    }

    #[test]
    fn new_colors_blend_in_and_reduced_motion_shows_them_at_once() {
        let dark = Theme::new(Appearance::Dark);
        let light = Theme::new(Appearance::Light);
        let mut fade = ThemeFade::default();
        assert_eq!(fade.step(dark, 0.0, false), (dark, false));
        assert_eq!(
            fade.step(light, 1.0, false),
            (dark, true),
            "it starts where it was"
        );
        let (between, moving) = fade.step(light, 1.1, false);
        assert!(moving && between != dark && between != light);
        assert_eq!(fade.step(light, 1.3, false), (light, false));
        fade.step(dark, 1.5, false);
        assert_eq!(
            fade.step(dark, 1.51, true),
            (dark, false),
            "turning reduced motion on ends an in-progress fade"
        );
        // Turned back halfway, it goes back from where it is, not from the far end.
        fade.step(dark, 2.0, false);
        let (turned, _) = fade.step(dark, 2.1, false);
        assert_eq!(fade.step(light, 2.1, false), (turned, true));
        assert_eq!(fade.step(dark, 3.0, true), (dark, false), "reduced motion");
    }

    #[test]
    fn the_terminal_menu_lists_built_ins_first_and_each_name_once() {
        let names = terminal_themes();
        assert_eq!(
            &names[..7],
            &annulus_term::themes::THEMES.map(str::to_owned)[..]
        );
        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a name appears twice");
        assert!(names.len() > 450);
        assert_eq!(app_schemes()[0], OWN_COLORS);
        assert_eq!(app_schemes().len(), SCHEMES.len() + BUILT_INS.len() + 3);
    }

    #[test]
    fn applying_default_variants_keeps_them_distinct_and_recent_is_bounded() {
        let mut settings = Settings::default();
        let pick = |name: &str| {
            app_schemes()
                .iter()
                .position(|known| known == name)
                .unwrap()
        };
        for (name, mode) in [
            (OWN_LIGHT, ThemeMode::Light),
            (OWN_DARK, ThemeMode::Dark),
            (OWN_COLORS, ThemeMode::System),
        ] {
            apply(&mut settings, pick(name));
            assert_eq!(settings.theme, mode);
            assert!(settings.app_scheme.is_none());
        }
        for name in app_schemes().iter().skip(1).take(10) {
            apply(&mut settings, pick(name));
        }
        assert_eq!(settings.recent_themes.len(), 6);
        let name = settings.recent_themes[2].clone();
        apply(&mut settings, pick(&name));
        assert_eq!(settings.recent_themes[0], name);
        assert_eq!(
            settings
                .recent_themes
                .iter()
                .filter(|known| **known == name)
                .count(),
            1
        );
    }

    #[test]
    fn a_mode_of_the_other_kind_drops_the_scheme_and_system_keeps_any() {
        let dracula = || Some("Dracula".to_owned());
        assert_eq!(scheme_kept(dracula(), ThemeMode::Light), None);
        assert_eq!(scheme_kept(dracula(), ThemeMode::Dark), dracula());
        assert_eq!(scheme_kept(dracula(), ThemeMode::System), dracula());
        let solarized = || Some("Solarized Light".to_owned());
        assert_eq!(scheme_kept(solarized(), ThemeMode::Dark), None);
        assert_eq!(scheme_kept(solarized(), ThemeMode::Light), solarized());
        assert_eq!(scheme_kept(None, ThemeMode::Dark), None);
    }

    #[test]
    fn terminal_names_resolve_to_built_ins_then_schemes() {
        // Our built-in wins over the scheme of the same name.
        assert_eq!(
            terminal_palette("Dracula"),
            annulus_term::themes::theme("Dracula").unwrap()
        );
        let ayu = terminal_palette("Ayu Dark");
        let scheme = Scheme::find("Ayu Dark").unwrap().terminal();
        assert_eq!(ayu.background.r, scheme.background.r);
        assert_ne!(ayu, Palette::annulus_dark());
        assert_eq!(terminal_palette("Gone"), Palette::annulus_dark());
    }
}
