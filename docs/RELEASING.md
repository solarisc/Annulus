# Releasing

Last verified: 2026-11-10 @ 51a7d79

How a version gets out. Nothing here happens by itself: a draft release stays a draft until it is published.

## Each release

1. Update `CHANGELOG.md`: give the top section its version and date, and move anything still unreleased under a new one.
2. Set `version` in the root `Cargo.toml`, and run `cargo check` so `Cargo.lock` follows.
3. Merge to `main` once `cargo xtask ci` passes on Windows, macOS and Linux.
4. Tag it and push the tag:

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

5. Build the downloads on each system with `cargo xtask dist`: the client's portable zip (the app and `annulus-agent`) and, on Windows, the installer, plus the server's own zip ([ADR-0027](adr/0027-client-and-server-ship-apart.md)). On a Mac, `cargo xtask mac-app` builds `Annulus.app`.
6. Pack each with `README.md`, the two licenses and the server's, and attach them to a **draft** release, with the changelog's top section as its notes:

   ```bash
   gh release create v0.1.0 --draft --prerelease --title "Annulus v0.1.0" --notes-file notes.md annulus-v0.1.0-*
   ```

7. Download each archive and run it on its system: unlock a vault, connect to a host, make a key.
8. Press Publish.

Until code signing is set up, the release notes say the programs are unsigned, as the README does.

## The README's media

Everything in `docs/media/` is made from the app itself by `crates/annulus-app/src/native/film.rs`, whose top comment says how. Render it again when a screen in it changes.

## Licenses in the downloads

The client archives and Windows installer include `LICENSE-THEMES`: community scheme authors and MIT terms, plus the rbit built-in palette license. `cargo xtask themes` refreshes it when the community catalog is regenerated.

Client packages also include `LICENSE-FONTS` with the bundled text/font-symbol copyright notices and OFL/Powerline terms. The font bytes are embedded for portable use; no OS font installation is required.
