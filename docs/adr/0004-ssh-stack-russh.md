# ADR-0004: SSH and SFTP on russh

- Status: Proposed
- Date: 2026-10-28

## Context

We need a full SSH client: every common auth method, certificates, agent and agent forwarding, local/remote/dynamic forwarding, jump hosts, keepalive and SFTP. It has to be async, memory-safe, interoperate with old network gear, and ideally avoid C dependencies.

Facts checked on 2026-10-28 (crates.io and the russh README):

- `russh` 0.65.0, Apache-2.0, pure Rust, Tokio-based, client and server. It is maintained under the warp-tech org.
  - KEX: `mlkem768x25519-sha256` (post-quantum hybrid), curve25519, DH group-exchange/14/16/18, legacy NIST ECDH and SHA-1 groups (opt-in). Strict-kex Terrapin mitigation.
  - Ciphers: chacha20-poly1305, AES-GCM, AES-CTR; legacy CBC/3DES opt-in. MACs: SHA-2 (ETM and non-ETM); SHA-1 is legacy.
  - Keys: ed25519, ECDSA P-256/384/521, RSA with SHA-2 and SHA-1, OpenSSH certificates, the PuTTY PPK format, and Pageant on Windows.
  - Auth: publickey, password, keyboard-interactive, none, certificates.
  - Forwarding: direct-tcpip, forward-tcpip, streamlocal, agent forwarding, keepalive, `server-sig-algs`.
  - It denies `unwrap`, `expect`, `panic` and indexing at the lint level.
- **There is no FIDO/`sk-*` key support** (upstream issue #219, still open).
- `russh-sftp` 3.0.1 (Apache-2.0) provides the SFTP client.
- `ssh-key` 0.6.7 (RustCrypto) parses and writes OpenSSH key and certificate formats, including `sk-*` public keys.
- russh needs one crypto backend. `aws-lc-rs` 1.19 provides ML-KEM and builds on Windows x64 without CMake; it needs NASM or the `prebuilt-nasm` feature. `ring` 0.17 is smaller but has no ML-KEM.

## Decision

- `annulus-ssh` builds on `russh` with the **`aws-lc-rs`** backend (with `prebuilt-nasm` so contributors need no NASM). This gives us OpenSSH 10's default post-quantum KEX.
- `annulus-sftp` builds on `russh-sftp`. We add our own pipelining (several outstanding reads/writes per transfer) if the throughput budget needs it.
- Key generation, import and export use `ssh-key`. Generation covers Ed25519, ECDSA and RSA 3072/4096. Import covers OpenSSH, PEM/PKCS#8 and PPK.
- **FIDO2 / `sk-*` keys** (`sk-ssh-ed25519@openssh.com`, `sk-ecdsa-sha2-nistp256@openssh.com`) use our own signer, which plugs into russh's custom-signer/agent auth path. If that path can't carry `sk-*` signatures, we contribute the support upstream to russh rather than fork.
  - **Windows:** through the WebAuthn API (`webauthn.dll`), because Windows blocks raw FIDO HID access for non-admin processes.
  - **macOS and Linux:** CTAP2 over USB HID. Linux needs udev rules for the authenticator, which is an install-docs item. Candidate crate: `ctap-hid-fido2` 3.6 (MIT). Verify it before relying on it.
- **Hardware-bound (non-exportable) keys** use the same custom-signer path. These are keys whose private part never leaves the TPM or Secure Enclave.
  - **Windows:** a TPM key through NCrypt (Microsoft Platform Crypto Provider), gated by Windows Hello.
  - **macOS:** Secure Enclave, P-256 only, gated by Touch ID.
  - **Linux:** no equivalent is guaranteed. Offer FIDO2 instead, and say so in the UI.
- Jump hosts: russh connects over any `AsyncRead + AsyncWrite` stream, so each hop's `direct-tcpip` channel becomes the transport of the next hop.
- Proxies: SOCKS5 through `tokio-socks`. HTTP CONNECT is our own small module. ProxyCommand spawns a process and uses its stdio.
- **Agent:**
  - **Windows:** the OpenSSH agent named pipe (`\\.\pipe\openssh-ssh-agent`) and Pageant (built into russh).
  - **macOS and Linux:** `SSH_AUTH_SOCK`.
  - Third-party agents that expose these interfaces (1Password, Bitwarden, gpg-agent) work through them.
  - We also run our **own in-process agent** for vault keys, so agent forwarding works with keys that live only in the vault.
- **Not supported:** ML-DSA user and host keys and standalone ML-KEM KEX. russh doesn't support them, and mainstream servers don't deploy them yet. They are tracked as a later feature.

## Consequences

- No C toolchain is needed beyond what aws-lc-rs bundles. MSVC is needed in any case.
- `aws-lc-rs` adds roughly a few MB to the binary. Measure it against the binary-size budget.
- FIDO keys are our own risk area. They get their own milestone task and integration tests against a software authenticator.
- We depend on russh's release cadence. Pin exact versions and upgrade deliberately, with the interop test suite as the gate.

## Alternatives considered

- **`ssh2` (libssh2 bindings).** A C dependency, blocking API, weaker algorithm coverage (no ML-KEM, limited certificates).
- **`libssh` bindings.** A C dependency, and its LGPL complicates distribution.
- **Our own SSH implementation.** Unjustified risk. SSH transport crypto is not where we differentiate.
- **`ring` backend.** It would lose post-quantum KEX, which OpenSSH 10+ servers prefer by default.

## Verify

- Spike: connect with russh+aws-lc-rs to OpenSSH 9.x/10.x, Dropbear, and a legacy device profile (SHA-1 KEX, CBC). Run auth matrix and forwarding.
- Measure the binary-size impact of aws-lc-rs against ring.
- FIDO: confirm russh's custom signer can produce `sk-*` signatures, or plan the upstream PR.
- Confirm the NCrypt TPM and Secure Enclave signers interoperate with OpenSSH (`ecdsa-sha2-nistp256`).
