//! Our design tokens (UI-SPEC §8) as egui visuals, so every egui widget draws in Annulus's
//! own colors, radii and spacing.

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};
use annulus_config::tokens::{Appearance, Palette, Rgb, radius, space};

/// An egui color from a token color.
#[must_use]
pub fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// The egui style for `appearance`.
#[must_use]
pub fn style(appearance: Appearance) -> Style {
    let palette = Palette::for_appearance(appearance);
    let mut style = Style {
        visuals: visuals(appearance, &palette),
        ..Style::default()
    };
    style.spacing.item_spacing = egui::vec2(space::S, space::S);
    style.spacing.button_padding = egui::vec2(space::M, space::XS + 2.0);
    style.spacing.interact_size.y = annulus_config::tokens::metrics::CONTROL_HEIGHT - 8.0;
    style
}

fn visuals(appearance: Appearance, palette: &Palette) -> Visuals {
    let mut visuals = match appearance {
        Appearance::Dark => Visuals::dark(),
        Appearance::Light => Visuals::light(),
    };
    let small = CornerRadius::same(rounded(radius::SMALL));
    let medium = CornerRadius::same(rounded(radius::MEDIUM));
    visuals.panel_fill = color(palette.bg_app);
    visuals.window_fill = color(palette.bg_raised);
    visuals.extreme_bg_color = color(palette.bg_surface);
    visuals.faint_bg_color = color(palette.bg_surface);
    visuals.override_text_color = Some(color(palette.text_primary));
    visuals.hyperlink_color = color(palette.accent);
    visuals.selection.bg_fill = color(palette.bg_selected);
    visuals.selection.stroke = Stroke::new(1.0, color(palette.accent));
    visuals.warn_fg_color = color(palette.status_warn);
    visuals.error_fg_color = color(palette.status_error);
    visuals.window_corner_radius = CornerRadius::same(rounded(radius::LARGE));
    visuals.menu_corner_radius = medium;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, color(palette.border_subtle));
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, color(palette.text_secondary));
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = small;
        state.fg_stroke = Stroke::new(1.0, color(palette.text_primary));
    }
    widgets.inactive.weak_bg_fill = color(palette.bg_surface);
    widgets.inactive.bg_fill = color(palette.bg_surface);
    widgets.inactive.bg_stroke = Stroke::new(1.0, color(palette.border_subtle));
    widgets.hovered.weak_bg_fill = color(palette.bg_hover);
    widgets.hovered.bg_fill = color(palette.bg_hover);
    widgets.hovered.bg_stroke = Stroke::new(1.0, color(palette.border_subtle));
    widgets.active.weak_bg_fill = color(palette.bg_selected);
    widgets.active.bg_fill = color(palette.bg_selected);
    widgets.active.bg_stroke = Stroke::new(1.5, color(palette.accent));
    widgets.open.weak_bg_fill = color(palette.bg_hover);
    visuals
}

/// A token radius as egui's whole-pixel corner radius.
fn rounded(value: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Token radii are small and positive.
    let value = value.round().clamp(0.0, 255.0) as u8;
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_and_text_use_the_palette_of_each_theme() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let palette = Palette::for_appearance(appearance);
            let visuals = style(appearance).visuals;
            assert_eq!(visuals.panel_fill, color(palette.bg_app));
            assert_eq!(
                visuals.override_text_color,
                Some(color(palette.text_primary))
            );
            assert_eq!(visuals.dark_mode, appearance == Appearance::Dark);
        }
    }
}
