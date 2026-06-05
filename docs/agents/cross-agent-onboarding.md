# AIDA MCP — Onboarding Brief for Non-Claude-Code Agents

*Audience: any MCP-speaking coding agent (Codex, Cursor, Continue, custom Claude Agent SDK runtime, etc.) being attached to an AIDA project. 2026-05-22.*

This brief is what you need to know to participate productively in an AIDA project. It assumes you can speak MCP, run shell commands, and read/write files in a worktree.

For agent-session communication details that cut across Claude, Codex, and
Antigravity, see `docs/agents/session-communication.md`. That file is the
durable reference for hook pause/abort/defer behavior, brief routing, and
which substrate channel to use when an agent must stop, ask, or resume.

For client-specific MCP configuration surfaces, marketplace/package channels,
and safe default write-tool posture, see
`docs/agents/aida-mcp-install-matrix.md`. Keep that matrix updated whenever a
major agent client changes its MCP config path or marketplace model.

## What AIDA is

AIDA (AI Design Assistant) is a **spec-graph-backed agent-collaboration layer**. The visible surface is a Rust CLI (`aida`) and a small TUI; the actual product is the substrate underneath:

- Every requirement (spec, story, bug, task, epic) has a **stable identifier** that survives renames/refactors.
- Specs have **typed relationships** to other specs (parent/child, blocked-by, depends-on, etc.).
- Code references back to specs via inline **`trace:SPEC-ID | ai:<tool>`** comments.
- The whole graph is **git-canonical** (orphan branch `aida-store`, one YAML per spec) with a SQLite read-cache (`.aida/cache.db`, rebuildable).
- An **MCP server** (`aida mcp-serve`) exposes the graph and the coordination layer to any MCP-speaking agent — that's where you come in.

The strategic positioning: the IDE-embedded coding assistants (Cursor, Cline, Aider, Claude Code) make a human at the keyboard faster. **AIDA is the substrate beneath the IDE-agent war** — the shared spec-graph + coordination tools that whatever agent you use can be given structured context through. You're not competing with Claude Code by using AIDA from Codex; you're using the same substrate, just through MCP instead of through `.claude/skills/`.

## What you can do via MCP today

AIDA's MCP server exposes **29 tools** in two clusters:

**Important:** the canonical argument names come from `tools/list` over MCP. The list below mirrors what the server actually advertises (verified via `aida-cli/src/mcp.rs` inputSchema descriptors). If a future edit to this doc drifts from the source, **trust `tools/list`**, file a finding, and `aida` will fix the doc.

### Cluster 1 — Spec graph (12 tools)

- `list_requirements({status})` → list specs (optionally filtered)
- `show_requirement({id})` → full spec content, relationships, comments
- `add_requirement({title, description, type, ...})` → file a new spec. `type` is required and must be one of the canonical taxonomy values: `functional`, `non-functional`, `system`, `user`, `bug`, `epic`, `story`, `task`, `spike`, `sprint`, `folder`, `meta`, `doc`. AIDA auto-assigns the ID prefix from that type (for example, `task` → `TASK-N`), so agents should not invent generic `SPEC-N` IDs.
- `update_requirement({id, ...})` → edit
- `search_requirements({query})` → FTS5 search
- `add_comment({id, text})` → comment on a spec  *(arg is `text`, not `body`)*
- `add_relationship({spec_id, relationship_type, target_spec_id, bidirectional?, force_parent?})` → add a typed relationship between existing specs. Built-ins include `parent`, `child`, `duplicate`, `verifies`, `verified-by`, `references`, `blocked-by`, and `blocks`; `depends-on` aliases to `blocked-by`, and custom non-empty names are accepted for CLI parity.
- `query_graph({spec_id, mode?, depth?, follow?})` → query the cross-spec relationship graph from a root spec, equivalent to `aida graph`. `mode` ∈ `tree` (Parent/Child descendants + status rollup, default), `blocked-by` / `blocks` (transitive chains), `impact` (reverse closure — what is blocked by the root). `follow` (array of type names, e.g. `["begets"]`) traverses arbitrary custom/built-in edge types outgoing, overriding `mode` (FR-282). Returns JSON `{root, mode, count, nodes:[{id,title,status,resolved}], rollup}`. The typed-graph query a flat per-feature spec store can't answer.
- `send_message({body, to?, broadcast?, thread?, in_reply_to?, from?})` → send an inter-agent peer message, equivalent to `aida mailbox send`. Address one agent via `to` or set `broadcast: true`. Distinct from briefs (operator→agent work) and directives (top-down control): agent↔agent conversation.
- `read_inbox({agent?})` → an agent's inbox (messages to it + broadcasts, excluding own-sent, oldest-first), equivalent to `aida mailbox inbox`. Returns JSON `{agent, count, messages}`.
- `list_features()` → list project features
- `history({spec_id?, since?})` → structured event ledger, equivalent to `aida history --events`

These mirror the `aida list / show / add / edit / search / comment / history` CLI verbs. **Use them for any spec-graph interaction.** Don't shell out to `aida` for these. *(STORY-82 and EPIC-27 will modernize the older spec-graph tools to match the coordination tools' vocabulary and capability — until then, expect a thinner surface than the coordination cluster.)*

### Cluster 2 — Coordination (17 tools, STORY-361 + STORY-426)

- **Punt channel:**
  - `post_punt({spec_id, detail, category?, lean?, raised_by?})` — required: `spec_id`, `detail`. Append a punt record to `.aida/punts.jsonl`. Does NOT modify spec status — pair with `update_requirement` to flip to `needs-attention`.
  - `list_punts({status?})` — list punt records
  - `read_punt({spec_id})` — read the most recent punt for a spec
  - `resolve_punt({spec_id, answer, reasoning, classification?})` — required: `spec_id`, `answer`, `reasoning`. Write a PuntResponse marking the punt resolved.
  - `escalate_punt({spec_id, reasoning, escalation_reason?, classification?})` — required: `spec_id`, `reasoning`. Kick a punt to a human.

- **Findings channel:**
  - `file_finding({title, description, source?, spec_id?, pr?, kind?, severity?})` — required: `title`, `description`. File a triage-able draft TASK with appropriate `from-*` tags.
  - `list_findings({pr?, source?, kind?})` — list findings.
  - `triage_finding({id, action, reason?})` — required: `id`, `action`. Promote (Approved) or dismiss (Rejected) a finding.

- **Task-claim channel:**
  - `claim_task({spec_id, role?})` — required: `spec_id`. Atomic lease on a queued spec (writes `.aida/sessions/<lease>.toml`).
  - `release_task({lease_id})` — required: `lease_id`. Delete the lease.
  - `list_active_leases()` — list all active leases.

- **Directive channel** (control plane over `.aida/worker.cmd`):
  - `post_directive({verb, args?})` — required: `verb`. Verbs: `drain`, `pause`, `exit` (per `aida-worker` discipline). Note: `args` must be a JSON array of strings (e.g., `["arg1"]`), not a bare string. Directives coordinate the local worker session thread flow, which is distinct from global queue commands.
  - `list_directives()` — list pending directives.
  - `ack_directive({index})` — required: `index`. Remove a directive by its 0-based position.

- **Agent-brief channel** (pickup briefs under `.aida/agent-briefs/<agent>/`):
  - `list_briefs({agent?, include_acked?})` — list substrate-resident pickup briefs; defaults to pending/unacked only.
  - `read_brief({path})` — read the full markdown brief content. Use paths returned by `list_briefs`.
  - `ack_brief({path})` — mark a brief acknowledged using the same `.acked` suffix convention as `aida brief ack`; idempotent when already acked.

These are the **agent-coordination primitives**. They're how multiple agents (you, a human, another agent) coordinate on the same spec graph without stepping on each other.

> **Schemas:** all 29 tools advertise `inputSchema` and descriptor-level `outputSchema`. Runtime responses still use MCP text content envelopes; structured emission of `structuredContent` is the Path B follow-up (**STORY-399**). Treat `outputSchema` as descriptor metadata until that ships.

## How to connect (minimum viable)

The MCP server runs locally. From inside an AIDA project (any directory containing a `.aida/` directory):

```bash
aida mcp-serve
```

That's the server. Configure your MCP client to connect to it. For a Codex client, use `docs/agents/codex-mcp-setup.md`; STORY-398 verified the local Codex roundtrip and captured the working steps.

A `.mcp.json` is scaffolded at project init for Claude Code MCP integration. Other MCP clients typically need their own config; the connection target is the same `aida mcp-serve` process. Auth is local-socket today; cross-machine + auth is a deferred SPIKE.

`aida mcp-serve` is a long-running process, but it now checks the on-disk `aida --version` after each handled request. If the package version is newer, or the same version has a different build SHA, the server flushes the current response and self-respawns so the next MCP request uses the new binary. If a client still appears stale, kill that agent's `aida mcp-serve` process and let the MCP client respawn it.

When working from an AIDA source checkout, `cargo run -p aida-cli -- pr ship` is supported. The wrapper reinvokes the current development binary for post-merge `aida pull` and `aida session end`, so agents do not need an installed `aida` on `PATH` just to complete the ship flow.

## Conventions you should follow

When you participate in an AIDA project — whether as implementer, reviewer, advisor, or other role — these conventions matter:

### 1. Spec IDs in code comments

When you write code that implements a spec, add an inline trace:

```rust
// trace:TASK-440 | ai:codex
fn add_output_schemas() { ... }
```

Format: `// trace:<SPEC-ID> | ai:<agent>[:<confidence>]`. The `<agent>` is your name (codex, cursor, claude, etc.). Confidence is high (implied), `med` (40-80% AI), or `low` (<40% AI). **SPEC-IDs live in code, commits, and plan files — never in user-facing CLI output or `--help` text** (those leak internal identifiers to first-users).

### 2. Commit message format

```
[AI:codex] feat(scope): description (SPEC-ID)
```

- `[AI:codex]` prefix when the commit includes AI-assisted code (any file with a `trace:` comment).
- `type` is required: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
- `(SPEC-ID)` parenthesized at the END of the subject — the auto-bump scanner looks here.
- End multi-paragraph commit messages with `Co-Authored-By: <Your Agent Name> <noreply@…>`.

### 3. PR titles mirror commit subjects

Use the same format. PR bodies should reference the spec(s) being addressed.

### 4. Tag canonicalization (file_finding specifically)

When filing a finding from a reviewer perspective, use `from-review:PR-<NUMBER>` (with the PR-prefix). When from an implementer perspective, use `from-implementer:<SPEC-ID>`. **TASK-437** just fixed a bug where pr-as-string in `file_finding` produced non-canonical tags; the now-canonical form is `PR-<NUMBER>` with prefix.

### 5. Lease-aware operations

Before claiming a spec, check `list_active_leases()`. Don't overwrite another agent's in-flight work. If you must take over, do so explicitly via `release_task(other_lease_id)` followed by your own `claim_task`. Don't silently race.

### 6. Punt when you can't safely resolve

If you hit a design fork you can't resolve from the spec text, the surrounding code, or the project's substrate (memories, discipline docs, prior commits): **call `post_punt` instead of guessing.** Punting is a first-class outcome, not a failure. AIDA's punt → advisor → human escalation tier (STORY-306) is designed around the assumption that agents punt when uncertain.

## Cross-agent skill-invocation surface

Different agent types have different conventions for invoking AIDA workflows (the bundled "skills" like `aida-pickup`, `aida-pr`, etc.). The substrate IS the same — only the operator-facing syntax varies. If you're a non-Claude agent looking at a CLAUDE.md or skill template that references `/aida-foo`, this map tells you the equivalent.

| Workflow | Claude Code (slash) | Codex CLI | Antigravity CLI / MCP agents | What it does |
|---|---|---|---|---|
| **aida-pickup** | `/aida-pickup [SPEC]` | `aida queue work [SPEC]` (or `.codex/skills/aida-pickup` when scaffolded) | `aida queue work [SPEC]` | Read spec + transition to in-progress + drive implementation |
| **aida-pr / pr ship** | `/aida-pr` | `aida pr ship` | `aida pr ship` | Commit + push + open PR + auto-queue reviewer story |
| **aida-req** | `/aida-req` | `aida add --type <T> --title <S>` | `aida add ...` (or MCP `add_requirement`) | File a new spec |
| **aida-commit** | `/aida-commit` | `git commit` with trailer | `git commit` with trailer | Enforce `[AI:tool] type(scope): subject (SPEC-ID)` format |
| **aida-implement** | `/aida-implement [SPEC]` | `aida show <SPEC>` + edit + ship | `aida show <SPEC>` + edit + ship | Implement a spec end-to-end |
| **aida-doc** | `/aida-doc` | manual file edits + commit | manual file edits + commit | Document architecture/design |
| **aida-search** | `/aida-search <q>` | `aida search <q>` (or MCP `search_requirements`) | `aida search <q>` (or MCP) | FTS5 search across specs |
| **aida-plan** | `/aida-plan [SPEC]` | `aida plan verify` / `aida ultraplan` | same | Plan an implementation; verify against template |
| **aida-findings** | (slash variants) | `aida findings add/list/promote/dismiss` (or MCP `file_finding`) | `aida findings ...` (or MCP) | Advisor observation entry + triage flow |
| **aida-onboard** | `/aida-onboard` | read AGENTS.md + this doc | read AGENTS.md + this doc | First-session orientation |

**Foundational rule**: `aida` CLI verbs are the substrate — Claude Code's slash commands and Codex's skill descriptors wrap them. If you don't know the slash/skill name for your agent type, run the CLI verb directly. It works for every agent type.

**MCP path (always available)**: regardless of agent type, the `aida mcp-serve` MCP tools (the 29 documented above) are the canonical machine-to-machine surface. Use MCP for spec-graph operations; use CLI for orchestration verbs (`aida session start`, `aida pr ship`, `aida queue work`, etc.) since those manage substrate state that doesn't fit a stateless MCP call.

## What's in flight / known rough edges

Specs you'll want to track because they affect your operation:

| Spec | What it does | Status (as of this brief) |
|---|---|---|
| **TASK-440** | Adds `outputSchema` descriptors to the MCP tools | Shipped |
| **TASK-438** | Fixes TOCTOU race on `claim_task` (two concurrent claims could both succeed) | Approved |
| **BUG-310** | MCP-created specs not consistently visible to local CLI | Shipped |
| **STORY-398** | Empirical Codex roundtrip + `docs/agents/codex-mcp-setup.md` | Shipped |
| **STORY-399** | Path B — emit `structuredContent` matching `outputSchema` | Approved, deferred until STORY-398 surfaces need |
| **STORY-82** | Modernize the older spec-graph tools to current CLI vocabulary | Approved, post-STORY-398 |
| **EPIC-27** | MCP server modernization: mirror the full AIDA CLI surface | Strategic container; ongoing |
| **BUG-307** | Orchestrator auto-cleans dormant leases (reduces "lease stuck" friction) | Shipped |
| **BUG-311** | `aida queue work --steal` reliability fix for dormant-lease cleanup | Shipped |

If you hit a rough edge in any of these areas, **file it via `file_finding`** rather than working around it silently. The finding becomes substrate the project uses to fix it.

## Key references

In priority order for an agent boarding the project:

1. **`CLAUDE.md`** (project root) — the project's own orientation; conventions, architecture, the MCP positioning. *Required reading.*
2. **`docs/aida/discipline/`** — six canonical guides on workflow, lifecycle vocabulary, advisor role, session discipline. The conventions that make an AIDA project run well.
3. **`docs/agents/aida-mcp-install-matrix.md`** — per-client MCP setup, marketplace/package surface, and safe write-tool posture for Claude, Codex, Cursor, Windsurf, Continue, Cline, Copilot, Devin, and others.
4. **`docs/spikes/2026-05-20-spike-9-mcp-as-bus.md`** — the architectural verdict on filesystem-canonical + MCP-as-transport. Explains *why* the surface looks the way it does.
5. **`docs/spikes/2026-05-20-spike-11-session-forking.md`** — fork-from-live advisor (STORY-360, shipped). Lets a live advisor session be consulted during a drain.
6. **`docs/multi-advisor-coordination.md`** — SPIKE-10 verdict on subsystem-scoping + sibling-advisor initiation. The shape this brief is the first concrete instance of.
7. **`docs/writeups/2026-05-20-autonomy-keystone-day.md`** — narrative of the autonomy keystone shipping. Useful context for how the project ships work end-to-end.
8. **`OVERVIEW.md`** — strategic vision, public face, surface inventory.
9. **`aida-core/templates/docs/aida/discipline/lifecycle-vocabulary.md`** — the precise verbs (Draft / Approved / Planned / In Progress / Done / Completed / Released) and the auto-bump mechanics that turn Done → Completed.

## The strategic context — why this matters

AIDA's bet is that the next phase of agent collaboration isn't "smarter agents" but "shared substrate that all agents can coordinate against." Today every coding agent runs in its own isolated context window with its own scratchpads and its own private notes. Switching agents — or running multiple in parallel — means losing context.

The MCP server is the **substrate-as-shared-coordination-surface** made operational. When you (Codex / Cursor / future agent) attach to an AIDA project and use these 29 tools, you're not running on AIDA's island; you're contributing to a graph that Claude Code, the human, and any other agent are also working in. Findings filed via MCP show up in `aida findings list`. Punts you raise route to the same advisor tier human punts route to. Briefs routed to you can be listed, read, and acknowledged through MCP. Specs you implement get traced via the same `trace:SPEC-ID` convention any other agent uses.

This is what makes the "agent-agnostic" positioning real rather than rhetorical. Your participation evidences it.

## When you encounter friction

File it via `file_finding`. The substrate captures the friction so it can be fixed. The next agent (or the next session of you) inherits the fix rather than rediscovering the problem. That's how the project gets better.

If something is broken in a way that prevents you from doing useful work — `aida mcp-serve` won't start, a tool returns unexpected errors, the schema doesn't match the body, anything that blocks — fall back to `post_punt({spec_id, detail: "tool dysfunction: …", category: "tool-error"})`. The advisor tier will escalate to a human.

---

*This brief was written 2026-05-22 by the AIDA project's live advisor for sibling-agent onboarding. STORY-398 produced the authoritative Codex setup doc at `docs/agents/codex-mcp-setup.md`; that doc supersedes the Codex-specific setup details here.*
