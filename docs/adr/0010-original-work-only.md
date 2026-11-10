# ADR-0010: Original work only

- Status: Accepted
- Date: 2026-10-28

## Context

Annulus is an SSH client, and SSH clients share many conventions: a sidebar of sections, a host list, a side editor panel, tabs, split panes, and generic terms such as host, group, keychain, snippet, port forwarding and known hosts. Names, logos, icons, illustrations, text, code and a distinctive overall look belong to whoever made them. This is engineering policy, not legal advice; a legal review comes before 1.0.

## Decision

**Allowed:**

- Conventional layouts and the generic terms common to SSH clients.
- Compatible import formats (OpenSSH config and known_hosts, PuTTY sessions and PPK keys).

**Not allowed:**

- Another product's name, or a confusable one.
- Another product's logo, icons, illustrations, screenshots, sounds, licensed fonts or color palette.
- Copying text of any kind. UI strings and docs are written fresh.
- Decompiling or inspecting another product's app, sources or network traffic.

**Our own identity:** an original name, logo, icon set (vector paths we draw), accent color, typography choices and illustrations, defined in [UI-SPEC.md §Design tokens](../UI-SPEC.md#8-design-tokens). The README, the app and the docs describe Annulus in its own terms.

## Consequences

- A reviewer rejects any change that pastes outside text or assets.
- Third-party material we do ship (fonts, color schemes) is listed with its license in `LICENSE-FONTS` and `LICENSE-THEMES`.
- A legal review of the name and identity is a 1.0 release gate.
