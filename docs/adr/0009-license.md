# ADR-0009: License

- Status: Accepted (2026-10-28)
- Date: 2026-10-28

## Context

The project is free and open source, and it now has two kinds of deliverable: a desktop **client** and a self-hosted **sync server** ([ADR-0008](0008-self-hosted-sync-server.md)). The license decides whether someone can take the code, especially the server, and run or sell a closed version. Key dependencies (russh, the RustCrypto crates, `windows`, `winit`, `tiny-skia`, `vte`, `accesskit`, `axum`, `opaque-ke`) are MIT, Apache-2.0, BSD or Zlib licensed, and all of them are compatible with every option below.

## Options

| Option | Client | Server and `annulus-proto` | Effect |
|---|---|---|---|
| **A** | MIT OR Apache-2.0 | MIT OR Apache-2.0 | Maximum adoption. Anyone, including companies, can fork closed, and could run a closed hosted service on the server. |
| **B (recommended)** | MIT OR Apache-2.0 | Server: AGPL-3.0-or-later. Proto: MIT OR Apache-2.0 | The client stays easy to reuse. A modified server that is offered to others over a network must publish its source. This is the Vaultwarden model. |
| **C** | GPL-3.0-or-later | AGPL-3.0-or-later | Nobody can ship closed forks of anything. Some contributors and companies avoid GPL code. |

## Decision

**Option B.** The client crates and `annulus-proto` use MIT OR Apache-2.0. `annulus-server` uses AGPL-3.0-or-later. The repository carries `LICENSE-MIT`, `LICENSE-APACHE` and `crates/annulus-server/LICENSE` (AGPL), plus the SPDX fields.

## Consequences

- `annulus-proto` must stay permissive under every option so that both sides can use it.
- `cargo deny` gets a per-crate license policy: the server crate allows AGPL, all other crates stay permissive.
