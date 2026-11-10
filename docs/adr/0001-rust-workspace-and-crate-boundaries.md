# ADR-0001: Rust workspace and crate boundaries

- Status: Accepted
- Date: 2026-10-28

## Context

The project requires Rust, top performance, minimal resource use and the highest code quality. The app has clearly separable concerns: the UI toolkit, the OS layer, terminal emulation, protocols, the encrypted data model, sync and importers. Most of them are pure logic that can be tested without a window or a network.

## Decision

- One Cargo workspace, edition 2024, stable toolchain pinned in `rust-toolchain.toml`.
- Crates and allowed dependencies are listed in [DESIGN.md §Crates](../DESIGN.md#4-crates). The direction is one-way: pure-logic crates never depend on `annulus-ui`, `annulus-platform` or `annulus-app`.
- `unsafe` code is allowed only in `annulus-platform`, which wraps Win32, COM, Direct2D, DirectWrite, ConPTY and DPAPI behind safe APIs. Every other crate has `#![forbid(unsafe_code)]`.
- `xtask` (a workspace binary) holds the developer automation, so CI and local runs execute the same code.
- The release profile uses `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` and `strip = "symbols"`. Debug info goes in separate PDBs for symbolication.

## Consequences

- Compile times are higher than with one crate, but incremental builds only touch the changed crate.
- Pure crates build and test on Linux CI, so protocol integration tests can use Linux containers.
- The safe wrappers in `annulus-platform` need careful review. That review happens in one place.
- With `panic = "abort"`, a panic kills the whole app. That is acceptable only because panics on remote input are treated as bugs and fuzzed ([TESTING.md](../TESTING.md)).

## Alternatives considered

- **Single crate.** Simpler, but nothing enforces the boundaries, and `unsafe` can spread anywhere.
- **Many tiny crates (one per protocol feature).** Build-graph overhead with no real isolation benefit.
