# ADR-0012: Three desktop platforms at 1.0, two renderers, portable windowing

- Status: Proposed
- Date: 2026-10-28
- Supersedes: an earlier Windows-first toolkit plan. This ADR sets the platform scope and the backends.

## Context

On 2026-10-28 the maintainer decided that 1.0 ships on **Windows, macOS and Linux**. Mobile is out. Four constraints shape the choice:

- The UI must be hand-built and drawn "like D2D", with no GUI framework.
- Startup time and memory must be as low as possible (P-01, P-02 in [PERFORMANCE.md](../PERFORMANCE.md)).
- A small team means few backends to maintain.
- Pixel tests must be deterministic in CI on every OS.

## Decision

1. **The toolkit is unchanged.** `annulus-ui` owns the widget tree, layout (flexbox subset), styling from design tokens, input routing, focus, animation, the accessibility tree and a backend-independent display list. We draw our own vector icons. No GUI frameworks and no webviews.

2. **There are two renderer backends behind the display list.**
   - **Windows: Direct2D 1.1 + DirectWrite.** It draws on a DXGI flip-model waitable swap chain and presents only dirty rectangles. This includes the rule that a D3D11 glyph atlas only gets built if the budgets demand it.
   - **The portable CPU backend, also called the "reference renderer".** It ships on macOS and Linux for 1.0. It also renders every pixel-snapshot test on every OS, using bundled fonts, so results are identical everywhere. It combines:
     - `tiny-skia` for paths, fills, strokes and clips
     - `skrifa` and `swash` to load glyph outlines and rasterize them into a glyph cache
     - `harfrust` for shaping
     - `parley` for UI text layout (line breaking, bidi, fallback)
     - `fontique` for system font discovery (it uses each OS's native font APIs)
     - `softbuffer` to present the frame (Wayland shm, X11, Core Graphics)
   - Both backends redraw only damaged rectangles. The CPU-rendered terminal foot is the precedent that this is fast and light.

3. **Performance gate.** If P-12 (full redraw) misses its budget on a Retina Mac at native resolution or on a 4K Linux display, write a new ADR adding a GPU backend for that OS, such as Core Animation/Metal on macOS. Measure first. wgpu is not used unless measurements justify it, because its GPU context and shader compilation work against P-01/P-02.

4. **Windowing and input use `winit` 0.30** on all three OSes: windows, events, DPI and IME. The app draws its own title bar and tab strip.
   - On macOS the native traffic-light buttons are kept, using a transparent title bar with a full-size content view.
   - On Linux, Wayland is first-class and X11 is supported. We draw client-side decorations.
   - The Windows custom title bar must keep Windows 11 snap layouts, through an HWND subclass that answers `WM_NCHITTEST` with `HTMAXBUTTON`. **A spike decides this.** If winit can't support this, Windows uses our own Win32 window code and winit is used on macOS and Linux only.

5. **Helper crates.** Each was checked on crates.io on 2026-10-28. None is a GUI framework: they open windows, rasterize shapes, shape or lay out text, or expose OS APIs. Widgets, layout, styling and interaction remain our own code.

   | Crate | Version | License | Role |
   |---|---|---|---|
   | `windows` | 0.62 | MIT/Apache-2.0 | Win32, COM, D2D, DWrite, DPAPI, WebAuthn, NCrypt bindings |
   | `winit` | 0.30.13 | Apache-2.0 | Windows, input, DPI, IME on all OSes |
   | `softbuffer` | 0.4.8 | MIT/Apache-2.0 | Present a CPU pixel buffer |
   | `tiny-skia` | 0.12 | BSD-3-Clause | 2D rasterizer |
   | `skrifa` / `swash` | 0.49 / 0.2.10 | MIT/Apache-2.0 | Font parsing, outlines, glyph rasterization |
   | `harfrust` | 0.15 | MIT | OpenType shaping (a HarfBuzz port) |
   | `parley` / `fontique` | 0.12 / 0.12 | Apache-2.0/MIT | Text layout and system font fallback (CPU backend) |
   | `objc2` + `objc2-app-kit` | 0.6 / 0.3 | MIT / Zlib/Apache/MIT | macOS APIs not covered by winit |
   | `arboard` | 3.7 | MIT/Apache-2.0 | Clipboard on all OSes |
   | `accesskit` + `accesskit_winit` | 0.25 / 0.34 | MIT/Apache-2.0 / Apache-2.0 | Accessibility: UIA, NSAccessibility, AT-SPI |
   | `unicode-segmentation`, `unicode-width` | 1.13 / 0.2 | MIT/Apache-2.0 | Graphemes and cell widths |

6. **Text service trait.** It covers measure, layout, shape and hit-test. DirectWrite implements it on Windows and parley/harfrust in the CPU backend. Layout unit tests use a deterministic fake text service with fixed advances.

7. **Per-OS conventions where users expect them:** the placement of window controls, the system UI font (Segoe UI Variable, SF Pro, or fontconfig's `sans-serif`), the Cmd vs Ctrl modifier, and menu-bar integration on macOS (App menu with About, Settings and Quit). The design tokens and layout are the same everywhere.

## Consequences

- Two renderers must stay visually aligned. The same fixtures run through both on Windows CI, and the outputs are diffed within a tolerance.
- macOS and Linux get grayscale antialiasing. That is fine on HiDPI screens, and macOS itself dropped subpixel AA in 10.14. Windows keeps DirectWrite's ClearType rendering.
- We live with winit's event-loop model. OS-specific gaps are bridged in `annulus-platform` through raw window handles.
- Linux font discovery depends on fontconfig. AppImage and Flatpak packaging must include or expose it.
- macOS distribution effectively requires Developer ID signing plus notarization. That costs money and is a decision still to make before 1.0.
- `unsafe` stays confined to `annulus-platform` (Win32/COM, Objective-C messaging, Unix PTY).

## Alternatives considered

- **Three native renderers** (D2D, Core Graphics/Core Text, and Linux GL + FreeType). The best per-OS text fidelity, but three times the renderer work and three sets of pixel baselines before 1.0.
- **wgpu everywhere.** One GPU path, but the startup and memory costs hurt P-01/P-02 and the binary grows. It is revisited only with measurements.
- **Hand-written Cocoa/Wayland/X11 windowing.** Full control, but IME, fractional scaling, drag and drop and clipboard on three OSes is months of work with no product value.
- **The CPU backend on Windows too.** Possible, but the maintainer asked for a D2D-style renderer, and DirectWrite gives the best Windows text.

## Decisions

- 2026-10-29: the UI toolkit and its two renderers are settled by [ADR-0017](0017-native-ui.md).
- 2026-10-28: winit is allowed for windows, input, DPI and IME (decided 2026-10-28). The Windows title-bar spike still decides whether Windows uses winit or our own Win32 window.

## Verify

- The winit custom title bar plus snap-layouts spike on Windows 11.
- A hello-window on all three OSes, measuring idle memory, startup and idle wakeups against P-01, P-02 and P-06.
- CPU backend full-redraw time on a Retina Mac and a 4K Linux display against P-12.
