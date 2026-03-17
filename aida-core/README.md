# aida-core

Core library for [AIDA](https://github.com/joemooney/aida) — AI-native requirements management.

## What is AIDA?

AIDA tracks what you build and why, with structured context for AI coding agents. Requirements link to code through trace comments. Commits reference spec IDs. AI assistants query typed, relational data instead of parsing prose.

## Features

- **5 storage backends** — YAML, SQLite, PostgreSQL, Git worktree, Git sibling
- **Distributed mode** — offline-capable with node-namespaced IDs and git sync
- **Hybrid Logical Clocks** — causal timestamp ordering across nodes
- **Sequence Dispenser** — 3 implementations (file, SQLite, Unix socket daemon)
- **Conflict detection** — field-level change detection with resolution strategies
- **Operation log** — append-only CRDT-inspired event log for merge-free sync
- **Analytics engine** — velocity trends, cycle time, churn, quality scores
- **Telemetry** — local usage tracking for measuring tool effectiveness
- **GitHub/GitLab integration** — bidirectional issue sync

## Usage

This crate is the shared library used by `aida-cli` (the CLI tool) and `aida-server` (the REST/gRPC server). Most users should install the CLI:

```bash
cargo install aida-cli
```

For library usage:

```rust
use aida_core::{DatabaseBackend, create_backend, Requirement};

let backend = create_backend(Path::new("requirements.db"), None)?;
let store = backend.load()?;

for req in &store.requirements {
    println!("{}: {}", req.display_id(), req.title);
}
```

## License

MIT — see [LICENSE](https://github.com/joemooney/aida/blob/main/LICENSE)
