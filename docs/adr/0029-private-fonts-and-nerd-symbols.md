# ADR-0029: Private font selection and licensed Nerd symbols

- Status: Accepted (2026-11-08)
- Date: 2026-11-08

## Context

The maintainer requested custom fonts, Nerd Font support and redistributable bundled choices. The CPU renderer previously knew only the bundled Fira fonts; Windows selected fixed system families. A renderer must measure and draw with the same selected face, and glyph fallback must preserve terminal columns.

## Decision

1. Appearance selects the app font; Terminal selects its monospace family independently. Both offer bundled, installed and privately imported families, with a searchable menu. TTF, OTF and TTC imports accept a pasted path or dropped file, validate it, and copy it into the settings directory’s private `fonts` folder. No font is installed into the OS. Settings store the family names and private file paths. Missing or invalid fonts leave usable bundled defaults and a readable status.
2. JetBrains Mono Regular/Bold is the terminal default; Fira Mono remains a bundled alternative. Both receive a curated Symbols Nerd Font Mono fallback, including BMP and supplementary-plane private-use glyphs. Installed or imported patched Nerd Fonts can be used directly. The pinned font source, copyright notices and licenses are in `crates/annulus-platform/fonts`; `LICENSE-FONTS` ships with portable and installed clients. The curated symbol build is documented as OFL-only by its [upstream distributor](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/LICENSE.md); the full mixed-license icon collection is not bundled.
3. Discovery, reading, validation and private copying run on a worker. Rendering consumes shared registered font bytes only. Imports are limited to 16 MB per file, 32 custom files and 64 MB total. Only selected installed faces are loaded, with a 32 MB bound; changing families replaces that cache. No network access is needed at runtime.
4. The CPU renderer registers the same selected bytes with Fontique/Parley and uses an ordered family fallback. DirectWrite uses a private in-memory font collection and explicit private-use fallback mappings, with system fallback for other characters. Font changes invalidate measurement/layout caches. Nerd icons are drawn within their terminal cells; proportional custom faces also keep each character in its grid column.
5. Missing font preferences in existing settings preserve the default app font and select bundled JetBrains Mono for terminals. App fonts remain platform defaults until explicitly changed. Reduced-motion and theme preferences are independent.

This extends ADR-0017’s text-rendering choices; it keeps the native renderers and toolkit.

## Consequences

Users can import a font without administrator access or modifying global font settings. Portable clients contain a complete licensed text/symbol default. Glyph tests cover real BMP and supplementary Nerd glyphs in both renderers; import and font-choice tests cover persistence and fallback. The remaining F-TERM-012 work (line-height controls, ligature toggles and per-tab preferences) remains separate.

## Alternatives considered

- Require OS installation: unnecessary permissions and no portable default.
- Bundle every patched font: much larger binaries and a broader set of licenses.
- Read fonts while drawing: blocks interaction and violates the rendering budgets.
