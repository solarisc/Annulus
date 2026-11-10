# ADR-0017: Our own native UI

- Status: Accepted (2026-10-29); font selection extended by [ADR-0029](0029-private-fonts-and-nerd-symbols.md)
- Date: 2026-10-29
- Builds on: the two-renderer plan of [ADR-0012](0012-three-platforms-two-renderers.md) decision 2, with an immediate-mode toolkit instead of ADR-0012's retained widget tree. The terminal engine stays `alacritty_terminal` ([ADR-0016](0016-alacritty-terminal-core.md)); only the drawing of it is ours.

## Context

A general-purpose GUI framework gets every screen on the glass quickly, but its look is the framework's: soft text, generic panels, and a frame that looks like a toolkit's, not a product's. An app that draws its whole window itself (Direct2D/DirectWrite on Windows, CoreGraphics on macOS, its own title bar with Snap Layouts kept) looks native and sharp, and its memory and startup stay ours to keep small.

## Decision

1. **No GUI framework.** The UI is ours, drawn from our design tokens.
2. **An immediate-mode toolkit in `annulus-ui`**: each frame, views paint through a `Canvas` and register interactive *zones* (rect, role, name, focusability). Input is resolved against the zones of the frame on screen, so painting and hit-testing never disagree. The toolkit owns hover/press/focus state, keyboard focus order, short eased animations (none under reduced motion), text editing, and the accessibility tree (each zone is an AccessKit node).
3. **Two renderers behind `Canvas`**, both in `annulus-platform` (the only crate with `unsafe`):
   - **Direct2D 1.1 + DirectWrite on Windows**, with the system UI font (Segoe UI Variable, falling back to Segoe UI) and ClearType-quality text. It can also draw into a WIC bitmap with no window, which is how CI photographs Windows screens.
   - **A CPU renderer**: `tiny-skia` for shapes, `parley` + `swash` for text, from bundled fonts. It ships on Linux (and on macOS until a CoreGraphics renderer exists), and it renders the deterministic golden images on every OS.
4. **The window**: on Windows, our own Win32 window whose client area covers the caption: Windows keeps the borders, resize, Aero Snap, the shadow and Snap Layouts on our maximize button. On Linux and macOS, `winit` + `softbuffer`.
5. **Text fields are our own editor**, not native EDIT controls. Password fields edit a `Zeroizing<String>` and keep no undo copies, as SECURITY.md asks.
6. **CI photographs every screen.** A scene catalogue renders each screen and state (empty, populated, hover, focus, error, editor open) in both themes and at several window sizes and scales, through the CPU renderer on Linux and Windows and through Direct2D on Windows. The images are uploaded with every run for review. CPU images of finished screens become goldens that fail the build when they change. Every scene is also checked for layout faults: interactive zones that overlap, and text cut off where no ellipsis was allowed.

## Consequences

- Every widget is ours to write and test: text input, IME, selection, scrolling, menus, accessibility.
- Sharper text and a native frame on Windows. Lower memory and startup than a framework would allow (P-01, P-02), measured on the reference machines.
- The terminal grid is drawn by us (DirectWrite glyph runs or a cached glyph atlas), so its speed is ours to keep within PERFORMANCE.md.
- UI-SPEC's rule that only the terminal is pixel-tested is replaced by decision 6.
- The project principle is "our own UI, no GUI framework" ([CONTRIBUTING.md](../../CONTRIBUTING.md#principles)).

## Alternatives considered

- **A GUI framework such as egui, restyled.** Days instead of weeks, but text stays softer than DirectWrite and the frame stays generic.
- **Native renderer on Windows only, a framework elsewhere.** Two UIs to maintain.
- **The retained tree of ADR-0012.** An immediate-mode toolkit is much less work, and the style holds up in practice.
