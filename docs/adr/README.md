# Architecture Decision Records

An ADR records one decision: its context, the choice, and its consequences. Once a record is **Accepted**, its decision is frozen. To change course, write a new ADR that supersedes it, and set the old one's status to `Superseded by ADR-NNNN`. Fixing typos and adding verification notes is fine.

A **Proposed** status means the decision rests on facts that a spike or measurement still has to confirm. The ADR lists those facts under *Verify*. Whoever does the verification flips the status and records the evidence.

`cargo xtask docs-check` keeps the index below in sync with the files.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-rust-workspace-and-crate-boundaries.md) | Rust workspace and crate boundaries | Accepted |
| [0004](0004-ssh-stack-russh.md) | SSH and SFTP on russh | Proposed |
| [0005](0005-threading-and-async-runtime.md) | Threading model and async runtime | Proposed |
| [0006](0006-local-shells-via-conpty.md) | Local shells via ConPTY | Accepted |
| [0007](0007-vault-storage-and-encryption.md) | Vault storage, keys and unlock | Proposed |
| [0008](0008-self-hosted-sync-server.md) | Self-hosted account sync server | Proposed |
| [0009](0009-license.md) | License | Accepted |
| [0010](0010-original-work-only.md) | Original work only | Accepted |
| [0011](0011-mosh-telnet-serial.md) | Mosh, Telnet and Serial | Rejected for v1 |
| [0012](0012-three-platforms-two-renderers.md) | Three desktop platforms at 1.0, two renderers, portable windowing | Proposed |
| [0013](0013-local-shells-on-unix.md) | Local shells on macOS and Linux | Proposed |
| [0016](0016-alacritty-terminal-core.md) | alacritty_terminal for emulation and local PTYs | Accepted |
| [0017](0017-native-ui.md) | Our own native UI | Accepted |
| [0018](0018-pinned-server-certificate.md) | A self-hosted server's own certificate, pinned by fingerprint | Accepted |
| [0019](0019-tinted-color-schemes.md) | Community color schemes for the window and the terminal | Accepted |
| [0020](0020-local-control-channel-for-cli-and-ai.md) | A local control channel for a CLI and AI agents | Superseded in part by ADR-0031 |
| [0021](0021-quiet-look-and-choreographed-motion.md) | A quiet look, and motion for the big moments | Accepted |
| [0024](0024-windows-programs-carry-their-c-runtime.md) | The Windows programs carry Microsoft's C runtime inside them | Accepted |
| [0026](0026-the-theme-mode-filters-the-color-schemes.md) | The theme mode filters the window's color schemes | Accepted |
| [0027](0027-client-and-server-ship-apart.md) | The client and the server ship apart; a portable zip, our own Windows installer, and `annulus-agent` | Accepted |
| [0028](0028-theme-browser.md) | A theme browser with families and a live preview | Accepted |
| [0029](0029-private-fonts-and-nerd-symbols.md) | Private font selection and licensed Nerd symbols | Accepted |
| [0030](0030-command-palette-backdrop-blur.md) | Capture and blur the view below a modal | Accepted |
| [0031](0031-agent-settings-approval-and-skill.md) | Agent settings, command approval and a portable skill | Accepted |
| [0032](0032-agent-file-transfers.md) | Agent file transfers through saved hosts | Accepted |

## Template

```markdown
# ADR-NNNN: Title

- Status: Proposed | Accepted | Superseded by ADR-NNNN
- Date: YYYY-MM-DD

## Context
What forces are at play? What constraints apply? Link to evidence.

## Decision
The choice, stated so someone could check the code against it.

## Consequences
What gets easier, what gets harder, what we now must do.

## Alternatives considered
Each with the reason it lost.

## Verify (Proposed only)
Facts to confirm, how to confirm them, and which milestone task does it.
```
