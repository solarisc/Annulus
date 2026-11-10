# ADR-0005: Threading model and async runtime

- Status: Proposed
- Date: 2026-10-28

## Context

Every desktop OS requires the UI to own its event loop on one thread: Win32 and winit on all three OSes, with AppKit even requiring the main thread. Network sessions are I/O-bound and naturally async, and russh requires Tokio. Heavy output, such as `cat` of a large log, must never stall typing in another tab. Key derivation (Argon2id), SQLite and file I/O block.

## Decision

- **UI thread** (the main thread). It runs the event loop, owns the widget tree and app state, and renders. It never blocks and never runs async code.
- **I/O runtime.** One Tokio multi-thread runtime with **2 worker threads**, which is configurable. It runs every session task (SSH, SFTP, forwarding, Telnet, Serial reader, ConPTY pumps) and sync.
- **Parsing happens off the UI thread.** Each session task feeds bytes into its `annulus-term` emulator, which sits behind a `parking_lot::Mutex`. The UI locks it briefly once per frame to copy the visible damaged rows into its render buffer. The lock is never held across I/O or rendering.
- **Wakeups are coalesced.** A session sets an atomic "dirty" flag and wakes the event loop only on the clean-to-dirty transition. The wake uses `EventLoopProxy::send_event`, or `PostMessageW` if Windows keeps its own window (ADR-0012 §4). The UI renders at most once per display refresh (waitable swap chain on Windows, frame callbacks or vsync timing elsewhere), however fast output arrives.
- **Blocking work** goes to `tokio::task::spawn_blocking`: Argon2id, SQLite writes and directory listings.
- **Messages:** `UI → I/O` commands travel on `tokio::sync::mpsc` (bounded). `I/O → UI` events go through a lock-free queue drained on each wake. Shared state is the exception and limited to the terminal model above.

## Consequences

- Typing latency stays independent of output volume in other sessions.
- Terminal model access needs care: fair locking, and frames copy rows instead of holding the lock.
- Tests can run sessions on a paused Tokio clock (`tokio::time::pause`) for deterministic timeouts and keepalives.

## Alternatives considered

- **Everything on the UI thread with a per-frame byte budget.** Simpler, but a slow parse of a hostile stream would freeze the window.
- **A thread per session.** Wastes memory (stacks) at 20+ sessions and gains nothing over async.
- **smol/async-std.** Lighter, but russh is Tokio-native.

## Verify

- Under a 100 MB output flood in tab A, keypress-to-echo latency in tab B stays within budget.
- Measure the idle threads and memory of the 2-worker runtime, and tune the worker count.
