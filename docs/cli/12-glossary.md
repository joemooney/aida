# Glossary

<!-- GENERATED FILE — do not edit by hand.
     Source: `aida docs glossary` (embedded product + machinery + lifecycle vocabulary).
     Regenerate: `bash docs/cli/generate-glossary.sh` (or `make book-glossary`).
     Edit the term definitions in
     aida-core/templates/.aida/discipline/{product-glossary,machinery-glossary,lifecycle-vocabulary}.md,
     rebuild the binary, then re-run. -->

> The shared vocabulary AIDA's docs, error messages, and agent-to-agent
> handoffs lean on — the plain-language **product** terms (merge hold,
> mandate, grant, hub, …), the **machinery** terms (orchestrator, phase, drain,
> lease, role, session, scope, worktree, sentinel, batch, autonomy mode)
> and the **lifecycle** verbs (committed / pushed / merged / completed /
> released). This page is generated from `aida docs glossary`, so it can
> never drift from the definitions the binary ships.

## Product glossary

<!-- trace:`<spec-id>` | ai:claude -->

Plain-language definitions of the words AIDA uses in its guides, commands and
messages. Each entry says what you see or do, and links to the page that
explains it in depth where one exists. Developer-facing machinery terms
(orchestrator, drain, lease, worktree and others) are defined in the
[machinery glossary](machinery-glossary.md), and lifecycle states (committed,
pushed, merged, completed, released) in the
[lifecycle vocabulary](lifecycle-vocabulary.md).

## Terms

### Agreed ID

The short ID a spec keeps for good: its type and a number, such as
`TASK-<number>`. It is unique across every clone of the project, because each
number is handed out once from a shared range. See [Support for air-gapped development](../air-gapped-development.md).

### Block

A small range of spec numbers set aside for one clone. The clone gives new
specs short agreed IDs from its own blocks, so it never has to ask anyone
else first. Blocks are carved from a [pool](#pool). See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Brief

A short note that assigns a piece of work to a named agent, such as "pick up
this spec next". The agent reads it, does the work, and acknowledges it so it
stops showing up. A brief goes one way; for a conversation, use the
[mailbox](#mailbox). See [`aida brief`](../cli/06-roles-sessions.md#aida-brief).

### Child grant

A [grant](#grant) that a launcher gives an agent it starts, for one named
seat. Your session can hand out only the seats you listed when you entered
your own seat, and a child grant cannot be passed on again. See
[Entering seats and delegating child seats](../cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Commit trailer

The spec ID in parentheses at the end of a commit subject, as in
`fix(api): handle missing ids (SPEC-ID)`. When that commit lands on the main
branch, AIDA marks the named spec completed. See the
[commit trailer convention](../commit-trailer-convention.md).

### Control plane

The part of AIDA that decides whether work may move forward and reports what
is in flight: the queue, seats, leases, reviews, merge holds and the status
views. The spec records themselves are the data it acts on, not part of it.

### Deputy

A helper you start in your own terminal with `aida deputy`. Agents hand it
commands to run under your seat grant, and it runs only what you allowed,
records each job and tells the submitting agent the result. See
[`aida deputy`](../cli/06-roles-sessions.md#aida-deputy).

### Grant

The authority your terminal session holds to act in a [seat](#seat). You get
one by running `aida role enter <seat>` at an interactive terminal; it expires
after a set time (24 hours by default) and works only for processes started
from that terminal. See
[Entering seats and delegating child seats](../cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Handoff

A note a session writes before it ends so the next session can carry on
without the conversation that came before: what was done, what is open and
what to do next. AIDA keeps handoff notes in the project's `.aida/` folder.

### Hub

A Git server that the requirement store syncs through, such as GitHub, GitLab
or a repository on an internal host. A mirror that only copies another hub
counts as the same hub. See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Local ID

A longer spec ID that also names the clone that made it, used when that clone
has no [block](#block) to draw short numbers from. Local IDs never collide, so
they need no coordination, and a spec can later be given an
[agreed ID](#agreed-id). You can give the clone a friendly label with
`aida node alias`. See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Mailbox

AIDA's message channel between agents and people, for things that are not a
change of state: a heads-up, a question, a warning about a shared file.
Messages are kept, so a different agent can read them the next day. See
[the inter-agent mailbox](../mailbox.md).

### Mandate

Time-limited permission, started by a person at a terminal with
`aida mandate start`, for the advisor seat to do named things while that
person is away, such as shipping held pull requests. It lasts 8 hours by
default and at most 24, and never covers [operator-only](#operator-only)
specs. See [`aida mandate`](../cli/04-git-lifecycle.md#aida-mandate).

### Merge hold

A stop placed on a pull request so it cannot merge, even when its checks and
review pass. It shows as the `aida:merge-hold` label and a failing required
check. It stays until a person clears it with `aida merge-hold clear`, or
AIDA releases it in the narrow cases it allows, such as under a
[mandate](#mandate). See
[What is AIDA](../what-is-aida.md).

### Operator-only

A tag that keeps a spec in human hands. No [mandate](#mandate) covers it, so
its pull request waits for a person to merge, and the tag's effect remains even
if the tag is later removed. See
[`aida mandate`](../cli/04-git-lifecycle.md#aida-mandate).

### Pool

A range of one spec type's numbers given to one hub. Clones that sync through
that hub take their [blocks](#block) from it, so two hubs never hand out the
same number. See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Ref

Git's name for a pointer to a commit, such as a branch. AIDA keeps its records
on the `aida-store` branch, called the store ref; a push to it succeeds only
if nobody else moved it first. See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Review story

The story AIDA files when it opens a pull request, titled
`Review PR-<number>: …` and routed to the reviewer seat. Whoever picks it up
reviews that pull request and records a verdict. See
[the review process](../review-process.md).

### Rework round

One trip back to the implementer after a reviewer asks for changes.
`aida rework` returns the spec to active work with the reviewer's reasons, and
the implementer pushes fixes to the same pull request for another review. See
[`aida rework`](../cli/04-git-lifecycle.md#aida-rework).

### Role

A named job in AIDA's workflow, such as advisor, implementer, reviewer or
integrator. Work can be routed to a role, and the role decides what a session
wearing it is expected and allowed to do. See
[Roles and sessions](../cli/06-roles-sessions.md).

### Root

The one clone of a project that owns every spec number not yet handed out.
You choose it once with `aida node enroll-root`; it splits the numbers into
[pools](#pool). See
[Support for air-gapped development](../air-gapped-development.md#the-parts).

### Roster

The list of seats each person is allowed to take, kept per project. It is a
ceiling: `aida role enter` refuses a seat that is not on your roster, and
`aida team allow-seat` adds one. See
[Entering seats and delegating child seats](../cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Seat

A [role](#role) as a place in the workflow that a session occupies, such as
the advisor seat. A seat can receive work and hold sessions, and some seats
allow only one active occupant at a time. See
[Roles and sessions](../cli/06-roles-sessions.md).

### Supervised spec

A sensitive spec, such as one touching security, containment, review or merge
authority, or one marked guided or keystone. A [mandate](#mandate) leaves its
[merge hold](#merge-hold) for a person unless that person started the
mandate with `--include-supervised`. See
[`aida mandate`](../cli/04-git-lifecycle.md#aida-mandate).

## How to add a term

This file is the canonical source for AIDA's product vocabulary. It is the
template master `aida-core/templates/.aida/discipline/product-glossary.md`.
Two outputs are generated from it: the CLI glossary (`aida docs glossary`,
published as the book's Glossary chapter by `make book-glossary`) and the
website glossary page (`python3 docs/showcase/build.py`). The terms that
earlier lived as `type=term` specs (local ID, role, seat, control plane) have
been folded in here; add new product vocabulary here, not as a term spec.

Each entry follows the same rules. (The website renderer reads one list item per line.)

- **Heading.** Add a `###` heading under Terms, in alphabetical order. The heading becomes the anchor: `### Merge hold` is `glossary.html#merge-hold`. Do not rename a heading once pages link to it.
- **Wording.** Write 1 to 3 plain sentences about what the reader sees or does, not how it is built. Do not cite spec IDs.
- **Links.** Link the page that explains the term in depth, relative to this file (`../../../../docs/<page>.md`), and other terms as `#anchor`.
- **Outputs.** Copy the master to `.aida/discipline/` (`make sync-templates`), rebuild the binary, run `make book-glossary` and `python3 docs/showcase/build.py`, and commit the regenerated files.

## Machinery glossary

AIDA's docs, error messages, and agent-to-agent handoffs lean on a small set
of machinery terms — **orchestrator, phase, drain, lease, role, seat,
stakeholder persona, session,
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
- `lifecycle:no-review` — skip the phase-3 reviewer model session. The merge
  still requires an independent approval at head; no tag waives it.
- `lifecycle:no-build` — skip the final local build verification.
- `lifecycle:no-preflight` — skip the local CI-parity guards before the
  implementer branch is published. Use only when the change is genuinely
  unrelated to those guards; the drain banner names this opt-out.
- `lifecycle:trivial` — shorthand for the CI-wait, review, and build skips
  (it does not skip preflight).

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

A set of specs sharing a `batch:NAME` tag (set via `aida edit <id> --add-tag
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

### autopilot (autopilot envelope)

A **grooming-stage posture**, *not* a drain mode. Where the three
[autonomy modes](#autonomy-mode) parameterise the **drain** (how the
implement→merge pipeline handles prompts while a human is or isn't present),
autopilot parameterises the **advisor disposition pass** (`aida groom`) — how
the advisor decides a spec's fate before it ever enters the ready set. The two
live on **orthogonal axes**: grooming decides *what enters the ready set*,
draining *works it*. Autopilot is therefore **not a fourth rung** on the autonomy
ladder; it is the grooming-stage analog of the draining-stage ladder, and the two
**compose** (a `groom --autopilot --then-drain` runs each stage under its own
setting) rather than contend.

Mechanically, autopilot is a **bounded-authority envelope** over `groom` (opt-in
`aida groom --autopilot` + a `[autopilot]` config posture) — *not* a new engine.
It layers a per-action authority map (`auto` / `propose` / `never`) and four
AND-composed gates (in-fence, action-authority, substrate-grounded, under
risk-ceiling) *inside* the existing candidate fence. The default is conservative:
only reversible actions auto-execute; approvals/rejections are held; uncertain
calls escalate. It reuses the **same keystone classifier**
(`presence::is_keystone_class`) as the groom fence, the solo posture, and the
drain — so all stages agree on "keystone" and no stage can disagree. See
`docs/architecture/autonomy-and-escalation.md` §8 (the grooming-stage analog of
the cascade) and `docs/solo-mode.md`.

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

A persistent workflow position such as **advisor**, **implementer**,
**reviewer**, **integrator**, or **product**. A role is stored in a role file,
can receive routed work (`aida queue add <id> --for <role>`), and supplies the
context and authority used by its sessions.

A role is **not** a Claude Code subagent — see
[`vs-claude-code-subagents.md`](../archive/positioning/vs-claude-code-subagents.md) for
the within-conversation (subagent) vs cross-conversation (AIDA role) layer
distinction.

### seat

The place a [role](#role) occupies in AIDA's build loop. A seat can receive
work and own leased sessions/worktrees; driver seats are subject to the
one-authoritative-driver rule. “Role” names the workflow position; “seat”
emphasizes its operational place and cardinality.

### stakeholder persona

A fileless, non-routable least-privilege gate activated with
`AIDA_SESSION_ROLE=guest|requester`: `guest` is read-only and `requester` adds
Draft-only intake. A persona is discoverable in `aida role list` but has no
seat, history, or lease. See
`docs/architecture/roles-seats-and-personas.md` in the AIDA repository.

### standalone implementer

An implementer launched by an orchestrator, relay or advisor session as a
separate process, never an in-session subagent:
`aida queue work <SPEC> --role implementer --no-human`, run from a shell
outside any [`--auto-complete`](#--auto-complete) drain. The child gets its
own [lease](#lease), [worktree](#worktree) and implementer [seat](#seat) (a
delegated child grant, or baseline authority; never the launcher's grant). It
opens its PR and stops: `aida pr ship` and `aida review record` refuse inside
it. The launcher waits and ends with one `implementer:` result line and an
exit code the caller branches on: 0 PR opened, 31 no PR, 32 punted, 33 held,
34 refused at the concurrency cap (`[queue] standalone_implementer_cap`,
default 3). See `docs/cli/03-work-autonomy.md`.

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

### NeedsAttention

An off-mainline stored spec status (distinct from the primary lifecycle
states: Draft → Approved → Planned → InProgress → Done → Completed /
Rejected). The display layer splits this status by its recorded reason:

- **Shelved** — mechanically parked with a `FailureReason` such as stale
  base, CI red, reviewer tool exit, or merge/pull failure. The retry or
  rework path is recorded; no decision is implied.
- **Needs Decision** — parked with an `AttentionReason` from a punt,
  advisor escalation, or other design/human fork. A person or advisor must
  decide before it moves.

This keeps the storage value stable (`NeedsAttention`) while making lists,
cards, `why`, and awaiting surfaces distinguish "known recovery path" from
"human decision needed". The drain *continues* past a parked spec (the
dependent specs skip via the pickability gate). Triage:

- `aida findings list` surfaces parked punts and shelved failures with their
  recorded reason.
- `aida queue work <SPEC> --resume` retries from where the prior
  phase failed.
- `aida edit <SPEC> --status approved` resets to Approved (clears
  the parking); use when the original failure no longer applies.
- `aida edit <SPEC> --status rejected` if the spec is no longer
  worth pursuing.

NeedsAttention is **not** the same as `human_only` (a typed boolean
field marking specs no agent should ever pick). NeedsAttention is a
transient parking state; `human_only` is a permanent classification.


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
review time via `implementation_complexity` in the PR's verdict record the
`/aida-review` skill records (read it with `aida review verdict PR-N --json`).
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
`implementation_effort` in the PR's verdict record
(`aida review verdict PR-N --json`).
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

### history (per-spec transition array)

Every `objects/TYPE/000/SPEC-ID.yaml` carries a `history:` array of
`HistoryEntry` records — one entry per substrate-modifying edit. Each
record:

- `id` — UUID of the entry
- `author` — who made the change
- `timestamp` — UTC instant of the change
- `changes:` — list of `{field_name, old_value, new_value}` triples

Every status flip (Approved → In Progress → Done → Completed), priority
change, tag edit, owner reassignment, etc. lands here as a structured
row. **This is the source-of-truth for spec-state time series.**

Why this matters for agent work:

- A reviewer building a burn-down chart, status-flow histogram, or
  Lead Time / Cycle Time analysis must walk this array — not
  approximate from `modified_at` (which only reflects the latest edit).
- The cache (`.aida/cache.db`) is a derived read-projection and does
  NOT currently expose history rows. For substrate-grounded time
  series, read the YAML directly or walk the orphan-branch git log
  (which gives both per-spec history AND inter-spec ordering).
- A spec that lost its history file (corrupted, manually edited away)
  is silently un-auditable. `aida doctor` does not currently flag
  empty-history specs — known gap.

Cross-references: `aida history <ID>` (shorthand for `aida history --id
<ID>`) filters to one spec's entries and shows its status progression by
default, one row per transition in human, TOON and JSON output alike; add
`--full` (or use `aida history events`) for the complete edit/comment
trail. The substrate-grounded equivalent in code:
`aida-core::object_store` walks the YAML files directly.

### doctor exit-code contract

`aida doctor` and `aida doctor check <category>` are **report-only by
design**: exit code `0` means "the scan ran successfully," NOT "the scan
found nothing." A category with thousands of findings and a category with
zero findings both exit `0` — the findings print (text or `--json`)
either way, but the process exit code does not encode them.

This is a deliberate default, ruled by ADR-style precedent (`<spec-id>`,
consistent with `<spec-id>`): a shared diagnostic surface should not
silently change its exit-code contract underneath every existing caller
just because one new caller wants a gate.

A caller that needs pass/fail — a CI step, a scheduled job, a substrate
gate — opts in explicitly:

```bash
aida doctor check <category> --fail-on-findings
```

This exits non-zero **only** when that one category has at least one
finding; every other `aida doctor` / `aida doctor check` invocation stays
reporting-only. Don't infer health from a bare `aida doctor`'s exit code —
parse its output (`--json`) or add `--fail-on-findings` to the specific
`check` you want to gate on.

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

The master template is `aida-core/templates/.aida/discipline/machinery-glossary.md`
(embedded via `build.rs`, scaffolded by `aida init`). Edit the master, not
a project-local copy — the latter is yours to tailor after init.

## Lifecycle vocabulary

A spec moves through many states between "someone started typing code" and
"users have it." Fuzzy verbs — especially **"ship"** — collapse those states
together and make it impossible to tell, from a sentence, whether a spec is
in a PR awaiting review, merged on the main branch, or released with a
version tag. Use the precise verb for the state.

## The lifecycle

```
   in-progress          done                    completed        released
   ───────────►  ─────────────────────►  ──────────────►  ─────────────►
   committed     pushed   PR-opened  reviewed   merged    auto-bumped   tagged
   (local git)   (origin) (gh PR)    (verdict)  (main)    (Completed)   (binaries)
```

## The states and their verbs

| Verb | What it means | AIDA spec status |
|------|---------------|------------------|
| **Committed** | Work exists in local git history | still In Progress |
| **Pushed** | The branch is reflected on `origin` | still In Progress |
| **PR opened** | A PR exists, awaiting CI / review | Done |
| **Reviewed** | A reviewer rendered a verdict (approved / rejected) | still Done — waiting for merge |
| **Merged** | The PR landed on the main branch | should auto-bump to Completed |
| **Completed** | The spec's status is Completed in AIDA | final state |
| **Released** | A version tag exists and binaries are published | cross-spec — aggregates many merges |

## The off-mainline state: Needs Attention

Not every spec travels the line cleanly. **Needs Attention** is an
*off-mainline* status: a spec that was **In Progress** but is now **paused**
because an autonomous agent hit a design-fork it could not safely resolve and
**punted** (`aida punt` / `/aida-punt`) rather than guess.

- Reached only from **In Progress** — punting is the "I was working this and
  hit a fork" move.
- Carries a structured reason: an obstacle **category** (`design-fork`,
  `ambiguous-spec`, `missing-context`, `blocked-dependency`, `other`), a
  human-readable **detail**, and an optional **lean** (the agent's
  best-guess-if-forced, kept separate from the fork itself).
- Excluded from normal queue pickup, but surfaces in `aida findings` for
  human / advisor triage — a Needs Attention spec *is* a punt awaiting triage.
- Resolved out only to **Approved**, **In Progress**, or **Rejected**.

Punting is a discipline, not a failure: a punted fork is a recorded decision
point. Guessing past it produces a silent wrong implementation; punting keeps
the wrong call from being made at all.

## The two terminal off-ramps: rejected vs superseded

Not every spec reaches Completed, and the two exits mean opposite things.
Say the precise one.

- **Rejected** — **declined.** We looked at it and said no. Nothing was
  adopted. `aida edit <SPEC> --status rejected`.
- **Superseded** — **adopted, then replaced.** This spec governed, and a later
  spec now governs instead. `aida edit <SPEC> --status superseded
  --superseded-by <NEW-SPEC>` — naming the successor is enough (the flag
  implies the status), and it records a **typed relationship**, not a tag, so
  `aida graph` can walk "what replaced this?" in both directions.

The archetype is an **ADR** whose documented lifecycle is *proposed → accepted
→ superseded*. Without a superseded state, a decision that was followed for
months and then replaced had to masquerade as *rejected* — reading, in any
listing, exactly like one that was turned down.

Both are terminal: excluded from the default `aida list` view, from the open
tally, and from the ready set. Superseded renders `⊡ Superseded` (dimmed
closed-green — it *was* adopted), never the red `✗ Rejected`.

## The view axis: archive / unarchive / archive-sweep

**Archive** is *orthogonal* to the lifecycle above — every state can be
archived or non-archived, and archive doesn't change the status. It's a
view-level flag for "no longer current context."

- **archive** (verb) — `aida archive <SPEC-ID>` sets `archived = true` and
  stamps `archived_at` with the current wall clock. The spec drops out of
  default `aida list` / `aida history` / `aida search`. It still appears
  in graph traversals (parent/child) and in `--all` views.
- **unarchive** (verb) — `aida unarchive <SPEC-ID>` clears the flag.
- **archive-sweep** (verb / phase) — `aida archive --older-than 30d
  --status completed,rejected` archives in bulk. Pair with `--dry-run` to
  preview. The same sweep runs automatically on `aida pull` when
  `[archive] auto_after_days = N` is configured (clamped to ≥7 days so a
  ship is never archived before the human sees it).

Why this exists: status `Completed` is the lifecycle end-state; archive is
the "is this still on my mind?" axis. A freshly-Completed spec stays
visible in the default view (so the "did my ship register?" sanity check
works without flags); a year-old Completed spec is archived and doesn't
drown out current activity. **Archive ≠ deletion** — the YAML, the
commit history, and the requirement graph all survive.

## How to apply

- Default **"ship"** to mean **merged to the main branch** — the
  developer-facing "out the door."
- For earlier states, use the precise verb: "PR opened for `<spec-id>`", "`<spec-id>`
  is reviewed, waiting on merge", "`<spec-id>`'s PR merged".
- For "out to users with a version number," say **released** — distinct from
  merged, because a merge does not auto-release.
- `lifecycle:*` tags change which orchestrator phases run; they do not change
  the spec-state vocabulary. A spec still becomes **Done** when the PR opens
  and **Completed** when a referenced commit lands on main. See
  [`machinery-glossary.md`](machinery-glossary.md#lifecycle-short-circuit-tags)
  for the phase-skip contract.

Better phrasing:

- ~~"`<spec-id>` shipped"~~ → "`<spec-id>`'s PR merged" / "`<spec-id>` is on main"
- ~~"shipped to a PR"~~ → "PR opened for `<spec-id>`"
- ~~"v1.2 shipped"~~ → "v1.2 was released" (binaries published)

## Why precision matters

Across a long conversation many specs sit at many different states. Precise
verbs let the user track which spec is where in the pipeline — exactly the
workflow-state awareness AIDA is meant to provide. `done` vs `completed` is
the load-bearing distinction: `done` means "work finished on a branch";
`completed` means "merged to the main branch." AIDA auto-bumps
`done → completed` when a commit referencing the spec lands on the default
branch, so you rarely set `completed` by hand — let the merge promote it.
