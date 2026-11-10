# ADR-0013: Local shells on macOS and Linux

- Status: Proposed
- Date: 2026-10-28

## Context

[ADR-0006](0006-local-shells-via-conpty.md) covers Windows (our own ConPTY wrapper). With macOS and Linux now in 1.0 scope ([ADR-0012](0012-three-platforms-two-renderers.md)), local terminals need a Unix pseudo-terminal too. `portable-pty` (0.9.0, MIT, last release 2025-02) would cover both families. ADR-0006 rejected it when only Windows mattered, so it is re-evaluated here.

## Decision

- `annulus-platform` implements a small Unix PTY module with `rustix` (1.1, Apache-2.0/MIT):
  - `openpt`, `grantpt`, `unlockpt` and `ptsname`
  - termios setup
  - `TIOCSWINSZ` for resize
- The child is spawned with `std::process::Command` and a `pre_exec` hook (`setsid` plus `TIOCSCTTY`). It is `unsafe` and documented.
- The master fd is non-blocking and pumped by the I/O runtime.
- Shell discovery uses the user's login shell (`$SHELL`, falling back to the passwd entry), the list in `/etc/shells`, and custom profiles from settings. Login vs interactive is a per-profile setting. The default is a login shell on macOS (the platform convention) and interactive on Linux.
- Both the Windows and Unix PTYs implement the same `Pty` trait in `annulus-platform::api`, so `annulus-session` doesn't know which one it has.

## Consequences

- About 300 lines of platform code, with integration tests on macOS and Linux CI:
  - spawn `sh -c 'echo ok'`
  - resize, then check `stty size`
  - exit status
  - SIGHUP when the tab closes while output is still flowing
- One set of PTY semantics (resize, close, exit), defined by our trait and tested identically on all three OSes.

## Alternatives considered

- **`portable-pty`.** It works, but it pulls in its own process and fd abstractions plus serial support we don't need, and we would still own ConPTY on Windows. Rejected for the same reasons as in ADR-0006.
- **`nix` instead of `rustix`.** Both are fine. `rustix` avoids libc in many paths and is already in most dependency trees.
