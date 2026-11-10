# ADR-0016: alacritty_terminal for emulation and local PTYs

- Status: Accepted (2026-10-28)
- Date: 2026-10-28
- Supersedes: an earlier plan for our own screen model on `vte`. Also replaces the own ConPTY and Unix PTY wrappers planned in [ADR-0006](0006-local-shells-via-conpty.md) and [ADR-0013](0013-local-shells-on-unix.md) for local shells.

## Context

The earlier plan was our own screen model on `vte` (about 4–6k lines plus conformance and fuzz suites) and kept `alacritty_terminal` only as a test oracle, partly because the maintainer wanted core pieces in-house. On 2026-10-28 the maintainer chose the proven terminal core for emulation.

## Decision

- `annulus-term` wraps **`alacritty_terminal`** (0.26, Apache-2.0): its `Term` is the screen model (xterm compatibility, scrollback, reflow, selection, search). `annulus-term` adds what is ours: the color palette, a plain snapshot of the visible screen for the terminal view, and key encoding.
- Local shells use `alacritty_terminal::tty` (ConPTY on Windows, `openpty` on Unix) and its PTY event loop thread, owned by `annulus-session`. No `unsafe` of ours is needed for PTYs.
- Until our own SSH stack landed, quick connect ran the system OpenSSH client in a local PTY. Since then it uses `annulus-ssh`; `Program::system_ssh` remains as a fallback for targets Annulus cannot parse.

## Consequences

- A daily-usable terminal in days. Upstream correctness and performance (Alacritty's) instead of ours to prove.
- Cells are Alacritty's size (larger than the 8-byte target); P-05 is re-measured with it and adjusted in PERFORMANCE.md if needed.
- The differential test planned in TESTING.md is moot; our tests cover the layers we add (palette, snapshot, key encoding, sessions).

## Alternatives considered

- **Own model on vte.** Full control, weeks more work; rejected for speed.
- **portable-pty for PTYs.** Fine, but alacritty_terminal already ships PTYs that match its event loop.
