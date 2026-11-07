Mailbox delivery resolves named agents against current registry liveness,
shares canonicalization across CLI/MCP/mentions, refuses unknown/dead agents
and empty role/vendor audiences, and lists group readers. Known people retain
offline inboxes; historical reads do not require a live recipient.
<!-- trace:STORY-1843 | ai:codex -->

Review watch discovers open non-draft GitHub/GitLab changes against pinned
origin and each change's own base-branch evidence and dispatches ordinary attested reviewers before
CI settles. Atomic version-1 state under the main worktree's `.aida/review-watch`
and permanent admission/per-change locks enforce one reservation per head, a
local cap of two, and the two-round limit. PID/start/boot receipts precede queue
work; capacity is released only by a terminal receipt or proof that the
dispatcher's exact process is gone. Stale overlap notices name the change's
base branch and publish idempotently to the exact forge author's local mailbox.
S1 waits for old reviews and never terminates reviewers, updates branches,
reports readiness, or merges. See [review watch](docs/cli/06-roles-sessions.md#watching-for-new-review-heads).
<!-- trace:TASK-1993 | ai:codex -->

Agent launch setup validates delegation before allocation, honors explicit
working directories in launch and preview paths, and retains an invocation-owned
setup receipt until foreground spawn or successful background dispatch. Fresh
worktrees allocate under a private name and publish with an atomic no-replace
rename followed by Git admin repair. Rollback verifies the `.git` pointer,
retains the original branch start SHA for compare-and-delete, and releases only
the exact newly acquired shared claim ID under the store lock. Claims adopted
from a prior invocation carry no rollback authority. Changed or occupied resources
are retained with recovery instructions; failed children revoke only the grant
issued for that launch. See [session launch behavior](docs/cli/06-roles-sessions.md).
<!-- trace:BUG-2188 BUG-2168 | ai:codex -->

CI fan-out keeps libtest: two CLI-library unit halves, two deterministic
CLI-integration halves, remaining workspace tests/doctests, and two gate lanes
run on seven parallel Linux runners. The aggregate `Build (ubuntu-latest)` fails unless all
lanes succeed. Test timing budgets remain per binary, and each test lane checks
for untracked root entries. See [CI fan-out](docs/internal/testing/ci-test-shards.md).
<!-- trace:TASK-1949 | ai:codex -->

`aida agent new` supplies the selected executable's reported `--version` to
foreground/background children as `AIDA_HARNESS_VERSION`. Failed, empty or
invalid probes clear inherited context; previews perform no version probe.
<!-- trace:TASK-1793 | ai:codex -->

No-number `aida pr ship` protects main/master and uses only the local origin/HEAD
symref before asking the forge. A stale origin/HEAD wins over forge evidence;
refresh it after a remote default change. Without that symref, the read-only
GitHub probe uses a three-second child-exit deadline. Unknown evidence (including
timeout, errors, or unsupported forges) requires an explicit PR number. Output
collection retains the shared helper's existing bounds. PR-base callers retain
conventional local/remote refs and a final main fallback after the probe fails.
Approved-change suggestions use numeric ship commands.
<!-- trace:TASK-1795 | ai:codex -->

Headless drains validate every headless phase seat before queue mutation or
plan startup, including refusing when the project root cannot be resolved.
Pooled-worktree reuse revokes the prior occupant's stamped child grant before
removing its lease, while leaving sibling worktrees and the parent grant intact.
Delegated children retain exactly their parent's expiration timestamp.
<!-- trace:TASK-1789 | ai:codex -->

Spec creation, history and comments carry small optional write context.
`aida usage show <SPEC> --provenance` joins exact session IDs to the existing
read-only token ledger through each write timestamp; it never reads transcripts
or infers model/effort. `AIDA_HARNESS_VERSION` is the sole harness-version source.
Missing evidence remains unknown, including legacy rows. Per-event session
usage is cumulative and overlapping. See
[write provenance](docs/cli/08-reporting.md#write-provenance).
<!-- trace:STORY-1674 | ai:codex -->

Mandate exclusion ledgers require a regular file owned by the current uid with
exact mode 0600, checked on the opened handle with nonblocking, no-follow flags.
An absent ledger stays empty; invalid metadata or content refuses mandate
coverage. Mandate start/stop retains the shared direct-human authority check,
with mandate-specific recovery commands. Hold re-arm uses the same composition
and marker write, but its single placement event explicitly names the failed
mandate ship; the event follows marker restoration and precedes label sync.
Marker and label failures remain explicit.

Mandate-only exclusion checks add trace IDs from both committed sides of changed
text files of every format, including deleted and renamed files. Source blobs
come from pinned Git object IDs, never checkout contents. Rust comments are
parsed separately from string fixtures. Changed hunks contribute their enclosing
fn/impl traces and always add the file’s leading header traces, including
inner/outer doc comments and same-line block comments before the first item. The
first code-token line is conservatively included in full. Module comments
outside fn/impl items and their attached leading prefixes also add evidence,
including between items and at EOF. Inline-module headers apply even when
attached to an unchanged first function. Leading comments owned by other
unchanged fn/impl items retain their scope. A hunk without safe item
attribution, a mode-only change, an unparseable Rust file, or any other text
format falls back to full-file union. For other formats that fallback
conservatively includes raw trace tokens, including strings. Documentation,
configuration, hooks and nested build paths are included. Trace markers are case
insensitive. Diff ranges and Rust item maps both count LF lines, including when
a file contains bare CR characters. Files without traces cannot identify unnamed
specs.

These IDs only add exclusions through the existing current/history/sticky-ledger
classifier. Verdict candidates, author identity and completion credit still use
named specs. An empty named subject stays unresolved even when traces exist.
The forge head must match the checked head; a missing local origin base, ambiguous
merge base, unreadable diff/blob/store, changed symlink, invalid text or
moving store or dirty canonical objects refuses coverage. Coordination leases,
mail and other non-evidence dirt do not block the object snapshot; untracked
objects remain evidence even when Git ignore rules hide them. The home exclusion
ledger is read separately and retains its fail-closed trust checks. Retry after
fetching missing evidence or finishing an object write. A stale local origin base
can exclude more specs by including older changes. Store HEAD and ref tips are checked around collection;
the existing history reader includes versions reachable through all store refs.
<!-- trace:TASK-1790 | ai:codex -->
<!-- trace:BUG-2213 | ai:codex -->

C7 lifecycle commands resolve invoking caller authority after acquiring the
seat lock; ack/release also revalidate persisted requester authority locally.
The dormant runtime gate refreshes its caller at the same locked boundary.
Absolute deadlines reconcile on lifecycle/gate entry without a live poller; expired or invalid-authority requests retain the holder. Legacy
requests without a subject binding fail closed. Fresh independent review is
still required before gateway activation.
<!-- trace:TASK-1607 | ai:codex -->

Rework-only merge holds can release through a current-head, granted reviewer
approval after older refusal rounds. Sensitive spec metadata, guided/operator
mode, keystone class and any non-Rework composition retain human clear.
Covered spec metadata and execution mode are re-read before clearance and
again before releasing labels and the marker.
See [clearance policy](docs/cli/08-reporting.md#merge-hold-gate-and-clearance-limits).
<!-- trace:STORY-1687 | ai:codex -->

Model-side mail poll findings clear after a session is silent beyond
`watchdog.window_minutes`, or no registry PID for the session is alive. Recent
activity keeps an older setup observable for live or unregistered sessions.
Existing night-shift watchdog failures age out of the two-hour quiet guard
normally.
<!-- trace:BUG-2120 | ai:codex -->

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

AIDA is Linux-only (ADR-72, no expiry). Windows and macOS are unsupported:
there are no Windows/macOS PR legs, schedules, release artifacts, cross-target
checks, or triage reminders, and releases do not require any cross-platform run.
`cross-platform.yml` is kept as a manual-only workflow so a future reversal is
cheap. Ubuntu Build plus merge-hold-gate remain the required checks.
See [CI policy](docs/internal/agents/aida-repository-guide.md).
<!-- trace:TASK-1723 | ai:claude -->


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
> **What this is — read first.** AIDA is the agent-collaboration layer for a codebase: stable spec IDs, typed requirement relationships, code-to-spec trace comments, and a git-canonical graph exposed through CLI and MCP so humans, Claude Code, Codex CLI, and other agents can coordinate from the same project-owned record. It is alpha software: the core graph, traceability, queue, and MCP workflows are proven in this repository's dogfood, while broader-team scale, unattended reliability, and onboarding remain the precise open slices documented in [docs/internal/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md](docs/internal/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md) and [docs/internal/research/ablations/](docs/internal/research/ablations).

**AIDA captures the intent behind your system — requirements, decisions, rejected alternatives, and the links from code back to them — so your coding agents work from it today, and your project is far better placed to be rebuilt tomorrow than it would be from commit messages and a README alone.** The claim is comparative, not absolute: AIDA does not promise to regenerate a whole system; it promises that a project which used AIDA is materially better positioned than the same project without it. The 60-second proof is `aida why <file:line>`. (Auto-derived code-graph tools index what the code *is*; AIDA keeps what it was *for* — and keeps the two linked. The earlier "your project's missing index" headline was superseded on 2026-09-26 by this one; see [the positioning deep-dive](docs/internal/positioning/2026-09-24-spike-86-positioning-from-engineering.md).)

Two value claims sit behind that sentence. **Survival:** the corpus outlives the code. **Collaboration:** the corpus improves agent work in flight, especially through what the code cannot show — rejected alternatives, contradictions, and deferred intent.

**Without it**, coding agents start every session cold, re-deriving the same context they had yesterday; humans rediscover and re-debate decisions for years; cross-references between code and intent rot silently. **With it**, *"does this already exist?"*, *"why did we choose X?"*, and *"is this code still tied to a live requirement?"* are one query away — for the agent and for you.

<!-- trace:TASK-885 -->
For a product tour, open the [website](docs/showcase/index.html) or [guide library](docs/showcase/library.html); the [capabilities](docs/product-capabilities.md) and [integrations](docs/product-integrations.md) guides connect intent, delivery and evidence. For day-to-day usage see the [CLI reference](docs/cli/README.md). For project conventions, build commands, and developer workflow see [CLAUDE.md](CLAUDE.md). For getting set up see [docs/getting-started.md](docs/getting-started.md). For *"how does AIDA fit alongside X?"* — neighbor-by-neighbor comparisons against `/ultrareview`, Karpathy-style structured markdown, Linear/Jira, etc. — see [docs/internal/positioning/](docs/internal/positioning).

---

## Vision

The defensible niche is the **agent-collaboration layer**: stable spec IDs, typed relationships, code-to-spec trace comments, and an MCP server that exposes the requirement graph to coding agents. Karpathy-style "structured markdown queryable by Claude" is the floor; AIDA is the *durable index* on top of it. Its nearest competitor, GitHub Spec Kit, produces structured specs per feature and then freezes them — AIDA's delta is keeping them a maintained, cross-cutting graph (stable IDs + typed relationships + enforced traces + lifecycle, queryable via MCP, portable across vendors because it lives in git) that outlives any single feature. A small invisible kernel that captures what exists, plus optional layered modules for everything else. (See [docs/archive/positioning/vs-spec-kit.md](docs/archive/positioning/vs-spec-kit.md) and the [2026-05-31 competitive synthesis](docs/archive/competitive-analysis/2026-05-31-round2-moat-gaps-moves.md), an archived dated snapshot.)

---

## The niche, concretely

<!-- trace:TASK-1187 | ai:codex -->
> Stated as product positioning with evidence attached: this is the niche AIDA is built to own. Backing: the theory paper [§7 (the apex claim) + §15](docs/internal/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md) and the two ablations cited below.

**What it's for.** AIDA is the neutral, **cross-vendor decision record** for a multi-agent codebase: the shared graph of *why* (specs, typed relationships, code-to-spec traces, and lifecycle state) that any agent reads and writes through one CLI/MCP surface. Around that record sits the coordination layer that lets agents and a human share one workspace: leases, queues, briefs, punts, findings, directives, and role/RBAC gates (see [roles/seats vs stakeholder personas](docs/internal/architecture/roles-seats-and-personas.md)). One role queue, one lease table, one intent graph — vendor-agnostic by construction.

**Why it's defensible.** The durable record is the product boundary. Agent runtimes, IDEs, PM tools, and vendor orchestration can coordinate inside their own walls; AIDA keeps the decision record in git with stable IDs and code links that survive vendor switches, machine switches, and years. The open-brief bake-off ([2026-06-18-open-brief-convergence.md](docs/internal/research/ablations/2026-06-18-open-brief-convergence.md)) found that vendors handed the same open brief converge on the same design because the shared record dictates the shape; execution quality still varies, which is why selection, verification, and regression-catching remain valuable.

**The concrete embodiment.** `aida why <file:line>` answers why a line exists from a trace comment or conventionally trailered commit. `aida graph impact <id>` shows what a requirement touches. `aida compete` can run a spec through multiple vendors in isolated worktrees and let objective gates plus review pick the strongest result. `aida spec dryrun` and `aida spec interview` tighten specs before implementation. The wedge is practical and machine-checkable: stable IDs, typed relationships, lifecycle state, and inline `// trace:SPEC-ID` links that connect code back to intent.

**Type protocols.** AIDA stores concise work contracts for spikes, bugs, stories, tasks, decisions, and docs as editable META requirements, with optional `research`, `docs`, and `keystone` lane overlays. Interactive pickup and headless implementer/reviewer prompts inject the resolved protocol before work begins, cite its META ids, cap the combined body at 40 lines, and label the precedence `type < lane < spec acceptance`; a leased session receives the compact type reminder again in its per-turn notice. Inspect them with `aida protocol show <type> [--lane <lane>]`; MCP clients read the identical text at `aida://protocol/<type>[/<lane>]`, and editing either META body changes the next pickup without rebuilding AIDA.

<!-- trace:TASK-1328 | ai:codex -->
**Queue completion ownership.** `aida queue done` uses both the current display
ID and the stored origin ID from the resolved requirement. Remapping an ID
mid-session therefore preserves ownership of an existing branch and its commit
trailers. Mixed branches naming unrelated requirements remain refused, even
when their commits name an accepted alias; unscoped branches retain the existing
commit-evidence and ledgered `--force` rules. Branch lookup failures,
including seat-clone refusals and detached HEAD, refuse before mutation even
with `--skip-pr-check`; `--force` warns and retains the ledgered override.
<!-- trace:TASK-2887 | ai:codex -->

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
redesign; see [coverage and limits](docs/internal/testing/bug-1683-drain-lock.md).

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

**Honest scope.** AIDA is alpha. The core graph, traceability, queue, and MCP workflows have held under this repository's own multi-agent dogfood; broader-team scale, turnkey unattended reliability, and onboarding outside this project are still being validated. Selective gating, not blanket: a programmatic gate beats a stated rule only when the invariant sits *far from the point of action* (attention-distance; [2026-06-18-gate-vs-rule-pilot.md](docs/internal/research/ablations/2026-06-18-gate-vs-rule-pilot.md) falsified the blanket form). Every claim above traces to a finding or a shipped command.

---

## First surface: `aida why` and the memory lane (the TUI is a view, not the face)

The humble first surface is **`aida why <file:line>` plus the memory lane**: point at a line, get the decision behind it; let a project remember decisions across ordinary agent chats before any queue, drain or role exists (see the README's *Start here* section and [The Memory Lane](docs/internal/positioning/memory-lane.md)). Depth — the typed graph, stable IDs, trace enforcement, the MCP server, the queue and review lifecycle, the control plane that keeps the store true under unreliable agent writers — is discovered **through the store**, not through a front-end.

The TUI (`aida tui`, [EPIC-26](docs/internal/positioning)) is **a view onto the control plane, not the product's face**. It hosts Claude Code as a child process, overlays status, and offers quick actions for review, queue, merge and pull; the CLI and MCP cover every operation without it. For the TUI itself — hosting model, keybindings, status overlay, autonomous drains, crash recovery — see [`docs/tui/README.md`](docs/tui/README.md).

When adding features or polish, the test is **"does this make the store easier to consult?"** — not "does this make the TUI's quiet depth stronger?".

> **Superseded history.** From 2026-05-14 to 2026-09-26 this section was titled *"Public face: the TUI is the product"* and carried the **Trojan-horse positioning**: ship a deliberately humble TUI, let the platform be discovered through use. The positioning deep-dive ([2026-09-24](docs/internal/positioning/2026-09-24-spike-86-positioning-from-engineering.md)) found it unsupported by the engineering record (the TUI took 1,277 of 176,968 attributed changed lines over 30 days and 0.7% of authored specs), and the decision record `ADR-59` replaced it. The *tactic* it carried — look humble, let depth be discovered — survives, moved to `aida why` and the memory lane.

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
- **Don't compete with Claude Code's session model** — extend it via worktree-isolated implementer sessions, role-pure boundaries, queue routing, and the TUI that hosts Claude Code itself as a view onto the control plane ([EPIC-26](docs/internal/positioning))
- **Compete vertically only where the platform structurally won't go** — the graph, the IDs, the trace network, the queue/role/session opinions, the requirement lifecycle

### The risk + how AIDA mitigates

Two risks worth naming explicitly:

**Risk 1: Anthropic ships a vertical that overlaps AIDA's core (a built-in requirement graph, a `/track` command, etc.).** Mitigation: AIDA's core value is the *composition* of graph + IDs + traces + MCP + queue + lifecycle. A built-in graph alone wouldn't replicate the trace network or the lifecycle. A built-in lifecycle alone wouldn't have the graph. AIDA's moat is the multi-layer stack, not any one feature.

**Risk 2: AIDA's CLI surface keeps growing as Anthropic adds primitives, leading to confused users.** Mitigation: keep one humble front door — `aida why <file:line>` and the memory lane — and let everything else be discovered through the store. Surfaces (CLI, MCP, TUI, web) are views onto the same store and control plane, so a new primitive adds a view, not a second product. (The earlier mitigation, the Trojan-horse TUI positioning, was superseded on 2026-09-26 — see *First surface* above.)

### Summary

AIDA's bet is **vertical depth on horizontal ground**: Anthropic ships the substrate; AIDA composes the substrate into a specific workflow domain (agent-collaboration on project intent). The bet stays sound as long as Anthropic stays horizontal, and Anthropic's structural incentives push them to stay horizontal. `aida why` and the memory lane are the front door; the intent store and the control plane that keeps it true are the vertical depth; the surfaces — CLI, MCP, TUI ([EPIC-26](docs/internal/positioning)), web — are views onto them; the Claude ecosystem is the ground all of it stands on.

---

## Architecture

**Store-independent observations.** `aida intent note`, `validate`, and `export`
produce or inspect an explicit six-field JSON envelope without requirements,
identity discovery, telemetry, store setup or network calls. A dedicated entry
path precedes shared CLI startup; late alias dispatch refuses and directs the
caller to the direct spelling. SHA-256 covers ordered compact JSON of the five
payload fields; the full digest is the identity, with a 12-hex display label.
Reads and serialized size are bounded, imported prose is data, and optional
explicit regex rules refuse sensitive content without certifying sanitization.
Manual-email export is a self-contained JSON attachment. Store-only transport
retains its separate authority. Independently enrolled intake transport uses
`aida-intake/` under the clone common Git directory, with a separate repository,
worktree, index, object database, refs and permanent lock. Only `aida-intent`
is transported; full incoming history is validated before fast-forward
acceptance. Explicit processing stores immutable receipts and journals
create/comment operations to recover confirmed writes without replay. See [the wire format and commands](docs/cli/02-specs.md#aida-intent-note-validate-and-export).
<!-- trace:STORY-2479 trace:TASK-3096 | ai:codex -->
<!-- trace:TASK-3114 | ai:codex -->

### Agent launch migration

`aida-cli-lib/src/launch/` resolves launch policy into `LaunchContext` and
constructs vendor processes through `spawn_agent`. Seat assignment is explicit;
background handoffs recover lane identity from the worktree lease. An exact-count
source ratchet inventories constructors and protected environment writers outside
the launcher. The effectful vendor profile owns Codex finalization, after seat
and identity decisions and before stdio or process hooks. Launched children mint
through one `launch::issue_launch_child` entry, retaining delegation and keyring
checks. AIDA control children execute the kernel-held `/proc/self/exe` image, surviving
promote/rollback renames and unlinking, with `argv[0]` set to `aida` so process
labels, loop-anchor detection and CLI usage retain the AIDA name. They forward
only the caller's grant selector; routing roles travel as explicit command args.
The TUI uses the same selector-only `control_env` policy. Resource validation
grants no write authority by itself.
<!-- trace:STORY-2326 | ai:codex -->

Execution input APIs live in `aida-core::exec_context`: immutable environment,
explicit home/project/authority roots and tool selections, plus an injected
wall/monotonic clock. Child projection belongs to `launch_policy::EnvPolicy`
and its existing owned-key tables. These types confer no authority. Production
reader migration and vendor env-clear activation are separate, incomplete work;
the [migration inventory](docs/internal/testing/exec-context-migration.md) defines
the shrinking legacy ledger and API ownership boundaries.
<!-- trace:STORY-2538 | ai:codex -->

`aida-core/src/vendors/` owns the pure `profile(LaunchVendor)` registry, with
identity spellings, interactive prompt transport, hook paths and closed
wake/resume/background/sandbox capabilities. `launch_policy` re-exports the
existing vendor key. Session identity, wake selection, resume eligibility and
operator hints consume these descriptors; `aida-cli-lib/src/vendors/` renders
vendor-facing hints. The effectful `launch/profile.rs` adapter references the
same descriptor and retains the existing finalizer. Capabilities do not admit
launches or grant containment credit; AGY's outer-only containment candidate
still refuses pending conformance. Headless argv/settings, hook event/backend
and resume/fork backend migration, the effectful adapter move and the shrinking
production vendor-literal ratchet remain later slices. See the
[approved sketch](docs/plans/2026-11-06-story-2539-vendor-profile-sketch.md).
<!-- trace:STORY-2539 | ai:codex -->

### The four layers

For contributors, reviewers of architecture sketches, and anyone choosing what AIDA benchmarks against. The layering is validated by dependency direction (store and intent modules import no control-plane module; the control plane depends heavily on the store) and by the removal test (remove the control plane and the whole memory lane still works; remove the store and nothing meaningful does). Source: the [2026-09-24 positioning deep-dive](docs/internal/positioning/2026-09-24-spike-86-positioning-from-engineering.md), §3.

1. **The intent store (the data plane).** The git-canonical requirement graph: YAML objects on the orphan `aida-store` branch, stable IDs, typed relationships, comments, the rebuildable cache, and the intent layer that makes the store worth keeping — acceptance criteria, `// trace:` comments, reconstitution, contradiction and gap detection. This is what "requirements management" names, and it is the product. Reconstitution reports probe failures even when recall has no traced-test denominator; a successful agent exit still requires readable, valid probe output, with artifact and headless-log paths retained in failure reasons (see [reporting](docs/cli/08-reporting.md#aida-reconstitute)).
2. **The control plane (the corpus-integrity layer).** The layer that decides whether work may advance and reports the state of work in flight: the queue, the drain, the orchestrator and its phases, seats, leases, worktree assignment, review verdicts, merge-holds, `BlockedBy` gating, required-check rollups, the event feed, and the surfaces that report on all of it (`aida ps`, drain status, awaiting, doctor). It exists to keep the store true while unreliable agent workers change the code; that is why it takes about half the engineering without being the product. Its two sub-facets have industry names — admission / merge gating (a merge queue) and state reconciliation (actual vs reported vs desired) — and its characteristic defect is a surface asserting a state that is not true: false-green, false-empty, stale reading as current.
3. **The execution layer.** Isolated worktrees and sessions in which agents (Claude Code, Codex CLI, Antigravity, any harness with a shell) do the work, one spec per worktree, with role-pure seats and deterministic handoffs.
4. **The surfaces.** CLI, MCP, TUI and web. They display store and control-plane state; none of them is the control plane and none of them is the product's face.

### Storage (EPIC-1-001) — git-canonical by default

Transport policy is separate from shared project policy. The optional `work`
profile is versioned JSON in the common Git directory, shared by linked
worktrees. Its endpoint fingerprint and optional explicitly enrolled public-safe
baseline never enter the requirement store. Profile configuration and push/pull
plans dispatch before store initialization; plans do not fetch or run lifecycle
actions. Push execution validates the attached clean store under its writer
lock, releases the lock, and makes one attempt to push the pinned commit through
verified store-only hooks. It preserves identity policy without code hooks or
other lifecycle actions. Outbound publication additionally requires an owner-only,
operator-reviewed policy and an explicit public-safe trust root. The shared core
scanner checks raw and decoded text, full new commit history, identities and tree
paths; unsupported or incomplete evidence refuses before any push. Private
profile/policy/receipt IO uses anchored no-follow handles. Exact successful-push
receipts bind endpoint/ref/tip/root/policy/scanner; every attempt rescans from the
original root, so receipts neither cache scans nor grant trust to tracking refs.
Prepared sanitized writes and recoverable fast-forward store pull are implemented; unsupported writers and divergence refuse. This bounded
known-identifier guard requires operator review; raw Git/foreign writers and
PUBLIC forwarding remain outside enforcement. Fixed store refspecs cannot expand
through environment defaults, and
changed/ambiguous origin endpoints refuse. Raw Git and other sync commands are
outside this client-only boundary; there is no receiving-side enforcement.
See [transport configuration](docs/cli/07-project-setup.md#aida-config-transport).
<!-- trace:STORY-1742 | ai:codex -->

The orphan branch `aida-store` is the writer of record. Each requirement is one YAML file at `objects/<TYPE>/000/<SPEC-ID>.yaml`, committed to that branch. Writes go to git first, then to a local SQLite cache (`.aida/cache.db`, gitignored, rebuildable).

- **Live worktree:** `.aida-store/` (gitignored on the main branch; populated by `aida init`)
- **Branch:** `aida-store` on origin
- **Cache:** `.aida/cache.db` — read projection for fast `list / search / filter`; ordinary reads use the [bounded cache-read contract](#bounded-cache-reads) when freshness differs
- **Sync:** `aida db sync --pull --push` (fetches the explicit store branch into a temporary ref, then rebases onto its pinned commit for linear orphan history; shared `FETCH_HEAD` is neither read nor written)
<!-- trace:TASK-1604 | ai:codex -->

**Adoption without product files.** The same branch can carry an `adoption/` namespace of shared, non-executable settings and guidance. Each worktree projects one pinned revision into git-ignored `.aida/` (config, guidance and an ownership manifest), excluded through `.git/info/exclude`; config readers and `aida agent` launches consume the projection, and scaffold and config writers refuse to touch product files in an adopted clone. `aida init` in an adopted clone projects instead of scaffolding. `aida adoption` covers status, refresh, a revision-pinned CI bootstrap, a dry-run migration inventory for tracked scaffolding, and an ownership-checked uninstall. Executable configuration stays under config-trust. See [`aida adoption`](docs/cli/07-project-setup.md#aida-adoption).
<!-- trace:STORY-1741 | ai:claude -->

The legacy centralized SQLite path (`aida init --centralized`) still exists but prints a deprecation warning. PostgreSQL is opt-in via the `postgres` feature flag for teams wanting a server-backed shared projection.

Store commits run only AIDA's shipped pre-commit, commit-msg, prepare-commit-msg,
and post-commit hooks, plus a shipped pre-push identity gate. Host git pins a versioned directory under its common git
directory; project `core.hooksPath` (including husky, pre-commit and lefthook)
applies to code commits. The host profile is installed atomically from embedded
template masters and checked before use: exact entries and bytes, regular files
with one link, current-user ownership, no symlinks, executable hooks, and no
write permission for other users. Unsafe parent directories also refuse writes;
root-owned sticky temporary roots are allowed. Existing suspect profiles are
never silently repaired. Own-repository stores use only their sandbox-denied
`.git` subtree; an authority path elsewhere inside the writable store refuses.
Contained launch prepares the profile on the host before the sandbox starts,
without adding writable paths. Host git authority directories are denied in
both the sandbox and direct tool rules, including a primary-checkout launch.
Store git removes relative/empty PATH entries
and paths into the store, and uses the empty tree as `GIT_ATTR_SOURCE` so the
store's `.gitattributes` cannot select a host filter or textconv driver. Older
Git ignores that variable. On Git below 2.40, or when `git --version` cannot be
parsed, contained mode is unavailable: launching a contained session or
enabling the contained posture refuses, and host store git refuses while
contained mode is in play for the store (`[agents] contained` /
`[contained] enable`, `AIDA_CONTAINED`, or the marker a contained launch leaves
in the git common directory). Without contained mode, store git keeps working
and warns once. The fix is Git 2.40 or newer.
<!-- trace:BUG-2131 | ai:claude --> The store-only hook marker keeps detached and override stores
away from project-local executable probes without bypassing code-commit gates.
Regression and mutation evidence: `aida-core/src/git_ops/tests/store_hook_tests.rs`
and `tests/test_bug_1929_hook_templates.py`.
<!-- trace:BUG-1929 | ai:codex -->

A contained session has no write access to the main git directory. Launch
settings deny all of `<main>/.git` in both the sandbox (`denyWrite`) and the
direct tool rules, whether or not the store write paths are granted, under
every spelling of the path (as given and canonical, so a symlinked checkout is
covered). When some spelling cannot be written as an exact rule (it holds a
character such as `( [ ! {`), the contained launch is refused with an error
naming the path: no settings are built and no session starts, because Claude
Code's own sandbox would otherwise leave `<main>/.git` writable to a
linked-worktree session. That
covers `worktrees/<wt>/commondir`, `gitdir` and `config.worktree`, which would
otherwise let a session point host git at a git directory it controls. It also
covers the shared config, hooks, refs and objects. Claude Code itself denies
writes to a worktree's own `.git` pointer. While these denies are in place,
Claude Code (verified on 2.1.302) leaves no writable path under `<main>/.git`.
This is conditional: it holds only for sessions launched with AIDA's contained
settings on a Claude Code whose sandbox applies `denyWrite` over its own grants.
As a consequence, a contained session in a linked worktree cannot commit,
because its index and objects live under the denied directory. A contained seat
commits in its own private clone instead (BUG-2169). Real-sandbox and unit
evidence: `aida-cli-lib/src/tests/bug_1982_main_git_metadata_tests.rs`.
<!-- trace:BUG-1982 | ai:claude -->

Host git over a code tree a contained session may have written (a linked
worktree, or any tree of a repository a contained session was launched
against) runs with `diff.ignoreSubmodules=dirty`, `submodule.recurse=false` and
`fetch.recurseSubmodules=false`, plus `submodule.<name>.ignore=dirty` for
every name in the `.gitmodules` git would read (the file, else the index or
`HEAD` copy), because a `.gitmodules` `ignore = none` outranks
`diff.ignoreSubmodules`. A `.gitmodules` that cannot be read, or that names a
submodule the override cannot express, makes host git refuse. So incidental
status and diff never enter a submodule, where a planted repository's own
config (a filter driver, for example) would run on the host. Before any safety
check in such a tree (pool, adopt, destroy, salvage, session reap, doctor
heal, worktree gc) and before host staging (`aida compete`), every gitlink
must be a host-owned submodule: registered in `.gitmodules`, with a `.git`
pointer that resolves to a submodule git directory (`<common>/modules/...` or
`<common>/worktrees/<wt>/modules/...`) inside the sandbox-denied common git
directory. Staging also refuses an untracked nested repository at any depth.
A tree that fails this is kept and treated as dirty, and so is a tree whose
`git status` fails: no dirty check reads a git error as clean. Git itself never recurses into a
submodule there, even once every gitlink passes, because it would read each
submodule's session-writable work-tree `.gitmodules`. The checks inspect each
checked-out submodule themselves instead: policy B again over that
submodule's own index, then its status through the git directory validated a
moment earlier (`--git-dir`, never re-reading the `.git` pointer), nested
submodules likewise. So uncommitted edits there still keep the tree. A
submodule with no `.git` is skipped (a deliberate choice: re-reading a
pointer would reopen the swap window), and the toplevel status never lists
files inside a gitlink path, so edits left in a checked-out submodule whose
`.git` was removed are invisible to the destructive gates. Only a session
tampering with its own submodule can reach that state. The non-destructive `aida ps` probe
never looks inside a submodule. Salvage refuses edits inside a submodule, because a patch cannot
hold them. Other checkouts keep git's submodule handling. Every host
`git status` goes through `git_ops::host_git_command`, directly or via a
wrapper; a guard test enforces it.
<!-- trace:BUG-1985 | ai:claude -->
<!-- trace:TASK-1952 | trace:TASK-1943 | ai:claude -->

### Distributed identity (EPIC-1-052)

New WORK clones get a random opaque UUID node through `aida init`. The versioned
binding and persistent per-type dispenser live in the clone's common Git directory,
shared by linked worktrees. Missing allocation settings mean WORK; only a bounded,
owned local `aida-allocation.toml` with explicit HOME authority permits global
allocation. Shared objects, preferences, environment variables and remote URLs
confer no allocation authority. Legacy node/registry schemas retain their meanings;
legacy counter high-water marks migrate once on admitted local allocation.
<!-- trace:BUG-2017 | ai:codex -->

- **Node-aware IDs** have `<TYPE>-<opaque-node>-<SEQ>` form. WORK uses per-type
  counters, including through CLI/MCP and metadata seeding, without global fallback.
- **Agreed IDs** such as `FR-052` are assigned only on HOME by `db merge-gate`.
  Native old-node IDs, UUIDs, relationships, comments and traces remain available;
  repeated promotion is a no-op. Destructive node-alias retirement refuses.
- **Global blocks** and legacy shared-node registration require HOME authority.
  WORK skips implicit global first/refill claims; `node acquire` repairs its local
  opaque binding instead of registering host/user/email identity.
- **Enrollment** uses `config allocation enroll-home` at a direct human terminal.
  It is never part of init or migration. Long-running MCP sessions restart after
  enrollment. Implementation never enrolls real HOME.
- **Bootstrap** binds the actual allocator before seeding in default, post-clone
  and separate-store init. A lost/malformed/linked counter refuses allocation;
  it is never silently reset. Non-Git directory MCP reads remain supported while
  unbound allocation refuses.

See [allocation setup](docs/cli/07-project-setup.md#clone-local-allocation-authority)
and the [writer/slice ledger](docs/plans/2026-10-24-bug-2017-allocation-writer-inventory.md).
All-conflict reconciliation/audit, STORY-1742 scanner integration and final slice-3
acceptance remain pending; slices 1+2 do not complete BUG-2017.

### Completion intent and deliberate reopen

`aida pr ship` derives completion credit from an explicit trailing title group,
then branch-name recovery, and accepts only IDs that resolve unambiguously in
the canonical store. References in title prose and PR bodies remain descriptive;
a partial-work PR with a neutral branch can ship without completing its owner.
When a title is present, an older branch-head trailer cannot supply missing
credits. When the title is empty, the branch-head subject supplies the title.

One PR of a multi-PR spec can declare itself a non-completing slice —
`aida pr ship --no-complete`, or a PR-body line `aida: no-complete` /
`Non-completing slice` — and ship then writes no completing trailer from any
source, so no spec is completed by that merge. The branch-name recovery
is also skipped on its own for a spec that carries `closure:pending` or still
has open slice children (Done children count as finished; an explicit title
trailer remains a deliberate directive). Ship prints the derived trailer and
its source — PR title, branch name, or none with the reason — before it
merges, and `--dry-run` prints the same line.

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

### Canonical gate ledger

The gate architecture is **effective gate state = f(head SHA, canonical IDs,
ledger)**. `gate::canon` resolves a change's spec labels against one committed
store snapshot to immutable requirement UUIDs (`SpecKey`); native, agreed,
node-local and retired IDs are spellings of that identity. Persisted
`spec_key` wins over an event's audit label. Unstamped compatibility events use
the snapshot's exact label mappings; unresolved or ambiguous identity cannot
grant approval. Superseded specs inherit their carrier's execution mode.
Operators can stamp existing compatibility events with the one-time
`aida gate migrate` rewrite, which stamps each event with its requirement's
canonical `spec_key`: `--dry-run` previews, `--execute` applies and `--rollback`
unstamps the current tree while preserving later events; execution and rollback
require an interactive host terminal outside a sandbox, with no drain running.

`gate::effective::effective_gate_state` computes verdict, hold, execution mode
and decision from supplied inputs without reading files, Git, the clock or the
environment. Evidence loading and reviewer authority checks are separate
stages: selecting an approval does not establish its grant or independence.
`aida gate state PR-<N>` exposes this state read-only; `--compare` shows it
beside the existing merge-gate readers, column by column, and counts
differences. `--json` provides the machine-readable diagnostic. These commands
do not clear holds, record reviews or authorize a merge.

The canonical ledger lives in the primary checkout's shared runtime store,
`<main>/.aida/gate-ledger/PR-<N>.jsonl` (or `MR-<N>.jsonl`). Spec indexes and
unbound pre-change entries live under `by-spec/`; they do not give a second
worktree authority over the change. Merge-hold markers and recorded clearances
belong to this primary store as hold/release events, with forge labels as
mirrors. BUG-2176's reviewer-worktree marker-copy workaround is retired as an
architecture: a disposable reviewer worktree must not be the durable owner of
a hold. One-way collection onto the primary ledger remains for legacy worktree
markers older than M1 slice 2; no current writer creates a worktree marker.
The host launcher validates and collects contained reviewer evidence
into shared state. Retained legacy files are compatibility evidence, not a
new place to write gate state.

STORY-2324's final documentation slice follows PR-C (#3485, `c09c3a0`). The
optional monitor consumer is separate: STORY-1392 is re-scoped to consume gate
state, and its implementation is outside this story's completion scope.
<!-- trace:STORY-2324 | ai:codex -->

### Forge observations

`forge_observed::Observed<T>` distinguishes settled `Known` values from
`Pending` computation and `Transient` read failures. `Forge::observe_*` exposes
status, metadata, check rows and branch lookup through this type; an empty check
rollup is pending. GitHub's `UNKNOWN` mergeability stays pending, and incomplete
status output is transient. Status observations preserve definitive refusals
(such as a missing PR or denied authentication) as `Known(Err(reason))`; ship
fails immediately on these without treating them as conflicts. Ship preflight
retries unsettled mergeability three times, two seconds apart, and refuses
mergeability only on a settled false; the forge still enforces conflicts at
merge time. The wave gate distinguishes a settled GitHub conflict from unknown
evidence.

The condition checker also reads PR merge states through this boundary, with a
15-second timeout; failed reads and malformed states evaluate to Unknown.

Question parking preserves the existing trigger and shelf timestamp when the
consequence is empty or whitespace. With no existing trigger, an empty
consequence defaults to `manual:<resolver>` without adding an empty comment.
Prose consequences retain their text as a comment and use a manual resolver.
<!-- trace:STORY-2402 | ai:codex -->

Train and idle-eligibility GitHub commands now live behind `forge.rs`. A per-file
ratchet freezes remaining raw GitHub reads outside that boundary and catches
calls/imports of the legacy train pass-through helpers outside `pr_train.rs`.
Destructive lifecycle consumers use `forge_confirmation`: two identical metadata
reads, three seconds apart (at most three reads), plus a fresh invocation-private
base fetch. Ancestry, equal tip trees or a PR merge-subject receipt on that base
supply independent Git evidence. Failed fetches, changed snapshots and conflicts
between Git and forge state retain the change, queue entry and spec status.
GitHub confirmation metadata reads are bounded to 15 seconds and reject partial
or unknown states. Publication retraction also preserves its ownership check.

Train retirement/abort share the same confirmed-negative gate, pinned to the
tested head and base. A confirmed landing takes the member-landing path; unknown
evidence suspends without ejection or closure. Red CI is re-read before retiring.
Unavailable CI reads suspend; required-check registration (including missing
aggregates) and explicit settlement timeouts fail deterministically. Typed
member-gate errors distinguish settled policy refusals from unavailable evidence: confirmed
refusals and deterministic CI failures eject members and retire the train only
after negative landing confirmation, while unavailable reads suspend.
Abandoned-train cleanup rechecks the body owner, stops new admission on unknown
evidence and preserves the branch when closure fails. Review-watch fetches retry three times with three-second backoff;
a failed pre-review fetch carries a process-bound retry marker through the
queue-work receipt, without spending the ordinary setup retry or a review round.
Review queue cleanup and stranded review-story completion require the stable
metadata/Git confirmation too.

Train intent is stored as versioned JSON under the main worktree's `.aida/train/`,
with a permanent `run.lock` serializing recovery and admission. Atomic replacement,
file fsync and directory fsync precede forge operations. Each entry pins the tested
head, base and member heads, records its lifecycle step and completed member cleanup,
and checkpoints nested forge reads and transport attempts, including the final
merge gate's legacy metadata retries and confirmation's independent base/head
fetches. GitLab CI registration, watch, post-watch rows and each nested
pipeline/job API read also checkpoint; unavailable post-watch rows suspend instead
of becoming a red verdict. A failed write refuses the call. A later
non-dry train invocation reconciles unfinished entries before admitting new riders:
exact-number metadata and fresh `forge_confirmation` evidence
recognize completed merges, finish outstanding member closes, or complete a recorded
retirement. Unknown or changed evidence retains the journal and stops admission.
Completed closes are recognized from forge state, and completed journal entries are
durably marked Done then pruned. Recovery reuses an existing PR and tested head,
rechecks CI and live gates under the ordinary merge lease, and never restores
persisted mandate authority.
Remote branches are retained by journaled retirement, avoiding deletion after an
uncertain close or a moved branch. Legacy trains without journals retain their
confirmed owner-liveness cleanup. Workflow-definition digest evidence remains slice 5.
<!-- trace:STORY-2401 | ai:codex -->

### Autonomy & escalation

The autonomous-collaboration layer — the three-mode autonomy ladder (default / `--zen` / `--no-human`), the implementer → advisor → human escalation cascade, the advisor's Type A/B/C resolve-vs-escalate calibration, and the file-based handshakes that coordinate the tiers — is described in [docs/internal/architecture/autonomy-and-escalation.md](docs/internal/architecture/autonomy-and-escalation.md). For the practical user guide to `--auto-complete` and `--no-human` see [docs/advanced/autonomous-drain.md](docs/advanced/autonomous-drain.md); for the MCP transport layer over the same filesystem substrate see [docs/internal/architecture/mcp-coordination-surface.md](docs/internal/architecture/mcp-coordination-surface.md). The **resilient-drain primitive** is EPIC-28 (`docs/advanced/autonomous-drain.md` → "Shelving on failure"): a shelvable phase failure parks the spec in `NeedsAttention` with a structured `FailureReason` and the batch drain continues past it, with dependents skipped automatically via the `BlockedBy` pickability gate (STORY-333) — exit 2 + `aida findings list` for triage.

For how this control plane — queue, drain, orchestrator phases, leases, verdicts, merge-holds, required-check rollups, the event feed — compares with the two families it most resembles, durable-execution engines (Temporal-style event history, replay, checkpointing, retries) and merge queues (Bors, Zuul, Mergify, Aviator, the GitHub merge queue), and which of their properties AIDA has, lacks, or deliberately declines because its workers are non-deterministic LLM agents, see [docs/internal/architecture/control-plane-vs-durable-execution-and-merge-queues.md](docs/internal/architecture/control-plane-vs-durable-execution-and-merge-queues.md).

---

### Child launch containment

Headless composition and spawn, interactive reviewer plans, and Claude resumes
strictly inherit `AIDA_CONTAINED` before creating logs or issuing child grants.
A literal `false` cannot downgrade the child. Stale uncontained reviewer plans
refuse at spawn. Compete validates every arm and the selected judge before any
brief, worktree cleanup or arm run; unsupported vendors refuse the whole run.
Contained arms and judges replace generated bypass arguments with explicit
`dontAsk` and containment flags, propagating `AIDA_CONTAINED=1`. Arms scope writes
to their child worktree; judges run at the project root and retain that root's
write boundary. Dry-run arm output shows the composed vendor posture with
settings and OS-wrapper preparation explicitly deferred to launch. Burndown and
intake share the permission-posture resolver.
<!-- trace:BUG-2172 | ai:codex -->

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

Hypothesis pre-registration uses a `hypothesis` tag on an existing spike or
decision. Canonical store transactions persist a sticky timestamp and description
hash when a `references` or `verified-by` neighbor is Completed; later description
edits direct amendments to comments. CLI and MCP share this preflight, including
indirect lock writes when an experiment completes. See
[pre-registration](docs/cli/02-specs.md#pre-registering-a-hypothesis).
<!-- trace:STORY-1675 | ai:codex -->
AIDA-owned store syncs keep that evidence across clones: rebase, union merge
and reconcile fast-forward pin both tips and their merge bases, refuse a
changed, deleted or divergent locked description before Git runs, and commit
any lock the combined graph first establishes before reporting success. A
failure after Git is reported with exact commits and is never reset. Raw Git,
older binaries and lock-free readers are outside this guarantee. See
[store sync](docs/cli/10-storage.md#store-sync-and-pre-registration).
<!-- trace:BUG-2162 | ai:claude -->

## Feature surface (high level)

`aida regen corpus <dir>` freezes committed requirements and knowledge rows,
including archived rows and comment threads, into versioned `corpus.json` plus
`manifest.json`. The manifest pins the store SHA and SHA-256 of the corpus bytes;
exports of the same commit are identical across checkout paths. Only projected
spec content is included: source artifacts, attachments, history, processing
records and implementation metadata are excluded. Store-authored prose is kept
verbatim. The output directory must be new or empty. Uncommitted changes under `objects/`
refuse unless `--allow-dirty` is explicit; runtime files outside `objects/` do not
count as dirty. The override still exports committed HEAD and adds
`dirty: true` to the manifest. Legacy stores must migrate to git-canonical storage.
<!-- trace:TASK-1729 | ai:codex -->

`aida regen score <system-dir> --corpus <corpus-dir> --model <name> --vendor <name>
--harness <name>` runs raw R1 scoring and records versioned JSON under
`regen/trials/<uuid>.json` in the canonical store. Configure `[regen]` on the
trusted default branch with `oracle_command` (required argv array), optional
`install_command` (argv array; defaults to `./install.sh` when present), and
`required_tools` (tool names guarded against fallback). Required nonempty
`oracle_paths` lists repo-relative files/subtrees for the held-out oracle. Inputs
are frozen via git from the same pinned trusted commit as config; worktree files
are never used. Absolute paths, `..`, escaping/cyclic symlinks, omitted or directory link
targets and submodules are refused. The content-addressed bundle lives under
`$XDG_CACHE_HOME/aida/regen/oracle/<hash>` (default `~/.cache`), outside `/tmp`, `/dev`, `/proc`,
project worktrees and the trial root. Its hash (sorted, length-framed paths,
modes and bytes) is rechecked before execution; tampering refuses scoring.

Generated commands have a 30-second execution timeout per invocation. Set
`AIDA_REGEN_COMMAND_TIMEOUT_SECS` to an integer from 1 to 86400 before scoring
to change it for the run (including reference/stub controls and R2). Invalid
values refuse scoring. Timeouts kill the child process group and retain a
failure reason in the trial, without scoring partial oracle output. The deadline
includes stdout/stderr collection even if the direct child exits before its
descendants. Each stream has a 1 MiB capture limit; exceeding either limit kills
the group and records an output-limit failure without child diagnostics. The
best-effort survivor probe runs after a successful installation; installer
failure paths (including timeout) return before that probe, so escaped
descendants on those paths are not detected by it. A completeness `--help`
probe that times out marks its component incomplete with the reason
`--help timed out`; the trial still runs the oracle and keeps its record.
<!-- trace:TASK-1791 | ai:codex trace:TASK-1791 | ai:claude -->

`oracle_command[0]` must name an executable inside the bundle (for example
`tests/oracle.sh`, with a shebang); it is resolved to an absolute bundle path,
never through PATH or the trial. The oracle runs from the read-only bundle via
GNU coreutils `/usr/bin/env --chdir` (required; missing or unsupported means
scoring fails closed), using `AIDA_REGEN_TRIAL_ROOT` and
`AIDA_REGEN_PREFIX` to locate the regenerated system and private install prefix.
The scorer first copies the selected system into a private `/tmp` trial root.
The installer runs there and receives `PREFIX`, whose `bin/` precedes guards and
system directories on its PATH. The oracle uses only frozen guards followed by
`/usr/local/bin:/usr/bin:/bin`; it invokes regenerated tools through the locator
variables, never by adding trial or prefix directories to PATH. Installer and
oracle use separate private HOME/XDG/TMPDIR trees; the oracle's directories are
created after installation. No Cargo/Rustup settings are inherited. Existing bubblewrap strict-read
confinement is reused, and scoring refuses if its source-visibility probe can
still see the original project (including projects in essential system paths).
Confinement is required unless `--unconfined` explicitly records a non-clean trial.
Fallback guards are generated only for `required_tools`, frozen and hash-verified
like the oracle bundle, and bound read-only. They record an event to a per-run
AIDA-owned FIFO before executing an absolute host tool path (or failing if the tool
is absent). The FIFO is outside writable roots and read-only-bound with write-only
inode permissions: subprocesses can append evidence but cannot consume it, unlink
it, change permissions or truncate it. Logging failure refuses tool execution;
channel errors and unexpected EOF forbid a clean result. An inherited descriptor
is retained only as a best-effort survivor identity marker, not as the guard log.
Each install-writable root is independently bound with no bound ancestor. After
installation, the host quarantines the trial and prefix roots and copies their
bytes into replacements at the same paths, preserving embedded absolute paths.
Copying pins source descriptors, never follows source symlinks, and creates new
files rather than hardlinks/reflinks. A best-effort process/fd scan kills detected
installer survivors and records their count; any detection forbids a clean result
without suppressing the raw score. This is snapshot isolation, not a guarantee
that every descendant was found. Unconfined execution has no mount-namespace
isolation and remains subject to concurrent-write races.

The oracle emits exact LF-delimited `ok   - <name>` and `FAIL - <name>` lines on
stdout. Oracle authors should not echo unchecked trial output as marker lines.
Duplicate/conflicting names or no exact checks are scoring errors.
Installer failures have no score; nonzero oracle exits with parsed checks retain
the score and an exit flag. Guard hits, dirty corpus provenance and unconfined
execution are recorded and forbid a clean result. Records include the trusted
`oracle_sha`, `oracle_bundle_sha256`, `guard_bundle_sha256`, the env executable
path and `survivors_detected` (older records default that count to zero). Trial timestamps support age
reporting; per-check outcomes are retained for later controls and comparisons.
Raw cleanliness describes execution conditions, not the pass rate.
<!-- trace:TASK-1730 | ai:codex -->

Every actual system (including reference controls and R2) must have the declared
installer or `./install.sh`. Missing, unlaunchable and nonzero installers record
`install_failed`, distinct from oracle/check failures. Only generated no-op stub
controls skip installation. No source-layout subset is selected by the scorer.

The trusted oracle can write optional per-failure JSONL to the scorer-created
`AIDA_REGEN_CHECK_RESULTS` path, supplied only during the oracle phase. Each line
is `{"v":1,"check":"exact failing marker name","kind":"crash","exit":127}`;
`kind` is `crash` or `assertion`, and integer `exit` is optional. No extra fields,
duplicate identities, passing/unknown checks or unsupported versions are accepted.
Absent, unreadable, over-1-MiB or invalid sidecars classify **every** failure as
unknown; a valid partial sidecar leaves omitted failures unknown. Marker text and
exit numbers never imply a classification. The scorer keeps its original file
handle rather than reopening the child-writable pathname. These are oracle-supplied
diagnostics, not independent attestation of generated tool behavior.

Trial JSON retains versioned `failure_diagnostics`. Trusted
`[regen].crash_failure_share_percent` is an integer 0–100, default 50. Crashes
strictly above that percentage of **classified** failures produce
`crash-dominated: review before reporting`; unknown failures strictly above 50%
of **all** failures produce `unclassified: review before reporting`. Both can
apply. Ordinary R1/R2 warnings are attached after control validation and forbid a
clean headline; expected stub failures retain diagnostics without changing control
execution cleanliness. Reports show counts/unknown share and put warnings beside
the headline, including for legacy or invalid diagnostic extensions. The shared committed reader
derives these ordinary-trial warnings without rewriting historical bytes, so
statusline/digest consumers also receive the non-clean marker.

`regen score --model-family <identity>` records an explicit optional family on the
trial (and nested R2); model/vendor strings never infer it. `regen report` lists
**oracle-assumption audit candidates** from the latest scored ordinary R1 per
family, ordered by completion then UUID. Comparison requires matching corpus,
oracle SHA and bundle hash. A candidate must be observed in every compared family
and fail in exactly one; repeated runs do not count as separate families. Fewer
than two families is unavailable, absent/invalid-family trials are counted as
excluded, and checks unobserved in some families are counted separately. The audit
is a prompt for human investigation, not proof of an oracle defect. No launcher or
continuation policy is introduced here.
<!-- trace:STORY-1676 | ai:codex -->


Before ordinary scoring, generate controls with `aida regen score --control
reference|stub --corpus <dir> --model <name> --vendor <name> --harness <name>`.
The reference command exports the pinned trusted commit into a fresh directory
per repeat. Trusted `[regen].reference_repeats` defaults to 3 and must be positive.
Every repeat in the latest matching reference batch must be clean and pass 100%;
an incomplete or failed new batch cannot borrow older successful repeats.
The stub command starts empty, skips the project installer and generates no-op
executables for `required_tools` in the private prefix, exposed through
`AIDA_REGEN_PREFIX`. Oracles invoke these tools by locator; bare tool names
reach immutable fallback guards on PATH. Ordinary trials receive no generated tools.

Each trial copies the selected system's files into a fresh scoring root; installs
cannot mutate the caller's directory or carry state between runs. Root git
metadata is omitted. Special files and links to external, omitted or directory
inputs refuse. A pre-run comparison asserts exactly the placed files, modes,
links and directories, including empty directories. HOME/XDG/PREFIX are fresh too.
`root_fresh` records this condition; it is execution evidence, not attestation
against hostile code inside the existing writable scratch area.

The gate reads committed controls with the same `oracle_sha`,
`oracle_bundle_sha256`, corpus content hash and exact check-name set. Controls
with errors, nonzero exits, dirty flags, guard hits, unconfined execution or
missing fresh-root evidence refuse. Controls must also carry the current
`guard_bundle_sha256` and env executable evidence with zero detected survivors;
older executor records remain readable but cannot qualify. A trial with sandbox contamination refuses
a reported score. An ordinary nonzero oracle exit with parsed checks retains its
flagged discriminating score; a nonzero control exit invalidates the control.
Raw R1 `aggregate` remains available in records; the separate
`discriminating` aggregate excludes and lists stub-passing checks. An empty
discriminating denominator refuses. Records retain the control IDs used, and
reference batches retain their run ID and repeat identities. These are additive
format-v1 fields; older records deserialize with absent evidence and cannot
qualify as controls. A refused trial is stored as diagnostics without printing
a score to stdout.
<!-- trace:TASK-1731 | ai:codex -->


Ordinary scoring optionally reads `GAPS.toml` from the private trial input
snapshot before any execution. Missing input leaves raw R1 scoring unchanged;
a present file must be a regular file with the strict shared v1 schema:

```toml
format_version = 1
[environment]
CONTRACT_ENDPOINT = "REGENERATED_ENDPOINT"

[[gaps]]
id = "renamed-endpoint"
category = "interface"
note = "The implementation uses a different environment name."
```

Unknown fields and unsupported versions refuse. Environment mappings are
contract source name to regenerated target name; both must match
`^[A-Z_][A-Z0-9_]{0,63}$`. This is a data-only environment adapter: there are
no values, shell expressions, executable mappings or argument mappings in the
file. Gap rows share this parser: IDs are unique `[a-z0-9-]{1,64}`, categories
are `interface|missing|behavior|other`, and notes are optional plain text.

`aida statusline` and every `aida digest` format show the latest scored ordinary
discriminating R1 percentage and age, with `!` for non-clean or flagged trials.
Digest JSON includes an optional `regeneration` object. Missing scores or invalid
committed records omit this optional display; reads use local committed evidence.
A derived summary cache under `$XDG_CACHE_HOME/aida/regen-display` (default
`~/.cache`) is keyed by store HEAD and CLI build. An unchanged HEAD uses no Git
subprocesses; a miss reads trial blobs in one batch. Failed reads are not cached.
Digest selection excludes trials after `--until`; text ages use compact days,
hours, minutes or seconds. The cache grants no scoring or control authority;
unusual Git discovery environments omit this optional segment.
<!-- trace:TASK-1734 | ai:codex -->

Before scoring each actual system, required installed `PREFIX/bin/<tool>` entries
must be regular executables (links may resolve inside the private system/prefix),
contain at least `[regen].min_bytes` non-whitespace bytes (default 64, positive),
and contain none of these ASCII case-insensitive markers: `TODO: implement`,
`NotImplemented`, `NotImplementedError`, `unimplemented!`, `placeholder`. This is
a conservative heuristic, not a proof of implementation. The complete file is
scanned, including across read boundaries; binary/text files use the same rule.
`--help` must exit 0 with nonempty stdout or stderr, under the existing score
confinement and private installation environment. Helper output is never stored.
After each helper, the existing survivor scan and same-path quarantine run
again; all tool content, mode and resolved-target fingerprints are rechecked
before another helper or the oracle. A helper that replaces itself or a later
tool loses completeness, including a full-sized replacement without markers.
Unconfined execution retains the existing concurrent-writer limits.

`[regen.components.<required-tool>]` may declare `checks = ["exact check name"]`.
Mappings come only from pinned trusted config; names must be required tools and
each exact check may map to only one tool. Without any mapping, the restricted
score reads `unavailable (no check mapping)`. Unmapped observed checks are listed;
observed checks mapped to complete components alone form the restricted pass rate.
Split rates are derived from recorded checks/outcomes when reading, not trusted
from redundant stored percentages.

Trial JSON gains an optional versioned `completeness` extension. An incomplete
actual system records `status: incomplete`, with per-component reasons, overall
oracle pass rate and the restricted score as diagnostic numbers. Oracle execution
still runs so those diagnostics are available. Expected generated stubs skip
completeness. Completeness status is attached after raw control validation;
incomplete reference controls cannot validate later trials. R1/R2 retain their
own evidence and parent cleanliness reflects both. The statusline/digest reader
skips incomplete trials (and invalid completeness metadata), selecting the latest
eligible scored ordinary R1. `regen report` shows the latest attempt and its
incomplete diagnostics without turning its number into a headline.

Harness continuation/re-invocation is separate launcher work; these checks do
not launch or continue regenerators.
<!-- trace:STORY-1677 | ai:codex -->


`aida regen report` reads committed ordinary trial records from a pinned store
commit, without a cache rebuild or rereading the caller's trial directory.
The headline is the latest attempt's discriminating **R1 oracle pass rate** and
age in seconds. An unsuccessful attempt reports unavailable rather than falling
back to an older passing score. Status, clean state and flags remain visible.
Controls and nested adapted runs are excluded from trial selection.

Scoring stores a versioned `gaps` extension with the original GAPS.toml content
SHA-256 and parsed rows (IDs and categories), captured from
the private input before execution. Category counts are diagnostic information,
never a quality score. Deltas compare the latest two ordinary trials with matching
oracle SHA, oracle bundle hash and corpus hash; they are labelled diagnostics.
Missing or invalid gap metadata displays “no gap data”, and no deltas are inferred
from it. A valid GAPS.toml containing no gap rows explicitly records empty data.
Older trial records without the extension remain readable. Invalid GAPS.toml
input still refuses scoring before execution; reporting never reparses it from
a mutable caller path. Optional notes are parsed as plain text but omitted from
durable diagnostics. Gap IDs containing a captured mapping value refuse before
execution, without printing the ID or value.
<!-- trace:TASK-1733 | ai:codex -->

`aida regen score --consensus <trial-uuid> <trial-uuid> <trial-uuid> ...` compares
at least three distinct committed ordinary scored R1 trials. It cannot be combined
with a system directory, controls, corpus/model/vendor/harness execution inputs,
or `--unconfined`. No tools are launched. Mixed oracle SHA, bundle hash or corpus
hash, invalid discriminating aggregates, or differing discriminating check sets
refuse the entire selection before any filing. Adapted R2, control and unsuccessful
attempts cannot be selected. Dirty/untracked trial files never count.

A discriminating check failed by every selected regeneration is a **store gap**;
partial failures are listed separately as **capability-dependent**, never as store
gaps. The report gives N, the number of unclean trials included, and counts of distinct
recorded models/vendors (vendor names are trimmed and ASCII case normalized); a single
vendor produces an explicit weaker-consensus warning. Repeated runs are separate
regenerations, and their actual diversity counts remain visible. These labels are
investigation evidence rather than proof of cause. Contract clause mapping is not
yet available: every gap explicitly says `contract mapping missing`.

By default consensus only reports. `--file` explicitly creates draft amendment
tasks through the normal atomic requirement write path, preserving type-based IDs,
filing provenance and store hooks. A tag `regen-gap:<sha256(exact check)[:16]>`
identifies a check within this project store; deduplication and insertion share
the store write lock. Deduplication is scoped to this store root. Any non-archived
item with that tag whose status is neither Completed nor Rejected suppresses a new
draft, including after promotion to Approved or InProgress. Completed, Rejected or
archived investigations allow a new draft. The body retains the exact check name and compared trial IDs. No parent is invented
without a clause mapping. Check-to-clause mapping and parenting are a separate
follow-up; the command retains its explicit missing-mapping diagnostic until then.
<!-- trace:STORY-1679 | ai:codex -->


Sources and targets cannot name PATH, HOME, SHELL, ENV, BASH_ENV, IFS, TMPDIR,
PWD, OLDPWD, LANG, any oracle-owned variable, or a name starting with LD_,
DYLD_, PYTHON, PERL, RUBY, NODE_, JAVA_, CLASSPATH, GIT_, XDG_, AIDA_ or LC_.
Source names containing TOKEN, SECRET, PASSWORD or KEY (case-insensitive)
refuse. A target that is another distinct source, or a target shared by two
mappings, refuses with the conflicting mapping names. Self-maps are allowed
and recorded as no-ops. The complete map is validated before any source is read. Only the
explicitly named sources are captured from the caller, once. Missing values
refuse with the mapping name; each value must be UTF-8, at most 4 KiB, and
contain no NUL or newline. Values are copied verbatim without expansion.

R1 receives captured sources in its oracle environment. R2 then starts a fresh
private root and install from the same frozen input; its oracle receives those
same sources plus target aliases. Installers receive the isolated base
environment. Both runs use the exact same immutable oracle and independently
pass the same controls. Different check-name sets refuse. A valid nonzero oracle
exit retains its flagged scores in either run; installer and parser failures
remain distinct. A passing R1 cannot conceal a refused or contaminated R2.

Trial format v1 has an optional independently versioned `adapter` block:
`format_version = 1`, `adapter_input_sha256` (hash of GAPS bytes only), sorted
`mappings` with `source`, `target`, `source_present`, explicit `r1` raw and
discriminating aggregates, full nested `r2` trial
execution evidence, `raw_delta`, and `discriminating_delta`. Top-level
checks, aggregates, flags and exits describe R1; status/cleanliness/completion
time include both runs. Deltas are R2 minus R1 pass-rate fractions and are null
unless both runs score with identical check identities. Captured values and
their hashes are never stored. Child error diagnostics are withheld on mapped runs (status and exits remain);
check identities containing captured values refuse with the output withheld.
R2 setup failures are attached as `setup_failed` evidence with no fabricated
exit, check, guard attestation or fresh-root claim. Older records
remain readable with no adapter block.
<!-- trace:TASK-1732 | ai:codex -->


External wall displays and dashboards use a narrow, versioned read-only
contract rather than depending on incidental CLI JSON fields. The eleven
polling lenses and local event follow feed are declared by `aida contract
--json`, guarded by fixtures in CI, and documented in
[`docs/internal/monitor-contract.md`](docs/internal/monitor-contract.md).

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

`aida dev activate` prepends `target/{release,debug}/` (whichever is more recently built) to PATH and prefixes the shell prompt with `(aida-debug)` or `(aida-release)`. For releases, `scripts/release.sh {major|minor|patch}` bumps the workspace version, generates tag notes, and lands the bump through a `release/vX.Y.Z` PR (squash-merged once its checks pass; main is never pushed directly), then tags the merged commit and pushes the tag — triggering the release workflow that builds and publishes binary tarballs.

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
| [docs/getting-started.md](docs/getting-started.md) | Conversational development with durable intent and separate operator setup; installation is in [README](README.md#install) |
| [docs/cli/](docs/cli/README.md) | The CLI reference manual — when and why to use every command (12 chapters) |
| [docs/advanced/admin-guide.md](docs/advanced/admin-guide.md) | Storage administration, multi-user configuration |
| [docs/advanced/storage-modes.md](docs/advanced/storage-modes.md) | Deeper dive on storage modes and migration paths |
| [docs/internal/architecture/autonomy-and-escalation.md](docs/internal/architecture/autonomy-and-escalation.md) | The autonomy modes, the implementer → advisor → human escalation cascade, the advisor's Type A/B/C calibration, the inter-agent comms substrate |
| [docs/internal/architecture/mcp-coordination-surface.md](docs/internal/architecture/mcp-coordination-surface.md) | The MCP transport layer over the filesystem-canonical coordination substrate |
| [docs/advanced/autonomous-drain.md](docs/advanced/autonomous-drain.md) | Practical user guide to `--auto-complete` and `--no-human` (paired with the autonomy architecture doc above) |
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

`aida pr hold` retains Git safety refusals and repository lookup errors and
explicitly reports that no hold was recorded. Only Git's quiet missing-ref
result reports a missing branch. This preserves diagnostics; it does not
enable native-seat publication. Regression fixtures isolate Git discovery
from `TMPDIR` and retain the existing seat-refusal coverage. See
[PR lifecycle](docs/cli/04-git-lifecycle.md#aida-pr).
<!-- trace:TASK-2852 | ai:codex -->

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
[pinned live investigation](docs/internal/testing/task-1327-reconstitution-launch.md);
TASK-1-224 tracks the authority/configuration-root repair for advisor triage.
<!-- trace:TASK-1327 | ai:codex -->

`aida pr ship` checks known review, mergeability, stale CI definition, and hold
blockers before waiting for CI, and repeats the checks before merge. An optional
`--wait <secs>` deadline covers registration and settlement; the default is
unbounded. Exit codes 20–25 distinguish CI red, timeout, rebase, review,
stale definition, and hold refusals for scripts. See
[git lifecycle](docs/cli/04-git-lifecycle.md) for the table and override behavior.
<!-- trace:TASK-1606 | ai:codex -->


Seat occupancy is at the early C7 core checkpoint: process identity, permanent
seat record/lock, graceful transitions and seat commands are implemented, while
production gateway wiring requires fresh independent review. The C7 rework
bounds lock acquisition to ten seconds, shares policy with the main-worktree
state root, and defines a delegated child's review scope through persisted
spawn identity and direct review/implements edges. One-shot shell Route/Spawn
validates authority without claiming; harness/loop operations retain implicit
claims. Hard takeover (`--kill`) remains disabled scaffolding. The
[sketch C7 addendum](docs/plans/2026-10-03-story-1485-seat-occupancy-sketch.md#c7-inventory-refresh-and-accepted-core-refinements)
records the actual phase operations and unattended launcher dispositions.
<!-- trace:TASK-1607 | ai:codex -->

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

Claude Code, Codex (`PreToolUse`), and Antigravity (`PreInvocation` via
`--client agy`) hooks deliver informational seat-grant expiry notices once at
the 2h, 30m and expired crossings, without requiring a user prompt. A local
grant-id ledger uses a permanent flock sidecar and atomic replacement. No
requirements graph or cache is initialized; the shared authority validator
retains its local roster lookup. Failed ledger access emits no context (`{}` for
Antigravity) and leaves SessionStart/statusline warnings as fallback. See
[verification and limits](docs/internal/testing/bug-2014-grant-expiry-notices.md).
<!-- trace:BUG-2014 | ai:codex -->
<!-- trace:TASK-1744 | ai:antigravity -->

Existing history indexes also catch up through the inline `history-index-tick`
maintenance job (`cache refresh --history --if-behind`). It uses only the index
lock and a 30-second chunk budget, skips disabled/missing/current/busy indexes
and a checkout that is behind the shared index (or whose indexed tip is live on
another checkout), and preserves foreground stall behavior. A replaced root or
rewritten history exits non-zero with `cache rebuild --history` advice. The scaffold runs it every 15 minutes.
See [cache commands](docs/cli/10-storage.md).
<!-- trace:STORY-1661 | ai:codex -->


### Explicit host constraint verification

Constraint rows can carry an optional version-1 `verify` block containing a
closed built-in probe ID, scalar arguments and a typed expectation. The explicit
`aida verify constraints` command reports match, drift, unknown or invalid per
configured row, as human text or JSON; only `--fail-on-drift` turns drift into an
error exit. It never changes requirement status or executes stored commands.
Linux procfs/sysfs supply CPU, memory, PSI and disk facts; path and executable
presence use metadata/access checks. PATH lookup never launches the target.
No sync, drain or hook invokes probes. Unknown/malformed blocks remain loadable
so the report can label them invalid. `edit --verify` and `--clear-verify` use
the protected canonical edit flow; YAML row roundtrips preserve the data, while
archived writers refuse unsupported blocks. See the
[manual](docs/cli/08-reporting.md#aida-verify) for the exact schema and types.
<!-- trace:STORY-1673 | ai:codex -->

Contained `queue work --plan-only` selects Claude's `plan` permission mode
alongside the existing contained settings and `AIDA_CONTAINED=1`. This applies
to inherited, explicit sandbox and configured containment. Ordinary contained
pickups retain `dontAsk`; caller permission overrides under contained parents
and bypass/contained combinations still refuse. The documented interactive-only
plan lane rejects headless flags at parsing, before lifecycle setup; headless
planning remains the separate `--with-plan` drain phase.
<!-- trace:TASK-1794 | ai:codex -->

Standalone reviewer pickup coexists with a live implementer only with explicit
`queue work --role reviewer` and an unambiguous backing spec. An inferred role
and direct session starts retain ordinary source-branch conflict handling.
Verdict collection validates metadata and reads bytes from the same opened
regular-file handle, with `O_NOFOLLOW | O_NONBLOCK` and a bounded read. The FIFO
regression runs under an exact-test subprocess watchdog.
<!-- trace:TASK-1786 | ai:codex -->

Verdict reads use a held `review-verdicts` directory descriptor and `openat`
with `O_DIRECTORY|O_NOFOLLOW` for archive descent. File opens use
`O_NOFOLLOW|O_NONBLOCK`; metadata and bounded reads use the same handle and
reject `st_nlink != 1`. Every production verdict reader shares this path: record,
archive and adoption; the ship approval and corpus merge gates; the orchestrator
phase-3/4 handshake; drain, burndown, rework-release and status readers. A
present but refused current file contributes no evidence, archived rounds included.
Only the reads are anchored. Before `review record` (or a merge close) rewrites
the current file or a per-commit archive entry, it reads the existing one through
this path; a refused file fails the record before anything is written, so its
rounds are never replaced by a shorter history. The writes themselves still go by
path (`create_dir_all`, then rename) and do not refuse links on their own.
Archive enumeration reads the held directory descriptor with `fdopendir`, so it
does not depend on `/proc`.
Retained-history enumeration also pins the archive directory, including empty
archives; linked archives or linked/non-regular JSON entries refuse rework release.
The no-replace publication fallback briefly creates a second hard link before
unlinking the temporary name. Concurrent readers fail closed during that window;
a successful unlink restores ordinary reads.
Reviewer coexistence also verifies that the anchor resolves to a requirement
credited by the change's title trailer or branch; unavailable metadata refuses
the exception. Direct `session start --role reviewer` has no coexistence input
and keeps ordinary conflict handling (headless live-lease refusal, terminal
conflict prompt). See [sessions](docs/cli/06-roles-sessions.md).
<!-- trace:TASK-1801 | ai:codex -->
<!-- trace:TASK-1801 | ai:claude -->

Store writer follow-up (TASK-1981): store config and compaction history
rewrites retain `store-write.lock` across their read-modify-write window. GC
runs after that lock is released using Git’s own maintenance locks, so long
repacks do not exhaust concurrent writers’ 120-second wait. Explicit auto-GC uses normal Git configuration precedence and detach policy;
`AIDA_STORE_GC_AUTO=off` skips it. The sync path retains AIDA’s existing
default local threshold (500) and detach setting unless that override is off.
Legacy ID retirement reloads through `update_atomically`; doc/finding filing
no longer rewrites objects after releasing that transaction. Doctor heals
lock their read/write/commit window. Interactive repairs restrict mutations to
confirmed edges or blocks, while counter-scope migration refuses new active
unconfirmed blocks before changing policy. Whole-store saves already lock and
perform optimistic conflict detection; the inventory distinguishes that
protection from in-lock reloads. See
[store writer inventory](docs/plans/2026-10-19-store-writer-inventory.md).
<!-- trace:TASK-1981 | ai:codex -->

## Codex mail wake adapter

The opt-in host adapter in `scripts/codex-mail-wake.py` reads a bounded AIDA
`mailbox wake-snapshot` envelope (AIDA's logical mail view across every
worktree, with a `partial` flag), deduplicates message IDs in a private durable
ledger, and uses native `codex queue` to notify an explicit existing advisor
thread. Quiet scans use no model inference. Uncertain
queue results require explicit recovery; a queue receipt is not mail read/ack
evidence. See [installation and recovery](docs/advanced/codex-mail-wake.md).
<!-- trace:TASK-2347 | ai:codex -->

### Read-only store service channel

The finalized shared headless launcher accepts an explicit
`AIDA_STORE_MUTATION_REQUIRED=1` assignment on its host-built `Command`.
This is an opt-in transport slice: CLI/MCP writers do not route through it,
no mutation operation exists, and current raw-store/cache grants remain in
place. Context construction, previews and native launches create no service.
The launcher requires a nonempty validated child grant, an unambiguous owned
worktree lease and a resolved canonical spec. Project, worktree and store roots
remain pinned by directory handles; grant/parent lifetime, roster, lease and
worker binding are checked through a private host-only authorization channel.
Requests never select those facts or change the server's session environment.

`aida-cli-lib/src/store_mutation/` owns the read-only protocol, client and
launch authorization owner. The shared connect broker recognizes exactly
`/run/aida-mutation/<fresh-uuid>/service` and replaces only an AF_UNIX
SOCK_SEQPACKET socket belonging to this lane. There is no host pathname
listener or prefix grant. Its peer requires SO_PASSCRED and SO_PASSPIDFD;
SCM_CREDENTIALS plus the kernel-held sender pidfd prevent a transferred FD or
queued packet from borrowing the socketpair creator's identity or a recycled
PID. Pinned worker pidfd, process start and PID/user/mount namespace handles
refuse foreign or dead senders. Unsupported kernel facilities refuse the
required launch. Host-private descriptors do not cross vendor exec, the helper
is non-dumpable, and the lane has its own procfs.

Version 1 accepts only `session_probe {challenge}` and
`request_status {request_id}` in a closed JSON envelope with `version`,
`launch_epoch`, `request_id`, `op` and `data`. UUID spellings are canonical.
Unknown/duplicate fields, paths/authority selectors, mutation verbs, rights,
truncated packets, control characters and invalid shapes refuse. Bounds are
64 KiB per packet/response, depth 16, 32 keys per object, 16 KiB per string,
128 bytes per challenge, eight connections, five seconds of channel idle/I/O
admission and 4096 accepted IDs. Dispatch is serial. Dedupe binds epoch and
request ID to the canonical typed payload digest; changed payloads conflict,
capacity refuses without eviction, and live authority is rechecked before
replay and acceptance. Status `unknown` says nothing about a target operation's
success. The client retains one request ID across at most two retries. S1's
memory-only dedupe is safe for read-only probes; recoverable canonical mutation
transactions still require separate approval before S2 enables any writer.

The lane probes its actual channel before the receipt barrier; the host checks
that receipt's exact epoch/readiness and the existing authority owner's worker
stamp before releasing tools. Revocation, lease loss or authority-channel
failure closes the service and stops the helper's child. Normal exit and failed
setup close channels, then reap the helper; resume creates a fresh epoch.
The authorization reader has one thread per launch, never one per request.
External storage stalls return unavailable to the helper within its deadline.
An authority timeout permanently shuts down the shared endpoint, including the
Rust receipt-barrier owner's duplicate, so late replies cannot authorize a later
request or worker binding. Unreadable leases and leases with unresolved or
unrelated worktrees are ignored during selection; matching ownership must still
be unique and remains mandatory throughout the session.
Teardown shuts down its capability immediately and waits at most 200 ms for
the reader thread, which may finish later in a long-lived launcher.

`python3 -m unittest tests.test_store_mutation` covers protocol refusals,
credentials/rights/truncation, replay/capacity, broker address/type refusal and
lifecycle. Its independently watchdog-bounded live fixture runs the actual
bubblewrap/seccomp helper through preflight, receipt and post-exec probes with
a synthetic authority responder and keyring bypass confined to test code.
It is transport evidence, not real grant issuance or vendor conformance.
`AIDA_REQUIRE_STORE_MUTATION_LIVE=1` makes missing live prerequisites fail
instead of skip and is mandatory in the full-CI gates shard after namespace
setup. Real sibling user/mount namespaces test direct connect refusal,
SCM_RIGHTS-transferred client descriptors and procfs descriptor reopening.
Rust owner fixtures exercise prepare refusals and live project/worktree/store,
roster, lease, child/parent grant and worker-binding changes. Launch tests also
cover absent/empty bindings and receipt mismatches. Native launch labeling and
grant behavior remain unchanged.
<!-- trace:TASK-3225 | ai:codex -->

### Explicit vendor containment policy

When containment is requested, `[contained.vendors]` in the trusted default-branch
`.aida/config.toml` selects `"aida"` or `"native"` per vendor. Claude defaults to AIDA containment
and keeps its existing settings, sandbox and parent protections. Non-Claude
vendors without a key still refuse. Branch-local opt-ins are ignored; this
repository’s opt-in becomes trusted when merged into `origin/main`. Only Codex supports `"native"`: it receives
`--sandbox workspace-write --ask-for-approval never`, including headless children,
and is reported as **native sandbox, not AIDA-contained**. Trusted `"aida"` for
Codex selects the foreground headless boundary described below. `"native"` for
Claude, agy or unknown tools refuses. Native policy overrides
conflicting per-tool posture defaults and refuses posture passthrough flags;
an explicit operator launch-posture override retains its existing precedence.
A native child does not receive `AIDA_CONTAINED=1` or AIDA containment credit.
Native `aida ship -m '<complete subject> (Refs ID)'` records a lease/spec-bound
publication declaration for the host after exit. A fresh launch token prevents
reuse across attempts and survives supervised rotations within an attempt;
lease release removes the host binding. The declaration stays inside the seat's
existing write boundary. The host refuses invalid, epic, multi-spec or mismatched
lease scopes before commit or forge probes. Existing Refs PRs or inconclusive
forge evidence downgrade declared completion to Refs; failed exits always use
Refs. Punt/hold vetoes take precedence. See the
[native publication handshake](docs/cli/04-git-lifecycle.md#aida-ship).
<!-- trace:TASK-2902 | ai:codex -->
Permission reports, doctor, launch output and statusline distinguish the posture;
`AIDA_CONTAINMENT_POSTURE` stamps actual child posture for display only.

The contained permission preset (`aida config permissions set contained`) writes
`sandbox_workspace_write.writable_roots = []` for Codex. Reapplying it clears
legacy write grants to `~/.cargo`, `~/.rustup` and `~/.aida` in project or user
scope. Host toolchains, Cargo registry sources and global AIDA state receive no
extra write grant. Like the read-only CARGO_HOME in BUG-2418's bwrap lanes, new
dependencies must be fetched on the host: run `cargo fetch` outside the lane.
Already-running Codex sessions need to be restarted to load the changed config.
<!-- trace:TASK-b6b1278890774ef993d2ebc327862470-729 | ai:codex -->

This repository keeps `[agents] vendor = "codex"` and explicitly opts into its
native sandbox until vendor-neutral containment (STORY-2238). Its AIDA network
allowlist includes `api.openai.com` for API-key use and `chatgpt.com` plus
`auth.openai.com` for ChatGPT inference and authentication, verified by static
inspection of installed codex-cli 0.166.0. Native sandbox egress remains Codex's
own policy; the AIDA allowlist does not configure it.
<!-- trace:TASK-2631 | ai:codex -->

### Detached Codex lanes

`aida agent new codex --bg <SPEC>` runs a detached Codex implementer or reviewer
as three processes. The launcher runs every ordinary preflight, reserves the
scope in a private host receipt store (`~/.aida/launch-attempts/`, 0700/0600,
evidence only, never authority), prepares the lease and worktree, and has the
seat gate mint the worker's own child grant together with a one-use launch
capability whose digest is the only trace on the grant record. A setsid
supervisor (`aida internal codex-bg-supervise`) receives that capability once
over a private inherited socket, proves it is the issuer's child, starts a gated
exec barrier, and takes ownership of the attempt by a durable compare-update
before anything runs. It then binds the grant through the gate (only to its own
child with an exact start identity, once), stamps lease, authorship ledger and
registry strictly, releases the barrier (which `execve`s Codex at the same PID
under a supervisor-tied parent-death signal), corroborates the native executable,
and owns wait, reap and exact-generation cleanup. The launcher's rollback guards
check the same owner record before destroying anything. "Accepted" means the
worker started, never that its work completed; an uncertain outcome is reported
as UNKNOWN and never retried. The lane runs only in the trusted native posture,
gets explicit environment only, and reports mail wake as unsupported.
<!-- trace:STORY-2254 | ai:claude -->

The closed `config permissions probe --vendor <name>` tracer is implemented in
`aida-cli-lib/src/containment/`: shipped typed capabilities, a pinned trusted
policy snapshot, descriptor-pinned synthetic filesystem mounts and an owned
bounded supervisor. Its successful report credits only the fixed diagnostic;
it cannot launch arbitrary programs or issue an applied-containment receipt.
This diagnostic remains separate from ordinary launch receipts. See [the tracer contract](docs/cli/07-project-setup.md#model-free-containment-tracer).
<!-- trace:TASK-2705 | ai:codex -->

The common foreground wrapper also has a descriptor-driven private-runtime
mechanism for outer-only vendors: fresh writable runtime state, a pinned
read-only credential leaf, explicit trusted auth-host approval and read-only
Git/store/cache authority. AGY's `vendors.toml` row selects that mechanism, but
its incomplete refresh-host inventory and missing integrated all-tool evidence
keep admission closed. See [AGY adapter preparation](docs/internal/testing/containment-conformance.md#agy-shared-boundary-adapter-preparation)
for the remaining acceptance and model-free test limits.
<!-- trace:TASK-3048 | ai:codex -->

### Foreground headless Codex boundary

Trusted `[contained.vendors] codex = "aida"` routes fresh headless implementer
and reviewer phases through `launch/contained.rs`. `/usr/bin/python3 -I -S`
and `/usr/bin/bwrap` are required. The host helper owns an isolated child-only
session keyring, a private namespace tree, the CONNECT proxy, and the seccomp
connect broker. Rust clears the helper environment and supplies an exact list;
bubblewrap inherits it unchanged. Only `OPENAI_API_KEY` carries authentication.
An existing Codex `auth.json` refuses by name until the ChatGPT-auth design lands.
The launcher writes no credential files and forwards neither registry nor handle
launch tokens. Tool shells cannot bind their own registry/handle records.

Each launch has private Codex state. HOME retains the issuer's canonical AIDA
home, read-only, so core-filtered tools resolve the same grant records without
expanding the signed metadata whitelist. Explicit public Cargo/Rustup home and
target paths retain toolchain selection, with compiler wrapping disabled. No
shared compiler cache is writable. A generated directory-based native permission profile with networking enabled under the outer network namespace.
Only the launch-owned loopback relay and pinned private Unix endpoints accept
connects; all other internet connects refuse. The broker replaces the socket
with an already connected descriptor, so options set before connect are lost.
CONNECT authorizes hosts and ports, not tunneled content or downstream relays.
The same-uid host replay residual from ADR-66 remains.

The shim runs the requested-seat probe inside both layers before the host emits
a receipt with the kernel-attested shim PID, start identity, namespaces, identity
UID map, and pinned policy commit. Rust validates that receipt while exec is
blocked, finishes host PID binding, then releases the vendor. Configured posture
and receipt-shaped JSON confer no applied credit. The temporary `ClaudeNative`
arm preserves existing Claude behavior without the trusted opt-in. Under
`[contained.vendors] claude = "aida"`, foreground headless phases and
coordinators use the shared boundary. Coordinators retain quiet or streaming
output, bind their explicit child grant to the attested worker before release,
and keep the private state owner until exit. Interactive, background and
resume/fork launches still refuse under that opt-in; their migration and
OpenCode integration remain later slices. The authenticated operator run is
still required for acceptance.
<!-- trace:TASK-3219 | ai:codex -->

Under `codex = "aida"` every other Codex surface fails closed. This covers
interactive `aida agent new`, `--bg`, resume, coordinator, phase-driver
advisor/CI-fix, compete, dispatch and `aida do`. They all finalize through
`codex_tool_env::prepare_launch`, which re-reads the trusted policy for the
child's project. It refuses by name ("Codex AIDA posture requires the
receipt-owning headless launch boundary") whatever posture the surface stamped.
A test-time inventory pins every Codex spawn path.
<!-- trace:TASK-2708 | ai:claude -->

The shared send broker pins sockets and snapshots arguments, with bounded workers,
notification-aware blocking waits, socket send timeouts and queued admission.
<!-- trace:TASK-2708 | ai:codex -->

Expiry-notice state now uses `~/.aida/grant-notices/<grant-id>/notice.{json,lock}`
(or the configured AIDA home). A Codex lane gets only its own notice directory;
canonical session grants and revocation locks remain host-only. Host notice calls
remove legacy notice files once, without migrating them. A marker in the
grant-notices base then skips the session-grants scan on every later per-tool-call
hook. One duplicate notice after upgrade is accepted.
<!-- trace:TASK-2707 | ai:codex -->

## Trusted review publication between hosts

An operator-enrolled public-key roster in `aida-store` authorizes configured host
recorders to publish version-1 review receipts after ordinary local authority
validation. OpenSSH signs canonical JSON in the `aida-review-receipt` namespace;
private keys, role grants and human-review receipts stay on their original host.
Signed author-session evidence binds the PR head to its author sessions, allowing
consumers to check reviewer independence after a conversation resumes elsewhere.
The shared publication overrides a contradictory local verdict view, while exact
head checks and current-roster signature validation remain mandatory. Revocation
removes a roster entry and affects future credit without undoing merges.
Publications use node-qualified UUID identity and signed per-node supersession;
an independent concurrent refusal cannot be overwritten by another node's approval.
The bounded committed-blob readers refuse malformed or unverifiable evidence.
Distributed watchers synchronize publications before forge discovery, including
on hosts without a local verdict directory. The trusted `[review] admission_hub`
and `admission_ref` select the single GitHub authority; GitLab and bridges only
mirror. Signed Reserved → Running → Terminal transitions and pre-spawn
Reserved → Cancelled use explicit expected-tip CAS and confirmed unpredictable
tokens. Active older heads fence newer ones; uncertain pushes/processes retain
claims without time/PID expiry or failover. Local capacity/process receipts stay
in force. Admission state is sharded into bounded per-PR blobs; historical
closed PRs cannot exhaust a global table budget. The direct-human
`review nodes admission-reconcile` command requires stopped reviewers and an
inspected exact hub tip, signs a recovery anchor, and fences abandoned or revoked
owner tokens without deleting Git history. Local release is allowed only after
remote claims are inactive. Hub configuration ships disabled until operators
enroll and upgrade every watcher. Admission snapshots config, roster and verdict
evidence under short store locks around hub I/O, retrying changed evidence;
unrelated store commits do not block lifecycle transitions. Readers validate
anchor signatures and scope but rely on trusted writers to preserve chains;
they do not detect shard truncation or replay against Git ancestors.
Untrusted-node envelopes remain separate work. See
[second-node operation](docs/agents/second-review-node.md).
<!-- trace:STORY-2492 | ai:codex -->
<!-- trace:TASK-3196 | ai:codex -->

## Process identity and holder admission

<!-- trace:STORY-2537 trace:BUG-2458 | ai:codex -->

`aida_core::proc_identity` names a process by boot ID, PID namespace, numeric
PID and raw field-22 start ticks. Its judge separates Alive, Dead and
ForeignUnknown; wall time and consumer expiry do not participate. Direct
observation verifies procfs coordinates and rejects masked views. A local
machine identity in the holder envelope supplies node provenance independently
of hostname. This is lifecycle evidence, not a grant or signal capability.

Admitted ownership and lease records select one worker generation with
cooperative non-subreaper capture evidence. Spawn handoffs bind a pre-exec
self-check receipt to the unreaped child; self-stamping captures the writer.
Init, managers, subreapers and unavailable evidence refuse admission. Legacy
lease readers retain their advisory-lock, worktree and expiry policies, and
legacy ownership retains its heartbeat policy. An admitted worker never borrows
liveness from creator metadata or a heartbeat. Foreign lease expiry remains a
separate consumer policy. Ownership repair pins the recorded generation without
recapturing today's process at an old PID.

The TASK-1750 ratchet inventories compatibility consumers in
`proc_identity_legacy_baseline.json` and prevents new private PID/start
decisions. See the signed
[process identity sketch](docs/plans/2026-11-05-story-2537-proc-identity.md).
