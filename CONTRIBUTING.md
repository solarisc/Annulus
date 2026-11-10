# Contributing

Last verified: 2026-11-05 @ 51a7d79

Thanks for wanting to help. Here's how to get going, and then the house rules, which apply to humans and agents alike.

## Getting started

You need [Rust](https://rustup.rs) and a C compiler; on Linux, also `build-essential` and `pkg-config`. Rustup installs the pinned toolchain by itself.

```bash
cargo run -p annulus-app --release -- --preview   # the app over a throwaway demo vault
cargo test --workspace                            # the tests
cargo xtask ci                                    # everything CI checks; needs cargo-deny and cargo-machete
```

Plain `cargo run -p annulus-app` opens your real vault, so use `--preview` while you're working on the UI.

Before starting on something large, please open an issue so we can talk it through. [FEATURES.md](docs/FEATURES.md) is the project's scope: what isn't listed there is out of scope until it's added, and team sharing, mobile apps, a built-in AI assistant, Telnet, serial and Mosh are out on purpose. [docs/README.md](docs/README.md) maps the rest of the docs, and [DESIGN.md](docs/DESIGN.md) explains how the crates fit together.

## Principles

These hold for every change:

1. **Rust only,** on the stable toolchain pinned in `rust-toolchain.toml`, edition 2024.
2. **Our own UI, no GUI framework** ([ADR-0017](docs/adr/0017-native-ui.md)): the toolkit is `annulus-ui`, the renderers and windows are in `annulus-platform` (Direct2D on Windows, the CPU renderer elsewhere). No egui or other GUI framework, no webviews.
3. **Performance budgets are requirements** ([PERFORMANCE.md](docs/PERFORMANCE.md)). A change that breaks one does not merge.
4. **Original work only** ([ADR-0010](docs/adr/0010-original-work-only.md)).
5. **`unsafe` only in `annulus-platform`,** and every `unsafe` block carries a `// SAFETY:` comment.
6. **No telemetry,** in the client or the server.
7. **The server can never read user data.** Encryption happens on the client; login uses OPAQUE ([ADR-0008](docs/adr/0008-self-hosted-sync-server.md)).
8. **Never lose user data silently** ([ADR-0008](docs/adr/0008-self-hosted-sync-server.md)).

## Quality bar

This is meant to be the best-engineered SSH client available, and it is read by strangers. Optimize for the reader:
small modules with one responsibility, names that say what things are, and types that make invalid states
unrepresentable. When in doubt, choose the boring and obvious solution, then measure.

## Toolchain and formatting

- Stable Rust, pinned in `rust-toolchain.toml`. Bump it in a dedicated commit after CI passes on the new version.
- `cargo fmt` with the repository `rustfmt.toml`. Only stable rustfmt options are allowed, so formatting never needs nightly.
- Edition 2024 across the workspace.

## Lints

Workspace lints live in the root `Cargo.toml` and every crate inherits them with `[lints] workspace = true`:

| Lint | Level | Why |
|---|---|---|
| `rust::unsafe_code` | deny (crates add `#![forbid]`, except `annulus-platform`) | Keeps memory-safety review confined to one crate |
| `rust::missing_docs` | warn | Public API of every library crate is documented |
| `clippy::pedantic` | warn (priority -1) | High baseline; individual noisy lints are allowed with a reason |
| `clippy::unwrap_used`, `clippy::expect_used` | deny outside tests | Remote input must never crash the app |
| `clippy::panic`, `clippy::todo`, `clippy::unimplemented`, `clippy::dbg_macro` | deny | Same |
| `clippy::print_stdout`, `clippy::print_stderr` | deny (except `xtask`) | Use `tracing` |
| `clippy::undocumented_unsafe_blocks` | deny | Every `unsafe` explains why it is sound |
| `clippy::module_name_repetitions` | allow | Too noisy for little value |

CI runs clippy with `-D warnings`. A local `#[allow(...)]` needs a comment that says why. `clippy.toml` relaxes the unwrap, expect, panic, print and dbg lints inside `#[test]` code only.

## Errors

- Library crates define error enums with `thiserror` at their public boundary. No `anyhow` in libraries.
- `anyhow` is allowed only in `xtask` and in test helpers.
- No `unwrap`/`expect` in non-test code. If an invariant truly cannot fail, use `#[allow(clippy::expect_used)]` with a comment that proves it.
- **Any panic reachable from bytes sent by a remote peer, a file we parse, or the clipboard is a security bug.** Fuzz those paths (see [TESTING.md](docs/TESTING.md)).
- User-facing error text is written in the app layer, never formatted from a library's `Display` output.

## Logging

- Use `tracing`. Spans per connection and session carry a non-secret `session_id`.
- Secrets use wrapper types (`Secret<T>`) whose `Debug` and `Display` print `[redacted]`. A test asserts that a full connect flow never writes the test password to the log ([SECURITY.md](docs/SECURITY.md)).
- Nothing is logged to disk by default. A user-enabled debug log goes to `logs/` in the per-OS data folder ([ADR-0007 §Data locations](docs/adr/0007-vault-storage-and-encryption.md)) and rotates.
- The server logs requests without bodies, tokens or record ids.

## Comments and docs

- Doc comments on every public item: what it is, what it guarantees, and any errors or panics.
- Inline comments explain *why*, never *what*. Delete a comment that restates the code.
- Every module starts with `//!` docs that state its single responsibility.
- No commented-out code. Git remembers.
- `TODO`/`FIXME` must name a task or feature ID: `// TODO(F-SSH-005): support ProxyJump with IPv6 literals`.

## Structure

- Dependency direction is enforced by the crate graph ([DESIGN.md §Crates](docs/DESIGN.md#4-crates)). Pure-logic crates (`term`, `vault`, `import`, `config`, `sync`, `proto`) never depend on UI or platform crates. `annulus-server` never depends on client crates.
- Platform-specific code lives only in `annulus-platform`, behind `#[cfg(target_os = ...)]` modules that implement the traits in `annulus-platform::api`. No `cfg(target_os)` anywhere else.
- Keep files under about 500 lines. Split by responsibility, not alphabetically.
- No blocking calls inside async code. No async inside the UI thread. Threads talk through channels ([DESIGN.md §Threading](docs/DESIGN.md#5-threading-model)).
- Prefer plain data plus functions over trait hierarchies. Add a trait when there is a second implementation, or to let a test substitute one.

## Dependencies

- Prefer `std`, then crates already in the tree, then new crates.
- A new crate needs a justification line in the commit message: why it's needed, its size cost, its license, and how well it is maintained.
- `cargo deny check` enforces the license allow-list (MIT, Apache-2.0, BSD-2/3, BSL-1.0, ISC, Zlib, MPL-2.0, Unicode-3.0, CC0-1.0; OFL-1.1 and the Ubuntu Font Licence for bundled fonts) and the RustSec advisories. No GPL/LGPL/AGPL dependencies in either binary. The server's *own* license is a separate question ([ADR-0009](docs/adr/0009-license.md)).
- `cargo machete` must report no unused dependencies.
- Pin direct dependency versions in `[workspace.dependencies]`. Crates inherit them with `foo.workspace = true`.

## Tests

[TESTING.md](docs/TESTING.md) is mandatory reading. In short: test behavior through public APIs, cover parsers with property tests and fuzzing, run integration tests against real servers, and don't write tests that cannot fail.

## Commits and pull requests

- Use Conventional Commits: `feat(ssh): add jump host chaining`, `fix(term): clamp cursor after DECSTBM`.
- Add a footer for the feature: `Refs: F-SSH-014`.
- Docs updates go in the same commit as the code that makes them necessary.
- The PR description answers the [Definition of done](#definition-of-done), with every point met or explained.
- `main` is always releasable: CI is green, there is no half-finished feature without a setting that keeps it off, and no failing tests are skipped.

## Original work only

Never paste another product's code, text, icons, screenshots or palette values into this repository. Design from our own [UI-SPEC.md](docs/UI-SPEC.md). See [ADR-0010](docs/adr/0010-original-work-only.md).

## Definition of done

- Behavior is covered by tests that fail when the behavior breaks.
- `cargo xtask ci` passes, including `cargo clippy --workspace --all-targets -- -D warnings`.
- No budget in [PERFORMANCE.md](docs/PERFORMANCE.md) regressed.
- UI work has keyboard access, a focus ring and accessibility roles ([UI-SPEC.md §Accessibility](docs/UI-SPEC.md#accessibility)).
- Secrets are never logged, never stored in plaintext and are zeroized after use ([SECURITY.md](docs/SECURITY.md)).
- Docs that the change makes untrue are fixed in the same change, and the touched rows in [FEATURES.md](docs/FEATURES.md) are updated.
