# ADR-0006: Local shells via ConPTY

- Status: Accepted
- Date: 2026-10-28

## Context

The desktop app opens local terminal tabs. On Windows the only supported pseudoconsole is ConPTY (`CreatePseudoConsole`, Windows 10 1809+). `portable-pty` wraps it, but it is MIT-licensed, last released 2025-02, carries a Unix-oriented abstraction we don't need, and spawns through its own command builder.

## Decision

- `annulus-platform` contains its own thin ConPTY wrapper: create, resize, close, and spawn with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`, with overlapped pipe I/O pumped by the I/O runtime.
- Shell discovery covers PowerShell 7 (`pwsh`), Windows PowerShell, `cmd`, each WSL distribution (`wsl -l -q`), Git Bash, and custom profiles from settings.
- The minimum OS is Windows 10 1809 (build 17763), the first with ConPTY.

## Consequences

- A few hundred lines of `unsafe` FFI, all in one module with `SAFETY` comments and integration tests (spawn `cmd /c echo`, resize, exit code, and closing while output is still flowing).
- ConPTY itself rewrites some sequences and limits throughput. Our local-shell benchmarks therefore measure the ConPTY round trip separately from our own parser throughput.

## Alternatives considered

- **`portable-pty`.** Reasonable, but it adds an abstraction layer and dependencies for about 300 lines of code we can own and test.
- **winpty.** Obsolete since ConPTY.
