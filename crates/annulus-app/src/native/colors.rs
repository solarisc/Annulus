//! Which colors the window and the terminal draw with (F-SET-002, F-TERM-013): our own
//! themes, or a community color scheme, chosen separately for each.

use std::sync::OnceLock;

use annulus_config::schemes::{SCHEMES, Scheme};
use annulus_config::settings::Settings;
use annulus_config::tokens::{Rgb, ThemeMode};
use annulus_term::Palette;
use annulus_term::alacritty_terminal::vte::ansi::Rgb as TermRgb;
use annulus_ui::kit::Theme;

/// The first entry of the window's scheme menu: no scheme, our own theme.
pub const OWN_COLORS: &str = "Annulus";

/// The window's colors: the chosen scheme, else our own theme in `settings.theme`, where
/// `system` is ours as the OS currently wants it.
#[must_use]
pub fn app_theme(settings: &Settings, system: Theme) -> Theme {
    if let Some(scheme) = settings.app_scheme.as_deref().and_then(Scheme::find) {
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
            .chain(SCHEMES.iter().map(|scheme| scheme.name.to_owned()))
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
        assert_eq!(app_schemes().len(), SCHEMES.len() + 1);
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
