Queue pickup resolves and reports role precedence, branch occupancy, and child-seat
delegation before persisting an implicit queue row or changing calibration, lease,
worktree, or spec state. Occupied branches receive a manual retirement offer only
when idle, clean, unlocked, and unleased. Setup failures retain a manual
`aida worktree enter` and guided-session continuation.
<!-- trace:TASK-1337 | ai:codex -->

Reconciliation and live auto-bump honor legacy human reopen history as well as
SHA markers. The latest deliberate Done/Completed → Approved decision fences
old merge evidence even if an automated bump subsequently overwrote status;
a later deliberate decision or later commit permits progress. Queue views
label old evidence as reopened work instead of recommending reconciliation.
<!-- trace:TASK-1338 | ai:codex -->

# AIDA — Overview

Required Ubuntu CI checks `aida-cli` with `--no-default-features` before the
workspace build so feature-disabled stubs stay buildable. This check-only guard
uses the existing full-CI filter and skips docs-only changes.
<!-- trace:TASK-1601 | ai:codex -->

`aida pr ship` reports live drive ownership at direct human terminals using
wave/PID, owning member phase, and session-scoped activity evidence.
`--wait [secs]` (default 300) polls ownership every two seconds and continues
through the existing shipping gates after release; timeout/refusal exits
non-zero. Drive seats and headless callers refuse immediately, preventing
self-waits. The explicit in-drive override does not bypass merge holds.
See [git lifecycle](docs/cli/04-git-lifecycle.md) for usage and evidence limits.
Wait release refreshes PR metadata; a drive merge takes the sync/cleanup path
without CI or merge credit. Shared run identity is reported as uncorroborated.
<!-- trace:TASK-1602 | ai:codex -->

Windows validation runs weekly on latest main (Sunday 06:00 UTC); non-main
manual dispatches fail before validation. Selected-path Windows PR checks stay
informational, macOS stays disabled, and Ubuntu Build plus merge-hold-gate remain
required. Product's 24h weekly-windows-triage reminder records the tracking issue
or no-issue result without queuing implementation. Releases still require a green
platform run within 24h. Weekly cadence does not shorten Linux PR CI.
See [CI policy](docs/agents/aida-repository-guide.md).
<!-- trace:TASK-1588 | ai:codex -->


Mirror code refs follow origin's confirmed tips. Pre-push hook proposals are
mirrored only when origin already advertises the exact SHA; rejected or pending
pushes are skipped. Mirror-sync ignores and reports local-only default/store
commits while fetching and pushing origin's SHA. See
[git lifecycle](docs/cli/04-git-lifecycle.md#aida-pull).
<!-- trace:BUG-1803 | ai:codex -->

The project schedule runs a read-only merged-worktree guard every 30 days and
routes failures to the advisor. The operator reviews `aida worktree gc` and
runs `aida worktree gc --yes --force` at a TTY; forced cleanup stays outside
scheduled and headless jobs.
<!-- trace:TASK-1596 | ai:codex -->

History supports scoped named templates and ordered event fields across CLI/MCP.
Template parsing, event-local field projection, and config persistence share
`aida-cli-lib/src/history_layout.rs`; builtin full/oneline keep legacy CLI modes.
Template saves preserve inline TOML tables; date formatting consumes the feed's
already-local minute timestamps without a second timezone conversion.
See [history layout docs](docs/cli/08-reporting.md#history-layouts-and-columns)
and the [accepted storage ADR](docs/aida/05-decisions/ADR-history-template-config.md).
<!-- trace:STORY-1477 | ai:codex -->

<!-- trace:TASK-1187 | ai:codex -->
> **What this is — read first.** AIDA is the agent-collaboration layer for a codebase: stable spec IDs, typed requirement relationships, code-to-spec trace comments, and a git-canonical graph exposed through CLI and MCP so humans, Claude Code, Codex CLI, and other agents can coordinate from the same project-owned record. It is alpha software: the core graph, traceability, queue, and MCP workflows are proven in this repository's dogfood, while broader-team scale, unattended reliability, and onboarding remain the precise open slices documented in [docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md](docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md) and [docs/research/ablations/](docs/research/ablations/).

**AIDA captures the intent behind your system — requirements, decisions, rejected alternatives, and the links from code back to them — so your coding agents work from it today, and your project is far better placed to be rebuilt tomorrow than it would be from commit messages and a README alone.** The claim is comparative, not absolute: AIDA does not promise to regenerate a whole system; it promises that a project which used AIDA is materially better positioned than the same project without it. The 60-second proof is `aida why <file:line>`. (Auto-derived code-graph tools index what the code *is*; AIDA keeps what it was *for* — and keeps the two linked. The earlier "your project's missing index" headline was superseded on 2026-09-26 by this one; see [the positioning deep-dive](docs/positioning/2026-09-24-spike-86-positioning-from-engineering.md).)

Two value claims sit behind that sentence. **Survival:** the corpus outlives the code. **Collaboration:** the corpus improves agent work in flight, especially through what the code cannot show — rejected alternatives, contradictions, and deferred intent.

**Without it**, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. **With it**, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

<!-- trace:TASK-885 -->
For day-to-day usage see the [CLI reference](docs/cli/README.md). For project conventions, build commands, and developer workflow see [CLAUDE.md](CLAUDE.md). For getting set up see [docs/getting-started.md](docs/getting-started.md). For *"how does AIDA fit alongside X?"* — neighbor-by-neighbor comparisons against `/ultrareview`, Karpathy-style structured markdown, Linear/Jira, etc. — see [docs/positioning/](docs/positioning/).

---

## Vision

The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA is the *durable index* on top of it. Its nearest competitor, GitHub Spec Kit, produces structured specs per feature and then freezes them — AIDA's delta is keeping them a maintained, cross-cutting graph (stable IDs + typed relationships + enforced traces + lifecycle, queryable via MCP, portable across vendors because it lives in git) that outlives any single feature. A small invisible kernel that captures what exists, plus optional layered modules for everything else. (See [docs/archive/positioning/vs-spec-kit.md](docs/archive/positioning/vs-spec-kit.md) and the [2026-05-31 competitive synthesis](docs/archive/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md), an archived dated snapshot.)

---

## The niche, concretely

<!-- trace:TASK-1187 | ai:codex -->
> Stated as product positioning with evidence attached: this is the niche AIDA is built to own. Backing: the theory paper [§7 (the apex claim) + §15](docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md) and the two ablations cited below.

**What it's for.** AIDA is the neutral, **cross-vendor decision record** for a multi-agent codebase: the shared graph of *why* (specs, typed relationships, code-to-spec traces, and lifecycle state) that any agent reads and writes through one CLI/MCP surface. Around that record sits the coordination layer that lets agents and a human share one workspace: leases, queues, briefs, punts, findings, directives, and role/RBAC gates (see [roles/seats vs stakeholder personas](docs/architecture/roles-seats-and-personas.md)). One role queue, one lease table, one intent graph — vendor-agnostic by construction.

**Why it's defensible.** The durable record is the product boundary. Agent runtimes, IDEs, PM tools, and vendor orchestration can coordinate inside their own walls; AIDA keeps the decision record in git with stable IDs and code links that survive vendor switches, machine switches, and years. The open-brief bake-off ([2026-06-18-open-brief-convergence.md](docs/research/ablations/2026-06-18-open-brief-convergence.md)) found that vendors handed the same open brief converge on the same design because the shared record dictates the shape; execution quality still varies, which is why selection, verification, and regression-catching remain valuable.

**The concrete embodiment.** `aida why <file:line>` answers why a line exists from a trace comment or conventionally trailered commit. `aida graph impact <id>` shows what a requirement touches. `aida compete` can run a spec through multiple vendors in isolated worktrees and let objective gates plus review pick the strongest result. `aida spec dryrun` and `aida spec interview` tighten specs before implementation. The wedge is practical and machine-checkable: stable IDs, typed relationships, lifecycle state, and inline `// trace:SPEC-ID` links that connect code back to intent.

**Type protocols.** AIDA stores concise work contracts for spikes, bugs, stories, tasks, decisions, and docs as editable META requirements, with optional `research`, `docs`, and `keystone` lane overlays. Interactive pickup and headless implementer/reviewer prompts inject the resolved protocol before work begins, cite its META ids, cap the combined body at 40 lines, and label the precedence `type < lane < spec acceptance`; a leased session receives the compact type reminder again in its per-turn notice. Inspect them with `aida protocol show <type> [--lane <lane>]`; MCP clients read the identical text at `aida://protocol/<type>[/<lane>]`, and editing either META body changes the next pickup without rebuilding AIDA.

<!-- trace:TASK-1328 | ai:codex -->
**Queue completion ownership.** `aida queue done` uses both the current display
ID and the stored origin ID from the resolved requirement. Remapping an ID
mid-session therefore preserves ownership of an existing branch and its commit
trailers. Mixed branches naming unrelated requirements remain refused, even
when their commits name an accepted alias; unscoped branches retain the existing
commit-evidence and ledgered `--force` rules.

**Drain launch ownership.** A single, batch, or nextN member publishes its
run UUID and zen provenance before the phase-1 status bump and phase-child
launch. Ownership persistence is required: missing/corrupt batch state or a
failed write returns a phase-1 failure before that run changes status or
spawns a phase child. Pipelined parents leave the bump to the registered
member process. Each active member publishes an independent UUID record in
`.aida/orchestrator-runs/`, including its PID and zen provenance. Children
require that record, a live member PID and a live parent drain snapshot;
current-run snapshot fields are telemetry only. Cleanup revokes only the
exiting member's UUID, so overlapping members keep authority. A delayed
child may leave its queue head Approved briefly; the scheduler waits for
progress and retries selection without treating that duplicate as exhaustion.
Stale or bare environment flags grant no authority. Refreshed batch/nextN
selection can admit newly unblocked or newly tagged members; checked run
registration adds them to the parent snapshot before token corroboration.
Snapshot read/modify/write updates serialize on the permanent
`.aida/drain-state-write.lock` sidecar, preserving dynamic membership through
concurrent phase updates and sibling cleanup. The child still requires both
live PIDs and recorded membership.
<!-- trace:TASK-1603 | ai:codex -->

**Drain ownership.** Local drain acquisition retains an observed-live PID/start
identity regardless of launch age. Queue work, burndown and integration share
the main checkout's lock, including launches from sibling worktrees. Shared
cross-clone claims retain their own heartbeat/TTL. This is not an atomic lock
redesign; see [coverage and limits](docs/testing/bug-1683-drain-lock.md).

<!-- trace:BUG-1682 | ai:codex -->
**Global queue persistence.** The per-home role queues in `~/.aida/queue/`
serialize cooperating writers with a permanent `<role>.lock` sidecar held
across reading, validation, mutation, and atomic YAML replacement. Successful
reads observe complete published content; only a final missing-file error
initializes an empty queue. Corrupt documents and other I/O failures propagate
from mutations, including CLI and autopilot pre-reads. Blocking locks release
on process death but provide no fairness or bounded wait guarantee. This
protects membership, not concurrent top/append placement, power-loss durability,
or writes by older binaries that ignore the sidecar. Never delete a live
sidecar to clear contention.

**Honest scope.** AIDA is alpha. The core graph, traceability, queue, and MCP workflows have held under this repository's own multi-agent dogfood; broader-team scale, turnkey unattended reliability, and onboarding outside this project are still being validated. Selective gating, not blanket: a programmatic gate beats a stated rule only when the invariant sits *far from the point of action* (attention-distance; [2026-06-18-gate-vs-rule-pilot.md](docs/research/ablations/2026-06-18-gate-vs-rule-pilot.md) falsified the blanket form). Every claim above traces to a finding or a shipped command.

---

## First surface: `aida why` and the memory lane (the TUI is a view, not the face)

The humble first surface is **`aida why <file:line>` plus the memory lane**: point at a line, get the decision behind it; let a project remember decisions across ordinary agent chats before any queue, drain or role exists (see the README's *Start here* section and [The Memory Lane](docs/positioning/memory-lane.md)). Depth — the typed graph, stable IDs, trace enforcement, the MCP server, the queue and review lifecycle, the control plane that keeps the store true under unreliable agent writers — is discovered **through the store**, not through a front-end.

The TUI (`aida tui`, [EPIC-26](docs/positioning/)) is **a view onto the control plane, not the product's face**. It hosts Claude Code as a child process, overlays status, and offers quick actions for review, queue, merge and pull; the CLI and MCP cover every operation without it. For the TUI itself — hosting model, keybindings, status overlay, autonomous drains, crash recovery — see [`docs/tui/README.md`](docs/tui/README.md).

When adding features or polish, the test is **"does this make the store easier to consult?"** — not "does this make the TUI's quiet depth stronger?".

> **Superseded history.** From 2026-05-14 to 2026-09-26 this section was titled *"Public face: the TUI is the product"* and carried the **Trojan-horse positioning**: ship a deliberately humble TUI, let the platform be discovered through use. The positioning deep-dive ([2026-09-24](docs/positioning/2026-09-24-spike-86-positioning-from-engineering.md)) found it unsupported by the engineering record (the TUI took 1,277 of 176,968 attributed changed lines over 30 days and 0.7% of authored specs), and the decision record `ADR-59` replaced it. The *tactic* it carried — look humble, let depth be discovered — survives, moved to `aida why` and the memory lane.

---

## AIDA in the Claude ecosystem: vertical depth on horizontal ground

A newcomer evaluating AIDA in mid-2026 has reasonable confusion to resolve: *Anthropic ships a lot. Claude Code already has so many capabilities. Why does AIDA exist?* This section names the breadth, names the structural relationship, and names the bet AIDA is making.

### The Claude ecosystem in 2026

Anthropic has shipped a striking density of primitives in the last six months. As of 2026-05-14, an incomplete inventory:

| Primitive | What it provides |
|---|---|
| **Claude Code** (CLI + web + IDE extensions) | The substrate — a coding agent that runs in your terminal, in the cloud, or in your editor |
| **`/ultraplan`** (research preview) | Cloud-based multi-agent plan generation (3 explorers + 1 critic); browser review surface; teleport-back-to-terminal |
| **`/ultrareview`** (3 free uses then quota) | Cloud-based multi-agent code review |
| **`/goal`** (2.1.139, 2026-05-12) | Set completion condition; loop until met; small evaluator decides; exits when done |
| **`/schedule`** | Cadence-driven Claude invocations (nightly, morning, weekly) |
| **Auto mode** (Shift+Tab in CLI) | Permission posture for long-running autonomous work |
| **Agent view** | Visual observability for autonomous runs |
| **Remote Control** | Browser-driven control of local Claude Code |
| **MCP (Model Context Protocol)** | Open protocol for exposing tools + resources to Claude |
| **Claude Code on the Web** | Cloud-hosted Claude Code sessions, github-integrated |

The cadence is real and the surface is genuinely useful. A reasonable first impression: *"Anthropic is shipping the platform; do I need anything else?"*

### Horizontal vs vertical: where AIDA sits

The Claude ecosystem's primitives are deliberately **horizontal** — generic, composable, workflow-agnostic. `/goal` works for "all tests pass" the same way it works for "all dependencies upgraded." `/schedule` runs whatever you point it at. MCP exposes any tool you can describe in a schema. The substrate accommodates many workflows by holding no opinion about any specific one.

AIDA goes **vertical** — opinionated about *one* domain: agent-collaboration on project intent. Stable spec IDs, typed relationships, code-to-spec trace comments, an MCP server that exposes a requirement graph, a queue + role + session model for human-agent workflow, a lifecycle for shipping. The horizontal primitives are the ground AIDA stands on; the vertical depth is what AIDA contributes.

The composition is symbiotic, not competitive:

| Anthropic provides (horizontal) | AIDA provides (vertical) |
|---|---|
| `/goal` — autonomous completion loop | The **vocabulary** for machine-checkable conditions (`/goal all specs tagged batch:X are Completed` is precise; `/goal make the queue empty` is vague and loops forever) |
| `/ultraplan` — dense plan generation | The **persistence + graph linkage** (plans live in `docs/plans/`, pinned to specs, surfaced in session manifests, verified by `aida plan verify`) |
| `/ultrareview` — multi-agent review | The **lifecycle hooks** (`/aida-review` walks each linked spec's acceptance; queue done flips status; auto-bump fires on merge) |
| MCP — tool/resource protocol | The **content** served over MCP (requirements, relationships, history, comments) |
| Claude Code — agent runtime | The **shared workspace** the agent collaborates on (graph + IDs + traces) |

### Why this composition is likely to remain stable

AIDA's bet is that **Anthropic has structural reasons to stay horizontal**, leaving vertical territory for tools like AIDA. Three reinforcing forces:

1. **Verticals shrink the market.** A "simple project tracker built into Claude Code" would lock users into Anthropic's specific opinions about how to track work. Many users have existing tools (Linear, Jira, GitHub Issues, Notion); a built-in vertical would compete with all of them, reducing Claude Code's appeal as a substrate that fits any workflow. Horizontal primitives compose with whatever the user already uses.

2. **The reusable primitives compound; the verticals don't.** `/goal` works for coding, ops, content review, data work, research workflows. A "Claude requirements graph" would only matter to teams using requirements graphs. The horizontal investment has higher ROI per Anthropic engineer-hour.

3. **Competing with integration partners is a known platform anti-pattern.** Anthropic benefits from a rich ecosystem of integrations. Going too vertical means competing with the people building the ecosystem — which historically leads to platform decline. Anthropic's published interest in MCP (an open protocol explicitly for external integrations) signals they understand this.

The exceptions are interesting: `/ultraplan` and `/ultrareview` ARE semi-vertical (they're opinionated about phases of work). But they're opinionated about *universal* phases (planning, reviewing) — every project plans and reviews. They're not opinionated about *what* you track or *how* your team structures intent. That stays open.

### What this means for AIDA's roadmap

The strategic implication: AIDA should keep doing the vertical depth that the horizontal primitives can't reach. Concretely:

- **Don't compete with `/goal`** — compose with it via `aida goal` ([TASK-242](docs/plans/)), which derives machine-checkable conditions from the requirement graph
- **Don't compete with `/ultraplan`** — compose with it via `aida ultraplan SPEC` ([TASK-113](docs/plans/)) for prompt assembly + `/aida-import-plan` ([TASK-114](docs/plans/)) for output persistence
- **Don't compete with `/ultrareview`** — compose with it via `/aida-review`'s spec-walk + adversarial-pass discipline (STORY-109, shipped) that uses requirement metadata `/ultrareview` doesn't know about
- **Don't compete with Claude Code's session model** — extend it via worktree-isolated implementer sessions, role-pure boundaries, queue routing, and the TUI that hosts Claude Code itself as a view onto the control plane ([EPIC-26](docs/positioning/))
- **Compete vertically only where the platform structurally won't go** — the graph, the IDs, the trace network, the queue/role/session opinions, the requirement lifecycle

### The risk + how AIDA mitigates

Two risks worth naming explicitly:

**Risk 1: Anthropic ships a vertical that overlaps AIDA's core (a built-in requirement graph, a `/track` command, etc.).** Mitigation: AIDA's core value is the *composition* of graph + IDs + traces + MCP + queue + lifecycle. A built-in graph alone wouldn't replicate the trace network or the lifecycle. A built-in lifecycle alone wouldn't have the graph. AIDA's moat is the multi-layer stack, not any one feature.

**Risk 2: AIDA's CLI surface keeps growing as Anthropic adds primitives, leading to confused users.** Mitigation: keep one humble front door — `aida why <file:line>` and the memory lane — and let everything else be discovered through the store. Surfaces (CLI, MCP, TUI, web) are views onto the same store and control plane, so a new primitive adds a view, not a second product. (The earlier mitigation, the Trojan-horse TUI positioning, was superseded on 2026-09-26 — see *First surface* above.)

### Summary

AIDA's bet is **vertical depth on horizontal ground**: Anthropic ships the substrate; AIDA composes the substrate into a specific workflow domain (agent-collaboration on project intent). The bet stays sound as long as Anthropic stays horizontal, and Anthropic's structural incentives push them to stay horizontal. `aida why` and the memory lane are the front door; the intent store and the control plane that keeps it true are the vertical depth; the surfaces — CLI, MCP, TUI ([EPIC-26](docs/positioning/)), web — are views onto them; the Claude ecosystem is the ground all of it stands on.

---

## Architecture

### The four layers

For contributors, reviewers of architecture sketches, and anyone choosing what AIDA benchmarks against. The layering is validated by dependency direction (store and intent modules import no control-plane module; the control plane depends heavily on the store) and by the removal test (remove the control plane and the whole memory lane still works; remove the store and nothing meaningful does). Source: the [2026-09-24 positioning deep-dive](docs/positioning/2026-09-24-spike-86-positioning-from-engineering.md), §3.

1. **The intent store (the data plane).** The git-canonical requirement graph: YAML objects on the orphan `aida-store` branch, stable IDs, typed relationships, comments, the rebuildable cache, and the intent layer that makes the store worth keeping — acceptance criteria, `// trace:` comments, reconstitution, contradiction and gap detection. This is what "requirements management" names, and it is the product. Reconstitution reports probe failures even when recall has no traced-test denominator; a successful agent exit still requires readable, valid probe output, with artifact and headless-log paths retained in failure reasons (see [reporting](docs/cli/08-reporting.md#aida-reconstitute)).
2. **The control plane (the corpus-integrity layer).** The layer that decides whether work may advance and reports the state of work in flight: the queue, the drain, the orchestrator and its phases, seats, leases, worktree assignment, review verdicts, merge-holds, `BlockedBy` gating, required-check rollups, the event feed, and the surfaces that report on all of it (`aida ps`, drain status, awaiting, doctor). It exists to keep the store true while unreliable agent workers change the code; that is why it takes about half the engineering without being the product. Its two sub-facets have industry names — admission / merge gating (a merge queue) and state reconciliation (actual vs reported vs desired) — and its characteristic defect is a surface asserting a state that is not true: false-green, false-empty, stale reading as current.
3. **The execution layer.** Isolated worktrees and sessions in which agents (Claude Code, Codex CLI, Antigravity, any harness with a shell) do the work, one spec per worktree, with role-pure seats and deterministic handoffs.
4. **The surfaces.** CLI, MCP, TUI and web. They display store and control-plane state; none of them is the control plane and none of them is the product's face.

### Storage (EPIC-1-001) — git-canonical by default

The orphan branch `aida-store` is the writer of record. Each requirement is one YAML file at `objects/<TYPE>/000/<SPEC-ID>.yaml`, committed to that branch. Writes go to git first, then to a local SQLite cache (`.aida/cache.db`, gitignored, rebuildable).

- **Live worktree:** `.aida-store/` (gitignored on the main branch; populated by `aida init`)
- **Branch:** `aida-store` on origin
- **Cache:** `.aida/cache.db` — read projection for fast `list / search / filter`; auto-rebuilt when the cache's recorded HEAD doesn't match the orphan's HEAD
- **Sync:** `aida db sync --pull --push` (fetches the explicit store branch into a temporary ref, then rebases onto its pinned commit for linear orphan history; shared `FETCH_HEAD` is neither read nor written)
<!-- trace:TASK-1604 | ai:codex -->

The legacy centralized SQLite path (`aida init --centralized`) still exists but prints a deprecation warning. PostgreSQL is opt-in via the `postgres` feature flag for teams wanting a server-backed shared projection.

### Distributed identity (EPIC-1-052)

Each clone of an AIDA-using project gets a unique **node id** and writes its identity to a per-clone, gitignored `.aida-store/.aida/node.toml`. The shared `.aida-store/registry/nodes.toml` is the source of truth.

- **Node-aware ids** look like `FR-1-052` (`<TYPE>-<NODE>-<SEQ>`) — issued offline, never collide across clones.
- **Agreed ids** look like `FR-052` — promoted from node-aware ids by `aida db merge-gate` once the work merges into trunk.
- **Pre-allocated blocks** (`FR-2-005`) let a clone reserve a contiguous range of agreed ids up front so trace comments can use the short form immediately, even offline. `aida node acquire` auto-allocates the first FR block.
- **`[id_format]` policy** in `.aida/config.toml`: `node-aware-only` | `blocks-then-fallback` (default) | `blocks-only`.
- **`aida init` post-clone bootstrap**: when origin already has the `aida-store` branch, `aida init` fetches it, sets up the worktree, and prompts for node-id acquisition.

### Completion intent and deliberate reopen

`aida pr ship` derives completion credit from an explicit trailing title group,
then branch-name recovery, and accepts only IDs that resolve unambiguously in
the canonical store. References in title prose and PR bodies remain descriptive;
a partial-work PR with a neutral branch can ship without completing its owner.
When a title is present, an older branch-head trailer cannot supply missing
credits. When the title is empty, the branch-head subject supplies the title.

Status edits and CLI/MCP rework record the code-repository HEAD when a spec
leaves Done or Completed for further work. Live auto-bump and manual
`reconcile-status` reject commits equal to or ancestral to that reopen marker,
including closure-held landings and review propagation. A new later commit
may complete the spec. `completion_sha` remains independent: it is retained
across reopen, while the stale completion date is cleared. The marker is
best-effort if the code repository is unavailable; existing squash-body
constituent commit trailers still participate in the landing scan.
<!-- trace:TASK-1600 | ai:codex -->

### Surfaces

- **CLI (`aida`)** — primary work surface. Embeds the MCP server (`aida mcp-serve`).
- **MCP server** — exposes requirements as native Claude Code tools over JSON-RPC 2.0 stdio: the **typed, structural** surface for MCP-native clients. (AIDA's own agent-surface benchmark found the token-efficient CLI path — `AIDA_AGENT_OUTPUT`/TOON — costs roughly **half** of MCP for identical agent tasks, so the CLI is the primary agent surface and MCP is the typed option, not the cost-winner. See `bench/agent-surface/`.)
- **REST + gRPC server (`aida-server`, port 8080)** — backs the React dashboard and provides a public API.
- **React dashboard (`aida-web-react/`, port 5173 dev)** — kanban / sprint / queue / chat UI; Vite proxies `/api` to the server.

The native desktop and WASM clients (egui-based) were extracted to a separate repo on 2026-05-02. The React dashboard's keyboard navigation (`j/k`, `g+key` chords, quick pickers) covers the vi-feel use case.

### Autonomy & escalation

The autonomous-collaboration layer — the three-mode autonomy ladder (default / `--zen` / `--no-human`), the implementer → advisor → human escalation cascade, the advisor's Type A/B/C resolve-vs-escalate calibration, and the file-based handshakes that coordinate the tiers — is described in [docs/architecture/autonomy-and-escalation.md](docs/architecture/autonomy-and-escalation.md). For the practical user guide to `--auto-complete` and `--no-human` see [docs/autonomous-drain.md](docs/autonomous-drain.md); for the MCP transport layer over the same filesystem substrate see [docs/architecture/mcp-coordination-surface.md](docs/architecture/mcp-coordination-surface.md). The **resilient-drain primitive** is EPIC-28 (`docs/autonomous-drain.md` → "Shelving on failure"): a shelvable phase failure parks the spec in `NeedsAttention` with a structured `FailureReason` and the batch drain continues past it, with dependents skipped automatically via the `BlockedBy` pickability gate (STORY-333) — exit 2 + `aida findings list` for triage.

For how this control plane — queue, drain, orchestrator phases, leases, verdicts, merge-holds, required-check rollups, the event feed — compares with the two families it most resembles, durable-execution engines (Temporal-style event history, replay, checkpointing, retries) and merge queues (Bors, Zuul, Mergify, Aviator, the GitHub merge queue), and which of their properties AIDA has, lacks, or deliberately declines because its workers are non-deterministic LLM agents, see [docs/architecture/control-plane-vs-durable-execution-and-merge-queues.md](docs/architecture/control-plane-vs-durable-execution-and-merge-queues.md).

---

## Workspace layout

```
aida/
├── aida-core/             Engine — models, storage, cache, dispenser, HLC, conflict
├── aida-cli/              `aida` binary + MCP server
├── aida-crate/            Published `aida` crate metadata
├── aida-server/           REST + gRPC server (port 8080)
├── aida-generate-types/   Rust → TypeScript types (ts-rs)
├── aida-tui/              `aida tui` terminal shell — a view onto the control plane (EPIC-26)
├── aida-web-react/        React 19 + Vite + Tailwind dashboard (port 5173 dev)
├── proto/                 Protocol Buffers definitions
├── docs/                  Markdown docs (incl. plans/ archive)
├── tests/                 Integration test scripts
└── (orphan branch:        aida-store — canonical YAML store)
```

---

## Feature surface (high level)

External wall displays and dashboards use a narrow, versioned read-only
contract rather than depending on incidental CLI JSON fields. The eleven
polling lenses and local event follow feed are declared by `aida contract
--json`, guarded by fixtures in CI, and documented in
[`docs/monitor-contract.md`](docs/monitor-contract.md).

For details on any of these see the [CLI reference](docs/cli/README.md).

### Requirements

- **Types** (19): functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc
- **Status workflows** are type-specific (e.g., the standard `draft → approved → in-progress → completed | rejected`)
- **Relationships** with typed cardinalities: parent/child, verifies/verified-by, references, duplicate, plus user-defined types via `aida rel-def`
- **Comments** with threaded replies and configurable emoji reactions
- **Custom fields** per type definition
- **Meta requirements** (META-002…006) store the AI prompt templates as editable requirements; `aida edit META-002 --description …` customizes evaluation/duplicates/relationships/improve/generate-children prompts

### Identity & traceability

- Stable node-aware ids (`FR-1-052`) and agreed-id promotion at merge-gate
- Pre-allocated blocks for offline-safe agreed ids (`aida db block claim`)
- Inline trace comments: `// trace:FR-1-052 | ai:claude[:confidence]`
- Portable commit trailer convention: `[AI:tool] type(scope): description (REQ-ID)` works without AIDA and can be adopted with a standalone commit-msg hook; see [Commit trailer convention](docs/commit-trailer-convention.md)

### AI / agent integration

- **Claude Code skills** scaffolded by `aida init` under `.claude/skills/` and `.claude/commands/` (47 skills, 48 commands as of writing): daily drivers `/aida-req`, `/aida-implement`, `/aida-commit`, `/aida-capture`, `/aida-search`, `/aida-plan`, `/aida-onboard`, `/aida-pickup`
- **Codex (AGENTS.md)** profile scaffolded in parallel; `aida init --agent codex` for Codex-only, `--agent both` (default) for both
- **MCP tools**: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`
- **MCP resources**: `aida://project/summary`, `aida://requirements/tree`
- **Hooks**: `aida-stop-check.sh` warns about untraced edits; `aida-session-context.sh` injects role + project context at session start
- **Roles & sessions**: `aida role` manages persistent named contexts (architect, implementer, reviewer, and **advisor** — "trusted counsel" across the project's lifetime; `dialog` is a deprecated alias for it) with per-role scope filters and Claude Code system-prompt addenda; `aida session list/resume/new` enriches Claude Code's session list with the active role and most-recent spec id
- **Integrator**: bare `aida integrate` shows Done-with-PR work as a merge queue with positions; `aida integrate --run` drains that queue through one serialized rebase/review/merge/pull loop so finished PRs do not stale-base race each other.
- **Statusline**: `aida statusline` is sub-50ms and suitable for `~/.claude/settings.json`'s `statusLine.command`

### Web dashboard highlights

- Kanban + list + sprint planning with drag-and-drop (@dnd-kit)
- Advanced query builder (`react-querybuilder` with json-logic), URL-persisted via `?aq=`
- Markdown rendering with auto-linked spec ids, `::color[text]` syntax, and Prism syntax highlighting
- Personal work queue (drag-to-reorder, dashboard widget, cross-user assignment via `added_by`)
- Skills browser — view/edit skills, run executable ones (e.g., compiler-warnings) with SSE streaming output
- AI Evaluate — one-click quality evaluation per requirement (Sparkles button) using the META-002 prompt
- Chat — Claude-powered Q&A with full requirements context (`ANTHROPIC_API_KEY` or runtime key via Admin)

### GitLab integration

Bidirectional sync with GitLab issues: configurable type/priority/status label mappings, content-hash-based drift detection (`aida gitlab status --diverged`), background polling, dashboard status indicators. See the GitLab use case in the user guide.

---

## Developer workflow (working *on* AIDA)

```bash
# One-time: install the `aida()` shell wrapper into ~/.bashrc or ~/.zshrc
aida dev shell-init --install

# Per-shell: activate the in-repo build (pyenv-style)
aida dev activate                      # the `aida()` wrapper auto-evals this — no `eval $(...)` needed
aida dev status                        # confirm activation, show binary mtime
aida dev serve                         # foreground supervisor: aida-server (8080) + vite (5173)
aida dev deactivate                    # the wrapper auto-evals this too
```

`aida dev activate` prepends `target/{release,debug}/` (whichever is more recently built) to PATH and prefixes the shell prompt with `(aida-debug)` or `(aida-release)`. For releases, `scripts/release.sh {major|minor|patch}` bumps the workspace version, generates tag notes, commits, tags, and pushes — triggering the release workflow that builds and publishes binary tarballs.

The shell helper applies role/session/dev/worktree environment updates only
after a successful CLI exit with one complete pair of standalone
`#aida:eval:begin` / `#aida:eval:end` markers. Failed stdout is sent verbatim to
stderr without evaluation; unmarked or malformed successful output is displayed.
Older binaries emitting bare shell output must be updated for automatic shell
mutations. Already-loaded helpers are not replaced by a binary upgrade: install
the updated helper with `aida dev shell-init --install` and reload the shell
configuration. The binary's existing bare-output behavior for absent or legacy
helpers is unchanged. Isolated regression: `bash tests/test_role_wrapper_boundary.sh`.
<!-- trace:BUG-1806 | ai:codex -->

For project conventions (commit format, scaffold/template architecture, CLI reference) see [CLAUDE.md](CLAUDE.md).

---

## Documentation map

| File | Purpose |
|---|---|
| [README.md](README.md) | Quick start and project structure |
| [CLAUDE.md](CLAUDE.md) | Conventions, build commands, scaffold/template architecture, CLI reference |
| [docs/getting-started.md](docs/getting-started.md) | First-time setup walkthrough |
| [docs/cli/](docs/cli/README.md) | The CLI reference manual — when and why to use every command (12 chapters) |
| [docs/admin-guide.md](docs/admin-guide.md) | Storage administration, multi-user configuration |
| [docs/storage-modes.md](docs/storage-modes.md) | Deeper dive on storage modes and migration paths |
| [docs/architecture/autonomy-and-escalation.md](docs/architecture/autonomy-and-escalation.md) | The autonomy modes, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, the inter-agent comms substrate |
| [docs/architecture/mcp-coordination-surface.md](docs/architecture/mcp-coordination-surface.md) | The MCP transport layer over the filesystem-canonical coordination substrate |
| [docs/autonomous-drain.md](docs/autonomous-drain.md) | Practical user guide to `--auto-complete` and `--no-human` (paired with the autonomy architecture doc above) |
| [docs/plans/](docs/plans/) | Archived implementation plans (one per `YYYY-MM-DD-<slug>.md`) |


### Bounded cache reads

Compatible cache-backed reads coordinate through a canonical `refresh.lock`
flock. An incremental winner makes one SQLite attempt; other readers poll
committed metadata for at most `AIDA_CACHE_READ_WAIT_MS` (default 1500ms), shared
across backend opens in the invocation. Advisory paths use a zero wait budget.
Single-spec CLI `show` also uses zero wait across human, TOON, JSON, card and
tree output: a live refresh holder never delays the lookup, and a human TTY
does not trigger an inline full rebuild. The spec object still comes from
canonical YAML, while stale derived graph context carries the existing cache
labels. Missing or incompatible caches and explicit strict overrides retain
their recovery behavior. `tests/test_show_latency.py` covers held-flock reads,
TTY delegation and subsequent explicit refresh.
<!-- trace:BUG-1801 | ai:codex -->

Rows and freshness metadata come from one pinned SQLite snapshot. An invocation
collector preserves stale observations across later fresh reads; CLI object JSON
outputs carry `cache`, arrays retain their shape with a stderr note, and MCP
calls that touch tolerant reads include `structuredContent.cache` plus text.

Before the worker slice lands, a compatible non-TTY full-rebuild winner returns
`refreshing: "deferred"`; an incremental SQLite busy result returns `writer_busy`.
These labels promise no autonomous progress. `worker_running` records an observed
refresh-flock holder, including an inline refresher. Explicit `aida cache rebuild`
or a later strict operation can restore freshness. Interactive human TTY full
rebuilds remain strict, as do mutations, gates and explicit cache operations.
Missing/unreadable cache and incompatible-schema paths retain their strict/error
handling; no incompatible rows are served. Durable requests, worker scheduling,
and the separately bounded single-spec show path are separate work.

GitHub `aida pr ship` waits on classified CI rows for up to 20 minutes. The
`[ci]` informational allow-list applies to pending as well as failed jobs;
branch-protection-required checks always take precedence. Unlisted failures
still block shipping. See [PR lifecycle](docs/cli/04-git-lifecycle.md).
<!-- trace:TASK-1331 | ai:codex -->

## Reconstitution launch limitation

Store-probe failures are surfaced on stderr even when the recall denominator is
empty. The current launcher resolves seat authority after entering the isolated
scratch cwd, which cannot resolve a project roster and can refuse before vendor
spawn. An exit-zero report therefore does not prove that an agent ran. See the
[pinned live investigation](docs/testing/task-1327-reconstitution-launch.md);
TASK-1-224 tracks the authority/configuration-root repair for advisor triage.
<!-- trace:TASK-1327 | ai:codex -->

`aida pr ship` checks known review, mergeability, stale CI definition, and hold
blockers before waiting for CI, and repeats the checks before merge. An optional
`--wait <secs>` deadline covers registration and settlement; the default is
unbounded. Exit codes 20–25 distinguish CI red, timeout, rebase, review,
stale definition, and hold refusals for scripts. See
[git lifecycle](docs/cli/04-git-lifecycle.md) for the table and override behavior.
<!-- trace:TASK-1606 | ai:codex -->

Reviewer handoffs identify GitLab changes as `MR-N` and GitHub changes as
`PR-N`, with explicit `--role reviewer` pickup commands. Autoqueue reuses an
existing canonical review story in distributed or legacy stores; an unreadable
store or failed queue insertion cannot establish a successful handoff. The
creation gate requires every canonical object to be readable and parseable,
without changing tolerant readers elsewhere or falling back to legacy state
when configured canonical storage is unavailable. The CI
checkpoint confirms the queue operation even when resuming without a lease.
<!-- trace:BUG-1807 | ai:codex -->

Review-story creation uses strict fallible object enumeration; missing roots,
iteration failures, symlinked type/shard directories, and invalid YAML filenames
refuse handoff without authorizing absence. Tolerant bulk readers retain their
existing behavior.
<!-- trace:BUG-1807 | ai:codex -->

The review-story config probe validates TOML before legacy fallback and refuses
configured but unavailable canonical storage. Its mode policy and existing
store-location checks are covered by `scripts/config-trust.toml`; the probe adds
no executable, command, or credential selection.
<!-- trace:BUG-1807 | ai:codex -->
