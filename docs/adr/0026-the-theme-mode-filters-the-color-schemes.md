# ADR-0026: The theme mode filters the window's color schemes

- Status: Accepted (2026-11-07)
- Date: 2026-11-07

## Context

[ADR-0019](0019-tinted-color-schemes.md) decision 2 kept two things apart: Settings → Appearance → *Theme* (System, Light, Dark) chose only between our own light and dark themes, and *Color scheme* listed all 498 schemes, each light or dark by itself, whatever the mode said. Choosing Light while a dark scheme was chosen left the window dark, and the long list mixed both kinds.

The maintainer asked for the Theme tabs to filter the scheme menu.

## Decision

1. **The mode filters the menu.** Under Light the *Color scheme* menu lists our own colors and the light schemes. Under Dark it lists ours and the dark ones. Under System it lists every scheme, as before. Our own colors come first in each, shown in the mode's look.
2. **A mode of the other kind drops the scheme.** Choosing Light while a dark scheme is chosen (or Dark while a light one is) goes back to our own colors, so the window turns the way the tab says. Under System a chosen scheme stays.
3. **Schemes stay fixed.** A scheme is still light or dark by itself, and System does not switch a scheme to a counterpart when the OS changes (ADR-0019, "Alternatives considered"). Only `app_scheme` is stored; no new setting.

This supersedes the sentence "the System/Light/Dark mode applies only to our own theme" in ADR-0019 decision 2. The rest of ADR-0019 stands.

## Consequences

- A scheme chosen under one kind is lost when the mode turns to the other. It is a preference picked from a list, so choosing it again is one click.
- The terminal's own *Colors* menu (Settings → Terminal) is not filtered: it is a separate choice (ADR-0019 decision 2).
- `colors::scheme_menu` and `colors::scheme_kept` hold the rules; `the_theme_mode_lists_only_schemes_of_its_kind_and_system_lists_all`, `a_mode_of_the_other_kind_drops_the_scheme_and_system_keeps_any` and `the_theme_tabs_offer_schemes_of_their_kind_and_drop_one_of_the_other` test them.

## Alternatives considered

- **A light and a dark scheme, switched under System.** It would let System follow the OS with schemes too, but it needs two settings and undoes the maintainer's earlier choice of fixed schemes. Not asked for.
- **Filter under System by the OS's current look.** The list would change under the user when the OS turns dark at sunset.
