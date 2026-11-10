# ADR-0024: The Windows programs carry Microsoft's C runtime inside them

- Status: Accepted (2026-11-06)
- Date: 2026-11-06

## Context

By default, Rust on Windows links Microsoft's C runtime as DLLs. Earlier builds imported `VCRUNTIME140.dll` and the `api-ms-win-crt-*` set, all three programs alike. Windows includes the second set, but `VCRUNTIME140.dll` comes from Microsoft's Visual C++ Redistributable, which a fresh Windows may not have. Without it the program doesn't start, and the user sees a system error about a missing DLL. The README tells people to unpack the archive and run `annulus-app`.

## Decision

`.cargo/config.toml` sets `-C target-feature=+crt-static` on Windows with the MSVC toolchain, so every build and test there links the runtime in. The C code follows the same flag: `cc` and `aws-lc-sys` both read it. The release job runs `cargo xtask standalone` over the three programs before it keeps them, and fails if any of them still imports a runtime DLL.

## Consequences

- The archive is all a user needs on Windows.
- Microsoft's fixes to the runtime reach users only in a new Annulus build, not through Windows Update. A security fix in the runtime means a new release.
- The programs grow by the runtime's code. P-16 still limits them to 15 MB.
- `RUSTFLAGS` in the environment replaces the flag, and a build made that way needs the runtime again. The release job would catch that.
- A change to the flags rebuilds everything, so a Windows build cache must be keyed on `.cargo/config.toml` too.

## Alternatives considered

- **`VCRUNTIME140.dll` beside the exe.** One more file to keep with the program, and a DLL loaded from a folder the user can write to.
- **An installer that installs the Redistributable.** There is no installer yet, and the archive is meant to run as it is.
- **Only `VCRUNTIME140.dll` linked in, the rest left to Windows.** It keeps Windows Update's fixes for that part, but takes linker flags that override the toolchain's defaults, for a build that's harder to keep right.
