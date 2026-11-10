# Documentation map

Every doc has a **kind**, which tells you how to treat it:

- **Living**: describes how things are or must be. Keep it true. It carries a `Last verified` line.
- **Log**: append-only history. Never rewrite past entries.
- **Record**: frozen once accepted. To change one, supersede it with a new record.

| Doc | Kind | What it answers |
|---|---|---|
| [../CONTRIBUTING.md](../CONTRIBUTING.md) | Living | Principles, code style, lints, errors, logging, dependencies, commits, definition of done |
| [FEATURES.md](FEATURES.md) | Living | Every feature (`F-*` ID), its source, milestone, status and tests |
| [DESIGN.md](DESIGN.md) | Living | Architecture: crates, threading, data flow, subsystems |
| [UI-SPEC.md](UI-SPEC.md) | Living | Layout, components, design tokens, interactions, shortcuts, accessibility |
| [PERFORMANCE.md](PERFORMANCE.md) | Living | Budgets and how each one is measured |
| [TESTING.md](TESTING.md) | Living | Test strategy, which tests are required, which are banned |
| [SECURITY.md](SECURITY.md) | Living | Threat model, crypto, secret handling, supply chain |
| [server/](server/README.md) | Living | Installing and running the sync server: Docker, TLS, admin commands, backup |
| [adr/](adr/README.md) | Record | Why each architectural decision was made |
| [../SECURITY.md](../SECURITY.md) | Living | How to report a vulnerability |
| [../CHANGELOG.md](../CHANGELOG.md) | Log | What changed in each version |
| [RELEASING.md](RELEASING.md) | Living | How a version gets out |
