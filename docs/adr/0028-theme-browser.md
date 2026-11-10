# ADR-0028: A theme browser with families and a live preview

- Status: Accepted (2026-11-08)
- Date: 2026-11-08

## Context

The theme picker needed: searchable families, light/dark filters, a preview and smooth transitions. The flat scheme dropdown made hundreds of choices difficult to explore. Annulus already shipped the same 498 tinted-theming schemes.

## Decision

1. Appearance’s Color scheme button opens a native modal. It searches theme names, families, authors and variants. Related schemes share family headings; individual schemes sit under More themes. A scrolling family index jumps through the list. Only visible rows draw and accept input.
2. All, Dark and Light filter the modal independently of saved preferences. The initial filter follows the existing theme mode. Search and variant filtering intersect, and empty families disappear. The result count updates with them.
3. Moving the pointer over a row, focusing it or using Up/Down changes the preview pane. Colors blend using the existing theme fade; reduced motion snaps to the destination. Click a row, use Apply theme or press Enter to save. Escape, Close and a click outside cancel. Closing restores focus to the opener, and the background is inert while the modal is present.
4. Recent contains up to six applied themes, newest first, stored in device settings. Previewing, searching, filtering and cancelling do not write preferences. Rows appear once, even when also recent. Applying a scheme aligns the mode with its variant, so reopening does not hide the chosen scheme.
5. Ship six built-in palettes (from rbit, MIT, credited in `LICENSE-THEMES`) alongside all 498 community schemes, with the same native contrast checks and licenses in the portable client and Windows installer. Annulus remains the default, with the mark’s charcoal, silver and teal. The modal offers explicit Annulus Light and Annulus Dark choices alongside the system-following default.

This supersedes ADR-0026’s dropdown presentation. Its mode-change rule remains: explicitly switching the Appearance mode drops a scheme of the other kind. The terminal still has a separate color preference.

## Consequences

Exploration is reversible. No window preference changes until a theme is applied. The modal owns keyboard focus and keeps only its own controls in the accessibility tree. Search metadata is cached once and long lists draw only the visible rows. Existing saved scheme names remain valid; missing recent-theme settings default to an empty list.

## Alternatives considered

- Keep the long dropdown: it has no family navigation or space for a useful preview.
- Save every hover: it makes exploring destructive and writes settings unnecessarily.
- Embed a web picker: it conflicts with the native UI requirement in ADR-0017.
