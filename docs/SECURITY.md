# Security model

Last verified: 2026-11-09 @ 51a7d79

An SSH client holds the keys to its user's servers, and our sync server holds a copy of every user's vault. A security bug here is an incident on every machine the user can reach. This document is the threat model and the rules that follow from it. To report a vulnerability, see [../SECURITY.md](../SECURITY.md).

## Assets

1. Credentials: private keys, key passphrases, passwords, certificates, FIDO2 handles, keyboard-interactive answers.
2. Vault metadata: hostnames, usernames, groups, notes, snippets, session logs. These map out the user's infrastructure.
3. The master password, the account key and the recovery key.
4. Trust decisions: known host keys.
5. The integrity of the client and server binaries and their updates.

## Adversaries and mitigations

| Adversary | Can do | Mitigations |
|---|---|---|
| Network attacker (MITM) on SSH | Intercept or alter traffic | Strict host-key verification, where a changed key is a hard stop. Only the key kinds already known for a host are asked for, so a host that suddenly shows another kind of key is a changed key too, never a new one to trust. SHA-1 `ssh-rsa` signatures are never used. Terrapin strict-kex. Post-quantum hybrid KEX by default. Per-host legacy algorithms are planned (F-SSH-006). |
| Network attacker on sync | Intercept or alter sync traffic | TLS (rustls, webpki roots, or only the server's own certificate once the user has compared its fingerprint and trusted it, ADR-0018). The client sends plain HTTP only to `localhost` or a loopback address, checked on the parsed URL, and refuses any URL with a user name in it. The server refuses plain HTTP off loopback. Payloads are E2E-encrypted anyway. Device tokens travel only over TLS, are stored hashed on the server, expire after 90 days unused, and can be revoked per device. |
| **Compromised or malicious sync server** (including its admin) | Read the database, tamper, replay, withhold updates | **It never has decryption keys.** OPAQUE login means it never sees the password or a password-equivalent. Records are AEAD-encrypted with the record id as associated data, and the kind and clock time sit inside the ciphertext. A deletion is sealed the same way, and one the server makes up is ignored. A version older than the one a device holds is refused and the newer one pushed back, and a version a device already had cannot undo an edit or deletion pending there, so the server cannot roll a record back or bring a deleted one back. Withholding updates can't be fully prevented, so the client shows "last synced". A key file asking for more than 1 GiB, 16 passes or 16 lanes of Argon2id is refused. With a stolen database, an attacker can guess offline against the OPAQUE record or the password-wrapped key, one Argon2id per guess, so a new master password needs an estimated 10^10 guesses (`annulus_vault::strength`, the zxcvbn crate with its password, word and name lists); creating an account refuses a master password below that. A password change needs a fresh OPAQUE proof of the current password, not just a device token. |
| Malicious or compromised SSH server | Send hostile escape sequences, filenames, SFTP responses, channels | Parser limits and fuzzing. Only the standard DA/DSR replies (no title echo, no attacker-controlled query echoes). OSC 52 is ignored in both directions (F-TERM-011 plans an opt-in write). Ctrl+hover on a link shows its real target, and only http, https and mailto open. Local SFTP writes are path-sanitized (no `..`, device names or ADS). Channels the server opens unasked (X11, agent, session, direct-tcpip, sockets) are refused. Each step of connecting has 30 s (the time a user spends on a question does not count), and a command's output is kept up to 1 MiB. No hidden exec channels. |
| Other local users | Read our files | The vault, settings, `known_hosts` and exported logs are written owner-only (0600) on Unix, flushed, and replaced atomically; the vault keeps its previous version as `vault.anv.bak`. The vault is encrypted. One window at a time per data folder (a file lock), so two never overwrite each other's changes. The sync server's database and its backups are created owner-only (0600) on Unix whatever the umask, and a backup is never written over an existing file. |
| Anyone who can reach the sync server | Wear it down | Logins are throttled per account name and per address. The server trusts forwarded addresses only from loopback or exact proxy addresses named with `--trusted-proxy`. Request bodies are size-limited, and only a signed-in push may be large. A push holds at most 2,000 changes (one push is one transaction other accounts wait behind). A whole request has 60 s. When the server terminates TLS itself, request headers must also arrive within 10 s; behind a proxy (`--behind-proxy`, the Docker setup) the proxy holds slow clients off. So slow clients cannot hold connections open. |
| Malware running as the same user | Nearly anything | Out of scope to fully defend. We minimize exposure: auto-lock, short-lived decrypted secrets, zeroization, nothing secret on disk or in logs. On Unix the app leaves no core dump, and on Linux it is not dumpable (no ptrace or `/proc/<pid>/mem` from other processes). Hardware-bound keys that malware can use but not export are planned (F-KEY-008). |
| Walk-up attacker on an unlocked machine | Use the open app | Master-password lock, auto-lock on idle. Locking hides every session and drops the passwords and keys open tabs held; they come back from the vault on unlock. PIN and biometric unlock are planned (F-SEC-002, F-SEC-003). |
| Malicious import file | Crash or exploit the parser, or plant a command | Fuzzed parsers, size limits, and a preview before writing. A copied `ssh` command single-quotes any word beyond what host names and logins hold, so an imported `$(…)` address never runs when pasted. A forwarding rule that listens beyond this computer (an imported `*:1080`) is marked "Open to your network" on its row. |
| Supply chain | A compromised dependency or build | `cargo deny` (advisories, licenses, bans, duplicates), committed `Cargo.lock`, minimal dependencies, and a published Docker image digest. |

## Cryptography

All primitives come from audited libraries. We never write primitives.

| Use | Algorithm | Crate |
|---|---|---|
| Master password → Master Key | Argon2id (params per account, initially m=64 MiB, t=3, p=1) | `argon2` |
| Key wrapping, record encryption | XChaCha20-Poly1305 with random 192-bit nonces | `chacha20poly1305` |
| Subkeys | HKDF-SHA256 with versioned labels | `hkdf` |
| Server login | OPAQUE (RFC 9807), Argon2id as the key-stretching function | `opaque-ke` |
| Transport to the server | TLS 1.3 (1.2 allowed) | `rustls` |
| Device secret (planned, F-SEC-002) | DPAPI, Keychain, Secret Service | `annulus-platform` |
| Hardware-bound SSH keys (planned, F-KEY-007/008) | TPM (NCrypt), Secure Enclave, FIDO2 | `annulus-platform` |
| Update signatures (planned, F-SET-006) | Ed25519 / minisign | `minisign-verify` |
| SSH | as negotiated by russh ([ADR-0004](adr/0004-ssh-stack-russh.md)) | `russh` + `aws-lc-rs` |

The key hierarchy is defined once, in [ADR-0007](adr/0007-vault-storage-and-encryption.md#decision). What the server can and cannot see is in [ADR-0008](adr/0008-self-hosted-sync-server.md#what-the-server-stores-and-what-it-can-see).

## Rules for code

1. Secret values use wrapper types that implement `Zeroize`/`ZeroizeOnDrop` and redact `Debug`/`Display`. Password widgets keep their text in zeroizing buffers. Keys are made in place, and Argon2's memory and the cipher's key schedule are wiped too.
2. Secrets never go to logs, crash reports, window titles, notifications, sync metadata or the clipboard. The one exception is an explicit user copy (`Ui::copy_secret`, used for the recovery key, for a private key copied from its Keychain card and for a command copied from the agent panel), which is kept out of clipboard history and the Windows cloud clipboard, and cleared after 30 s or when the window closes unless something else was copied since. Error messages never quote decrypted text.
3. Nothing secret is written to disk unencrypted, temp files included.
4. Host-key decisions are explicit user actions. Code never auto-accepts a key. The one exception is a per-host "accept new (never changed) keys" option that the user turns on.
5. The server crate never imports client crypto and never logs request bodies, tokens or record ids.
6. Every `unsafe` block is in `annulus-platform`, carries a `SAFETY` comment, and is reviewed by someone other than its author.
7. Changes to auth, crypto, the vault format, the sync protocol or the parsers of untrusted input need a second reviewer, and must update this document if the model changes.

## Known gaps

What the code does not do yet, so nobody relies on it:

- Windows Error Reporting can still write a crash dump of the process.
- serde's own buffers, which hold decrypted JSON briefly, are not wiped.
- A password change re-wraps the account key but does not rotate it, so an old server backup and the old password still open old records.
- A deletion of many records from another device applies at once; the trash and the server's history are the way back.
- The server never purges tombstones or history, and has no per-account quota (device names are cut to 64 characters).
- Registration does not check the uploaded key file's shape (a password change does).
- Deletions made by builds before sealed tombstones are ignored by current builds (the device's copy goes back to the server instead). The protocol version did not change with sealed deletions, so a new client on an old server loses its deletions there (they are not applied elsewhere; nothing is deleted by mistake).
- The strength rule is enforced where the app makes or changes a master password and creates an account, not inside `Vault::create` itself (tests use cheap passwords).
- The agent door (ADR-0020, off by default):
  - The *Allow the command line and AI agents* switch lives in plain `settings.json`. A program running as the same user can switch it on, and after the next unlock it can drive the door without the master password. That is the same-user malware this model does not fully defend against, but the door makes it easier. Moving the switch inside the vault is planned.
  - *Approve commands* is off by default (ADR-0031). When it is on, `run` waits in the app and sends no bytes to the terminal until the user approves the exact command. `scp` also waits before copying files, showing its paths and flags in the AI file tab (ADR-0032). Denial sends an error to the caller. Turning approval off releases held calls. This check does not cover reads, host notes or a person's typing in the tab.
  - Agent file copies use the saved host's login through SFTP. They never export its password or key. Stop, lock and turning access off cancel copies. Copies refuse existing targets unless the agent asks to overwrite. Failed copies may keep partial files for retry; output asks agents to remove only their own temporary files after checking ownership, exact paths and whether a copy still uses them.
  - While the switch is on, agents may open a local shell, read the user's own tabs (on by default) and read host notes. An agent misled by text a server printed (prompt injection) can therefore do what the user could. Keep secrets out of notes.
  - On Windows the pipe keeps its default access list. Another local account cannot send the token, but it can hold the door's eight seats and lock agents out.
- Deleting a key takes it off a server on that server's word: one that answers `annulus-removed` without changing `authorized_keys` keeps the key's public half. The private half is gone either way.
- Ten failed logins in 15 minutes stop new sign-ins for that account name from anywhere (devices already signed in keep working), so someone who knows the name can delay adding a device.

## Privacy

- No telemetry, no analytics and no crash upload, in the client or the server.
- The update check is off by default and sends no identifiers.
- The server stores only what ADR-0008 lists.

## Release gates

- `cargo deny` and `cargo audit` are clean.
- Each fuzzer has run for at least 1 CPU-hour since its target last changed.
- The security review checklist for 1.0 is complete. An external audit of the vault format, OPAQUE flow and sync protocol is sought once funding exists.
