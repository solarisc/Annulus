# ADR-0018: A self-hosted server's own certificate, pinned by fingerprint

- Status: Accepted
- Date: 2026-10-29

## Context

A sync server on a home network (the usual homelab case) has no DNS name that a public authority will certify, so it cannot get a certificate the app trusts through the Mozilla roots. The app refuses plain HTTP off loopback ([ADR-0008](0008-self-hosted-sync-server.md)), and turning verification off would let anyone on the network read the bearer token and replay OPAQUE messages. Asking the user to install a private root into the OS store is hard to do right on three OSes and trusts that root for every program.

`ureq` 3 offers no custom certificate verifier, but it can use a given set of certificates as the only roots, with the usual rustls checks (chain, name, dates) against them.

## Decision

1. `annulus-server tls-init --name <address>[,<name>…]` writes a self-signed certificate for the addresses the app will use (no CA flag, so rustls accepts it as both the root and the server's certificate) and a key readable only by its owner, never over existing files, and prints its SHA-256 fingerprint. `annulus-server fingerprint` prints it again, and `serve` prints it at start. The format is `openssl x509 -fingerprint -sha256`'s (`AB:CD:…`), computed by `annulus_proto::fingerprint` on both sides.
2. When creating an account or signing in fails because the certificate is not trusted, the app reads the certificate the server shows (`annulus_sync::pin::certificate_of`: one TLS handshake that accepts any certificate but still checks that the server holds its key; the connection carries nothing) and shows its fingerprint with the question "Trust this certificate". The certificate pinned is the last one the server sends: its own self-signed one, or the top of its chain.
3. After a yes, that certificate (DER) is kept with the sync account in the encrypted vault (`SyncAccount::pin`) and becomes the **only** root trusted for that account's server (`HttpServer::pinned`). A publicly trusted certificate for the same name is refused too. A different certificate later fails with `SyncError::Untrusted`; sync stops instead of asking again. Changing the server address forgets a pin not yet used.
4. With no pin, the public roots are used exactly as before.

## Consequences

- A LAN server needs no DNS, no public authority and no change to the OS trust store.
- The user's comparison of the fingerprint is the trust decision, as with SSH host keys. The UI says where to find the fingerprint to compare with.
- A replaced server certificate means signing out and in again on each device (and comparing the new fingerprint). Renewal is not needed: the certificate `tls-init` makes does not expire in practice, and rustls checks the dates against the certificate itself.
- New dependency: `rcgen` (from the rustls project) in the server, and in the sync tests for a real TLS server on loopback.

## Alternatives considered

- **Pinning only a fingerprint and verifying it ourselves:** needs a custom verifier, which `ureq` does not expose; the transport would have to be rewritten.
- **A private CA installed in the OS store:** platform-specific, error-prone, and trusts that CA for every program.
- **Trust on first use without showing anything:** the first connection could already be intercepted; the user must compare.
- **Turning verification off on the LAN:** the token and the OPAQUE exchange would be open to anyone on the network.
