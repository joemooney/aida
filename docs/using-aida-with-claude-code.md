# Using AIDA with Claude Code

*Last updated: 2026-06-01. Claude Code's surfaces move fast — if a primitive named here has changed, re-verify against the current Claude Code docs and update this page. Per-neighbour deep dives live in [`docs/positioning/`](positioning/).*

**If you already know Claude Code, here is AIDA in one breath:**

> Claude Code orchestrates a **task** — it fans out subagents, runs Workflows, spawns background agents, and produces an answer or a diff, then the conversation ends. **AIDA remembers your project** — a persistent, git-canonical graph of *what exists and why*, with stable IDs, code→spec traces, and a spec lifecycle that survives every conversation ending. AIDA runs **on top of** Claude Code's orchestration, and the same graph is queryable by **every** vendor's agent, not just Claude.

Claude Code is the engine. AIDA is the project's memory and the rails the engine runs on. They are different layers, and they compose.

---

## The one distinction that explains everything

Almost every Claude Code primitive shares one property: **it lives and dies with a conversation (or a run).** A subagent's context window closes when the chat ends. A Workflow records its results in script variables and finishes. A background agent completes its task. None of them, by design, maintain a durable model of your project across time.

AIDA is the part that *doesn't* end:

| | Claude Code primitives | AIDA |
|---|---|---|
| **Scope** | a task / a conversation / a run | a project, over its whole life |
| **Lifetime** | ephemeral — ends with the chat or run | persistent — lives in git |
| **State** | in the context window or script variables | in the orphan `aida-store` branch (YAML per spec) + trace comments in source |
| **Identity** | none — agents know the prompt you gave them | stable `SPEC-ID`s that survive renames, merges, releases, *and vendor switches* |
| **Question it answers** | "do this thing now" | "what exists, why, and is this code still tied to a live requirement?" |
| **Vendor** | Claude Code | any agent — Claude, Codex, Antigravity — reading one git-canonical store over MCP |

Hold that table in mind and every primitive below slots into place.

---

## Where AIDA sits vs each Claude Code primitive

### Subagents (`/agents`, `.claude/agents/`)
A subagent is a **callable** — a specialized prompt + tool allowlist + fresh context window, invoked inside one conversation, gone when it ends. AIDA's **roles** (`implementer`, `reviewer`, `advisor`) are **positions in a lifecycle** — full `claude` processes in their own git worktrees, holding leases, anchored to a SPEC-ID, with state that outlives every session. A subagent is a *thing you delegate to*; a role is a *seat in a system*. **They compose:** an AIDA role can spawn subagents inside it.
→ deep dive: [vs-claude-code-subagents.md](positioning/vs-claude-code-subagents.md)

### Workflows (`/workflows`)
A Workflow is **within-task orchestration**: a JS script fans out dozens-to-hundreds of subagents, holds the plan in code, and ends with an answer/artifact. AIDA's `aida queue work --auto-complete` is a **spec-lifecycle** orchestrator: it drives one requirement through implement → CI → review → merge and *records the outcome in the persistent graph*. A Workflow produces a report; an AIDA drain produces a merged PR **and** a spec that now knows it's `Completed`. The orchestration *mechanism* overlaps — and Claude Code commoditizing it is good news: AIDA delegates to it rather than competing.
→ deep dive: [vs-claude-code-workflows.md](positioning/vs-claude-code-workflows.md)

### Agent teams (experimental)
A lead agent splits a project into pieces across a shared task list with inter-agent messaging — within a session. AIDA provides the *durable* version of that coordination: the queue (`--for <role>`, `batch:` tags, scope routing), leases that prevent two agents touching the same scope, and a spec graph both agents query. Where agent teams coordinate *now*, AIDA coordinates *across sessions, agents, and vendors*.

### MCP
This is the tightest fit: **AIDA *is* an MCP server.** `aida mcp-serve` exposes the requirement graph as tools (`list_requirements`, `show_requirement`, `query_graph`, `add_requirement`, `search_requirements`, `add_relationship`, the mailbox + brief tools…) and resources (`aida://project/summary`, `aida://requirements/tree`). So any Claude Code session — and any subagent or Workflow agent, which inherit the session's MCP connections — can *query your project's graph mid-task*. This is the channel through which all the composition below happens.

### Background agents (`claude --bg`)
Claude Code's background supervisor runs detached sessions you watch in `claude agents`. AIDA *uses* this: `aida agent new claude --bg --spec <ID>` launches a tracked, worktree-isolated, spec-anchored background session and records the sessionId on the lease so the cross-substrate view links them. AIDA is deliberately **divesting** its hand-rolled process plumbing toward native `--bg` (SPIKE-34) — let Claude Code own supervision; AIDA owns the substrate.

### Skills & slash commands
`aida init` scaffolds ~40 skills under `.claude/skills/` + matching `/commands` — `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-plan`, `/aida-review`, `/aida-capture`, and more. These are ordinary Claude Code skills; AIDA's contribution is that they're **substrate-aware** — they read and write the graph, route the queue, and enforce traces, instead of operating on loose markdown.

### Memory & `CLAUDE.md`
`aida init` scaffolds a discipline pack (`docs/aida/discipline/`), a `CLAUDE.md` discipline section, and (opt-in) a starter memory pack. Where Claude Code's memory is per-developer guidance, AIDA's scaffolding ships the *project's* vocabulary and workflow habits so a new agent inherits them.

---

## How they compose — three directions

The relationship is **complementary**, and it runs in three directions:

**1. Claude Code reads AIDA (grounding).**
A subagent or Workflow agent calls AIDA's MCP tools mid-task: *"does this already exist?"*, *"what are STORY-489's acceptance criteria?"*, *"which specs trace to `git_backend.rs`, and are they still live?"* The ephemeral agent borrows durable project context it has no other way to know.

**2. AIDA delegates to Claude Code (execution).**
AIDA uses Claude Code primitives as the *implementation* of its phases:
- the **implementer** phase → a `claude` (or `claude --bg`) session in an isolated worktree;
- the **reviewer** phase → could fan out adversarial reviewers as a **Workflow**;
- the **planning** phase → a multi-angle judge-panel Workflow (this is what `aida ultraplan` already feeds `/ultraplan`);
- the **drain** itself → compiled to a *saved* `workflow.js` artifact that Claude Code's runtime replays (SPIKE-32).

**3. Claude Code writes back to AIDA (hooks).**
The first two assume AIDA is *driving* (it delegates) or being *read*. But Claude Code also orchestrates on its own — you run a Workflow, spawn subagents, launch a `--bg` agent — with AIDA nowhere in the loop. AIDA's **hook bundle** (SPIKE-41) closes that gap: it captures the *effects* of harness-driven work into the substrate as they happen, so the graph stays live no matter who orchestrates.
- `WorktreeCreate` / `WorktreeRemove` (fired by a Workflow's `isolation: "worktree"`) → register/release an AIDA **lease** (TASK-634) — the harness's parallel worktrees become lease-tracked.
- `PostToolUse` on `git commit` / `gh pr` → flip spec **lifecycle**, run the auto-bump, enforce `// trace:` comments.
- `SubagentStart` / `SubagentStop` → record **provenance** (which agent did what).

The boundary that matters: hooks make the substrate **capture** any orchestrator's effects — they do **not** make the orchestrator *read the graph to decide* (that "decide-from-the-graph" role stays AIDA's drain alone). So this direction makes the substrate **antifragile to which orchestrator runs**: when AIDA's drain isn't driving — even when it's *unavailable* — the work still lands in the graph instead of leaking away.

> The motivating case: a sibling project hit a drain bug (BUG-431), fell back to a Claude Code Workflow, shipped the code — and populated *zero* substrate (no lifecycle, no leases, no traces). The hook bundle is what would have kept that work in the graph. See [vs-claude-code-workflows.md → "Worked example: quizdom"](positioning/vs-claude-code-workflows.md).

> Net: **AIDA supplies the substrate and the lifecycle semantics; Claude Code supplies the orchestration and the raw model work — and the hooks keep the substrate populated even when Claude Code orchestrates alone.** Each is stronger because of the other.

---

## A day using both

1. You're in a Claude Code session. You ask it to plan a feature; it queries AIDA's graph over MCP to see what already exists and what blocks what. — *Claude Code reads AIDA.*
2. You file the work: `/aida-req` (or `aida add`) creates a spec with a stable ID. — *AIDA persists.*
3. You drain it: `aida queue work <ID> --auto-complete`. AIDA spins up a worktree-isolated implementer session, waits on CI, runs a reviewer, merges. — *AIDA delegates to Claude Code, then records the result.*
4. The spec auto-flips to `Completed`; a `// trace:<ID>` comment lands in the code; the commit carries `(<ID>)`. — *AIDA's durable layer.*
5. Six months later, a *Codex* agent asks "why does `validate_token` exist?" and gets the answer from the same graph. — *Cross-vendor, cross-time. The part no single conversation could hold.*

---

## What AIDA deliberately does NOT do

AIDA is not trying to be a better orchestrator than Claude Code. It deliberately *defers*:

- It doesn't reinvent within-task fan-out — that's Workflows.
- It doesn't reinvent process supervision — that's `claude --bg`.
- It doesn't reinvent the model work — that's Claude (and Codex, and Antigravity).

AIDA owns the layer those tools structurally don't: the **persistent, vendor-neutral, git-canonical record of what your project is** — stable IDs, enforced traces, typed relationships, and a lifecycle — queryable by any agent, surviving every conversation. *The orchestration getting better makes AIDA better.* The only mistake would be confusing the orchestrator for the moat.

---

## See also

- [vs-claude-code-subagents.md](positioning/vs-claude-code-subagents.md) — roles vs subagents (the *position* vs the *callable*).
- [vs-claude-code-workflows.md](positioning/vs-claude-code-workflows.md) — the spec-lifecycle vs within-task distinction + the commoditization read.
- [docs/agents/aida-mcp-install-matrix.md](agents/aida-mcp-install-matrix.md) — connecting AIDA's MCP server to Claude Code, Codex, Cursor, Windsurf, and the rest.
- [docs/agents/claude-plugin-package.md](agents/claude-plugin-package.md) — packaging AIDA's Claude Code-facing setup for the marketplace.
- [OVERVIEW.md](../OVERVIEW.md) — the Trojan-horse framing and the full vision.
- [docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md](competitive-analysis/2026-05-31-round2-moat-gaps-moves.md) — the current moat / commoditization synthesis.
