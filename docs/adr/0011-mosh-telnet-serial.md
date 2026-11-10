# ADR-0011: Mosh, Telnet and Serial

- Status: Rejected for v1 (decided 2026-10-28: v1 supports SSH and local shells only). The analysis below is kept for any future reconsideration.
- Date: 2026-10-28

## Context

Some SSH clients support Mosh, Telnet and Serial in addition to SSH. Mosh is GPL-3.0. Its protocol is UDP State Synchronization (SSP) with AES-128-OCB, bootstrapped by running `mosh-server` over SSH. Rust crates claiming wire-compatibility exist (`mosh-rs` 0.1.0, and the `mosh`/`mosh-network` family at 0.1.3), but they are weeks old and their licenses must be checked. A Rust *port* of mosh is a derivative work of GPL code.

## Decision

- **Telnet:** our own implementation in `annulus-session` (RFC 854 plus the NAWS, TTYPE, ECHO and SGA options). It is small and fully testable against a scripted peer.
- **Serial:** the `serialport` crate (MPL-2.0, allowed). Settings cover baud rate, data bits, parity, stop bits and flow control.
- **Mosh:** implemented **clean-room from the protocol description** in `annulus-session`, behind its own feature ID. We do not vendor or port GPL code while the project license is MIT/Apache (ADR-0009). If the maintainer picks GPL, reusing an existing implementation becomes an option. The terminal on the Mosh path is our own `annulus-term`. Mosh's predictive local echo is a separate task.

## Consequences

- Mosh is the riskiest protocol to get right. It needs interop tests against a real `mosh-server` in a Linux container, with packet loss and reordering injected.
- Writing Mosh clean-room means engineers must not read GPL Mosh source while implementing it. Use only the published papers, protocol docs and the wire behavior observed in tests.

## Verify

- Check the licenses and maturity of the existing Rust Mosh crates. Record them here.
- Decide whether predictive echo is in scope for 1.0.
