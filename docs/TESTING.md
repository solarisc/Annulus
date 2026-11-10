# Testing

Last verified: 2026-10-29 @ 51a7d79

A test exists to **fail when a user-visible behavior breaks**. If a test can't fail that way, it is noise. It slows CI, hides real coverage and makes refactors painful. Delete it.

## Rules

**Required**

1. Test behavior through public APIs. A refactor that keeps the behavior must keep the tests green without editing them.
2. Every bug fix starts with a test that reproduces the bug.
3. Every parser of untrusted input has property tests and a fuzz target. That covers:
   - VT streams
   - key files and certificates
   - `ssh_config`, `known_hosts` and PuTTY `.reg` files
   - sync protocol messages, on both client and server
   - vault export files
   - settings TOML
4. Name tests after behavior: `changed_host_key_blocks_connection`, not `test_verify_2`.
5. Tests are deterministic. Async tests run on a paused Tokio clock. Nothing sleeps. Randomness uses seeded generators, and failing seeds are saved as regression cases.
6. Every snapshot (`insta`) is reviewed like code. A snapshot update in a PR must be explained in the PR.

**Banned**

- Tests that only check that a mock was called the way the code calls it.
- Tests of trivial code: getters, `Default` impls, derived traits, constants.
- Testing our dependencies (e.g. "XChaCha20 round-trips"). Test *our* formats instead: tamper, wrong key, swapped AAD.
- Coverage-driven tests. Coverage is a map of untested areas, not a target.
- `#[ignore]` without a linked task ID. Flaky tests are fixed or deleted the same day.
- Sleeping, wall-clock assertions, and real network access outside the integration tier.

**Usefulness check.** Run `cargo mutants` per crate before each milestone closes. A surviving mutant in core logic means a missing test. A test that kills no mutants is reviewed for deletion.

## Test tiers

| Tier | Where | Runs on | What it proves |
|---|---|---|---|
| Unit | `#[cfg(test)]` beside the code | every push, all 3 OSes | Pure logic: layout math, inheritance, key encoding, conflict resolution |
| Property | `proptest` in `tests/` | every push | Invariants over generated inputs (below) |
| Golden / conformance | `annulus-term/tests/conformance/` | every push | Terminal state after recorded byte streams matches reviewed snapshots |
| Differential | `annulus-term/tests/differential.rs` | before a release | Our grid equals `alacritty_terminal`'s for shared-feature fuzz input |
| Integration: SSH | `tests/` in `ssh`, `sftp`, `session` | every push (Linux, Docker) | Real interop with OpenSSH and Dropbear |
| Integration: sync | `annulus-server/tests/`, `annulus-sync/tests/` | every push (all OSes, in-process server) | Real client↔server flows over TLS on loopback |
| Integration: platform | `annulus-platform/tests/` | every push, per OS | PTY, secret store, clipboard and key stores on the real OS |
| UI | `annulus-ui`, `annulus-app` unit tests | every push | Zones, click and focus rules, the text editor, menus, theme contrast, shell state (tabs, navigation) |
| Scenes | `annulus-app` `native::scenes` | every push | Every screen and state of the new UI drawn by the CPU renderer in both themes, at several sizes and scales, driven by real input. Fails on layout faults (cut-off labels, overlapping controls, controls outside the window), on Tab going backwards in reading order, on two zones sharing an id, and on a broken accessibility tree (a node without exactly one parent, a child outside its parent, focus on no node). The PNGs are uploaded as `scenes-<os>` for review ([ADR-0017](adr/0017-native-ui.md)) |
| Screen reader (Windows) | `scripts/ci/uia-tree.ps1` | every push (Windows) | Starts the preview and reads its window through UI Automation, as Narrator and NVDA do: the named controls must be there with their control types, and no password field may expose a value. The dump is uploaded with the scenes |
| End-to-end | `annulus-app/tests/e2e/` | before a release, all OSes | Launch the app, sign in to a test server, connect to a test SSH server, run a command, read the screen |
| Real servers | `annulus-sftp/tests/e2e.rs`, `annulus-sync/tests/e2e_server.rs`, `annulus-app` `e2e_` (ignored by default) | on demand, against the test servers below | Logins, host keys, shells, SFTP, all forwards, jump hosts and keychain keys against stock OpenSSH on Ubuntu |
| Fuzz | `fuzz/` (`cargo fuzz`) | before a release, 10 min per target | No panics or unbounded memory on hostile input |
| Benchmarks | `benches/` (criterion + iai-callgrind) | push (iai gates, Linux), before a release (criterion, all OSes) | The budgets in PERFORMANCE.md hold |

## Test servers (real servers)

Two small Ubuntu 24.04 machines (VMs work well) stand in for real servers: stock OpenSSH, a
user with your key and a test password, password login and TCP forwarding enabled. The second
is the jump-host target, reached through the first. Run the suite with:

```text
ANNULUS_E2E_VM1=<first host> ANNULUS_E2E_VM2=<second host> ANNULUS_E2E_USER=<user> \
ANNULUS_E2E_PASSWORD=… ANNULUS_E2E_KEY=~/.ssh/id_ed25519 \
    cargo test -p annulus-sftp --test e2e -- --ignored --test-threads 1
```

The app's own path (a grouped host resolved to its group's user, key and jump host, then
connected) runs with the same variables: `cargo test -p annulus-app e2e_ -- --ignored`. It
also ends forward-only logins of that user on the first server with `pkill -f '^sshd: <user>$'`.
The servers should hold no real data.

The sync suite needs an `annulus-server` reachable from the test machine, for example on the
first server, listening on loopback and reached through an SSH tunnel (the client allows plain
HTTP to loopback only):

```text
ssh -f -N -L 18443:127.0.0.1:8443 <user>@<first host>
ANNULUS_E2E_SYNC_URL=http://127.0.0.1:18443 cargo test -p annulus-sync --test e2e_server -- --ignored
```

## By crate: what must be tested

**`annulus-term`**

- Conformance: byte streams covering cursor movement, erase, scroll regions, SGR, alt screen, wide and combining characters, emoji, wrap and reflow, tabs, origin mode, insert/delete, OSC handling and DA/DSR replies. Each produces a golden snapshot of the grid text and attributes. The cases are derived from vttest/esctest *behaviors* and written as our own fixtures.
- Real-app recordings: vim, tmux, htop, less, PowerShell (PSReadLine), and shell prompts with OSC 133, replayed to golden screens.
- Properties:
  - The cursor stays in bounds and every row is exactly `cols` wide.
  - `resize(a)` then `resize(b)` preserves the text under reflow.
  - Selection always yields valid UTF-8.
- Limits: hostile OSC, title or grapheme floods stay within the documented caps.

**`annulus-ssh` / `annulus-sftp` / `annulus-session`**

- A Docker compose of OpenSSH (current and previous major), Dropbear, a legacy-algorithms server, two chained jump hosts, a SOCKS5 proxy and an HTTP CONNECT proxy.
- Auth matrix: password, Ed25519/ECDSA/RSA keys, encrypted keys, certificates, keyboard-interactive (2 prompts), agent, and wrong credentials (each with the right error variant).
- FIDO2: a software authenticator (virtual CTAP2 device on Linux CI). Real devices are tested by hand from a checklist at release.
- Host keys: unknown → prompt event; known → silent; changed → `HostKeyMismatch`.
- Forwarding: local, remote and dynamic each move 10 MB both ways byte-exact. A bind conflict is reported.
- Jump chains of 1 and 2 hops, and a jump through a proxy.
- SFTP: list and recursive operations, a 1 GB transfer with checksum, resume after a killed connection, a permission-denied path.
- Resilience: the server drops mid-session → `Closed(reason)`, and auto-reconnect keeps the output. Keepalive detects a dead peer.
- **No hidden exec:** a server that logs every channel request sees only the channels the user configured.

**`annulus-vault`**

- Envelope:
  - A flipped byte fails.
  - Swapped records fail (AAD).
  - A wrong password gives `WrongPassword`, never `Corrupt`.
- The recovery key unlocks after a forgotten password. A password change leaves the records byte-identical and re-wraps only the AK.
- PIN: 5 failures delete the PIN blob. The PIN blob alone (without the device secret) can't unlock.
- Migrations: every committed fixture DB from every schema version opens and migrates.
- Inheritance: a table-driven test for every inheritable field over nested groups, including unset vs empty.
- After `lock()`, no decrypted entity is reachable through the API.

**`annulus-sync` / `annulus-server` / `annulus-proto`**

- **Deterministic simulation:** N clients plus one server. Clients make random edits and deletes and go offline for a while. Delivery is reordered and duplicated, and the server restarts mid-run. **All clients converge to identical state, and no accepted edit is lost.** Each overwritten version is in history. This runs in proptest with seed shrinking.
- **Server never sees secrets:** the integration test captures every byte the server stores and receives, and asserts that the test password, host names and key material never appear.
- Auth: wrong password, unknown account, rate limiting after N failures, a revoked device token, and protocol version mismatch.
- Data-loss rules (ADR-0008): logout keeps the local vault. A join from a device with local data asks for a choice. Restore from the trash works for 30 days on a paused clock.
- `annulus-server backup` produces a DB that a fresh server starts from with identical state.

**`annulus-import`**

- A fixture corpus of anonymized real-world configs, each with an expected preview snapshot. Malformed input yields per-line diagnostics and never a panic.

**`annulus-ui`**

- Layout: table-driven geometry tests (constraints → rects).
- Interaction: simulated input (click, type, Tab, arrows, shortcuts, IME) → view-model state and focus.
- Display-list snapshots of each widget in each state, in light and dark.
- Accessibility: every interactive widget has a role and a name, and the Tab order matches the visual order.

**`annulus-app`**

- The command registry has no duplicate default chords per context and OS. Every command has a title and a palette entry. In terminal focus on Windows/Linux, no default app chord uses bare `Ctrl+<letter>` (UI-SPEC §Shortcuts).
- Flows run through view models with fake sessions: connect → host-key prompt → accept → connected; edit host → list updates → change queued for sync.
- **Secret hygiene:** a full connect plus sync with a known password, after which the captured `tracing` output contains no secret.

## CI matrix

- **Windows (latest):** fmt, clippy, unit/property/golden/UI tests, pixel tests (CPU and D2D/WARP), platform integration, the in-process sync integration.
- **macOS (latest, arm64):** unit/golden/UI tests, CPU pixel tests, platform integration (PTY, Keychain), the sync integration.
- **Linux (Ubuntu LTS):** everything portable, plus the Docker SSH suite, iai-callgrind gates, `cargo deny`, `cargo machete`, docs-check, and the server container build.
- **Nothing runs on a schedule.** Before a release, by hand: fuzzing, the differential terminal test, e2e on all OSes, criterion benches, and `cargo mutants` per crate.
