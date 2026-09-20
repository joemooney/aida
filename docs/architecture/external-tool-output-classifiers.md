# External-tool output classifiers

This inventory records production decisions that inspect output owned by an
external executable. Structured output is preferred; unavoidable prose tokens
for the orchestrator live in `aida-cli-lib/src/external_tool_output.rs`.

| Site | Producer / observed version | Input | Decision driven | Structured alternative |
|---|---|---|---|---|
| `pr_ship::classify_gh_pr_checks_registration` | GitHub CLI 2.78.x | “no checks …” variants | wait for check registration rather than treating the invocation as a hard failure | `gh pr checks --json` has rows after registration, but the empty pre-registration error has no structured row |
| `auto_complete::is_merge_conflict_failure` | GitHub CLI 2.78.x | mergeability/conflict phrases | permit bounded rebase recovery; unknown text fails closed as a genuine merge failure | PR JSON exposes mergeability before the attempt, but cannot describe the failed merge race itself |
| `auto_complete::is_database_locked_message` | SQLite 3.x via rusqlite | locked/table-locked phrases | classify the phase as transient cache contention | rusqlite exposes structured codes at the cache boundary, but this classifier receives flattened subprocess failure text |
| `auto_complete::is_environmental_failure` | Linux/macOS kernels and toolchains | ENOSPC/disk/OOM phrases | suppress a misleading product-bug finding | no portable structured channel survives arbitrary child-process boundaries |
| `overlay::ci_style` | GitHub/GitLab rollup summaries | pass/fail/pending keyword stems | cosmetic TUI colour only; unknown text is dim | underlying adapters use structured state where available, but the display model currently carries a summary string |
| `forge::gitlab_merge_response_*` | GitLab REST API via `glab api` | JSON `message` values/status-like phrases | distinguish stale reviewed SHA and retryable merge races | already parses JSON and discards wrapper prose |
| `git_ops::{commit,push,looks_like_index_lock_failure}` | Git | index-lock, nothing-to-commit, rejected-push phrases | retry lock contention or select benign/retry paths | no stable machine-readable error taxonomy in Git CLI |
| `compete::parse_gate_result` | Cargo/custom shell gate | Cargo compiler markers | report whether the build stage ran successfully | custom gates prevent a single structured protocol; this is reporting, not a safety permission |
| `network_retry::classify_transient` | operator-configured external commands | configured substring allow-list | retry a command | intentionally user-configured; empty patterns are rejected |

Unknown GitHub merge output does **not** authorize recovery, and unknown `gh pr
checks` failures with diagnostic output remain errors. Those are the safety
decisions in this inventory and deliberately fail closed.

<!-- trace:BUG-1310 | ai:codex -->
