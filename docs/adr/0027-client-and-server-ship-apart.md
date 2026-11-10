# ADR-0027: The client and the server ship apart; a portable zip, our own Windows installer, and `annulus-agent`

- Status: Accepted (2026-11-07)
- Date: 2026-11-07

## Context

Each build used to produce one artifact per OS holding the bare programs: the app, the CLI (`annulus`) and the sync server, side by side. The maintainer asked for:

- the app and the CLI together in a folder in a zip, portable, with the CLI named `annulus-agent`;
- an official installer with a window of its own that installs the app and the agent CLI;
- the server never in the client's builds: a zip of its own, and no installer for it, since it is set up on servers.

[DESIGN.md §7.17](../DESIGN.md#717-packaging-and-updates) planned MSIX, a zip and winget for Windows. An MSIX installs only when signed by a certificate the PC trusts, and the code-signing provider is still open, so it cannot ship yet.

## Decision

1. **The CLI is `annulus-agent`.** The crate stays `annulus-cli`; its program, its usage text and its error prefix say `annulus-agent`. This supersedes the program's name in [ADR-0020](0020-local-control-channel-for-cli-and-ai.md) decision 8. The MCP server still calls itself `annulus`, and its tools keep their `annulus_*` names: agents already configured keep working once their command line is changed.
2. **The client's downloads:** `annulus-<version>-<os>-<arch>.zip`, a folder `Annulus` with the app (on macOS `Annulus.app`), `annulus-agent` and the licenses, run from wherever it is unpacked; and on Windows `annulus-setup-<version>-windows-<arch>.exe`.
3. **The installer is ours**, a Rust program (`crates/annulus-setup`) drawn with our own toolkit like the app ([ADR-0017](0017-native-ui.md)). It installs for the current user into `%LOCALAPPDATA%\Programs\Annulus`, so no administrator is needed: the programs, a copy of itself as `uninstall.exe`, an Apps & features entry under `HKEY_CURRENT_USER`, a Start menu shortcut, optionally a desktop shortcut, and optionally the folder on the user's PATH so `annulus-agent` runs from any new terminal. Removing it (from Apps & features) takes all of that away and leaves the vault and settings. The programs travel inside it, deflated at build time. Its manifest asks for no elevation.
4. **The server's download** is `annulus-server-<version>-<os>-<arch>.zip`, the server and its license, with no installer. Servers are set up with the scripts in `deploy/` or by hand.
5. `cargo xtask dist` builds all of them into `target/dist/client` and `target/dist/server`, writing the zips itself (deflate, Unix modes kept), and CI uploads the two folders as separate artifacts: `annulus-<os>-<sha>` and `annulus-server-<os>-<sha>`.

## Consequences

- Scripts and agent setups that ran `annulus` must run `annulus-agent`.
- The installer is unsigned until code signing is decided: SmartScreen warns on first run, as for the zip's programs.
- macOS and Linux get the portable zip only; `.dmg`, AppImage and `.deb` come later. The installer builds on every OS (so the workspace builds and tests everywhere) but only installs on Windows.
- A release takes a second LTO build on Windows, for the installer, about three minutes.
- The Apps & features entry, shortcuts and PATH are written through `annulus-platform::install`, the only place with `unsafe`; the installer's own steps are tested over a stand-in for Windows, so the tests never touch the real registry, Start menu or PATH.

## Alternatives considered

- **MSIX now.** It needs signing to install at all.
- **WiX (an MSI) or Inno Setup.** A standard look, but their own dialogs and visuals, a build tool outside Rust on the CI runner, and a second language for the install steps.
- **The app installing itself** (a menu item in the portable app). It does not give an installer to download, which was the ask.
