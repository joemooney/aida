---
marp: true
title: "AIDA — the agent-collaboration layer"
description: "Introductory demo (semi-technical). Live-demo-led; these slides are the scaffold."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/presentation/2026-06-management-demo.md -o aida-demo.html
  npx @marp-team/marp-cli@latest docs/presentation/2026-06-management-demo.md --pdf -o aida-demo.pdf
AUDIENCE: technical leadership.  GOAL: buy-in / resources.
FORMAT: the LIVE DEMO is the argument; these slides are the spine + fallback.
Speaker notes live in HTML comments under each slide. Live-demo cues are marked >>> LIVE.
The runbook (exact commands, timing, fallbacks) is docs/presentation/demo-runbook.md.
-->

# AIDA

### The agent-collaboration layer

A git-canonical requirement graph that survives across **agents, sessions, and vendors.**

<small>June 2026 · v0.11.0</small>

<!--
Open cold. One sentence: "I'm going to show you the thing, not tell you about it — but first, 90 seconds on the problem it solves, because if you've run coding agents at any scale you've already felt it."
Don't sell here. Set up the pain.
-->

---

## The problem: agents are powerful and often amnesiac

- **Session context management is deceptively complex**
- **Decision reevaluation is costly and error-prone** — we need improved memory
- Code and intent **drift apart silently** — no link from what to why
- Now multiply by **N agents** (Claude, Codex, Antigravity…) and **N people**

> A markdown Product Requirements Document (PRD) is the floor. 
It does not survive multi-agent, multi-session, multi-vendor reality.

<!--
the floor exists (everyone has a TODO.md / structured markdown). The question is what's above the floor.
AIDA is what's above the floor.
-->

---

## What AIDA is

**Your project's missing index** — a stable, queryable graph of *what exists and why*, served to agents via **MCP** and to you via a small **CLI**.

- **Stable spec IDs** 
- **typed relationships** 
- **code→ spec trace comments** 
- **lifecycle**
- Lives in **git** — portable, vendor-neutral, **no SaaS dependency**

Three questions become *one query away* — for the agent and for you:

- *"Does this already exist?"* / *"Why did we choose X?"* 
- *"Is this code still tied to a live requirement?"*

<!--
Keep this tight — 45 seconds. Don't enumerate features; name the shape.
Transition: "Rather than walk the architecture, let me show you the loop. This is a real spec, a real drain, a real merge — not a mockup."
Switch to terminal. → demo-runbook.md §1.
-->

---

## >>> LIVE: idea → merged, autonomously

<!-- _class: lead -->

**file a spec → queue it → walk away**

```
aida add --title "..." --type task
aida queue work <ID> --auto-complete
```

implementer → CI → review → merge → the spec **auto-flips to Completed** → the **trace comment lands in the code**

<!--
THIS IS THE CENTERPIECE. Runbook §1 + §2.
Narration while it runs:
  - "I file a requirement. It gets a stable ID — that ID will follow it through code, commits, and the PR."
  - "I hand it to the orchestrator and stop typing."
  - "Implementer works in an isolated worktree. CI runs. A reviewer agent reads the *code* — not the commit message — and votes. It merges."
  - "Watch the spec." → aida show <ID> → status Completed, linked PR, linked commit, trace comment.
KILLER LINE: "I didn't merge that. The orchestrator did — and the requirement knows it's done. Nobody updated a status by hand."
FALLBACK if the live drain stalls: cut to the recorded asciinema cast (runbook §6). Do not debug live.
-->

---

## >>> LIVE: the graph *inside* the agent

<!-- _class: lead -->

In Claude Code, over **MCP**:

```
query_graph · show_requirement · blocked-by chains · search
```

The agent holds **structured context** — not a flat file it has to re-parse every session.

<!--
Runbook §3. Open Claude in the repo; ask it a graph question out loud:
  "What's blocking EPIC-30, and what would completing STORY-X unblock?"
Let Claude call query_graph live and answer from the graph.
KILLER LINE: "The agent isn't grepping a markdown file. It's querying a typed graph. That's the difference between the floor and a moat."
-->

---

## Why not "20 lines of bash"?

The surface looks trivial **on purpose.** The depth compounds:

| Looks like | Actually is |
|---|---|
| a TODO list | a **typed graph** — transitive blocked-by / impact queries |
| **file names that drift** | **stable IDs** that survive renames, merges, vendor switches |
| **an unchecked comment** | **enforced traces** — code→spec links checked at commit |
| one agent | **MCP**: any agent (Claude/Codex/Antigravity) reads one **git-canonical** substrate |
| a status field | **lifecycle**: auto-bump on merge, autonomous drain, advisor escalation |

> Apparent simplicity masks a deep web of complexity.

**"But doesn't Claude Code already do this?"** → Claude Code orchestrates a *task*. **AIDA remembers your *project*** — and runs *on top of* their orchestration, across *multiple* vendor.

<!--
This is the buy-in slide for a technical audience — they will respect "the hard part is underneath."
If asked "couldn't a vendor add this?" → "They'd have to ship the YAML-canonical store, node-aware IDs, the cache/projection model, the MCP server, the trace convention, the relationship graph, the role/session/worktree model, and the lifecycle engine. Months. And because ours lives in git, it's vendor-neutral by construction — that's the part a single-vendor tool structurally can't copy."
THE CLAUDE-CODE QUESTION WILL COME (technical room). Expand the one-liner: "Claude Code's subagents, Workflows, and agent teams orchestrate a TASK — fan out agents, produce an answer, end. AIDA is the persistent layer underneath: the requirement graph, stable IDs, code→spec traces, the lifecycle. A Workflow can't tell you what exists or why six months from now — and it can't even drive a Codex session. AIDA runs ON their orchestration and outlives any single run, across every vendor. Their orchestration getting better makes AIDA better — we delegate to it." Backing detail: docs/positioning/vs-claude-code-workflows.md + vs-claude-code-subagents.md.
-->

---

## Where we are — and what's next

**Proof: AIDA builds AIDA (fully dogfooded)**

- **1,671** specs in the graph · **983** completed · **19** releases (latest **v0.11.0**) · **1,429** commits
- **Multi-agent**: Claude + Codex + Antigravity drive one shared git-canonical substrate
- **Evolution of formalism through experimentation**
- **Persistent autonomous worker** (EPIC-30) — continuous queue drain, not one-shot
- **Multi-vendor substrate interop** — the durable, structurally-defensible wedge
- **Public launch + marketplace distribution**

<!--
Numbers are live from the substrate as of the deck date — re-run `aida list --all` the morning of to refresh.
The numbered list is the roadmap / what's next — speak the resource ask out loud rather than putting a number on the slide.
Close: "Everything you just watched is built by this system, on this system — and it's getting more capable as we go."
-->

---

## Architecture

```
        you ────CLI────┐                ┌── Claude/Codex/Agy (MCP)
                       ▼                ▼
                ┌────────────────────────────┐
                │  AIDA engine (aida-core)   │
                │  graph · IDs · lifecycle   │
                └────────────────────────────┘
              write-through │  ▲ rebuildable
                            ▼  │ projection
   git: hidden `aida-store` branch     .aida/cache.db
   (YAML per spec — the writer of record)  (fast reads)
```

- **Writer of record = git.** One YAML file per requirement. Survives offline, clones cleanly, diffable.
- **Cache is disposable** — rebuilt from git on staleness. No SaaS, no lock-in.

---

## Spec lifecycle

<style scoped>
pre { font-size: 12px; line-height: 1.1; }
</style>

Every spec travels the same path — *filed an idea* → *users have it*:

```
   ┌─────────┐
   │  Draft  │   filed, not yet agreed
   └────┬────┘
        │        aida edit SPEC --status approved
        ▼
   ┌──────────┐
   │ Approved │  agreed — ready to schedule
   └────┬─────┘
        │        (optional) aida edit SPEC --status planned  ·  /aida-plan
        ▼
   ┌─────────┐
   │ Planned │   scheduled into a sprint or cycle
   └────┬────┘
        │        aida queue work SPEC   → spawns a Claude session in a fresh worktree
        ▼
   ┌─────────────┐                ┌─────────────────┐
   │ In Progress │ ──aida punt──► │ Needs Attention │  paused — an agent
   └────┬────────┘                └─────────────────┘  punted a design-fork
        │    /aida-pr   → push branch + open PR        it couldn't resolve;
        ▼                                              awaits triage
   ┌────────┐
   │  Done  │      PR open on GitHub, awaiting CI + review
   └────┬───┘
        │          /aida-review → approve · gh pr merge --squash · aida pull
        ▼
   ┌───────────┐
   │ Completed │   merged to main — auto-bumped by aida pull
   └────┬──────┘
        │          make release-minor   → aggregates many completed specs
        ▼
   ┌──────────┐
   │ Released │    version tagged, binaries published
   └──────────┘
```

<!--
The state machine behind the LIVE drain. "Draft → Approved → Planned → In Progress → Done → Completed → Released — each transition is a command or an agent action. The drain you watched walks In Progress → Completed on its own. 'Needs Attention' is the one off-mainline state: an agent punts a fork it can't safely resolve rather than guessing."
-->

---

## The autonomy ladder

Three modes, picked per session — **quality vs. throughput is an explicit dial:**

- **default** — human at the keyboard, agent pauses on design forks
- **`--zen`** — agent proceeds on defensible defaults, pauses only on real forks
- **`--no-human`** — headless drain; on an unsafe fork the implementer **punts** → a headless **advisor** resolves it from recorded principle or **escalates to a human**

> Implementer → advisor → human. The system knows what it can't decide, and asks.

---

## The drain, concretely — `--auto-complete`

<style scoped>
pre { font-size: 13px; line-height: 1.15; }
</style>

Steps 3–7 (implement · CI · review · merge · pull) collapse to one command — a process tree:

```
   $ aida queue work SPEC --auto-complete       ← orchestrator process (your terminal)
   │
   ├─▶ Phase 1: spawn implementer Claude  ─────►  [Claude session — implements SPEC, ─┐
   │   (waits for it to exit)                     runs /aida-pr, exits]               │
   │◀──── detects exit ───────────────────────────────────────────────────────────────┘
   │
   ├─▶ Phase 2: end session + wait for CI       (deterministic — no Claude session)
   │
   ├─▶ Phase 3: spawn reviewer Claude  ────────►  [Claude session — reviews PR,  ─────┐
   │   (waits for it to exit)                     writes verdict, exits]              │
   │◀──── detects exit ───────────────────────────────────────────────────────────────┘
   │
   ├─▶ Phase 4: gh pr merge                     (deterministic)
   ├─▶ Phase 5: aida pull + auto-bump           (deterministic)
   └─▶ Phase 6: cargo build verify              (deterministic)
```

Two Claude sessions spawn (phases 1 + 3), each in its own worktree; the orchestrator runs phases 2/4/5/6 itself. `--zen` / `--no-human` change *which prompts pause vs. auto-resolve*, not the shape.

<!--
This is the "what just happened in the LIVE demo, under the hood" slide — pairs with the autonomy ladder.
"The one-liner you saw is this tree. The two judgment phases — implement, review — are Claude sessions; everything else the orchestrator does deterministically. That's why it can run unattended."
-->

---

## Competitive

- **vs. GitHub Spec Kit** — they produce structured specs per feature, then *freeze* them. AIDA keeps them a **maintained, cross-cutting graph** that outlives the feature.
- **vs. Karpathy-style structured markdown** — that's the **floor**. AIDA adds the graph, identifier stability, enforced traces, MCP.
- **vs. SaaS PM tools (Linear/Jira)** — **git-canonical, no SaaS dependency**; the data is yours and vendor-neutral.

<small>Full neighbor-by-neighbor analysis: `docs/positioning/`, `docs/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md`</small>

---

## The Trojan-horse framing

The visible product is *intentionally humble*: a veneer that wraps Claude Code/Codex/Agy sessions.

1. **Low adoption barrier** — looks like something you'd build yourself, so you try it
2. **The depth is discovered after** — graph, IDs, traces, MCP, lifecycle surface through use
3. **The platform is the durable value** — the TUI is the easy part; the substrate underneath is months of foundation

> *The CLI is what people think AIDA is. The platform is what AIDA actually is.*

---

## Miscellaneous

---

## AIDA vs RAG ("isn't this just retrieval?")

**No — opposite retrieval models, and they compose.**

RAG answers *"what text resembles my question?"* 
AIDA answers *"what is this requirement, why does it exist, and what code implements it?"*

**They compose:** RAG can index AIDA's YAML for fuzzy recall; agents get the *precise* answer from AIDA's graph over MCP and the *fuzzy* one from RAG. 
AIDA is the **structured ground-truth layer** a RAG pipeline retrieves *against*, not a competitor to it.

---

## AIDA vs RAG ("is this embeddings?")

RAG is similarity search over unstructured prose — lossy, no identity, the spec→ code link is a guess. 
AIDA is a typed graph with stable IDs queried exactly — the link is enforced, not retrieved. 
You point a RAG pipeline AT AIDA, not replace AIDA with one.
We could add semantic search over the graph as a convenience — but the critical part is the deterministic structure underneath

---

## AIDA / RAG

| | RAG | AIDA |
|---|---|---|
| Retrieves | text chunks *similar* to a query | the *exact* spec, by ID |
| Method | vector similarity (probabilistic, fuzzy) | graph query (deterministic, exact) |
| Returns | prose that *looks* relevant | status, typed relationships, the code that traces to it |
| Truth | approximate recall, can hallucinate the link | ground truth — the link is enforced at commit |
| Freshness | re-embed on change | a structured write; always current |



