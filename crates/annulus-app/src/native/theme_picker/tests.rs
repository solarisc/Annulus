//! Search, grouping and exploration contracts.

use super::*;

#[test]
fn hovering_a_theme_previews_it_on_the_first_frame() {
    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_ui::kit::{Event, Theme, UiState};

    let settings = Settings {
        theme: ThemeMode::System,
        recent_themes: vec!["Solarized Light".into(), "Dracula".into()],
        ..Settings::default()
    };
    let mut picker = ThemePicker::new(&settings);
    let mut canvas = CpuCanvas::new(1040, 700, 1.0, TextEngine::new()).unwrap();
    let mut state = UiState::default();
    state.reduce_motion = true;
    let rect = Rect::new(0.0, 0.0, 1040.0, 700.0);
    let system = Theme::new(Appearance::Dark);
    let mut draw = |picker: &mut ThemePicker, state: &mut UiState, events: &[Event], time| {
        state.begin_frame(events, time);
        picker.draw(
            &mut Ui::new(&mut canvas, system, state, rect),
            &settings,
            system,
        );
        state.end_frame();
    };
    draw(&mut picker, &mut state, &[], 0.0);
    let row = state
        .zones()
        .iter()
        .find(|zone| zone.label == "Dracula")
        .unwrap()
        .rect;
    draw(
        &mut picker,
        &mut state,
        &[Event::PointerMoved(row.center())],
        0.01,
    );
    assert_eq!(
        picker.terminal.as_ref().unwrap().screen().background,
        colors::terminal_palette("Dracula").background
    );

    draw(
        &mut picker,
        &mut state,
        &[Event::Key(Key::Up, Modifiers::NONE)],
        0.02,
    );
    draw(&mut picker, &mut state, &[], 0.03);
    assert_eq!(
        picker.terminal.as_ref().unwrap().screen().background,
        colors::terminal_palette("Solarized Light").background
    );
}

#[test]
fn the_browser_contains_every_scheme_once_and_keeps_families_together() {
    let entries = catalog();
    let mut indices: Vec<_> = entries.iter().map(|entry| entry.index).collect();
    indices.sort_unstable();
    assert_eq!(
        indices,
        (0..colors::app_schemes().len()).collect::<Vec<_>>()
    );
    for (name, expected) in [
        ("Catppuccin Mocha", "Catppuccin"),
        ("Gruvbox dark, hard", "Gruvbox"),
        ("Rosé Pine Dawn", "Rosé Pine"),
    ] {
        assert_eq!(catalog::family(name), expected);
    }
}

#[test]
fn recent_themes_are_unique_filtered_and_keep_their_applied_order() {
    let settings = Settings {
        recent_themes: vec!["Solarized Light".into(), "Dracula".into(), "Gone".into()],
        ..Settings::default()
    };
    let mut picker = ThemePicker::new(&settings);
    let results = picker.results();
    assert_eq!(colors::app_schemes()[results[0].index], "Solarized Light");
    assert_eq!(colors::app_schemes()[results[1].index], "Dracula");
    assert_eq!(picker.group(results[0]), "Recent");
    let indices: std::collections::HashSet<_> = results.iter().map(|entry| entry.index).collect();
    assert_eq!(indices.len(), results.len());
    picker.filter = Filter::Dark;
    assert_eq!(colors::app_schemes()[picker.results()[0].index], "Dracula");
    assert!(
        !picker
            .results()
            .iter()
            .any(|entry| colors::app_schemes()[entry.index] == "Solarized Light")
    );
}

#[test]
fn search_and_variant_filters_intersect_without_changing_preferences() {
    let settings = Settings::default();
    let mut picker = ThemePicker::new(&settings);
    picker.query = "solarized".into();
    picker.filter = Filter::Light;
    let results = picker.results();
    assert!(!results.is_empty());
    assert!(results.iter().all(|entry| entry.dark == Some(false)));
    picker.filter = Filter::Dark;
    assert!(
        picker
            .results()
            .iter()
            .all(|entry| entry.dark == Some(true))
    );
    picker.query = "no-such-theme".into();
    assert!(picker.results().is_empty());
    assert!(settings.app_scheme.is_none());
}

#[test]
fn the_theme_preview_uses_the_selected_schemes_actual_terminal_palette() {
    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_ui::{
        gfx::Rect,
        kit::{Theme, Ui, UiState},
    };
    for name in [
        "Dracula",
        "Solarized Light",
        "rbit Dark Colorblind",
        colors::OWN_LIGHT,
    ] {
        let settings = Settings::default();
        let mut picker = ThemePicker::new(&settings);
        picker.selected = colors::app_schemes()
            .iter()
            .position(|known| known == name)
            .unwrap();
        let mut canvas = CpuCanvas::new(440, 500, 1.0, TextEngine::new()).unwrap();
        let mut state = UiState::default();
        state.begin_frame(&[], 0.0);
        state.reduce_motion = true;
        let rect = Rect::new(0.0, 0.0, 440.0, 500.0);
        let system = Theme::new(Appearance::Dark);
        let mut ui = Ui::new(&mut canvas, system, &mut state, rect);
        picker.preview(&mut ui, rect, &settings, system);
        let screen = picker.terminal.as_ref().unwrap().screen();
        let palette = colors::terminal_palette(name);
        assert_eq!(screen.background, palette.background, "{name}");
        assert!(
            screen
                .rows
                .iter()
                .flatten()
                .any(|cell| cell.fg == palette.ansi[2] && cell.ch != ' '),
            "{name}: actual ANSI green output"
        );
    }
}
