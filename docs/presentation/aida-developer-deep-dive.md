---
marp: true
title: "AIDA — Developer Deep Dive (under the hood)"
description: "How AIDA works internally: storage, IDs, the orchestrator, MCP, traces."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/presentation/aida-developer-deep-dive.md -o aida-developer-deep-dive.html
  npx @marp-team/marp-cli@latest docs/presentation/aida-developer-deep-dive.md --pdf -o aida-developer-deep-dive.pdf
AUDIENCE: engineers who want to understand / extend / trust the internals. GOAL: "I see how it's built and why."
DEPTH: code-level. File:symbol refs are in speaker notes so the slide stays readable.
COMPANION DECKS: executive-briefing, administrator-guide, user-walkthrough.
-->

# AIDA — under the hood

### A developer's tour of the substrate

Storage · distributed IDs · the orchestrator · MCP · traceability

<small>v0.12.0 · developer deep dive</small>

<!--
Framing: "AIDA looks like a CLI. Underneath it's a git-canonical graph database, a distributed-ID scheme, and a six-phase orchestrator. Here's each layer."
-->

---

## The five subsystems

1. **Storage** — git-canonical YAML + a rebuildable SQLite cache
2. **Identity** — HLC + node + dispenser → offline-safe, collision-free IDs
3. **The graph** — typed relationships + per-spec history, queryable
4. **The orchestrator** — a six-phase autonomous drain with escalation
5. **The surfaces** — CLI, MCP server, REST/gRPC, TUI

> Everything is **git-canonical**: the writer of record is a branch, not a database server.

---

## Workspace: seven crates

```
aida-core/            engine — models, storage, cache, dispenser, HLC, conflict
aida-cli/             thin `aida` binary stub over aida-cli-lib
aida-cli-lib/         the CLI implementation + MCP stdio server + orchestrator
aida-server/          REST + gRPC (port 8080), optional PostgreSQL backend
aida-tui/             `aida tui` — terminal shell hosting Claude Code (EPIC-26)
aida-crate/           published `aida` crate metadata
aida-generate-types/  Rust → TypeScript codegen for the React dashboard
```

- `aida-core` holds **no orchestrator concepts** — phase identities live in `aida-cli-lib`.
- Opt-in features: `postgres`, `github` / `gitlab` / `jira` integrations.

<!--
aida-core/Cargo.toml + aida-cli/Cargo.toml. The split keeps the engine reusable + testable independent of the agent-orchestration layer.
-->

---

## Storage: git is the writer of record

```
   write ──► orphan branch `aida-store`        ──►  .aida/cache.db
            objects/<TYPE>/000/<SPEC-ID>.yaml      (SQLite, gitignored)
            (one YAML per requirement)             rebuildable read projection
   read  ◄── cache (fast) ◄── stale-check vs orphan HEAD ◄── rebuild on mismatch
```

- One **YAML file per requirement**, sharded `objects/TYPE/000/SPEC-ID.yaml` (1000/shard).
- **Write-through:** commit to git first, then upsert the cache, then re-stamp the cache's HEAD SHA.
- The cache is **disposable** — `aida cache rebuild` reprojects it from git.

<!--
aida-core/src/object_store.rs (object_path/parse_spec_id), db/git_backend.rs (YAML I/O), db/cache.rs (SQLite + FTS5), db/cached_git_backend.rs (the write-through wrapper). The live worktree is .aida-store/ (gitignored).
-->

---

## Storage: the cache is a projection, not a source

- `.aida/cache.db` records the **orphan-branch HEAD SHA** it was built from.
- Every read first compares cached SHA vs `git rev-parse aida-store` — **mismatch → rebuild**.
- So the cache is always either current or rebuilt-on-next-read; it can be deleted with zero data loss.

> Source of truth = git. Cache = speed. Delete the cache and nothing is lost.

<!--
db/cached_git_backend.rs: list_summaries()/search() delegate to cache after the stale-check; rebuild_cache() forces a full load-and-project. This is why "no SaaS, no lock-in" is literally true — the projection is throwaway.
-->

---

## Identity: minting an ID offline, without collisions

Three pieces compose so any clone can mint IDs **offline** that never collide:

- **HLC** (hybrid logical clock) — `wall_time_ms + counter + node_id`, monotonic, causality-preserving.
- **Node id** — a short per-clone string (`JM`, `1`…), validated, registered in the shared store.
- **Dispenser** — tracks the next sequence per type prefix.

```
node-aware id     FR-JM-048        minted offline, guaranteed unique
      │  aida db merge-gate (at merge-to-trunk)
      ▼
agreed (short) id FR-048           the canonical id, collision-checked
```

<!--
aida-core/src/hlc.rs (HlcTimestamp), node.rs (validate_node_id), dispenser.rs (Centralized vs Distributed modes). Pre-allocated blocks (`aida db block claim`) let a clone reserve a contiguous range of short IDs up front so trace comments use the short form immediately, even offline.
-->

---

## The graph: typed relationships + per-spec history

- **Typed relationships** (parent/child, blocks/blocked-by, verifies, references, duplicate…) — stored by **UUID**, so renaming IDs never breaks links.
- **Transitive queries**: `aida graph <ID> --blocked-by`, `--impact`, `--tree`.
- **Per-spec history array** lives *inside* each YAML:

```yaml
history:
  - id: <uuid>
    author: joe
    timestamp: 2026-06-01T...
    changes:
      - { field_name: status, old_value: Done, new_value: Completed }
```

> The `history:` array is the **source of truth for spec-state time series** — walk it (or the orphan-branch git log) for burn-downs, not the cache.

<!--
aida-core/src/models.rs: HistoryEntry { id, author, timestamp, changes: Vec<FieldChange> }. Every status flip / priority / tag / owner change lands a structured row. The cache does NOT currently project history rows.
-->

---

## The MCP server: the graph, inside the agent

`aida mcp-serve` exposes the substrate to any MCP client (Claude Code, Codex, Cursor…) over JSON-RPC stdio.

**Spec-graph tools:** `list_requirements` · `show_requirement` · `add_requirement` · `update_requirement` · `search_requirements` · `add_comment` · `add_relationship` · `query_graph` · `list_features` · `history`

**Resources:** `aida://project/summary` · `aida://requirements/tree`

**Coordination tools:** punts (`post_punt`/`resolve_punt`/`escalate_punt`), findings, task-claims/leases, agent briefs, worker directives.

> The agent holds **structured context** — not a flat file it re-parses every session.

<!--
aida-cli/src/mcp.rs. The MCP server self-respawns after a handled request when the on-disk `aida --version` reports a newer build, so long-running clients pick up upgrades.
-->

---

## Traceability: code ↔ spec, enforced

**Inline trace comment** (stays in developer artifacts — never user-facing output):

```rust
// trace:FR-1-042 | ai:claude        // high confidence (implied)
// trace:TASK-85  | ai:claude:med    // 40–80% AI
```

**Commit message** (the `(REQ-ID)` trailer is what closes the loop):

```
[AI:claude] feat(auth): add login validation (FR-0042)
```

- On merge, `aida pull` scans commit trailers and **auto-bumps Done → Completed**.
- A commit-msg hook can reject non-conforming commits (`AIDA_COMMIT_STRICT=true`).

<!--
The (SPEC-ID) trailer auto-completes that spec on merge — so you only trailer a spec when THIS merge finishes it. This is the enforcement loop that keeps code↔intent from rotting.
-->

---

## The orchestrator: six phases

`aida queue work <SPEC> --auto-complete` is a process tree, not a script:

```
 orchestrator (your terminal)
   ├─▶ 1 Implementer  → spawns a Claude session in an isolated worktree → opens PR
   ├─▶ 2 CI           → waits for CI to reach a terminal state (deterministic)
   ├─▶ 3 Reviewer     → spawns a reviewer Claude session → writes a verdict file
   ├─▶ 4 Merge        → gh pr merge (deterministic)
   ├─▶ 5 Pull         → aida pull + auto-bump (deterministic)
   └─▶ 6 Build        → cargo build verify (deterministic)
```

- Only phases **1** and **3** are model sessions; **2/4/5/6** the orchestrator does itself.
- The phases sit behind a **`PhaseDriver` trait**, so the sequencing is unit-tested against a mock.

<!--
aida-cli/src/auto_complete.rs: enum Phase (index = 1..6 = the failure exit code), trait PhaseDriver, orchestrate_with_lifecycle_skip. The two judgment phases are Claude; everything else is deterministic — that's why it runs unattended.
-->

---

## Resilience: shelve-and-advance + escalation

**EPIC-28 — a batch keeps moving:** a *shelvable* phase failure (CI red, no verdict, …) parks the spec in `NeedsAttention` with a structured `FailureReason`; the drain **continues** to the next member, dependents skip automatically. Exit code **2** → triage with `aida findings list`.

**The escalation cascade — the system asks when it can't decide:**

```
implementer hits a fork it can't safely resolve
        │  /aida-punt   (parks the spec)
        ▼
headless advisor  ── resolves from recorded principle ──► implementer resumes
        │  can't safely decide
        ▼
human triage  (escalate-blocks: park for morning; escalate-defaults: ship the default)
```

<!--
FailureKind::is_shelvable gates shelve-vs-stop (env failures stop; spec failures shelve). The advisor tier is a fresh `claude -p` per punt (cold-boot), routed via the AIDA_PUNT_SIGNAL_FILE handshake. drain-state.json + a per-run corroboration token (AIDA_AUTO_COMPLETE_TOKEN) let a phase child verify it's really under a live orchestrator, not guessing from a bare env var.
-->

---

## Resilience: recent hardening (in flight)

The keystone is being made **crash- and stall-proof**:

- **Retry-then-shelve** — a transient GH-verify blip retries on a backoff, then shelves (batch advances) instead of stalling.
- **No-progress + ceiling watchdog** — a degenerate headless phase that stops committing is killed and shelved.
- **Resumable drain** — `--resume-drain` reconciles a crashed drain from git/PR/spec reality and re-enters at the first incomplete phase, **refusing if the original process is still alive** (the double-drive guard).

> The design principle throughout: **reconcile from reality, never replay a log.**

<!--
TASK-615 + STORY-492 (landing). The resume probes pr_merged/spec_completed accurately and is idempotent — re-running an already-merged merge is redeemed by the BUG-241 reconcile. Lease-coupled phases (implementer/CI) can't be replayed by a fresh process, so a reconciled CI re-entry clamps up to the reviewer.
-->

---

## Working *on* AIDA

```bash
aida dev shell-init --install   # one-time: install the `aida()` shell wrapper
aida dev activate               # pyenv-style: use the in-repo build (wrapper auto-evals)
aida dev status                 # which binary is active + does its SHA match HEAD
aida dev serve                  # aida-server (8080) + vite (5173)

cargo test -p aida-cli --bin aida     # the orchestrator/storage unit tests
cargo fmt --all -- --check            # CI runs --check; match it locally
```

- Master templates live in `aida-core/templates/`, embedded at build time; `.claude/` mirrors them via symlinks (`make sync-templates`).
- Releases: `scripts/release.sh {major|minor|patch}` bumps, regenerates CHANGELOG, tags, publishes tarballs.

<!--
CLAUDE.md is the authoritative dev-conventions reference. Note the dual-copy template system: edit the master in aida-core/templates/, never the .claude/ symlink.
-->

---

## Takeaways

- **Git-canonical** — the source of truth is a branch; the cache is throwaway.
- **Offline-safe IDs** — HLC + node + dispenser; node-aware → agreed at merge.
- **A real graph** — typed, UUID-linked relationships + structured per-spec history.
- **An orchestrator, not a script** — six phases, two of them model sessions, the rest deterministic, with shelve-and-advance + escalation.
- **MCP** serves all of it to any agent.

<small>Admin operations: `aida-administrator-guide`. Daily use: `aida-user-walkthrough`.</small>
