# ADR-0030: Capture and blur the view below a modal

- Status: Accepted (2026-11-08)
- Date: 2026-11-08

## Context

The command palette must blur the live app view behind it. Annulus has two renderers. The CPU renderer owns pixels it can change in place. Direct2D cannot read from the target while it draws to that target. Copying the window back from the screen would be slow and could include pixels from another window.

## Decision

Add backdrop capture calls to `Canvas`. The app starts a capture before it draws the main view and ends it just before it draws the modal.

The CPU renderer keeps its normal path. On capture end, it runs a two-pass box blur over the requested bounds with one reused row or column buffer.

Direct2D changes its target only while capture runs. It draws the main view into a low-resolution compatible bitmap target, restores the window target, and scales that bitmap up with linear sampling. It then draws the modal on the window target. A frame with no backdrop capture takes the old path.

The app pairs the blur with a dark scrim. It marks the view below as inert and removes it from the accessibility tree while the modal stays open.

## Consequences

- Both renderers expose the same modal rule.
- The Direct2D path allocates a window-sized target only while the palette opens or stays open.
- The CPU path does work only inside the requested bounds and reuses a line-sized buffer.
- The scaled Direct2D capture is a soft blur, not a full Gaussian blur.
- Tests cover bounds, softened edges and the dialog-only accessibility tree.

## Alternatives considered

- A translucent scrim alone did not meet the requested look.
- Reading the finished window back would stall the GPU and could capture other windows.
- A platform blur effect would split the two renderers and make scene tests differ.
