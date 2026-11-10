# ADR-0021: A quiet look, and motion for the big moments

- Status: Accepted (2026-11-05)
- Date: 2026-11-05

## Context

By 2026-11-05 the UI worked but did not feel finished: it needed more animation and a calmer, more confident look. A review of the code and the photos found two causes.

**The look was busy.** Everything was a box with an outline, nested three deep on some screens. Teal marked selection as well as the main action, so one screen could show several teal surfaces. Host tiles, saturated and glossy, were the loudest thing on screen. The largest text was 18 px against 13 px body.

**Motion was thin where it mattered.** Nearly every change was the same small fade and 8 px rise, and the largest moments had less than that: unlocking, opening a terminal tab and closing a menu were all hard cuts, and the host editor's list jumped to its new size on the first frame. Under that sat the toolkit's limits. A fading layer faded each shape by itself, so overlapping shapes showed through each other; scaling a layer changed the font size each frame, so letters shuffled; and there were no gradients.

## Decision

1. **Selection is neutral.** Hover, press, selection and text fields are veils of the text color, so they read on any surface. The accent is kept for the screen's main action, focus, links, progress and things that are on (a switch, a check box). Warnings and errors keep their colors.
2. **Warm graphite neutrals,** with the teal as the only color that speaks, in both themes and in the Annulus terminal theme. Tokens stay ours under [ADR-0010](0010-original-work-only.md).
3. **Fewer boxes.** Cards and rows are a step off the sheet with only a faint edge to keep them crisp, fields have no outline at rest, and the sheet has none around it. Elevation means something: floating surfaces (menus, dialogs, toasts) get a two-part shadow and a hairline.
4. **Quiet tiles and larger type.** Host tiles are a wash of their hue with a mark in the same hue. Page titles are 24 px in the display cut of the UI font, with their count set quietly beside them. Labels are sentence case.
5. **The title and the actions share a row, above the search.** This departs from the layout UI-SPEC first used (actions under the search). The places someone coming from another SSH client looks for are still all there, and the hosts start about 40 px higher.
6. **Every big moment gets its own motion,** and whatever leaves is gone quickly so it never holds up what comes next:
   - Things that leave are drawn one last time, taking no input (`Ui::push_inert`), while what replaced them already works.
   - Unlocking opens the lock's shackle while the screen fades up and away over the vault coming in. A refused password shakes the field.
   - The editor's panel travels its whole width with its form on it, and the list's top row and search follow its edge. Cards in the same columns glide to their new places on springs. Into other columns their paths would cross, so the old grid fades out first and the new one fills in: two layouts are never on screen together. A window being resized moves them at once.
   - Tabs fade through from the side they come from. Sections fade through with no overlap of their words. Closed menus fade out, and one highlight glides between a menu's rows.
   - A connected host's tile flies from its card to its tab's icon and melts into it, and a tab shows a turning arc while its session connects.
   - The pointer is answered in 70 ms and let go in 220 ms.
7. **The canvas grows three primitives** every renderer implements: `push_group`/`pop_group` (real group opacity: Direct2D layers, a CPU offscreen pass), `fill_gradient`, and `draw_text_zoomed` (Direct2D scales the shaped run). Group opacity costs an offscreen pass, so it applies only while a layer that asked for it is fading.

## Consequences

- Terminal text is never animated, and neither is anything else while the window is idle: all of this runs only during a transition, so P-06 (no idle wakeups) still holds, and the scene check enforces it.
- Hover and selection veils cannot be `mix`ed with opaque colors. Use `Color::over`.
- A color scheme whose text sits close to its panels gets thinner veils and, if needed, lifted secondary text, so every scheme stays AA on them (`both_themes_keep_text_readable_on_every_surface`).
- Reduced motion skips all of it, by the same rules as before.
- Zones glide with what they belong to, so the layout lint skips overlaps while cards are reflowing.

## Alternatives considered

- **Keep teal selection and only add animation.** Quickest, but it was the busyness as much as the motion that made the UI feel unfinished.
- **Delay the screen change until the old one has left.** Simple to draw, but input would wait on an animation. Drawing the old screen inert keeps the new one working from the first frame.
- **Glide cards across a change of columns.** It reads as a pile of crossing cards.
- **An editor that overlays the list with no reflow at all.** Calm, but it would hide the toolbar and a card's own column while a host is edited.
