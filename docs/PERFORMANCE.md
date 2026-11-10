# Performance

Last verified: 2026-10-28 @ 51a7d79

Performance is a feature with numbers. Each budget below has a scenario, a measurement method and a gate. The numbers are **initial targets**. The real baseline is measured on the reference machines, and a PR that confirms or adjusts a number edits this file and shows its evidence. Once confirmed, a budget can only be loosened through a superseding ADR.

## Reference machines

Record the CPU, RAM, GPU, display resolution, scale and refresh rate for each machine. CI runners are used for *relative* regression gates only, never for absolute budgets.

| OS | Machine | Notes |
|---|---|---|
| Windows 10 Pro 22H2 (19045) | a typical desktop | primary reference |
| macOS (latest) | Apple Silicon, Retina display | worst case for the CPU backend (P-12) |
| Linux (Ubuntu LTS, Wayland) | 4K display at 200% scale | worst case for the CPU backend (P-12) |

## Client budgets (each applies on all three OSes unless noted)

| ID | Scenario | Target | How measured | Gate |
|---|---|---|---|---|
| P-01 | Cold start to the first interactive frame (host list visible, 500 hosts, vault unlocked via quick unlock) | ≤ 200 ms (stretch 120 ms) | `xtask measure startup`: process start → first present + input ready (ETW on Windows, `os_signpost` on macOS, timestamps on Linux) | release checklist |
| P-02 | Idle memory, main window only, 500 hosts | ≤ 30 MB | `xtask measure memory`: private working set (Win), `phys_footprint` (macOS), PSS (Linux), after 10 s idle | release checklist |
| P-03 | Idle memory with 1 SSH session and 10k lines of scrollback | ≤ 45 MB | same | release checklist |
| P-04 | Marginal memory per extra session (empty scrollback) | ≤ 3 MB | 10 sessions vs 1 | release checklist |
| P-05 | Scrollback cost | ≤ 12 bytes per cell on average for typical output | allocation count via `dhat` test (deterministic, OS-independent) | CI test |
| P-06 | Idle CPU, cursor blink off, sync connected | 0 wakeups/s from our process | ETW / `powermetrics` / `perf sched` over 60 s | release checklist |
| P-07 | Idle CPU, cursor blink on | ≤ 2 wakeups/s | same | release checklist |
| P-08 | Keypress to glyph on screen (local shell, echo) | ≤ 1 frame + 5 ms at 60 Hz | camera/Typometer method, plus an internal trace from key event to present | release checklist |
| P-09 | Parser + grid throughput, plain ASCII | ≥ 400 MB/s | criterion `term/advance_ascii`, iai on CI | CI: iai ≤ +5% |
| P-10 | Parser + grid throughput, SGR-heavy | ≥ 150 MB/s | criterion `term/advance_sgr` | CI: iai ≤ +5% |
| P-11 | Parser + grid throughput, Unicode/CJK/emoji-heavy | ≥ 80 MB/s | criterion `term/advance_unicode` | CI: iai ≤ +5% |
| P-12 | Full redraw of a 240×70 grid at native resolution | ≤ 4 ms CPU on the UI thread | criterion through each backend, plus an in-app frame trace. **This is the CPU-backend gate in ADR-0012.** | release checklist |
| P-13 | Typing latency in tab B while tab A floods 100 MB | P-08 still holds | `xtask measure flood` | release checklist |
| P-14 | SFTP download throughput over a LAN-speed link | ≥ 90% of OpenSSH `sftp` | Docker integration bench with `tc` shaping | before a release |
| P-15 | Vault unlock with 5,000 hosts (after KDF) | ≤ 100 ms decrypt + index | criterion `vault/unlock_5k` | CI: iai ≤ +5% |
| P-16 | Release binary size (stripped, per OS) | ≤ 15 MB | `xtask dist` reports the size | CI: hard fail |
| P-17 | Host search over 5,000 hosts | ≤ 4 ms per keystroke | criterion `app/search_5k` | CI: iai ≤ +5% |
| P-18 | Edit on device A visible on device B (same LAN server) | ≤ 2 s | e2e test with two clients | before a release |

## Server budgets

| ID | Scenario | Target | How measured |
|---|---|---|---|
| S-01 | Idle server memory with 50 accounts × 5,000 records | ≤ 40 MB RSS | load test |
| S-02 | Pull of 5,000 records for a new device | ≤ 1 s on a Raspberry Pi 4 | load test |
| S-03 | Docker image size | ≤ 30 MB | CI |

## Reference comparison

At each release, measure some native peers (Windows Terminal + OpenSSH, PuTTY, macOS Terminal) in the P-02, P-03 and P-06 scenarios on the reference machines. Publish numbers only with their date and version. Our claims must rest on these measurements.

## Rules of thumb (not budgets)

- Nothing redraws unless something is dirty. No timers when idle, except an optional cursor blink.
- No allocation per byte or per cell on the hot path. Reuse buffers. No `String` per cell or per line.
- Lists are virtualized.
- Blocking work never runs on the UI thread.
- Lazy initialization: SFTP, the importers, the settings UI and the log viewer cost nothing until they're opened.
- Sync idles on one SSE connection. It doesn't poll.

## Benchmark hygiene

- The corpora for P-09…P-11 are generated deterministically by `xtask gen-corpus`.
- iai-callgrind gates CI, because instruction counts are deterministic on shared runners. criterion is for local and pre-release trends.
- A PR that intentionally trades performance for correctness updates the baseline and explains the trade in its description.
