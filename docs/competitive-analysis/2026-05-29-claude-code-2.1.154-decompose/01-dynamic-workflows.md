# SPIKE-14: Claude Code Dynamic Workflows (2.1.154)

**Date**: 2026-05-29
**Source-verified**: Yes — official docs at [code.claude.com/docs/en/workflows](https://code.claude.com/docs/en/workflows), changelog entries 2.1.154 / 2.1.152 / 2.1.149, Anthropic blog announcement, multiple third-party guides cross-referenced.
**Verdict**: **COMPOSE** — workflows are the right substrate for *orchestration runtime*. AIDA should divest the bespoke orchestrator state machine and become the spec-graph + reasoning-supervisor + workflow-compiler layer above it.

---

## What dynamic workflows actually are

Verbatim from Anthropic:

> A dynamic workflow is a JavaScript script that orchestrates subagents at scale. Claude writes the script for the task you describe, and a runtime executes it in the background while your session stays responsive.

The key structural shift, in their own framing:

> A workflow moves the plan into code. With subagents and skills, Claude is the orchestrator: it decides turn by turn what to spawn next, and every result lands in Claude's context. A workflow script holds the loop, the branching, and the intermediate results itself, so Claude's context holds only the final answer.

This is the inversion AIDA's orchestrator embodies in Rust today — but Anthropic ships it as the platform primitive, in JavaScript-as-data, with `/workflows` as the management surface and **up to 16 concurrent / 1000 total agents per run**.

### Comparison table (Anthropic's own)

| | Subagents | Skills | Workflows |
|---|---|---|---|
| What it is | Worker Claude spawns | Instructions Claude follows | Script the runtime executes |
| Who decides what runs next | Claude, turn by turn | Claude, following the prompt | The script |
| Where intermediate results live | Claude's context window | Claude's context window | Script variables |
| What's repeatable | Worker definition | Instructions | The orchestration itself |
| Scale | A few delegated tasks per turn | Same as subagents | Dozens to hundreds of agents per run |
| Interruption | Restarts the turn | Restarts the turn | Resumable in the same session |

## Concrete mechanism

1. **Workflow definition**: JavaScript script. Claude writes it on the fly from a prompt containing the word `workflow`, OR from `/effort ultracode` mode (which auto-plans workflows for substantive tasks).
2. **Invocation surface**:
   - `/workflows` slash command — lists running and completed workflows with progress overlay
   - Inline trigger: any prompt with the word `workflow` highlights and triggers
   - `/effort ultracode` — automatic for substantive tasks
   - Saved as `/<name>` commands via `s` in the progress view
3. **Save locations**:
   - `.claude/workflows/` — project-scoped, team-shared via git
   - `~/.claude/workflows/` — user-scoped, personal
   - Project wins on name collision
4. **State management**:
   - Intermediate results live in **script variables** (not Claude's context, not the substrate)
   - Cached agent results enable mid-session resume
   - **Exit Claude Code while running → workflow is lost** (the explicit doc admission: *"If you exit Claude Code while a workflow is running, the next session starts the workflow fresh"*)
5. **Failure handling**:
   - Stop/pause/resume per-agent via the `/workflows` keys (`p` pause, `x` stop, `r` restart agent)
   - No automatic retry policy documented
   - No "advisor punts on inconclusive" equivalent — the script must encode resilience itself
6. **Concurrency / limits**:
   - 16 concurrent agents max (fewer on CPU-limited machines)
   - 1,000 agents total per run
   - "No mid-run user input — only agent permission prompts can pause a run"
7. **Permission model**:
   - Default / accept edits: prompted every run unless "don't ask again" selected
   - Auto mode: first launch only
   - Bypass / `claude -p` / SDK: never prompted
   - **Subagents always run in acceptEdits regardless of session mode** (load-bearing detail)
8. **Observability**:
   - `/workflows` opens the run, shows phases × agent count × tokens × elapsed
   - Drill-down into per-agent prompt + recent tool calls + result
   - Task panel below input shows running-workflows line
9. **Bundled**: `/deep-research` — fan-out web search with cross-checking, claim voting, citation, filtering of claims that didn't survive cross-check
10. **Disable knobs**: `/config` toggle, `disableWorkflows: true` in settings, `CLAUDE_CODE_DISABLE_WORKFLOWS=1`, admin-tier managed-settings

## What AIDA's orchestrator does that workflows DON'T

The full list, mapping AIDA primitives to "where would this live if AIDA composed with workflows":

| AIDA primitive | Workflow analog | Gap |
|---|---|---|
| Spec graph (typed, stable IDs, lifecycle) | None | AIDA's substrate moat. Workflows are ephemeral. |
| Code↔spec linkage (`trace:SPEC-ID` comments) | None | AIDA-unique. |
| Queue with role routing (`for:implementer`) | None — workflows run for the session that started them | Workflows can't be picked up by Codex/AGY/etc. |
| Lease registry (`.aida/sessions/`) | None | Workflows are session-scoped, no cross-session ownership semantics. |
| Lifecycle (Approved → InProgress → Done → Completed → Released) | None | Workflows have run-states (running, paused, complete) — flat, no graph. |
| Auto-bump on commit `(SPEC-ID)` trailer | None | The "merge triggers spec close" loop is AIDA-only. |
| Briefs to sibling agents (Codex, AGY) | None | Workflows are Claude Code single-runtime. |
| Findings substrate + recurrence promotion | None | Capture-then-promote is AIDA-only. |
| Calibration substrate (cold-boot vs fork-from-live, STORY-347) | None | |
| Punt → advisor → resume (STORY-306) | None — the script must handle its own failure recovery | This is exactly the gap that bit us in last night's 12-hour stall. Workflows would have the same shape: a step that hits an inconclusive outcome has no advisor tier to reason about it. |
| Cross-session persistence | None — *"the next session starts the workflow fresh"* | The defining gap. |

## What workflows do that AIDA's orchestrator DOESN'T

| Workflow capability | AIDA equivalent | Gap |
|---|---|---|
| Up to 16 concurrent agents per run | AIDA drains sequentially (one orchestrator per spec, one spec at a time) | AIDA's drain wall-clock could be ~16x faster on independent specs. |
| Script-defined branching + loops | Hardcoded 6-phase pipeline | AIDA can't easily express "if X then Y else Z" workflows without a code change. |
| `/deep-research`-style adversarial cross-check | None | The "have independent agents adversarially review each other's findings" pattern is workflow-native. |
| Save run as a reusable command | AIDA scaffolds skills via templates; no "save this run" verb | A run-becomes-a-command flywheel AIDA doesn't have. |
| Background-runtime isolation from session context | AIDA's orchestrator IS the foreground process | Workflows free up the operator's Claude session entirely. |
| Permission auto-grant for subagents (acceptEdits regardless) | AIDA worktrees use bypassPermissions | Comparable but workflows' guarantee is platform-enforced. |
| Native progress UI in Claude Code | AIDA has `aida queue progress` (CLI-only) | Workflow progress is in the same surface the operator is already in. |

## The strategic call: COMPOSE

The workflow runtime is structurally where orchestration should live. AIDA shouldn't compete on this axis — Anthropic owns the runtime, ships it on every paid plan, and has the concurrency primitives AIDA lacks.

But workflows are **runtime-only**. They don't have:
- Persistent substrate
- Stable IDs that survive across runs
- Code↔spec linkage
- Cross-tool dispatch
- Lifecycle as substrate-of-truth
- Reasoning supervisor for inconclusive outcomes
- Findings / calibration / discipline pack

These are AIDA's actual moat, exactly as the article-derived analysis predicted (`docs/competitive-analysis/2026-05-26-agent-memory-libraries.md`). Workflows make the case sharper.

### The compose architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  AIDA substrate (spec graph, traces, lifecycle, memory, findings)  │
│  ├─ Reads:  spec to drain, dependencies, plans, briefs            │
│  └─ Writes: status transitions, commits, lease updates, findings  │
└────────────────────┬───────────────────────────────────────────────┘
                     │ ① compile spec → workflow.js
                     ▼
┌────────────────────────────────────────────────────────────────────┐
│  AIDA workflow compiler — `aida queue work --as-workflow SPEC-X`   │
│  Emits a JavaScript workflow script the Claude Code runtime runs.  │
│  Script structure: 6-phase pipeline + AIDA-specific phases (lease  │
│  acquisition, auto-bump, brief sweep) + advisor punt callbacks.    │
└────────────────────┬───────────────────────────────────────────────┘
                     │ ② handoff to runtime
                     ▼
┌────────────────────────────────────────────────────────────────────┐
│  Claude Code workflow runtime (Anthropic platform)                 │
│  Executes the script, spawns subagents, holds intermediate state.  │
│  Up to 16 concurrent. Resumes on stop within session.              │
└────────────────────┬───────────────────────────────────────────────┘
                     │ ③ substrate callbacks via `aida` CLI
                     ▼
┌────────────────────────────────────────────────────────────────────┐
│  AIDA reasoning supervisor (Pattern A advisor wrapper)             │
│  Watches workflow events; intervenes on inconclusive/idle/stall.   │
│  Punts to a fresh `claude -p /aida-advise` for judgment.           │
└────────────────────────────────────────────────────────────────────┘
```

Each layer's role:
- **Substrate layer**: AIDA's moat. Persists across runs, agents, tools.
- **Compiler layer**: AIDA-built; emits workflow scripts. Replaces the Rust state machine.
- **Runtime layer**: Anthropic-built. AIDA does NOT build.
- **Supervisor layer**: AIDA-built. The advisor wrapper for inconclusive outcomes that broke last night.

### What AIDA divests

The Rust orchestrator state machine. Everything in `aida-cli/src/auto_complete.rs` and the phase machinery that today runs `claude -p` directly. **This is a lot of code — and it just bit us with the 12-hour stall.** Moving to workflow definitions means AIDA writes JS templates instead of Rust state transitions; Anthropic's runtime handles the actual phase execution.

### What AIDA invests in

1. **Workflow compiler** (`aida queue work --as-workflow SPEC` outputs `.claude/workflows/spec-X-pipeline.js`)
2. **Substrate-callback CLI surface** — make every state transition the workflow needs to record callable as `aida` subcommands the workflow script can invoke (most already exist: `aida edit --status`, `aida session start`, `aida queue done`, `aida brief codex`, etc.)
3. **Bundled workflow templates** — AIDA ships `.claude/workflows/` files that the workflow runtime can run: `aida-spec-pipeline.js`, `aida-batch-drain.js`, `aida-spec-research.js`
4. **Advisor wrapper** — Pattern A from yesterday's discussion. The supervisor that catches inconclusive outcomes the workflow script can't reason about, and substrate-corroborates from `git`/`gh` state.

## Specific scoped follow-ups to file

1. **SPIKE**: prototype `aida queue work --as-workflow SPEC` for a single spec. Measure: workflow file size, runtime cost, fidelity to current orchestrator behavior. **Target: 1 week.**
2. **SPIKE**: substrate-callback CLI audit — what `aida` subcommands does a workflow script need to call to record substrate updates? Inventory + identify gaps. **Target: 3 days.**
3. **STORY**: AIDA's advisor wrapper (Pattern A) — extend the punt mechanism (STORY-306) to fire on "inconclusive verification" + "idle stall." Independent of the workflow migration; valuable even if AIDA keeps the Rust orchestrator longer.
4. **DOC**: update `positioning.md` post-2.1.154. The "substrate + supervisor + compiler" framing replaces "we orchestrate multi-agent work" wherever the latter appears.
5. **SPIKE**: characterize cross-session persistence gap — can a workflow encode "AIDA substrate transitions" such that exiting Claude Code mid-run leaves the substrate in a recoverable state (NeedsAttention + clear resume verb)?

## Sources

- [code.claude.com/docs/en/workflows](https://code.claude.com/docs/en/workflows) — official documentation
- [code.claude.com/docs/en/changelog](https://code.claude.com/docs/en/changelog) — 2.1.154, 2.1.152, 2.1.149 entries
- [claude.com/blog/introducing-dynamic-workflows-in-claude-code](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code) — Anthropic announcement
- [marktechpost.com/2026/05/28/anthropic-ships-claude-opus-4-8-alongside-dynamic-workflows-and-cheaper-fast-mode-with-workflows-capped-at-1000-subagents](https://www.marktechpost.com/2026/05/28/anthropic-ships-claude-opus-4-8-alongside-dynamic-workflows-and-cheaper-fast-mode-with-workflows-capped-at-1000-subagents/) — third-party context on 1000-agent cap
- [github.com/barkain/claude-code-workflow-orchestration](https://github.com/barkain/claude-code-workflow-orchestration) — community plugin already extending workflows

## Operator follow-up (when you resume)

Drop the output of `claude /workflows --help` and `claude /effort --help` here so I can verify the doc-described commands match the actual installed surface. Especially interested in:
- Whether `/workflows` exposes a JSON or programmatic interface (an `aida` integration would need this)
- The exact schema of a saved workflow script (what AIDA's compiler would emit)
- Whether `/effort ultracode` exposes its triggering rules (could AIDA tag specs to auto-promote to workflow when picked up)

---

## Update 2026-05-29: confirmed at the operator's keyboard (Claude Code 2.1.156)

Operator dropped `/workflows --help`, `/effort --help`, `/deep-research --help` output from the actually-installed binary. Confirms and sharpens the earlier doc-only analysis:

### Verified surface

- **`/workflows`** opens a TUI dashboard, NOT a JSON-emitting command. Keybindings: `↑/↓` select · `Enter/→` drill · `Esc` back · `j/k` scroll · `p` pause/resume · `x` stop · `r` restart agent · **`s` save run as script**. No programmatic interface — operator can't pipe `/workflows` output.
- **Save artifact format**: pressing `s` saves the run's script to either `.claude/workflows/<name>.js` (project, team-shared via git) or `~/.claude/workflows/<name>.js` (personal). **Project workflows win on name collision**. Saved workflows become `/<name>` slash commands automatically.
- **`/effort ultracode`**: confirmed as a composed tier — *"combines xhigh reasoning effort with automatic workflow orchestration. With it on, Claude plans a workflow for each substantive task instead of waiting for you to ask."* Single request can produce several workflows in sequence (understand → change → verify). Available ONLY on models supporting `xhigh` effort. NOT in `claude --help`'s `--effort` choices list — confirms it's session-scoped via `/effort ultracode`, not CLI-flag-triggerable.
- **`/deep-research`**: the one bundled workflow. Fans out web searches across angles, fetches + cross-checks sources, votes on each claim, returns cited report with non-surviving claims filtered. Requires WebSearch tool. Triggers Claude Code's permission prompt before running.
- **Trigger surface**:
  - In-session: `/workflows`, `/deep-research`, saved `/<name>` commands
  - Any prompt containing the word `workflow` triggers a one-off workflow run
  - **Alt+W** ignores false trigger of the word in current prompt
  - `/effort ultracode` makes workflows the default for substantive tasks
- **Cross-cutting constraints** (verified):
  - Subagents a workflow spawns **always run in `acceptEdits` mode regardless of session mode** — load-bearing for AIDA's substrate-callback design (workflow agents can call `aida edit --status ...` without permission prompts)
  - Subagents inherit the session's tool allowlist
  - Available on Anthropic API, Bedrock, Vertex AI, Foundry — full multi-cloud, important for GovCloud/regulated contexts
  - Disable: `disableWorkflows: true` in managed settings or `CLAUDE_CODE_DISABLE_WORKFLOWS=1`

### Sharpened recommendations

The verified-by-keyboard details collapse one design decision:

> **AIDA's workflow compiler doesn't need a new format.** It just writes a `.js` file to `.claude/workflows/<name>.js`. That's the same shape Claude Code's `s`-save would produce. AIDA-emitted workflows ride on the existing save infrastructure. `/aida-batch-drain` becomes a slash command the same way `/deep-research` is a slash command.

And clarifies one constraint:

> **The substrate-callback CLI surface MUST be permission-free.** Workflow agents run in `acceptEdits`. They can edit files, run bash, and invoke AIDA's CLI. AIDA's `aida edit --status`, `aida session start`, `aida queue done` etc. all already work non-interactively — no UX work needed. But anything that today prompts (e.g. `aida queue done <SPEC>` confirmation, `aida session end` cleanup prompt) needs a `--yes` flag forwarded by the workflow's spawning code.

### One follow-up SPIKE to file

**SPIKE: AIDA workflow compiler proof-of-concept** — write a hand-crafted `.claude/workflows/aida-spec-drain.js` script that:
1. Takes a `SPEC-ID` from the prompt
2. Runs 6 phases as workflow agents (impl / CI / review / merge / pull / build)
3. Each phase agent calls `aida` CLI commands to read/update substrate
4. The advisor wrapper (Pattern A) is itself a phase agent that watches the others

If the hand-crafted version works, the compiler is just templating that JS file from spec metadata.

### What we still need to learn

- **The runtime API surface for workflow scripts** — what helpers do workflow JS scripts have? `agent()` to spawn, `parallel()`/`pipeline()` to compose, but the FULL set isn't in `/workflows --help`. Would need to look at `.claude/workflows/<name>.js` from a saved `/deep-research` run, OR pull from official runtime docs.
- **Whether workflow scripts can read AIDA's substrate** — they can shell out to `aida list`, `aida show`, etc. (since `acceptEdits` + tool allowlist), but is there a way for the runtime to inject substrate state at workflow start, or does every phase agent re-load?
- **Cross-session persistence** — operator exiting Claude Code mid-workflow loses it. Can AIDA's substrate snapshot enough state that resuming from a clean session re-attaches?

**Next concrete step when convenient**: run `/deep-research What is dynamic workflows` to trigger a real workflow run, then in `/workflows` view press `s` to save the script. Paste the resulting `.claude/workflows/deep-research-clone.js` — that's the canonical runtime API surface AIDA needs to compile against.
