# Bundled fonts

Fira Sans Regular/Bold and Fira Mono Medium are the existing OFL-1.1 fonts in this directory; their copyright notices are in the matching `*-OFL.txt` files.

JetBrains Mono Regular/Bold and Symbols Nerd Font Mono Regular are pinned to the font assets in [WezTerm commit 458a100c1bf4134d36cb5c42dd90e4c8266d9e1a](https://github.com/wezterm/wezterm/tree/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/assets/fonts). Upstream documents that this Symbols Nerd Font Mono build contains only clearly OFL-1.1 icon sets and excludes Pomicons in [its license](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/LICENSE.md). The bytes and family names are unmodified. See `LICENSE_OFL.txt` and `LICENSE_POWERLINE_EXTRA.txt`.

JetBrains Mono is the default terminal font. Fira Mono is an alternate bundled choice. Both receive Nerd/Powerline symbols through the bundled Symbols Nerd Font Mono fallback, without installing a font into the OS. Installed or privately imported patched Nerd Fonts can be selected directly. The fallback is deliberately the curated OFL build rather than the full mixed-license icon collection.

`LICENSE-FONTS` in the repository root contains all these notices and ships with the portable client and Windows installer. Custom fonts are private imports: users supply their own files and Annulus does not redistribute them.
