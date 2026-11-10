# Design

Last verified: 2026-11-08 @ 51a7d79

This document describes **how** Annulus is built. **What** it does is defined in [FEATURES.md](FEATURES.md), which is the scope, and in [UI-SPEC.md](UI-SPEC.md). **Why** each major choice was made is in the [ADRs](adr/README.md). If this document and the code disagree, fix one of them in the same change.

## 1. Goals and non-goals

**Product in one sentence:** a focused, reliable desktop app for local terminals and for managing SSH connections, with secure account sync to a server you host yourself.

**Goals (1.0 on Windows, macOS and Linux)**

1. A familiar workflow for SSH: hosts, groups, keychain, SFTP, port forwarding, jump hosts, snippets, split view. It uses a conventional layout with our own visual identity ([ADR-0010](adr/0010-original-work-only.md)).
2. **Self-hosted, end-to-end encrypted account sync** across your desktops. This is the core differentiator ([ADR-0008](adr/0008-self-hosted-sync-server.md)).
3. **Every SSH authentication method and key type** in common use ([FEATURES.md §Keychain](FEATURES.md#keychain-every-common-auth-method-and-key-type-f-key)).
4. Resource use an order of magnitude below Electron clients ([PERFORMANCE.md](PERFORMANCE.md)).
5. No way to lose data silently ([ADR-0008 §Data-loss rules](adr/0008-self-hosted-sync-server.md#data-loss-rules-hard-requirements)).
6. A codebase a stranger can read, test and extend.

**Non-goals (decided 2026-10-28; see [FEATURES.md §Out of scope](FEATURES.md#out-of-scope))**

- Teams, shared vaults, roles, SSO, audit logs, live session sharing.
- Mobile apps.
- Telnet, Serial, Mosh ([ADR-0011](adr/0011-mosh-telnet-serial.md)).
- AI features.
- Cloud-provider host import (AWS, DigitalOcean, Azure).
- A project-hosted sync service.

## 2. Principles

1. **Measure, then optimize.** Every budget has a benchmark.
2. **Idle means idle.** No polling, no redraw timers and no background work unless the user asked for it.
3. **Pure core, thin shell.** Logic lives in crates without I/O or platform calls and is tested exhaustively. Platform glue is thin.
4. **Remote input is hostile.** Bytes from servers, files, the sync server and the clipboard are parsed defensively and fuzzed.
5. **One registry per concept.** Commands, settings, themes and feature IDs each have exactly one definition.
6. **Small scope, done well.** A feature not in FEATURES.md is not built.

## 3. System overview

```
┌──────────────────────────────── annulus-app (desktop binary) ───────────────────────────────┐
│ App state + view models · command registry · windows/tabs/panes · lock screen · i18n       │
└──────┬───────────────────────┬──────────────────────────┬───────────────────────────────────┘
       │ widget tree           │ session commands          │ entity reads/writes
┌──────▼───────┐   ┌───────────▼─────────────┐   ┌─────────▼────────┐   ┌──────────────┐  HTTPS   ┌────────────────┐
│ annulus-ui   │   │ annulus-session          │   │ annulus-vault    │◄──┤ annulus-sync │◄────────►│ annulus-server │
│ widgets,     │   │ Session = SSH | Local    │   │ entities, crypto,│   └──────┬───────┘          │ (self-hosted)  │
│ layout, text,│   │  ├─ annulus-ssh (russh)  │   │ SQLite, history  │          │ annulus-proto     └───────┬────────┘
│ input, focus,│   │  ├─ annulus-sftp         │   └──────────────────┘          └───────────────────────────┘
│ a11y,        │   │  └─ annulus-term (VT)    │   ┌──────────────────┐   ┌──────────────────┐
│ display list │   └───────────┬──────────────┘   │ annulus-config   │   │ annulus-import   │
└──────┬───────┘               │ PTY, secrets,    │ settings, themes,│   │ ssh_config, PuTTY│
       │ display list          │ FIDO, agent      │ keymaps, tokens  │   └──────────────────┘
┌──────▼───────────────────────▼──────────────────┴──────────────────┘
│ annulus-platform (the only crate with unsafe)
│ Windows: D2D/DWrite renderer · ConPTY · DPAPI · Hello · WebAuthn · NCrypt
│ macOS/Linux: CPU renderer (tiny-skia + swash) · Unix PTY · Keychain/Secret Service · CTAP2 HID
│ all: winit windows + input + IME · arboard clipboard · AccessKit
└──────────────────────────────────────────────────────────────────────
```

## 4. Crates

| Crate | Responsibility | May depend on | `unsafe` | License |
|---|---|---|---|---|
| `annulus-app` | Desktop binary. App state, view models, command registry, window/tab/pane management, wiring | all client crates | no | client |
| `annulus-ui` | Our toolkit ([ADR-0017](adr/0017-native-ui.md)): geometry, the `Canvas` renderers implement, zones, input, focus, animation, the theme from the design tokens, widgets, our icons, and the AccessKit tree built from each frame's zones | `annulus-config` (tokens) | no | client |
| `annulus-platform` | The renderers (CPU; Direct2D on Windows) and windows of our UI (with their AccessKit adapters), and platform implementations: secret stores, biometrics, FIDO2, hardware-bound keys | `annulus-ui` (types) | **yes** | client |
| `annulus-term` | VT parser glue (`vte`), screen and scrollback model, selection, search, reflow, key encoding. Bytes in, state out | none of ours | no | client |
| `annulus-session` | `Session` over SSH and local PTY. Reconnect, logging hooks, broadcast fan-out | `annulus-ssh`, `annulus-term` | no | client |
| `annulus-ssh` | Connection resolution, transport (russh), auth pipeline, host-key verification, jump chains, proxies, forwarding, built-in agent | `annulus-vault` (types) | no | client |
| `annulus-sftp` | SFTP operations and the transfer queue | `annulus-ssh` | no | client |
| `annulus-vault` | Entities, inheritance resolution, encryption, SQLite storage, history, migrations | none of ours | no | client |
| `annulus-sync` | Client sync engine: push/pull, conflict resolution, cursors, SSE | `annulus-vault`, `annulus-proto`; `annulus-server` in tests only (they sync through the real server logic) | no | client |
| `annulus-proto` | Versioned wire types shared by client and server | none of ours | no | **permissive always** |
| `annulus-server` | Self-hosted sync server: accounts, OPAQUE auth, devices, record store, history/trash, admin CLI | `annulus-proto` | no | server ([ADR-0009](adr/0009-license.md)) |
| `annulus-import` | Importers: OpenSSH config (+ known_hosts, keys), PuTTY sessions | `annulus-vault` (types) | no | client |
| `annulus-config` | Device settings schema, terminal themes, keymaps, design tokens, hot reload | none of ours | no | client |
| `annulus-control` | The local control door ([ADR-0020](adr/0020-local-control-channel-for-cli-and-ai.md)): its line protocol, the server the app opens and the client the CLI uses | none of ours | no | client |
| `annulus-cli` | The `annulus-agent` command: drives a running, unlocked app through the control door | `annulus-control` | no | client |
| `annulus-setup` | The Windows installer ([ADR-0027](adr/0027-client-and-server-ship-apart.md)): puts the app and `annulus-agent` in place for the current user, with shortcuts, PATH and an Apps & features entry, and removes them again | `annulus-ui`, `annulus-platform` (the window, the registry and shortcuts), `annulus-config` (tokens) | no | client |
| `xtask` | Dev only: ci, docs-check, the release downloads (`dist`), the color scheme import, the app icon and the README's logo, the macOS bundle, the check that Windows programs stand alone | `annulus-ui` (the mark) | no | — |

Rules:

- The crate graph is acyclic and follows the table. `cargo xtask docs-check` verifies that the `Cargo.toml` dependencies match the "May depend on" column.
- All crates except `annulus-platform` and `annulus-app` compile and test on every OS, with no window or display.
- `annulus-server` never depends on client crates. It cannot decrypt anything by construction: it has no crypto beyond OPAQUE and TLS.

## 5. Threading model

See [ADR-0005](adr/0005-threading-and-async-runtime.md).

```
UI (main) thread: event loop            I/O runtime (Tokio, 2 workers)            blocking pool
────────────────────────────            ──────────────────────────────            ─────────────
input → commands ──mpsc───────────────► session tasks: read socket/PTY             Argon2id
                                          └─► term.lock().advance(bytes)           SQLite
render ◄── wake (coalesced) ◄──────────── dirty flag (atomic) + event-loop wake    file I/O
  └─ lock term, copy damaged rows,        sync task: SSE stream, push/pull
     unlock, draw
```

## 6. Data flow

**Keystroke to remote.** The OS key event (winit) reaches `annulus-ui`, which routes it to the focused terminal widget. `annulus-term` encodes it, honoring the application cursor/keypad modes and modifyOtherKeys. The bytes go out as `SessionCommand::Write`, and the session writes them to the channel or PTY. If broadcast is on, the app fans the bytes out to every session in the broadcast group.

**Remote to screen.** The session task reads bytes and calls `term.advance(bytes)` under the lock. The model marks damaged rows and queues DA/DSR replies, which the task writes back. Then the dirty flag flips and the event loop wakes. On the next frame the UI copies the damaged rows and redraws only those rectangles.

**Vault edit to other devices.** The editor view model validates the change and calls `vault.upsert(entity)`. The entity is updated in memory, encrypted, and written to SQLite with the previous version moved to history. The sync engine then queues the change and pushes it to the server, which assigns a sequence number and announces it over SSE. The other devices pull, decrypt and apply it, and their observers re-render the affected list items.

## 7. Subsystems

### 7.1 Platform layer (`annulus-platform`)

> **Superseded in part by [ADR-0017](adr/0017-native-ui.md):** on Windows the window is our own Win32 window drawn by Direct2D/DirectWrite (it answers UI Automation through AccessKit); elsewhere it is `winit` + `softbuffer` drawn by the CPU renderer (AT-SPI or macOS accessibility through AccessKit). The rest of this section is the platform layer's other duties.

See [ADR-0012](adr/0012-three-platforms-two-renderers.md).

| Concern | Windows | macOS | Linux |
|---|---|---|---|
| Window and input | winit (custom title bar, snap layouts via HWND subclass; own Win32 fallback) | winit, transparent title bar, native traffic lights, our items in the menu bar's app menu | winit, Wayland first-class + X11, client-side decorations |
| Renderer | Direct2D 1.1 + DirectWrite, DXGI flip model, dirty rects | CPU backend + softbuffer | CPU backend + softbuffer |
| UI text | DirectWrite | parley + harfrust + fontique (Core Text fonts) | parley + harfrust + fontique (fontconfig) |
| Local PTY | ConPTY ([ADR-0006](adr/0006-local-shells-via-conpty.md)) | Unix PTY ([ADR-0013](adr/0013-local-shells-on-unix.md)) | Unix PTY |
| Device secret store | DPAPI | Keychain | Secret Service (optional) |
| Biometric unlock | Windows Hello | Touch ID | — |
| FIDO2 | WebAuthn API | CTAP2 over HID | CTAP2 over HID (udev rules) |
| Hardware-bound SSH keys | TPM via NCrypt | Secure Enclave (P-256) | — (use FIDO2) |
| SSH agent | OpenSSH pipe, Pageant | `SSH_AUTH_SOCK` | `SSH_AUTH_SOCK` |
| Accessibility | AccessKit → UIA | AccessKit → NSAccessibility | AccessKit → AT-SPI |
| Clipboard | arboard | arboard | arboard (Wayland + X11) |

### 7.2 UI toolkit (`annulus-ui`)

> **Superseded in part by [ADR-0017](adr/0017-native-ui.md):** the toolkit is immediate mode, not the retained tree below. Each frame the app draws the screen and declares zones (rect, role, accessible name, state); input is resolved against the previous frame's zones, focus follows draw order, and the accessibility tree is built from the zones (`annulus_ui::access`). There are no dirty bits: a frame is drawn only on input or when asked (P-06).

- **Tree:** an arena of nodes with generational ids. Each node holds a widget kind, style, layout result, an accessibility role/name/state, and dirty bits (`LAYOUT`, `PAINT`).
- **State flow:** view models in `annulus-app` own the state. Widgets emit typed messages, and the view models update and mark the affected nodes dirty. There is no global virtual-DOM diff. Lists are keyed and virtualized, so only visible rows exist as nodes.
- **Layout:** our own flexbox subset (row/column, gap, padding, fixed/fill/fit sizes, min/max, align/justify), absolute overlays for popovers and side panels, scroll containers, and wrapping card grids. It is a pure function of the tree plus constraints and is unit-tested without a window.
- **Paint:** each widget emits display-list ops: `Rect`, `RoundedRect`, `Border`, `Shadow`, `TextRun`, `Icon(PathId)`, `Image`, `PushClip`/`PopClip`, `PushLayer(opacity)`/`PopLayer`. The display list is plain data, so snapshot tests run anywhere.
- **Widgets for v1:**
  - Controls: button (variants), icon button, split button, text field, password field, text area, search field, checkbox, switch, radio, select, segmented control, slider.
  - Collections: list, virtualized card grid, tree (groups), tabs (draggable), split pane (resizable, up to 16 panes).
  - Containers and overlays: side panel, modal dialog, popover/menu, context menu, tooltip, toast, banner.
  - Indicators: progress bar, spinner, badge, tag chip, status dot, color swatch.
  - Special: key-capture field, file-path field, and the terminal view.
- **Text editing:** a shared editor core covering grapheme-aware movement, selection, undo/redo, IME composition and clipboard. Secrets live in zeroizing buffers.
- **Animation:** time-based tweens and springs on paint-only properties, and layers that move, scale and fade what they hold ([ADR-0021](adr/0021-quiet-look-and-choreographed-motion.md)). Something on its way out is drawn inert: it shows, and takes no input. A frame is requested only while an animation runs. The OS "reduce motion" setting is honored. A modal may capture the view below it and blur that capture through `Canvas`; the CPU renderer blurs its current pixels, while Direct2D draws the view to a short-lived compatible target and then blends it back ([ADR-0030](adr/0030-command-palette-backdrop-blur.md)).
- **Theming:** design tokens in `annulus-config`. Light and dark themes can follow the system ([UI-SPEC.md §Design tokens](UI-SPEC.md#8-design-tokens)).
- **Accessibility:** every widget maps to an AccessKit role. The terminal view exposes its text through a document role with text ranges. Focus order is explicit and testable.
- **Strings:** every user-visible string comes from a string table keyed by id. v1 ships English only.

### 7.3 Terminal emulation (`annulus-term`)

See [ADR-0016](adr/0016-alacritty-terminal-core.md).

- **API:** `advance(&[u8])`, `resize`, `visible_rows`, `damage`, `take_replies`, `take_events` (title, bell, clipboard write request, cwd, prompt mark, hyperlink), `selection_*`, `search(regex)`, `encode_key(event)`.
- **Safety limits:** caps on OSC payload length, grapheme cluster length, title length and reply queue size.
- **Shell integration:** OSC 133 prompt and command marks and OSC 7 cwd, when the shell emits them. Autocomplete (§7.12) and workspace restore build on these.

### 7.4 Rendering

> **Superseded in part by [ADR-0017](adr/0017-native-ui.md):** the terminal is drawn by our own renderers through the `Canvas` interface (`native::terminal`), with the rules below.

- **Terminal:**
  - The visible grid is copied into runs of the same style.
  - Backgrounds are drawn as merged rectangles.
  - Text is drawn per style run: DirectWrite glyph runs on Windows, cached swash glyph bitmaps on the CPU backend.
  - Box-drawing, block-element and Powerline glyphs are drawn procedurally.
  - The cursor, selection, search highlights and hyperlink underlines are overlays.
  - Ligatures are optional and off by default.
- **UI:** the display list is drawn per damaged rectangle.
- **Fonts:** the bundled default terminal font is JetBrains Mono (OFL-1.1) plus a symbols fallback. The pinned font and curated Nerd symbol licenses ship in `LICENSE-FONTS` ([ADR-0029](adr/0029-private-fonts-and-nerd-symbols.md)). App and terminal families can be selected independently or privately imported. Font discovery and file I/O run on a worker; renderers consume registered bytes and invalidate layout caches on changes. UI text uses the platform default until overridden.

### 7.5 Sessions (`annulus-session`)

- `Session` is an async task that owns one transport plus one `Terminal`. It accepts `SessionCommand` (Write, Resize, Close, Reconnect) and emits `SessionEvent` (Connected, OutputDirty, NeedsInput, Closed(reason), Error).
- Transports: `SshShell` and `LocalPty`. Nothing else in v1.
- **Reconnect:** on an unexpected drop, auto-reconnect with backoff is on by default and can be turned off per host. The old output stays, with a visible "reconnected" separator. A manual Reconnect action is always available.
- **Session logs** (§7.11) tap the session's output stream.
- **Broadcast:** a group of sessions receives the same input bytes. The app enforces that broadcast never includes a session the user didn't select.

### 7.6 SSH (`annulus-ssh`)

See [ADR-0004](adr/0004-ssh-stack-russh.md).

1. **Resolve.** `Host` plus group inheritance plus defaults gives a `ResolvedTarget { hops, proxy, auth_candidates, options }`. This is a pure function with exhaustive tests.
2. **Connect.** TCP (or proxy, or ProxyCommand), then the SSH handshake, then the host-key check against known hosts.
   - An unknown key prompts the user with its SHA256 fingerprint and key type.
   - A **changed key is a hard stop**. The user can only continue through an explicit "remove old key, then trust new key" flow.
3. **Authenticate.** Candidates are tried in configured order:
   - built-in agent and vault keys (including certificates, FIDO2 and hardware-bound keys)
   - the OS agent, if enabled
   - password
   - keyboard-interactive prompts, rendered by the UI
   The UI can also ask which method to use at connect time. A failure reports what was tried and what the server offered. We never brute-force every key on disk.
4. **Jump chain:** each hop's `direct-tcpip` channel is the transport for the next.
5. **Channels:**
   - shell with a PTY request (TERM, size, modes)
   - environment variables
   - startup snippet
   - local, remote and dynamic forwarding
   - agent forwarding (off by default, per host)
   - the SFTP subsystem
6. **No hidden exec probes.** Extra exec channels for OS detection and history import can drop connections on some appliances. We guess the host OS from the server's SSH banner only, and run nothing the user didn't configure. The one exception is a keychain action the user starts (install a key on a host, or take it off): it first asks the server's shell what it is with one `echo` (`annulus_ssh::remote::PROBE`), so a Windows server, whose OpenSSH runs commands in cmd or PowerShell, gets a PowerShell script instead of an sh one.
7. **Keepalive** interval and dead-peer detection are configurable per host.

### 7.7 SFTP (`annulus-sftp`)

- **Operations:** list, stat, mkdir, rename, remove (recursive with confirmation), chmod, symlink resolution, read and write.
- **Transfer queue:** a per-session concurrency limit, pipelined requests (several outstanding 32–256 KiB reads or writes), resume by offset, progress and throughput events, cancel, and a conflict policy (ask, overwrite, skip, keep both).
- **Remote-to-remote** copies relay through the client.
- **Remote edit:** download to a temp folder, open it in the configured app, watch the file, offer to upload on save, and clean up when the tab closes.

### 7.8 Vault data model (`annulus-vault`)

Storage, keys and unlock are in [ADR-0007](adr/0007-vault-storage-and-encryption.md). There is **one vault per account** (no team vaults). Every field is listed in [FEATURES.md](FEATURES.md).

| Entity | Purpose | Key relations |
|---|---|---|
| `Group` | Tree of folders. Carries default settings that hosts inherit | parent `Group` |
| `Host` | One SSH target | `Group`, `Tag`s, `Identity` or inline credentials, jump `Host`s, startup `Snippet`, `PortForward`s |
| `Tag` | Label for filtering (AND logic across selected tags) | many-to-many with `Host` |
| `Identity` | Reusable username plus password and/or key | `Key` |
| `Key` | Private key (+ passphrase, + optional certificate), FIDO2 key handle, or hardware-bound key reference | — |
| `Snippet` / `SnippetPackage` | Saved commands, grouped | — |
| `PortForward` | Local, remote or dynamic rule | `Host` |
| `KnownHost` | Trusted host-key fingerprints | — |
| `Workspace` | Saved tabs, splits, layout and per-pane cwd | `Host`s |
| `SessionLog` | Recorded output of one session (metadata + encrypted body) | `Host` |
| `VaultSettings` | Settings that sync (terminal defaults, keymap choice) | — |

**Inheritance:** each inheritable host field is `Option<T>`, where unset (`None`) is distinct from empty. (Today the user, port, key, jump host, proxy, startup command and environment variables (merged by name) inherit; an empty user counts as unset. Hosts and groups name their group by a random uid that is the same on every device, so sync does not translate those links.) Resolution walks Host, then its group and parent groups, then the vault defaults, then the app defaults. The editor shows where each inherited value comes from. Changing a group value updates every host that inherits it without rewriting the hosts.

**Device-bound items** (hardware-bound keys, PIN and biometric unlock blobs) never sync. The other devices see a placeholder: "This key lives on <device>".

### 7.9 Sync (`annulus-sync`, `annulus-proto`, `annulus-server`)

See [ADR-0008](adr/0008-self-hosted-sync-server.md).

- Conflict resolution is a pure function. Its convergence is proven by simulation tests ([TESTING.md](TESTING.md)).
- The server is a small, separately deployable binary or Docker image. Its admin guide lives at `docs/server/`.

### 7.10 Settings, themes, keymaps (`annulus-config`)

- **Device settings** are a human-editable TOML file in the per-OS settings folder ([ADR-0007 §Data locations](adr/0007-vault-storage-and-encryption.md)). The file is hot-reloaded and validated with readable errors, and a bad file never causes a crash. The Settings UI is generated from the same schema metadata.
- **Terminal themes** are TOML. The built-ins are our own palettes plus open-licensed community schemes, each with its license recorded.
- **Color schemes** ([ADR-0019](adr/0019-tinted-color-schemes.md)): the tinted-theming catalog in `annulus-config::schemes`, generated by `cargo xtask themes`, with its license and authors in `schemes/LICENSE.txt`. From each scheme it derives the window's design tokens and the terminal's colors.
- **Keymaps:** the defaults come from the command registry (§7.13). A built-in alternative keymap is available. Conflicts are detected and shown ([UI-SPEC.md §Shortcuts](UI-SPEC.md#7-keyboard-shortcuts)).

### 7.11 Session logs

- Opt-in per host or group. Each log is stored as an encrypted vault record and synced. It captures what was printed, so hidden password input is never captured.
- Retention: the last N logs per host (default 5). Bookmarked logs are kept until unbookmarked and can carry a comment. A log can be deleted individually.
- The log viewer is a read-only terminal view with search. Logs can be exported as plain text.

### 7.12 Autocomplete

- **Sources:**
  - command history recorded locally from our sessions (using OSC 133 marks when the shell emits them, otherwise heuristics on the line before Enter)
  - snippets
  - optional remote path completion via SFTP listing
- It works the same in PowerShell, cmd, bash, zsh and fish.
- **No hidden exec probes and no AI.** Suggestions appear in a popup next to the cursor. Accepting one types its text, and nothing auto-runs.
- **Spike:** tracking the current command line reliably without shell integration.

### 7.13 Commands

Each shortcut action is a `Command { id, title (string id), default_chords per OS, when (context predicate), run }`, registered once. Menus, context menus and keyboard shortcuts read it. A test checks that no two commands share a default chord in the same context. The command palette takes a fixed copy of its search list when it opens. That list joins saved items, open sessions, pages, settings and app commands. Its search parser can limit a match to an action such as Edit or SFTP without putting a second command beside each item.

### 7.14 Import (`annulus-import`)

- OpenSSH: `~/.ssh/config` (with `Include` and `Host` patterns; `ProxyJump` becomes a jump chain; `Match` is reported as unsupported), `known_hosts`, and the referenced key files.
- PuTTY: the saved sessions, read from the registry on Windows or from a `.reg` export.
- Every importer returns a preview (create, merge or skip per item) before anything is written.
- Encrypted vault export and import are covered in ADR-0008.

### 7.15 Errors shown to users

Library errors are typed. The app maps each variant to a message with a cause and a next step. There are no raw `Display` dumps and no stack traces. "Copy diagnostic info" gives the details, with secrets redacted.

### 7.16 Diagnostics

`tracing` with per-session spans. Nothing is logged to disk by default. A debug log can be enabled in Settings. A panic hook writes a local crash report with no secrets and no network.

### 7.17 Packaging and updates

| OS | Artifacts | Signing |
|---|---|---|
| Windows 10 1809+ / 11 | Built today ([ADR-0027](adr/0027-client-and-server-ship-apart.md)): a portable zip (the app and `annulus-agent` in a folder) and our own installer, `annulus-setup`, per user with no administrator. Later: a winget manifest, and MSIX once signing is settled | code-signing provider: not decided yet |
| macOS (minimum to be set) | Built today: the portable zip with `Annulus.app` and `annulus-agent`. Later: a universal `.dmg` | Developer ID + notarization: not decided yet |
| Linux | Built today: the portable zip. Later: AppImage and `.deb`, Flatpak after them | GPG-signed checksums |
| Server | Built today: a zip of the server per OS, apart from the client's downloads and with no installer. Later: static binaries for arm64 and a multi-arch Docker image | signed checksums, image digest published |

`cargo xtask dist` builds them into `target/dist/client` and `target/dist/server`; every push's CI run uploads the two folders as separate artifacts.

**Updates:** an opt-in check against a signed manifest (ed25519/minisign). Downloads are verified before install. There are no silent updates, and an update never touches the vault.

## 8. Cross-cutting

- Performance budgets: [PERFORMANCE.md](PERFORMANCE.md)
- Security model: [SECURITY.md](SECURITY.md)
- Test strategy: [TESTING.md](TESTING.md)
- Accessibility requirements: [UI-SPEC.md §Accessibility](UI-SPEC.md#accessibility)
