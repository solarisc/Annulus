# ADR-0007: Vault storage, keys and unlock

- Status: Proposed
- Date: 2026-10-28

## Context

The vault holds hosts, groups, tags, identities, private keys, certificates, snippets, port-forwarding rules, known hosts, workspaces and session logs. Several of these are secrets, and all of them sync to a self-hosted server that must never be able to read them ([ADR-0008](0008-self-hosted-sync-server.md)). Decisions of 2026-10-28 shape the design:

- **One vault per account.** No team or shared vaults.
- The app works fully **offline with no account** (local mode), and can attach to a server later.
- **App lock** with a master password, plus an optional PIN or OS biometrics.

The failure modes to avoid: a forgotten master password destroys the vault, and signing out wipes local data.

Realistic sizes run from tens to a few thousand hosts, plus session logs, so the metadata fits in memory once unlocked. Log bodies are loaded on demand.

## Decision

**Storage**

- One SQLite file per vault, using `rusqlite` (bundled SQLite) in WAL mode.
- Table `records(id BLOB PRIMARY KEY, kind INTEGER, hlc BLOB, server_seq INTEGER NULL, deleted INTEGER, nonce BLOB, ciphertext BLOB)`.
- Table `record_history` keeps the previous versions (default: last 20 per record, plus every version from the last 30 days).
- Tables `vault_meta` and `sync_state`.
- Migrations are embedded SQL files with a version table. A test migrates every historical fixture to the latest version.

**Records**

- Entities are serialized with `postcard`, encrypted, and decrypted into memory at unlock. Session-log bodies are separate records, decrypted on open.
- Searching and sorting happen in memory. Nothing secret appears in plaintext on disk, not even host names.

**Ids and time**

- Record ids are **random (UUIDv4)**, so the server doesn't learn creation times.
- Each record carries a hybrid logical clock (HLC) timestamp, used for conflict resolution.

**Key hierarchy**

```
master password ──Argon2id(account salt, params in vault_meta)──► Master Key (MK)
MK ──HKDF-SHA256("annulus v1 kek")──► KEK
Account Key (AK): random 256-bit, generated once per account; never changes on password change
KEK          ──XChaCha20-Poly1305──► wrapped AK  (stored locally and on the server)
Recovery Key ──XChaCha20-Poly1305──► wrapped AK  (second copy; recovery key shown once at setup)
AK ──HKDF-SHA256("annulus v1 record")──► record key
record key ──XChaCha20-Poly1305(random 192-bit nonce, AAD = id‖kind‖hlc)──► record ciphertext
```

- **Password change** re-wraps the AK and leaves the records alone. The server login is re-registered (ADR-0008).
- **Recovery key:** 256 random bits, shown at setup as a grouped base32 string with a printable sheet. The user must re-type part of it before setup finishes. It unwraps the AK, so a forgotten master password does not lose data.

**Unlock**

- **The master password always works.**
- **Quick unlock** is optional. Each option wraps the AK with a key that only this device can use:

  | OS | Biometric quick unlock | Device secret store | PIN |
  |---|---|---|---|
  | Windows | Windows Hello (`KeyCredentialManager`) releases the device secret | DPAPI (`CryptProtectData`, current user) | PIN + device secret |
  | macOS | Touch ID: Keychain item with a `biometryCurrentSet` access control | Keychain (`security-framework`) | PIN + device secret |
  | Linux | none guaranteed | Secret Service (`secret-service`) when present | PIN + device secret only if Secret Service is present; otherwise master password only. **Never plaintext.** |

- **PIN:**
  - It is 6+ digits.
  - The PIN key is HKDF(Argon2id(PIN, device salt) ‖ device secret), so a copied blob alone can't be brute-forced.
  - After 5 wrong PINs the PIN blob is deleted and the master password is required.
- **Auto-lock** triggers after an idle timeout (default 15 min, configurable, can be off), when the OS session locks, and on sleep. Locking drops the decrypted records and keys.
- Key types implement `Zeroize` and `ZeroizeOnDrop`.

**Crypto crates:** `chacha20poly1305`, `argon2`, `hkdf`, `zeroize` (RustCrypto, MIT/Apache-2.0). OS stores go through `annulus-platform`.

**Data locations**

| OS | Vault and data | Settings |
|---|---|---|
| Windows | `%LOCALAPPDATA%\Annulus` | `%APPDATA%\Annulus` |
| macOS | `~/Library/Application Support/Annulus` | `~/Library/Application Support/Annulus` |
| Linux | `$XDG_DATA_HOME/annulus` | `$XDG_CONFIG_HOME/annulus` |

Portable mode keeps everything beside the executable.

**Never lose data silently**

- Logout, unlinking from a server, a failed migration or an app update never delete the local vault.
- Removing a vault from a device is an explicit action. It offers an encrypted export first.

## Consequences

- Any byte flip fails authentication and is reported. The AAD binding prevents swapping records between ids or kinds.
- The Argon2id parameters must be measured on low-end hardware (target 300–800 ms). They are stored per account so they can be raised later.
- Linux without Secret Service gets no PIN or remember-me. The UI explains why.
- A lost master password **and** a lost recovery key means the data is gone. Setup makes that very clear.

## Alternatives considered

- **SQLCipher.** Whole-file encryption doesn't fit per-record sync, and it brings a C/OpenSSL build dependency.
- **`redb` (pure Rust).** Attractive, but SQLite has better tooling, recovery and format stability.
- **A single JSON or CBOR file.** A full rewrite on every change is fragile.
- **AES-256-GCM.** Its 96-bit nonce makes random nonces risky. XChaCha's 192-bit nonce is safe to choose at random.
- **libsodium bindings.** They would add a C dependency. RustCrypto offers equivalent primitives in pure Rust.

## Verify

- Argon2id timing on a low-end laptop for each OS. Record the parameters here.
- Cold unlock time and memory with a 5,000-host synthetic vault (P-15).
- The quick-unlock paths on all three OSes, including Linux without Secret Service.
