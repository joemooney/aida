# Machinery glossary

AIDA's docs, error messages, and agent-to-agent handoffs lean on a small set
of machinery terms — **orchestrator, phase, drain, lease, role, session,
scope, worktree, sentinel, batch, autonomy mode**. Each has a precise
meaning; conflating them produces handoffs that almost-work and specs that
almost-reproduce. This page is the canonical definition for each.

Lifecycle verbs (committed / pushed / merged / completed / released) are
*not* in scope here — those live in
[`lifecycle-vocabulary.md`](lifecycle-vocabulary.md) and the two pages
cross-reference rather than duplicate.

## The terms

### orchestrator

The running `aida` process executing `aida queue work … --auto-complete`. A
Rust state machine (`orchestrate()` in `auto_complete.rs` / `orchestrator.rs`)
that drives the six-phase pipeline (implementer → CI → reviewer → merge →
pull+auto-bump → build) for a single spec.

- **NOT** every `aida` invocation. `aida list`, `aida show`, `aida queue
  work` *without* `--auto-complete` are plain CLI calls, not orchestrators.
- Each `--auto-complete` invocation is **one orchestrator instance**, with
  a run-UUID for telemetry and lease attribution.
- An orchestrator owns a *single* spec at a time. Draining a batch with
  `--auto-complete` launches **one orchestrator per spec, sequentially** —
  not one orchestrator across many specs.

### phase

One of the orchestrator's six pipeline stages: **(1) implementer**, **(2)
CI**, **(3) reviewer**, **(4) merge**, **(5) pull + auto-bump**, **(6)
build**.

- Phases 1 and 3 **spawn Claude Code sessions** (interactive or headless).
- Phases 2, 4, 5, 6 run **deterministic commands** (`gh pr checks`,
  `gh pr merge`, `git pull`, `cargo build`) — no model.
- A phase *failure* halts the orchestrator at that phase, leaving the queue
  intact for retry. The previous phase's artifacts (commits, PR) remain.

### --auto-complete

The flag that turns `aida queue work` from a single-session launcher into a
full **orchestrator** run. Without `--auto-complete`, `aida queue work`
launches phase 1 only and exits; with it, phases 1–6 run in sequence.

Variants compose: `--auto-complete=through-ci` stops after phase 2;
`--auto-complete=through-merge` stops after phase 4.

### lifecycle short-circuit tags

`lifecycle:*` tags are per-spec switches for deliberately skipping expensive
non-integrity phases during `aida queue work --auto-complete`:

- `lifecycle:no-ci-wait` — do not wait for CI to become terminal after the PR
  opens. CI still runs on GitHub; the orchestrator just continues while it is
  in progress.
- `lifecycle:no-review` — skip the phase-3 reviewer model session.
- `lifecycle:no-build` — skip the final local build verification.
- `lifecycle:trivial` — shorthand for all three skips.

Merge and pull/auto-bump never short-circuit. These tags are for small,
low-blast-radius specs where the saved latency is worth the reduced
redundancy; they are not a substitute for review discipline on risky changes.

### drain

Working a queue (or a [batch](#batch)) of specs through to completion,
one after another. Example: *"drain the implementer queue"* means run an
orchestrator per queued implementer spec until the queue is empty.

A drain is a *workflow pattern*; the orchestrator is the *machine* that
implements one iteration of it.

### batch

A set of specs sharing a `batch:NAME` tag (set via `aida edit <id> --tags
batch:NAME`), drained as a unit. `aida queue work --batch NAME` picks the
head queued member of that batch; `--batch NAME --auto-complete` drains
the whole batch sequentially.

A batch is a *tagging convention*; the queue and orchestrator already
understand it via the tag filter.

### autonomy mode

One of three modes governing **what the orchestrator's Claude Code sessions
do when they would normally prompt a human**:

- **default** — drive every step. The human is at the keyboard; every
  `AskUserQuestion` renders interactively.
- **`--zen`** — advisor on standby. Mechanical prompts (`kind:confirmation`)
  auto-resolve to their recommended option; design-fork prompts
  (`kind:design-fork`) still pause for the human.
- **`--no-human`** — absent. All prompts auto-resolve; design-forks that
  can't be safely resolved get **punted** (routed to the advisor tier or
  parked in Needs Attention for triage).

Two orthogonal axes: *is a human present?* and *what kinds of prompts
warrant pausing?*. See `feedback_three_mode_autonomy_taxonomy` in the
starter memory pack for the full taxonomy.

### session

An AIDA-tracked Claude Code session: a **lease** + **worktree** + **role**,
tied to a **scope**. Created by `aida queue work` / `aida session start`,
ended by `aida session end`.

A session is one *unit of ownership* — exactly one role working on exactly
one scope. The orchestrator spawns sessions; sessions don't spawn each
other.

### lease

The on-disk ownership record for a [scope](#scope) — `.aida/sessions/<id>.toml`.
A lease prevents two sessions claiming the same scope concurrently:
`aida session start` refuses if a live lease exists.

A lease can go *stale* (its owning process died without `aida session end`).
`aida session leases` surfaces stale ones; the recovery is explicit
release, not silent reclaim.

### scope

What a session owns: a **SPEC**, an **EPIC**, or a **PR**. The unit of
exclusivity for [leases](#lease).

- `SPEC` scope (most common) — implementer working a single spec.
- `EPIC` scope — work spanning multiple child specs.
- `PR` scope — reviewer working a single PR.

A session has exactly one scope; a scope has at most one live lease.

### role

A workflow position: **implementer**, **reviewer**, or **dialog**
(user-facing identity: *advisor*). The role decides queue routing
(`aida queue add <id> --for <role>`), worktree naming, and which skill
templates a session loads.

A role is **not** a Claude Code subagent — see
[`vs-claude-code-subagents.md`](../positioning/vs-claude-code-subagents.md) for
the within-conversation (subagent) vs cross-conversation (AIDA role) layer
distinction.

### worktree

The per-session git worktree a session works in — typically
`../aida-<scope>` (a sibling directory to the main checkout). Created by
`aida session start`, removed by `aida session end`.

Multiple worktrees mean multiple sessions can run *in parallel without
fighting over the working tree*. The trade-off is a `target/.fingerprint/`
cache that references absolute paths — see CLAUDE.md's "Cross-worktree
cargo cache gotcha" for the recovery recipe.

### sentinel

A signal file the orchestrator and a session use to coordinate graceful
exit: the skill (inside the Claude Code session) **touches** the sentinel
when its work is finished, and the orchestrator **reaps** the session when
it sees the touch.

Without a sentinel the orchestrator can't tell *"session paused for human
input"* from *"session finished its work cleanly"* — both look like an
idle Claude Code process. The sentinel is the explicit "I'm done" signal.

### complexity (tag convention)

`complexity:low | complexity:med | complexity:high` — a spec tag the
operator (or implementer) sets at pickup time and the reviewer may
override based on the diff. Surfaces in `aida queue list --tag-prefix
complexity:` and feeds the three-way calibration view
(`aida autonomy calibration mismatches`).

**Best-effort, not graded.** The point of capturing this is *substrate
self-knowledge* — when pickup-predicted complexity consistently
diverges from reviewer-assessed complexity, that gap names a class of
work the agents systematically misjudge (a memory candidate). The tag
is never an approval criterion and never blocks a pickup or a merge.

Set at pickup with `aida queue work <SPEC> --complexity {low|med|high}`;
at ship time with `aida pr ship --complexity {low|med|high}`; at
review time via `implementation_complexity` in the
`.aida/review-verdicts/PR-N.json` file the `/aida-review` skill writes.
Each capture point also writes a per-spec record to
`.aida/complexity-calibration/<SPEC>.yaml`.

### estimated-assistance (tag convention)

`estimated-assistance:none | estimated-assistance:advisor |
estimated-assistance:human` — pickup-time prediction of how much help
the spec is expected to need. Companion to `complexity:` and same
"best-effort, not graded" framing.

The *actual* intervention count comes from the punt ledger
(`.aida/punts.jsonl`); this tag captures only the prediction. The gap
between predicted and actual is the same kind of substrate signal the
complexity calibration surfaces — both feed the maturity trend.

Set at pickup with `aida queue work <SPEC> --assist-est
{none|advisor|human}`.

### effort buckets (tag convention)

`effort:<touchpoint>:<bucket>` — quantitative effort estimates captured
at four lifecycle touchpoints: `open`, `plan`, `impl`, and `review`.
Buckets are `15m`, `1h`, `4h`, `1d`, and `1w`.

Conversion convention: `1d` means **8 work-hours**, not a 24-hour
calendar day; `1w` means **5 work-days / 40 work-hours**, not seven
calendar days. `aida load` uses those conversions when summing queue and
backlog load.

Set at open time with `aida add --effort <bucket>`, at pickup/plan time
with `aida queue work <SPEC> --effort <bucket>`, at ship time with
`aida pr ship --effort <bucket>`, and at review time via
`implementation_effort` in `.aida/review-verdicts/PR-N.json`.
Each capture point also writes a per-spec record to
`.aida/effort-calibration/<SPEC>.yaml`.

### archived

A view-level flag on a spec, orthogonal to its lifecycle **status**.
Archived specs are hidden from default `aida list` / `aida history` /
`aida search` views, but stay reachable via `--archived` (archive-only)
and `--all` (everything-escape-hatch), and they still appear in graph
traversals (parent/child/relationship walks). **Archive is not deletion:**
the YAML stays on disk, the audit trail is intact, the graph stays
coherent. Three triggers:

- **manual:** `aida archive <SPEC-ID>` / `aida unarchive <SPEC-ID>` flip
  the flag explicitly.
- **on-demand sweep:** `aida archive --older-than 30d --status
  completed,rejected --dry-run` previews; drop `--dry-run` to apply.
- **auto-sweep:** with `[archive] auto_after_days = N` in
  `.aida/config.toml`, `aida pull` archives every Completed/Rejected spec
  whose `modified_at` is older than N days (clamped to ≥7).
  `AIDA_AUTO_ARCHIVE=0` disables.

Distinguishes "still active context, even if Completed" from "shelved,
not part of today's view." Why this matters: a freshly Completed ship
stays visible in the default view (so "did my ship register?" works
without flags); a Completed spec from six months ago doesn't drown out
current activity.

## Adjacent terms (defined elsewhere)

These show up in the same sentences but live in other pages:

- **committed / pushed / PR-opened / reviewed / merged / completed /
  released** — lifecycle verbs. See
  [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md).
- **Needs Attention / punt** — off-mainline status. See
  [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md#the-off-mainline-state-needs-attention).
- **finding** — a captured observation awaiting triage (`aida findings`).
  Friction surfaced during a session, not the same as a *spec*.
- **trace comment** — `// trace:SPEC-ID | ai:claude` linking code to a
  spec. See CLAUDE.md and `.claude/AIDA.md` for the format.

## How to add a term

This glossary grows as new machinery appears. When you add or rename a
machinery concept:

1. **Add the term here**, not inline in every doc that references it.
   Definitions belong in one place; everything else cross-references.
2. **Pick the right scope.** Machinery terms (this page) describe how
   AIDA *runs*. Lifecycle verbs (`lifecycle-vocabulary.md`) describe how
   a spec *progresses*. If a new term blurs the line, prefer this page
   and cross-reference from the other.
3. **Be precise enough to disambiguate.** A definition that doesn't
   exclude something is too loose — see "orchestrator" excluding plain
   `aida` invocations as the template.
4. **Cross-reference, don't duplicate.** When a term touches another's
   territory, link to it (`[term](#term)` or
   `[lifecycle-vocabulary.md](lifecycle-vocabulary.md)`).
5. **Keep it terse.** One paragraph plus optional bullets. Long-form
   discussion belongs in a dedicated doc, not the glossary.

The master template is `aida-core/templates/docs/aida/discipline/machinery-glossary.md`
(embedded via `build.rs`, scaffolded by `aida init`). Edit the master, not
a project-local copy — the latter is yours to tailor after init.
