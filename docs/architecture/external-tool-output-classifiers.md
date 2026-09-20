# External-tool output classifiers

This inventory records production decisions that inspect output owned by an
external executable. Structured output is preferred; every unavoidable prose
token lives in the workspace-wide registry
`aida-core/src/external_tool_output.rs`, which all three consuming crates can
use without reversing dependency direction.

| Site | Producer / observed version | Input | Decision driven | Structured alternative |
|---|---|---|---|---|
| `pr_ship::classify_gh_pr_checks_registration` | GitHub CLI 2.81.0 | “no checks …” variants | wait for check registration rather than treating the invocation as a hard failure | `gh pr checks --json` has rows after registration, but the empty pre-registration error has no structured row |
| `auto_complete::is_merge_conflict_failure` | GitHub CLI 2.81.0 | mergeability/conflict phrases | permit bounded rebase recovery; unknown text fails closed as a genuine merge failure | PR JSON exposes mergeability before the attempt, but cannot describe the failed merge race itself |
| `auto_complete::is_database_locked_message` | SQLite 3.45.1 via rusqlite | locked/table-locked phrases | classify the phase as transient cache contention | rusqlite exposes structured codes at the cache boundary, but this classifier receives flattened subprocess failure text |
| `auto_complete::is_environmental_failure` | Linux/macOS kernels and toolchains | ENOSPC/disk/OOM phrases | suppress a misleading product-bug finding | no portable structured channel survives arbitrary child-process boundaries |
| `gh_stderr_is_network_error` | GitHub CLI 2.81.0 / Go net and TLS | connectivity, DNS, timeout, TLS, and `githubstatus.com` phrases | map PR lookup to `GhUnreachable`, hence orchestrator Inconclusive | `gh` exposes no structured error kind for transport failures |
| `forge::glab_stderr_is_transient` | GitLab CLI 1.36.0 / Go net/http | timeout, DNS, connection, EOF, and HTTP 502/503/504 phrases | map change lookup to `Unreachable`, hence orchestrator Inconclusive | `glab` exposes no structured error kind for transport failures |
| `claude_log_indicates_api_outage` | Claude Code 2.1.276 / Anthropic API / edge proxies | API 5xx, overloaded, upstream-connect, and stream-disconnect phrases | classify a headless phase as Inconclusive rather than implementer failure | input is JSONL, but the upstream error subtype is embedded only as human-readable event text |
| `status_context::load_pr_facts` | GitHub CLI 2.81.0 | “no pull requests found” variants | report a clean no-PR state instead of a CLI failure | command failure has no empty structured result; successful responses already use `--json` |
| `overlay::ci_style` | GitHub/GitLab rollup summaries | pass/fail/pending keyword stems | cosmetic TUI colour only; unknown text is dim | underlying adapters use structured state where available, but this display model carries a summary string |
| `forge::gitlab_merge_response_*` | GitLab REST v4 via GitLab CLI 1.36.0 `glab api` | parsed JSON `message`, `status`, and `status_code` fields only | distinguish stale reviewed SHA and retryable merge races; malformed/non-JSON wrapper prose matches nothing | structured alternative implemented: raw stderr is parsed as JSON and wrapper prose is discarded |
| `git_ops::{commit,push,looks_like_index_lock_failure}` | Git 2.43.0 | index-lock, nothing-to-commit, rejected-push phrases | retry lock contention or select benign/retry paths | no stable machine-readable error taxonomy in Git CLI |
| `pr_cmd::pr_fetch_failure_message` | Git 2.43.x | checked-out-worktree/fetch-refusal phrases | choose the actionable worktree recovery diagnostic | no machine-readable error taxonomy for this fetch refusal |
| `compete::parse_gate_result` | Cargo/rustc 1.92.0 / custom shell gate | Cargo compiler markers | report whether the build stage ran successfully | custom gates prevent a single structured protocol; this is reporting, not a safety permission |
| `terminal_cmd::run_terminator_command` | systemd 255 `busctl` / Terminator DBus plugin | missing service/argument/object phrases | replace raw DBus failure with plugin-install guidance | `busctl` exit status distinguishes failure but not the absent-plugin cause used for the hint; refusal remains refusal on no-match |
| `network_retry::classify_transient` | operator-configured external commands | configured substring allow-list | retry a command | intentionally user-configured; empty patterns are rejected |

Unknown GitHub merge output does **not** authorize recovery, and unknown `gh pr
checks` failures with diagnostic output remain errors. Those are the safety
decisions in this inventory and deliberately fail closed. GitLab merge wrapper
prose and malformed JSON likewise match neither stale-head nor retryable: an
unknown response can never authorize a retry against a newly read head.

<!-- trace:BUG-1310 | ai:codex -->
