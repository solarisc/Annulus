# ADR-0008: Self-hosted account sync server

- Status: Proposed
- Date: 2026-10-28

## Context

Decision of 2026-10-28: **simple, fully secure account sync to a self-hostable server is the main point of the project.** There are no teams, no sharing and no company cloud. The app also works offline with no account.

What sync must get right:

- It is free and runs on a server the user hosts, not on a vendor's cloud.
- It never loses data: a password reset must not delete the vault, signing out must not wipe local data, and hosts must not vanish. The server never holds credentials it could read.
- The design is published in full: the Argon2 costs, what metadata the server sees, and the conflict policy.

## Decision

### Components

- **`annulus-server`.** A Rust binary built on Tokio, `axum` 0.8 and `rustls` 0.23. It ships as a single static binary per OS and as an official multi-arch Docker image.
- **`annulus-proto`.** Wire types shared by client and server, with an explicit protocol version on every request (`Annulus-Protocol: 1`). Bodies are `postcard`-encoded.
- **`annulus-sync`** (client). It runs the sync engine against `annulus-vault`.

### Server operation

- **Storage:** one SQLite file (WAL). `annulus-server backup <file>` takes a consistent online backup. Postgres is a later option, not v1.
- **TLS:** built in, from a certificate and key file, or run behind a reverse proxy (Caddy or nginx). The server refuses plain HTTP on non-loopback addresses unless it's started with an explicit `--behind-proxy`.
- **Accounts:** multiple separate accounts per server, so a family or a homelab can share one server. Accounts **never** see each other's data. An admin setting `signups = "closed" | "open"` controls registration (default `closed`). While closed, registering needs a single-use invite code from `annulus-server invite [--expires 24h]` (30 days at most; only its hash is stored), sent in a `Annulus-Invite` header; the first account is made with one. The admin CLI lists, disables and deletes accounts and shows storage use.
- **Config:** a TOML file plus environment variables. `/healthz` is the only unauthenticated endpoint besides login and registration. No telemetry. The logs never contain ciphertext, tokens or record ids.

### Authentication: the password never leaves the client

- **OPAQUE** (RFC 9807) through `opaque-ke` 4.x (MIT/Apache-2.0), with **Argon2id as the key-stretching function, run client-side**. The server stores only the OPAQUE registration record. It never receives the password or anything derived from it that would decrypt data.
- **Sessions:** a successful login returns a random 256-bit **device token**. The server stores only its hash. Each device has its own token; one unused for 90 days stops working and its device is removed. The app shows the device list (name, OS, last seen) and can **revoke** any device.
- **Abuse resistance:** login attempts are rate-limited per account and per IP, counted when a login starts (an OPAQUE client can tell a wrong guess without sending a proof) and taken back by a success. Full tables drop their oldest entries, an account's pause last. Names are length-checked before anything is kept. `X-Forwarded-For` is read only with `--behind-proxy`, from the right past private and loopback hops. Login error messages are generic and never say whether an account exists.
- **Password change:** a new OPAQUE registration plus a re-wrapped account key ([ADR-0007](0007-vault-storage-and-encryption.md)). A device token alone is not enough: the same exchange (`/v1/account/password/begin`, then `/change`) carries a fresh OPAQUE login with the current password, made for this account and for a password change, and the new key file must have the vault's key-file shape (4 KiB at most). The records are not re-encrypted. Every other device keeps working after its next unlock with the new password.
- **Optional TOTP second factor for login** comes later (see FEATURES). It protects the ciphertext from download, and is not needed for confidentiality.

### What the server stores, and what it can see

| Stored | Visible to the server |
|---|---|
| OPAQUE record, Argon2 salt and params | Account name (username or email), device names (chosen by the user, editable) |
| Wrapped account key (KEK copy and recovery copy) | Number, sizes and change times of records |
| Records: random id, opaque ciphertext, server sequence number, deleted flag (a deletion is a sealed tombstone too) | Nothing about record contents: no host names, kinds or labels (the kind lives inside the ciphertext envelope) |

This metadata exposure is documented in [SECURITY.md](../SECURITY.md).

### Sync model

- The server assigns a **per-account, strictly increasing sequence number** to every accepted change: an upsert or a tombstone. Each client keeps a cursor and pulls `changes since <seq>`.
- **Push uses optimistic concurrency per record.** A change carries the server sequence it was based on. If the record changed on the server in the meantime, the server answers `409` with the current version. The client then resolves the conflict:
  - by default the newer HLC wins;
  - the losing version stays in the record's history;
  - the user is notified ("db-1 was edited on two devices; kept the newer version. [Compare]").
- The server never merges anything, because it can't read the data.
- **The server's word counts for nothing a client can check.** A deletion is a sealed record with an empty body; a deletion flag with nothing sealed is ignored. A version older (by HLC) than the one a device holds, with no edit pending there, is a rollback: it is not applied, and the device pushes its newer copy back. So an edit that beats a deletion is given a time newer than the deletion, and a restore from the trash is a new edit with a new time.
- **Live updates:** a Server-Sent Events stream announces new sequence numbers, so devices sync within seconds. Polling is the fallback.
- **History and trash on the server.** It keeps the last 20 versions of each record plus every version from 30 days, and keeps tombstones for 30 days. Any device can restore a deleted or overwritten item.
- **Offline first.** Edits queue locally and push when the server becomes reachable. A server outage never blocks connecting to hosts.

### Data-loss rules (hard requirements)

1. **Setup:** creating an account shows the recovery key and asks the user to confirm it.
2. **Logout or unlink:** the local vault stays. Removing it from the device is a separate, explicit action that offers an encrypted export first.
3. **Encrypted export and import:** a single file that can restore into local mode or any server.
4. **Joining an existing account from a device with a local vault** asks: merge, replace (after an export), or keep separate. Nothing is overwritten silently.
5. **Sync never deletes local data** except to apply a tombstone that a user created on some device. That deletion is recoverable from the trash for 30 days.

## Consequences

- One small server to run (a single binary or `docker run`). With no teams, the server is a dumb, encrypted, versioned record store with accounts. That keeps the attack surface small.
- OPAQUE is less familiar than SRP to reviewers. We rely on an audited implementation and document the flow in SECURITY.md.
- Losing both the master password and the recovery key loses the data. That is inherent to E2EE and stated plainly at setup.

## Alternatives considered

- **Folder sync** (OneDrive, Syncthing): offered to the maintainer and declined. It would also need a server for real-time updates and device revocation.
- **SRP6a**: older and has known implementation pitfalls. OPAQUE gives stronger guarantees against precomputation after a server compromise.
- **Bitwarden-style split key:** sends a password-derived hash to the server at every login, which is a password-equivalent verifier. It is simpler, but weaker than a PAKE.
- **CRDT merge** (automerge): field-level merges, but heavy, and the server still can't help. The per-record approach plus history is enough for single-user, multi-device editing.
- **A project-hosted service:** contradicts "self-hosted, free forever" and adds liability.

## Implementation notes (2026-10-29)

- `opaque-ke` 4.0.1 with the `argon2` feature; the suite is ristretto255, 3DH with SHA-512, Argon2id (client-side). It pulls RustCrypto releases older than ours (sha2 0.10, rand 0.8), named `opaque-sha2` and `opaque-rand` in the workspace. Its generic elliptic-curve support brings crypto-bigint 0.5, whose timing advisory (RUSTSEC-2026-0387) does not reach ristretto255; it is ignored in `deny.toml` with that reason.
- `opaque-ke` was audited by NCC Group at an earlier major version; 4.x has no separate audit. It is maintained by its original authors and implements the final RFC 9807.
- Live updates use long polling (`GET /v1/records/wait?since=N`, answered on the next push or after 25 s), the fallback this ADR allows, instead of an SSE stream: the blocking HTTP client stays simple and proxies treat it as plain requests. SSE can replace it later without a protocol break.
- The server is `axum` over plain HTTP for now, bound to loopback unless `--behind-proxy`; built-in TLS came later.

## Verify

- The audit status and maintenance of `opaque-ke` 4.x. If it is unsuitable, fall back to SRP6a (the `srp` crate 0.6) with the same session design, through a superseding ADR.
- The protocol spike. Two clients edit offline, reconnect, conflict, restore from the trash, and revoke a device.
- A server load test of 50 accounts × 5,000 records on a Raspberry Pi 4–class machine. Record memory and CPU in PERFORMANCE.md.
