//! Search, grouping and exploration contracts.

use super::*;

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
