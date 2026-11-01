# What is AIDA?

AIDA is a governed software factory with selectable autonomy: a coordination and enforcement layer for a software team made of AI agents, with explicit choices about where people stay involved.

Begin with your usual conversational coding agent. AIDA-aware agents can record agreed intent alongside implementation, leaving the system you developed and a durable record for maintenance, enhancement and collaboration. See the [getting-started guide](getting-started.md); autonomous delivery is optional.

AI coding agents can now write real code. On their own, though, several agents on one codebase tend to lose track of what has been decided, collide on the same files, review their own work, and leave no record of why a change was made. AIDA's aim is to give them a shared record, defined roles, independent review, and limits that a human sets, so their combined output can be checked and trusted.

## In short

**What it is.** A command-line tool (`aida`) plus an MCP server (`aida mcp-serve`) that sits inside a git repository. Requirements, decisions, comments, mail and lease records live in git on an orphan `aida-store` branch. Operational records such as review verdicts, merge holds, dispatch requests, handoff notes and standing orders live in local, git-ignored files under `.aida/`, and mandates live under `~/.aida/`. AIDA controls how AI agents pick up work, review each other, and merge.

**Who it is for.**

- A developer who runs several AI coding agents (Claude Code, Codex CLI, Antigravity, or anything with a shell) on one codebase at the same time.
- Anyone who wants agents to work for hours without supervision while keeping control of what gets merged.
- Teams that need to answer, later, "why does this code exist, who approved it, and on what evidence?"

**What it does, in plain terms:**

- **One shared source of truth.** Requirements, decisions, bugs and design rulings are stored as versioned spec objects. Agents are told to read them before acting (through the project's instructions and skills), and `// trace:<SPEC-ID>` comments link code back to the spec that justified it.
- **Separation of duties.** AIDA refuses an approval recorded by a session it identifies as the change's author, and if it cannot establish who the author is, it refuses. The one exception is an opt-in, audited "solo human" mode, in which a human verified at a terminal may approve their own work. The review is recorded against written acceptance criteria. Changes to the gate code itself (`aida-cli-lib/src/gate/`) and to `.aida/config.toml` are on a protected list that a mandate cannot ship, so they need a human merge.
- **Roles.** An advisor plans and rules on designs, implementers build, reviewers check, and an integrator lands the work. Seat grants and gate checks enforce the key authorities: who may record a review, ship, clear a hold, or start a mandate.
- **Delegated autonomy with limits.** A human at a terminal can start a time-boxed mandate (8 hours by default, at most 24) that lets the advisor seat ship held PRs and queue dispatch work while the human is away. Work outside the mandate's scope waits for the human, and specs marked operator-only are never covered.
- **A recorded trail.** Review verdicts are recorded against exact commits. Mandate-driven actions, dispatch requests and their results, and spec status history are also recorded, so most "who decided this, on what evidence?" questions have a written answer.

**Why it matters.** The bottleneck in AI-assisted software is shifting from writing code to trust and coordination. AIDA is built so that a team, from a single developer up to a larger group, can supervise many agents working in parallel without losing control of quality or accountability. Multi-human use is still being hardened: today everything runs under one Unix user, and AIDA treats a same-user process that forges its files as outside its threat model, so separating one human from another is not yet a security boundary.

### In one sentence

**AIDA is a governed software factory with selectable autonomy: people set the intent and limits; agents carry out the work to the level of independence chosen for each spec.**

AIDA also acts as an operating system for AI engineering teams: it grants authority, keeps shared state, and stops parallel workers from trampling each other. The coding agents write the code; AIDA coordinates and checks their work.

In plain terms: AIDA gives a team of AI coding agents shared memory, defined roles, independent review and human-set limits, so that what they ship can be checked the way you would check a well-run engineering team's work.

AIDA is alpha software and supports Linux only (README; internal spec).

---

## A governed software factory with selectable autonomy

**Software factory** describes the repeatable path from a specification to an implemented, tested, reviewed and merged change. **Governed** means that path has assigned responsibilities, recorded decisions, acceptance criteria and enforced authority limits. **Selectable autonomy** means the team chooses where human participation is required for each spec, rather than choosing one automation level for an entire project.

### Four execution modes, plus a decision stop

These descriptive names explain the existing modes to a management audience; the CLI values in the second column remain the actual mode names. They are operating choices, not industry certification levels.

| Descriptive name | AIDA mode | What the agents do | Where people participate | Example |
|---|---|---|---|---|
| **Human-led delivery** | `operator` | No implementation agent is launched; AIDA presents the checklist and stops. | A person carries out the work. | An action requiring human credentials or judgment. |
| **Collaborative delivery** | `guided` | Work through an interactive design session and prepare a change; no automatic merge. | A person answers recorded design forks and reviews the resulting PR. | An architectural change with unresolved trade-offs. |
| **Supervised delivery** | `drive` | Implement, open a PR and run through CI, then stop. | A person reviews and merges the change. | A feature whose implementation can be delegated but whose release into the main branch needs human approval. |
| **Autonomous delivery** | `drain` | Run the lifecycle through implementation, CI, independent review, merge and pull, subject to the applicable gates. | People define and authorize the work beforehand and handle escalations or work outside the delegated scope. | A bounded fix with clear acceptance criteria and no unresolved design decision. |

**Decision pending** (`decide`) sits outside this ladder. It blocks execution until the pending decision is resolved; no implementation harness runs. It is how AIDA represents work that is not ready to delegate, rather than asking an agent to invent an answer.

The execution mode is recorded on the spec. The advisor classifies the work, and `aida do <SPEC>` uses that classification to select the execution path. Moving toward more human involvement is permitted; reducing involvement requires an explicit `--force` override or a durable reclassification. A `decide` block cannot be bypassed with a mode override. See [Work & autonomy](cli/03-work-autonomy.md) for operational commands.

### Where a dark factory fits

In the software use of **dark factory**, people define intent and validation while agents implement and validate changes without routine human coding or code review. [StrongDM's software factory](https://factory.strongdm.ai/) is one explicit example of that approach.

**`drain` is AIDA's dark factory mode.** For approved, authorized specs, agents implement, test, independently review and merge changes without routine human coding or code review. People define the intent, acceptance criteria and authority limits; the factory executes within them and escalates exceptions. AIDA lets a team run this mode alongside supervised, collaborative and human-led delivery, choosing the level of human involvement for each spec.

Selecting autonomy does not grant authority to bypass reviews, CI, merge holds or protected operations. Mandates and seat grants determine which actions are authorized and for how long; the execution mode determines how the work is carried out. A stopped gate or unresolved decision can still require a person. Here, delivery means reaching the merged code state; production deployment requires its own policy and automation.

For management, this supports a practical adoption path: start with supervised delivery, retain collaborative delivery for design-heavy work, and delegate well-specified work to autonomous delivery as its validation and operating evidence justify it. The goal is to choose the human attention each change needs and retain a record of what happened.

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-100 | ai:codex -->

---

## How big is it, and why

Historical snapshot: counts are for tracked files at commit `28bbe9d540b`, dated 28 October 2026 (then `origin/main`). These are not live product metrics.

| What | Lines |
|---|---|
| All Rust | about 1,042,000 |
| The main CLI library crate (`aida-cli-lib`) | about 810,000 (78%) |
| `aida-core` | about 152,000 |
| Test files (`tests/` directories and `*_tests.rs`) | about 269,000 (tests inside source files aren't counted) |
| Comment lines (`//`, `///`, `//!`) | about 174,000 (17%) |
| Markdown, on top of that | about 156,000 |
| Largest source files | `queue_cmd.rs` 17,170 lines (about 786 KB), `doctor_cmd.rs` 15,230, `mcp.rs` 14,997, `auto_complete.rs` 14,774 |

Why it is large:

- **The scope is wide.** A requirements store, git-backed sync, a merge train, a review gate, mandates and grants, sessions, mail, a scheduler, and four interfaces (command line, MCP, terminal UI and web server).
- **The safety rules are written as tests.** Ratchet tests fail the build if a forbidden code pattern comes back. They are verbose, but they are what keep the guarantees in the section above true.
- **Comments carry explanations.** Many record why a piece of code exists, with trace links to the spec, for the next agent to read.

There is some overlap. Approval decisions used to be made in more than one place. The ship gate and the review watcher's readiness report now share a single decision function (internal spec), so they cannot disagree. The size has a real cost, whatever the cause: every agent session spends context and build time navigating it. The work under way is to make the size cheaper to work in. The `lib.rs` split is done, the follow-on split of the crate into dependency-ordered crates is still open (internal spec), and a 28 October spike looked at carving out `aida-core`. The plan is to propose deleting code only where tools or usage data show it is unused.

---

## What AIDA can measure and support for engineering management

AIDA reports on captured delivery evidence. People develop and review conversationally while configured agents maintain structured intent; reporting coverage depends on what those tools recorded. This capability map was checked on 29 October 2026. It does not present live project values or claimed savings. See the [Metrics guide](metrics-guide.md) for sources and interpretation.

- **Reported today:** cycle-time activity percentiles and work/wait/unknown splits; commit-trailer attribution and spec/criteria evidence; recorded drain outcomes; effort/load and calibration reports; local command latency and error reports when telemetry is enabled. Unknown time and excluded items remain explicit. CLI invocation counts include agent activity and do not measure human productivity.
- **Recorded with incomplete coverage:** token usage from retained Claude Code/Codex artifacts with attributable session evidence, offline cost estimates for the pinned rate table's supported model/token classes, and local review findings/classification. The built-in ledger does not parse Antigravity artifacts. Unpriced or missing usage is not zero; cost estimates are not billing. The bundled rate table is versioned 2026-09-22.1 and prices only Claude Sonnet 4 (20250514) and Opus 4.1 (20250805), plus their listed aliases. Newer Claude models and Codex models remain unpriced.
- **Derivable without a finished joined report:** lines changed per spec and review rounds joined across change history require additional analysis, deduplication and coverage accounting. Existing cycle-time counters are not a complete verdict-history analysis.
- **Not supported:** measured hours saved against equivalent human work, proven ROI and reliable feature-duration forecasts. Recorded buckets and counts are proxies, not proof of business value.

Effort can be captured when work is opened, planned, shipped and reviewed. The capture model has open, plan, implementation/ship and review touchpoints; available records feed `aida load report` and `aida load calibration`. Calibration compares estimates/assessments with the implementer's declared ship bucket, not stopwatch measurements or equivalent human time. Buckets are 15m, 1h, 4h, 1d (8 work-hours) and 1w (40 work-hours). Missing touchpoints limit coverage; heft is a graph score, not an effort estimate.

Use `aida metrics cycle-time`, `aida metrics agent-lift`, `aida metrics ai-lift`, `aida usage show`, `aida usage show --cost`, `aida review classes` and command help to inspect the reports. Token provenance and local log coverage require interpretation; a project-wide total cannot be inferred from one attributed spec.

**Air-gapped development.**
- **Works without a network:** the requirements store, coordination, leases, mail, telemetry, the token ledger and the cost estimates are all local files or a git branch, and the MCP server talks over stdio. Any git server can act as the hub; this repo mirrors to a self-hosted GitLab. `aida pr ship` supports GitLab merge requests, but the merge train is GitHub only. Releases are prebuilt Linux tarballs (x86_64 and arm64), so installation can be offline, though `aida upgrade` needs a network.
- **Network containment:** for agent sessions launched in contained mode, an opt-in `[contained] allowed_hosts` setting restricts outbound network access.
- **The coding agents are the limit:** AIDA launches Claude Code, Codex CLI or Antigravity, and those CLIs need their model APIs. An air-gapped deployment therefore needs on-premises or local models that the vendor CLI itself can be pointed at. AIDA has no model layer of its own for coding agents. The only built-in local-model option is that the spec evaluator can call an OpenAI-compatible local endpoint (Ollama or vLLM, set with `AIDA_LOCAL_LLM_ENDPOINT`).

### Proposed extensions (not built)

The following extensions are proposals. Existing open/plan/ship/review effort capture and load calibration are already implemented and are covered above.

| Need | Proposal | Data it would build on | Status |
|---|---|---|---|
| Per-spec history metrics | **History backfill** (illustrative name `aida metrics backfill`), dry-run by default and idempotent. For each spec it would compute: time from first commit to merge; review rounds and changes-requested count; follow-up and carried-forward tasks (rework); lines changed; and lane-hours. It would write these as separate derived records in the store and never rewrite history. | Trailers in git; per-commit verdict files; status history; the lease records already on the store branch; the local usage and drain logs. | PROPOSED |
| Tokens and cost per spec | Extend the token ledger to attribute sessions to specs by matching a session's working directory and branch to the spec's lease, instead of only by launch receipts. | Verified fields. Each Claude Code assistant message has `message.usage`, with `input_tokens`, `output_tokens`, `cache_creation_input_tokens` and `cache_read_input_tokens`, alongside `message.model`, `cwd`, `gitBranch`, `sessionId`, `isSidechain` and a timestamp. Codex `token_count` events carry `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens` and `total_tokens`. | PROPOSED |

Caveats for the token and cost proposal:
- Prices change, so cost must use the rate in force on each message's date.
- Cache reads and cache writes are priced differently from fresh input and must not be added together as one figure.
- One session can span several specs, so its time and tokens need splitting by timestamp, not assigning whole.
- Subagent messages (`isSidechain`) and headless runs can live in other files and must be included without being counted twice.
- Transcripts are local and may be pruned, so a backfill can only cover what is still on disk.

---

## Capabilities, and when to use each

Each row names the situation that should make you reach for the capability. The commands below exist in `aida` 0.16.0; run `aida <command> --help` for the details.

### Requirements and traceability

| Capability | When to use it | Commands |
|---|---|---|
| **Requirements graph.** Specs (stories, tasks, bugs, decisions, epics and more) with a status, acceptance criteria, typed relationships and comments. Stored as YAML objects on a separate orphan `aida-store` git branch, so they are versioned, shareable and not tied to any one agent tool or vendor. | You are about to start work, or an agent is, and needs to know what was asked for and what has already been decided. Also when you make a decision you want the next session to find. | `aida add`, `aida list`, `aida show`, `aida search`, `aida edit`, `aida comment add`, `aida done` |
| **Trace comments.** A comment such as `// trace:<SPEC-ID> \| ai:claude` beside code links it to the spec that justified it. | You write or change code for a spec. Later, when someone asks "why is this line here?" | `rg 'trace:'` to find the spec for some code; `aida show <ID>` lists the commits and traced files for a spec; `aida doctor validate-trace-comments` checks the comments; `aida graph` shows relationships |
| **Commit trailers that complete specs.** A commit subject ending in `(SPEC-ID)` links the commit to the spec. Once the commit is on the default branch and you run `aida pull`, eligible specs move to Completed. A plain `git pull` does not do this, and `(Refs SPEC-ID)` links a spec without completing it. | Every commit for spec work, so specs don't have to be closed by hand. | `aida commit`, `aida pull` |
| **History and digests.** What changed, when, and how specs moved through their statuses. | Catching up after time away, writing a status update, or checking what landed overnight. | `aida history` (`events`, `opened`, `completed`, ...), `aida digest` |

### Doing work safely in parallel

| Capability | When to use it | Commands |
|---|---|---|
| **Isolated sessions and worktrees.** Each piece of work gets its own git worktree on its own branch, with a lease that records who holds it. | Two or more agents (or you and an agent) work at the same time. Each needs a working copy no one else touches. | `aida session start --owns <SCOPE>`, `aida worktree add`, `aida session leases`, `aida session end` |
| **Claims.** An advisory, spec-scoped lease. A duplicate `aida queue work <spec>` is refused while the claim holds, and `aida edit <spec>` warns. A claim holds only while the process that made it is alive, so a crash doesn't lock the spec forever. | You hand a spec to an agent by some route AIDA didn't launch itself (for example a subagent), and want the duplicate-dispatch checks to see it. | `aida claim`, `aida unclaim` |
| **Roles and seats with grants.** Advisor, implementer, reviewer and other seats. Entering a seat issues a time-limited grant (24 hours by default, configurable up to a 168-hour ceiling) bound to that terminal session. Single-occupancy seats can be claimed, released, and handed over with an acknowledgement. | Before any agent does seat-specific work: planning and rulings (advisor), building (implementer), reviewing (reviewer). | `aida role enter <seat>`, `aida role status`, `aida session seat claim\|status\|takeover\|ack\|release` |
| **Launching agents from several vendors.** Starts Claude Code, Codex CLI or Antigravity with the right directory, environment and registry tracking. | You want an agent started in a known state and visible to AIDA's monitoring. | `aida agent new claude\|codex\|antigravity`, `aida init --agent ...` |

### Review and merging

| Capability | When to use it | Commands |
|---|---|---|
| **Independent, recorded review.** A reviewer records a verdict (approved, request-changes or rejected) against an exact commit. An approval recorded by the author's own session is refused. An approval of an older commit doesn't cover new code, except after a clean merge of the base branch that leaves the PR's own diff unchanged. | Every change before it merges. Use `aida review classes` to see how often a kind of defect recurs. | `aida review record`, `aida review verdict`, `aida review list`, `aida review watch`, `aida review classes` |
| **Merge holds.** A marker file and the `aida:merge-hold` label, which fail a required check so the PR cannot be auto-merged until the hold is cleared. `aida merge-hold clear` requires a human at a terminal. A sensitive spec's hold is released either by a human or under a mandate started with `--include-supervised`. | A change touches something that needs a person's judgement, or you want to pause a PR outside the normal flow. | `aida merge-hold list\|add\|clear` |
| **Ship.** Create the PR if needed, watch CI, squash-merge, pull, and clean up, after checking the approval, hold and CI gates. Ship refuses any PR whose head is behind its base branch. | One approved, up-to-date PR is ready to land. | `aida pr ship`, `aida ship` (human-implemented work) |
| **Merge train.** Builds main plus one squash commit per approved PR, opens a `train/*` PR so normal CI runs on exactly that combination, and merges only if it is green and main hasn't moved. A red train is rerun once, then split in halves until each bad PR is isolated and left out. GitHub only for now. | Several approved PRs are ready at once, or a PR is behind main, and you don't want PRs that are each green on their own to turn main red together. | `aida pr train <N...>`, `aida pr train --all-ready`, `--dry-run` |

### Autonomy under human control

| Capability | When to use it | Commands |
|---|---|---|
| **Mandates.** A time-boxed grant (8 hours by default, at most 24) that lets the advisor seat ship held PRs, with an independent approval and green CI still required. `--drive` widens it to sketch signoff, dispatching fixers and a 15-minute drive tick. Starting, renewing and stopping a mandate are refused unless the process has an interactive terminal and is not an AIDA-launched agent session. `aida mandate renew` extends the active one in place without retyping its flags. | You are leaving and want agents to keep shipping reviewed work while you are away. | `aida mandate start --ship-held`, `--include-supervised`, `--scope dispatch`, `--drive`, `--ttl`, `aida mandate renew`, `aida mandate stop`, `aida mandate status` |
| **Typed dispatch requests.** A sandboxed advisor queues a typed request: drive a PR to merge (`flow`), review a PR (`review`), take over a PR (`take`), start an implementer lane (`impl`), or train PRs (`train`). An executor outside the sandbox runs it: `aida dispatch tick` handles `take` and `impl`, and `aida dispatch serve`, started from a human advisor terminal, handles all five. Requests are validated and de-duplicated when queued, and authority is re-checked from disk at every checkpoint after a wait and before each side effect. | The advisor runs in a sandbox that cannot write sibling worktrees or `.git/config`, but needs work launched or shipped. | `aida dispatch request flow\|review\|take\|impl\|train`, `aida dispatch serve`, `aida dispatch tick`, `aida dispatch list\|show\|cancel` |
| **Scheduler and due jobs.** A registry of periodic jobs. Each tick runs the substrate jobs (cleanup, health checks) that are due. Seat jobs need judgement, so the tick only marks them due for whoever holds the seat; `aida schedule run` on a seat job prints its prompt rather than running it. | A chore must happen whether or not any agent is alive, or a seat has recurring duties that shouldn't live in one session's memory. | `aida schedule tick\|status\|due\|done`, `aida schedule install-cron`, `aida schedule install-systemd` |
| **Night shift.** Launches bounded drain waves from the scheduler while no seat is awake, only if every safety guard passes. Off unless enabled for a clone. | You want approved, low-risk work to keep moving overnight. | `aida shift status\|tick --dry-run\|enable` |

### Communication and continuity

| Capability | When to use it | Commands |
|---|---|---|
| **Coordination inbox.** One list of everything waiting on you: mergeable PRs, unacknowledged briefs, findings awaiting triage, reviewer verdicts, escalations, unread mail and pending directives. | Start of a session, or any time you ask "does anything need me?" | `aida awaiting`, `aida status`; `aida human` lists the specs a human must decide, review or triage |
| **Agent mailbox.** Messages between agents, addressed to an agent id or role (or broadcast), with threads. `aida mailbox sync` digests local mail into the git store so it is durable and shared across clones. | One agent needs to tell another something: a stale branch, a question, a handover. | `aida mailbox send --to <id>`, `aida mailbox inbox`, `aida mailbox thread` |
| **Handoffs and standing orders.** A seat writes a handoff note (capped at about 4k tokens) that seeds the next session. Standing orders are dated rulings in a repo-local file per seat, printed before the handoff by `--show`. | A session is near its context limit, or ending. Or you've made a ruling ("always do X") that should outlive the conversation. | `aida session handoff --write`, `--show`, `--standing-orders add\|list`, `--check` |

### Watching the fleet and the infrastructure

| Capability | When to use it | Commands |
|---|---|---|
| **Fleet view.** One row per active session or agent, with a liveness verdict (live, dormant or stale). Also lists specs marked In-Progress that have no spec-scoped lease behind them. | You want to know what is really running right now, and what only claims to be. | `aida ps`, `aida session leases`, `aida health` |
| **Multi-hub sync.** `aida remote status` reports drift between git hubs (for example GitHub and GitLab). `mirror-sync` pushes origin's tips of main and the spec store to mirror hubs. `reconcile` repairs a diverged spec store by union-merging it, and is dry-run unless given `--execute`. Neither command force-pushes. | You push to more than one hub, or a hub's spec store has diverged from the others. | `aida remote status`, `aida remote mirror-sync`, `aida remote reconcile` (add `--execute` to act) |
| **Guards.** `aida doctor check <category>` covers categories such as performance budgets, disk headroom, hub drift, stale remote branches, runaway seats and merged agent worktrees. Each reports by default, and fails only with `--fail-on-findings`, which is how the scheduled jobs use them. | Run on a schedule so problems surface before they block a night of work. | `aida doctor check performance\|disk-headroom\|remote-drift\|stale-remote-branches\|merged-agent-worktrees --fail-on-findings` |
| **Disk reclaim.** Deletes stale build caches in worktrees that are being kept, plus aged agent scratch. Dry-run by default. Skips the main checkout, anything held by a live session or job, worktrees whose branch has an open PR, and caches touched in the last hour. | The disk is filling up with build caches from many worktrees. | `aida worktree reclaim`, then `--apply` |
| **MCP server.** Exposes the requirements graph as typed tools to MCP clients over stdio. For agents that have a shell, the CLI's compact output is the default and costs fewer tokens. | Your agent client prefers MCP tools to shell commands. | `aida mcp-serve`, `aida init --with-mcp` |

---

## What AIDA adds to GitHub, CI and Jira when the developers are AI agents

The real alternative to AIDA is not a pile of scripts. It is a team's standard tooling with AI agents plugged in: GitHub or GitLab with branch protection and required reviews, a merge queue, CI, an issue tracker such as Jira, and an agent such as Codex. That stack is mature, and AIDA builds on it rather than replacing it: AIDA ships through GitHub or GitLab and relies on their CI and required checks. The question is what that stack misses when the developers are agents. The common answer is that standard tooling assumes each developer is a separate, accountable human account. In this repository, all 36 PRs merged on 28 October were opened under one GitHub account (`joemooney`), whichever agent wrote them. GitHub therefore cannot tell the implementing agent from the reviewing agent. AIDA supplies that missing layer: agent identity and authority, coordination between concurrent agents, independent AI review in the loop, and traceability down to which agent acted on whose authority. The examples are from 28 October 2026.

### 1. Parallel work that can't break main

**What standard tooling already does.** GitHub's merge queue (and GitLab's merge trains) tests each PR on top of the latest main plus the PRs queued ahead of it, and drops a PR that fails. Branch protection can also require a branch to be up to date before merging. This is a real equivalent. AIDA's merge train does the same job, and in this repository it runs through ordinary `train/*` PRs and normal CI (39 opened on 28 October, 35 merged).

**What AIDA adds because the developers are agents.** Agents need coordination *before* the merge. Each agent works in its own leased worktree. A second pickup of the same spec is refused while a live lease or claim holds it. Agents can send each other mail, and a stalled PR can be handed to a new agent (`take`). The review watcher holds back an automatic review of a head that is behind main with overlapping paths, so no agent's review time is spent on stale code. On 28 October, an agent's request to ship PR #3040 was refused because the branch was 2 commits behind main; the refusal named the fix (`aida pr train 3040`), and the PR then landed through train #3046. GitHub's equivalent is a human noticing. The merge step itself has no real advantage over a merge queue.

### 2. Approvals tied to exact code

**What standard tooling already does.** GitHub records the commit each review was made on. It can dismiss stale approvals when new commits are pushed and require approval of the most recent push. It also blocks a PR's author from approving their own PR. All of this is real, but it is keyed to GitHub accounts.

**What AIDA adds because the developers are agents.** When every agent pushes through the same account, GitHub's author check cannot separate them, and an account cannot approve its own PR at all. So this repository's `main` protection requires CI and AIDA's `merge-hold-gate` check rather than GitHub reviews. Reviews are recorded by AIDA and posted as PR comments (PR #3074 has no GitHub reviews). Each AIDA verdict carries:
- the exact `reviewed_sha`;
- the reviewer's seat;
- an attestation tying it to that seat's grant and session.

AIDA refuses an approval from the session that authored the work (`gate/approval.rs`), and the merge is pinned to the approved head. An approval carries over only a clean merge from main that leaves the PR's diff unchanged (checked by patch-id). The review itself is an independent AI review run inside the loop against written acceptance criteria: on 28 October reviewers recorded 37 approvals and 21 request-changes verdicts.

### 3. Delegated authority that a human starts and only a human stops

**What standard tooling already does.** Tokens can expire. Deployment environments can require a named human reviewer or a wait timer. Admins can bypass protection. None of these is a time-boxed grant that lets an agent ship reviewed-but-held work while the human is away.

**What AIDA adds because the developers are agents.** A mandate is a time-boxed, scoped grant (8 hours by default, at most 24). It can be started or stopped only from an interactive terminal outside an AIDA-launched agent session (`gate/mandate.rs`, `gate/seat_authority.rs`), it is checked on every action, and every action it drives is ledgered. On 28 October an attempt to stop the mandate from the operator's agent loop was refused ("a mandate can only be stopped by a human at an interactive terminal"). After the mandate expired at 21:17:04Z, the advisor's next request (`aida dispatch request review 3065`) was refused at submit, and the refusal named the command to start a new mandate. AIDA states its own limit: a same-user process that forges files in the AIDA home directory is outside its threat model.

### 4. Sandboxed agents that can still get work done

**What standard tooling already does.** Bots and workflow triggers (for example GitHub Actions `workflow_dispatch`) can run jobs on request, and hosted agents run in the vendor's own sandbox. Neither checks which agent asked, or whether it currently holds the authority to ask.

**What AIDA adds because the developers are agents.** A local agent kept in a sandbox (one that cannot write sibling worktrees or `.git/config`) queues typed requests (`flow`, `review`, `take`, `impl`, `train`). Each request is validated and de-duplicated when queued, and authority is re-checked from disk at every checkpoint before a side effect (`gate/dispatch_auth.rs`). On 28 October the advisor queued five: one shipped, one was refused as behind main, one was wrongly refused by a fail-closed lease check (a bug fixed the same day in #3052) and then succeeded, and one take-over launched.

### 5. Sessions that end without losing the thread

**What standard tooling already does.** Issues and PR comments keep history, and agent CLIs can resume or compact their own conversations. That covers a human team well.

**What AIDA adds because the developers are agents.** Agents hit context limits several times a day, and each new session starts with no memory. Each seat writes a size-capped handoff note (about 4k tokens) that seeds the next session, and dated standing orders from the operator are printed before it. On 28 October seats wrote more than 40 such notes (38 by the advisor). The advantage is real but modest: this is structure and convention more than hard enforcement.

### 6. Guards on the agents' own infrastructure

**What standard tooling already does.** CI flags flaky tests. Monitoring catches full disks and slow services. Git hosts mirror code. For most of this example, standard tooling is the equivalent, and AIDA's guards mainly keep AIDA's own machinery healthy. For instance, the inode-reuse collision found in a merge-train run on 28 October was a bug in AIDA's own mailbox code (fixed in #3043).

**What AIDA adds because the developers are agents.** One part is specific to agents. Dozens of agent worktrees each build their own cache, and deciding what is safe to delete depends on agent state. `aida worktree reclaim` skips anything held by a live session or job, any worktree with an open PR, and recently touched caches. On 28 October `--apply` freed 532.9 GiB across 151 directories (disk use went from 94% to 55%).

### The common thread

| Need | What standard tooling gives you | What AIDA adds for agent developers |
|---|---|---|
| **Identity and authority** | One identity per account; a PR's author can't approve it | Identity per agent session and seat, even under one shared account; authored work can't be self-approved; time-boxed mandates that only a human at a terminal can start or stop |
| **Coordination between concurrent developers** | Assignees, branch naming, people talking | Leased, isolated worktrees; duplicate pickups refused; agent mail; take-overs of stalled PRs |
| **Merging safely** | Merge queue or merge train, required CI | The same, plus refusal to ship a branch that is behind main, and review held back on stale heads |
| **Review** | Human reviewers; approvals pinned to commits | Independent AI review in the loop, recorded per commit with the reviewer's seat and grant |
| **Continuity** | Issue history; per-tool resume | Per-seat handoffs and standing orders sized for the next agent's context |
| **Traceability** | Issue keys in commits; development panels (in Jira, for example) | Spec ↔ code ↔ action: trace comments, commit trailers, and ledgers that record which agent acted under which grant or mandate |
| **Refusing when state is unknown** | Mostly a matter of configuration | A design rule in AIDA's gate code: refuse and say why when evidence can't be read. It is checked in review, not guaranteed: on 27–28 October reviewers still found and sent back fail-open edge cases |

---

## Why not just Codex skills plus a wrapper?

A fair question: why not write a few skills (instructions the agent loads) and a wrapper script instead?

**When that is good enough.** One human, one or two agents, low stakes, no parallel merges, and it's acceptable if the agent sometimes ignores a rule. In that setting, skills plus a wrapper are simpler and cheaper, and AIDA would be overkill.

**The core difference.** A skill is instructions to the model, so it works as advice. The model can misread it or skip it, and an agent can't reliably enforce rules on itself, especially one running in a sandbox that can't see the whole system. AIDA puts its key rules in code that runs outside the model's control:

- `aida pr ship` refuses a PR whose head is behind its base, and the review watcher holds back an automatic review of a behind head with overlapping paths, whatever the agent thinks.
- Verdicts are bound to an exact commit, and the merge is pinned to the approved head.
- Merge holds fail a required check until cleared. Clearing one by hand needs a human at a terminal, and a sensitive spec needs either that or a mandate the human started with `--include-supervised`.
- Mandates can only be started or stopped from an interactive terminal, outside an AIDA-launched agent session.
- Dispatch requests are validated when queued, and authority is re-checked at every checkpoint before a side effect. A replacement mandate does not revive requests that lapsed under an earlier one.
- Claims and leases are tied to live processes (PID plus start time). In the paths checked here, unreadable ownership or evidence makes the operation refuse.

**What is already stable and isolated.** These guarantees are not scattered through the codebase:

- The authority decisions (ship, merge, hold, verdict, seat, dispatch, mandate) live in one module, `aida-cli-lib/src/gate/`: 34 modules and about 24,000 non-test lines. That directory, plus the drift ratchet test that keeps the list complete, is on the mandate's protected list, so a mandate cannot ship changes to it.
- Ratchet tests keep that boundary and the other conventions from eroding. They catch new gate-like functions outside `gate/`, aliased imports into the dispatch code, raw `git` command builders beyond an allow-list, and growth of the legacy CLI `Command` enum.
- CLI commands are registered through a typed registry (`command_registry::REGISTRY`). A golden help snapshot test guards it, along with a test that every top-level name has exactly one owner.
- Skills, hooks and agent docs come from one template master (`aida-core/templates/`, with 59 skills), and `make sync-templates` and `make check-templates` keep the mirrors in line.

**What sitting above the vendors buys you.** The deeper question isn't size. It is whether a layer that sits above the vendor tools gives you security and options that being tied to one vendor's agent would not. In short, it means owning your data and keeping control of your own direction.

- **You own the data.** Specs, decisions, comments, mail, leases and the operation log live on the `aida-store` branch of your own git repository, so they move to any hub you choose. Review verdicts, merge holds, dispatch requests and results, handoff notes and standing orders are plain files in your checkout's `.aida/` directory (git-ignored, so local to that clone), and mandates live in `~/.aida/`. None of it is held in a vendor's cloud. Skills are authored once in a template master and shipped both as Claude skills and as a vendor-neutral pack (`.agents/skills/`) that Codex and Antigravity read. A unit test fails if a skill is neither shipped to that pack nor explicitly excluded.
- **You aren't tied to one vendor.** AIDA launches and drives Claude Code, Codex CLI and Antigravity, interactively (`aida agent new claude|codex|antigravity`) and headless (`--vendor`), behind the same roles, gates and records. You can mix vendors or switch between them, which helps when negotiating on price and protects you if a vendor changes its terms or features. The token ledger already reads both Claude Code and Codex logs.
- **Enforcement doesn't rely on a vendor's model.** The gates are AIDA code: ship and train checks, verdicts bound to a commit, merge holds, terminal-only mandates and dispatch re-checks. They behave the same whichever model wrote the change. They are not prompts that a different model might follow differently.
- **You keep strategic options open.** Because history and coordination aren't tied to a vendor, the whole record can move with you, and on-premises or air-gapped models remain an option later. Today the only built-in local-model hook is the spec evaluator's OpenAI-compatible endpoint (Ollama or vLLM). The coding agents still need whatever model API their vendor CLI uses.
- **Today's limits, stated plainly.** Vendor parity is uneven. Contained (sandboxed) sessions are supported only for Claude: the code applies containment to Claude alone, and refuses a contained launch on Codex or Antigravity with "the `codex` vendor cannot run a contained session" rather than quietly launching it uncontained. Much of the tooling and dogfood evidence is Claude-first. And the data lives in two places, the git store and local files, so moving the full history means copying `.aida/` as well as the repository.

These protections aren't free to build. Concurrency control, crash-safe retries and identity that survives reused PIDs and inodes are where real bugs were found and fixed on 27–28 October: the inode-reuse identity collision (#3043), the dispatch occupancy scan misreading lease companion files (#3052), and writer build identity being compared by executable path instead of build (#3037).

**To be fair to the other side.** AIDA is large partly because it grew fast, and some of its approval logic has had overlapping paths (see "How big is it" above). Skills are still the right tool for guidance: how to plan, review or write a commit. AIDA ships its own skills for exactly that. So the real comparison is "skills plus an enforcement layer" against "skills alone".

| Concern | Skills plus a wrapper | AIDA |
|---|---|---|
| Guidance for the agent | Skills written by hand for each tool and kept in sync by hand. The skill is the only safeguard: if the model skips it, nothing stops the action. | Skills come from one template master (`aida-core/templates/`). They ship as Claude skills and as a vendor-neutral pack for Codex and Antigravity, are kept in sync by `make sync-templates` and `make check-templates`, and are versioned with the project. Their size is capped by budgets in `context-budget.toml` that CI tests enforce. They also have enforcement behind them: a skill tells the agent to run AIDA commands (`aida pr ship`, `aida review record`), and those commands apply the gates even if the agent misreads the skill. |
| Rules being followed | Depends on the model complying | Key rules enforced in code outside the model |
| Many agents at once | Usually not handled | Leases, locks, claims, merge train |
| Who may approve or ship | Convention | Verdicts recorded per commit, merge holds, terminal-only mandates |
| Crashes and retries | Ad hoc | Recovery records, de-duplicated requests |
| History | Git log and chat transcripts | Spec store, verdict and mandate ledgers, history |
| Data ownership | Wherever the wrapper and vendor keep it | Your git store plus local `.aida/` files |
| Vendor choice | Usually written for one vendor | Claude Code, Codex CLI, Antigravity behind the same gates (containment: Claude only) |
| Size and setup cost | Smaller | Large; takes time to learn |

---

# Appendices

## Appendix A: Experience so far

From 27–28 October 2026 (local time), with an advisor agent coordinating implementer and reviewer lanes, and an 8-hour human-started mandate for part of 28 October:

- **Throughput.** 58 commits landed on main (38 of them on 28 October). The main lines of work were (internal spec) mailbox and relay slices, (internal spec) adoption, gate-boundary ratchet follow-ups, (internal spec) and a follow-up `aida show` latency fix, and (internal spec) ID allocation.
- **The checks held.** Reviewers recorded 59 approvals and 51 request-changes verdicts against PR commits. Among the defects they sent back were fail-open paths, for example an adoption check that failed open on a discovery error. Earlier, on 27 October, the mandate refused an approval the advisor had carried over after an update-branch, treating it as self-approval (#2925).
- **Where it's still rough (advisor's observation).** Much of the delay came from coordination overhead: re-reviews after branch updates, rounds of follow-up findings, and handoffs to the human that the safety rules require. On 28 October the advisor recommended ruling each minor non-blocking finding once, instead of opening new follow-up PRs.

## Appendix B: Verification notes

Checked on 2026-10-28 against `origin/main` `28bbe9d540b`, `aida` 0.16.0 `--help`, the code under `aida-cli-lib/src/`, `.aida/review-verdicts/`, `.aida/dispatch/`, `.aida/handoff/`, `aida mandate status`, `git log origin/main`, and `gh pr list`. Changed or removed claims, with the evidence:

1. **"Keeps requirements, decisions and work state in git."** Only partly true. The `aida-store` branch holds spec objects, coordination leases, mailbox, schedule and registry. Verdicts, holds, dispatch, handoffs and standing orders are git-ignored local files (`git check-ignore`), and mandates are in `~/.aida/mandates`. Reworded.
2. **"The agent that writes a change can never approve it."** The refusal is real (internal spec, `gate/approval.rs`), but (internal spec) adds an opt-in solo-human mode in which a TTY-verified human may approve their own claim. Exception added.
3. **"Sensitive areas, such as the rules that decide what may merge, can only be changed with a human's sign-off."** Narrowed to what `GATE_PROTECTED_PATHS` lists: `.aida/config.toml`, `gate/`, and the ratchet test.
4. **"Sensitive work can only be cleared by a human" (merge holds).** `gate/merge_hold_core.rs` also lets an `--include-supervised` mandate release it. Corrected.
5. **Mandate "requires a human at an interactive terminal, so an agent cannot grant itself authority."** The check (`require_direct_human`) is: no `AIDA_AGENT_NAME`/`AIDA_AGENT_TYPE`, and stdin and stderr are TTYs. `--help` says same-uid forgery is outside the threat model. The absolute claim is softened, and the script comparison no longer implies identity "a process can't fake".
6. **Example 1, "the review gate refused to review it... No reviewer approved code it hadn't seen against current main."** No review-watch overlap notice dated 28 October was found in the primary mailbox, and the second sentence could not be verified. Replaced with the recorded #3040 ship refusal and the train that landed it (#3046), keeping the watcher behaviour as a code fact (`review_watch.rs`).
7. **Example 3, "When the mandate expired, every dispatch request was refused."** Narrowed to the one request made after expiry. An earlier check found no post-expiry request in `.aida/dispatch/`, because a refusal at submit writes no dispatch record. The advisor pointed to the evidence, which was verified against the "DEMO EVIDENCE step 9" comment on an internal spec (`aida show <ID> --comments --full`): `aida dispatch request review 3065` was refused at submit with the expiry message, and a non-TTY `aida mandate stop` was refused before that. "Every request" is reduced to that single request. The message matches `dispatch_authority.rs:1139`.
8. **Example 4, "malformed or out-of-scope requests were refused when submitted... authority that lapsed mid-run stopped the run."** There is no record of either on 28 October. Replaced with the actual five requests, including the false fail-closed refusal caused by (internal spec) and fixed in #3052.
9. **Example 5, "when an agent lane hit its context limit".** The cause of each handoff was not recorded. Replaced with counts of handoff notes.
10. **Example 6, inode.** "Failed only in CI" refined: it failed on train runs #3039 and #3041 while main was green (internal spec description). The "operating system reused its inode" detail is kept, with the actual subject (the mailbox source key).
11. **Example 6, "a performance budget tripped and flagged a slow path before it spread."** "Before it spread" was unsupported. Replaced with the (internal spec) figures, the #3012 fix, and the advisor's "pre-fix samples" ruling.
12. **Example 6, 533 GiB.** Now cited as command output, as reported first-hand by the advisor: `aida worktree reclaim --apply` printed "freed 532.9 GiB across 151 dir(s)", and `df` went from 94% to 55% used. The advisor's 28 October handoff notes say the same ("Reclaimed 533 GiB of build caches"). No saved copy of that output exists in the repo or in `~/.aida` logs. The "nothing in live use" part is stated as the tool's skip rules (`worktree reclaim --help`).
13. **"Multi-hub sync... Dry-run by default."** True only for `reconcile`. `mirror-sync` pushes, though it never force-pushes (`--help`). Corrected.
14. **"Authority can only be lost during a run, never gained."** Not found as a general rule. Replaced with the verified rule that a replacement mandate does not revive requests that lapsed under an earlier one (`dispatch_authority.rs:1930`).
15. **"These aren't theoretical... the reused inode, the expired mandate, and the diverged hubs" (as bugs found and fixed).** The expired mandate and the hub divergence were not bugs. Replaced with three bugs fixed on 27–28 October (#3043, #3052, #3037).
16. **"Approvals don't carry forward" (only exception a clean merge of main).** Added the patch-id condition (internal spec) and the `--override-stale-approval` escape hatch, which is non-author only and refused under a mandate (`gate/ship.rs`, `gate/mandate.rs`).
17. **Trace-comment commands.** `aida why` explains why a spec is still open, and does not answer "why is this line here?". Replaced with `rg 'trace:'`, `aida show` git-linkage and `aida doctor validate-trace-comments`.
18. **Commit-trailer completion, "when the change merges and you pull".** Clarified that `aida pull` triggers it and `git pull` does not, and that `(Refs ...)` does not complete (`db_reconcile.rs`, repository guide).
19. **Guard category names.** "Stale branches, merged worktrees" become the real categories `stale-remote-branches` and `merged-agent-worktrees` (`doctor_cmd.rs`).
20. **Smaller command and flag corrections.** Added `session seat ack`, `review classes`, `--scope dispatch` and the tick/serve split for dispatch verbs. Mailbox addressing is "agent id or role (or broadcast)"; "user" is not a `--to` form in `--help`. `aida human` is the human-decision list, not the full inbox.
21. **Size table.** Test files: 258,000 → about 269,000. Comments: 171,000 → about 174,000. Largest files: "15,000 to 17,000 each" → the four named files, 14,774 to 17,170 lines. An earlier draft's claim that `lib.rs` is "over 1 MiB" was wrong: it is about 20 KB. The 786 KB largest file was added. All Rust (1,042,330), aida-cli-lib (809,786) and Markdown (156,392) were verified with `git ls-files | xargs wc -l`.
22. **"An earlier estimate that the code could be cut to a third of its size was withdrawn."** No record of this was found in the repository. Removed.
23. **"Capping file sizes" as current direction.** No file-size cap spec or CI gate was found. Removed. The `lib.rs` split (internal spec) and the 28 October core-split spike were verified and are cited instead.
24. **"Several approval paths that have disagreed with each other and caused stalls."** Reworded to the verified history: the ship gate and the watcher readiness report were unified into one decision (internal spec, `gate/approval.rs`), and a carry-over stall was refused as self-approval (#2925, advisor handoff of 27 October).
25. **Experience section.** "About 40 reviewed changes and half a dozen major features" and "mostly unattended" became measured counts (58 commits; 59 approved and 51 request-changes per-commit verdicts) and named lines of work. **"Blocked the coordinating agent... from impersonating a human"**: no instance in the two-day window. Removed (the closest is (internal spec), from 8 October). The "rough" bullet is labelled as the advisor's observation.
26. **Marketing phrasing.** "Turns them into a team that can be trusted", "operating system for a team of AI software engineers" and "Every decision, approval, refusal and merge is recorded" were toned down to what the code records. "Why it matters" is the author's judgement; the label "(opinion)" was dropped on request as implied.
27. **Added: "What is already stable and isolated".** Verified facts: `gate/` (34 modules, about 24,000 non-test lines, on the protected list), the ratchet tests (`story_1888_gate_paths_tests.rs`, `task_2143_clone_git_ratchet_tests.rs`, the import ratchets in `gate/dispatch_deps.rs`), the typed registry tests (`story_1728_command_registry_tests.rs`), and the template master (`aida-core/templates/`, 59 skills, Makefile targets).
28. **Verified unchanged** (sampled): alpha and Linux-only; vendor list; mandate default of 8h, maximum 24h and no extension; grant binding to the terminal session; train behaviour (rerun once, then split; GitHub only); shift guards; scheduler seat jobs; claim liveness; `ps` orphaned In-Progress; reclaim dry-run; MCP over stdio; the four interfaces (`aida-tui`, `aida-server`, `aida-web-react`).
29. **Management metrics capability map (updated 29 October 2026).** The categories above and the [Metrics guide](metrics-guide.md) replace the earlier RECORDS TODAY table. Commands and scope were checked against help and source in `metrics_cycle_time.rs`, `metrics_cmd.rs` and `token_ledger.rs`; the earlier live report values are historical evidence, not current project measurements. The ledger reads retained Claude Code and Codex artifacts, with attribution coverage limits. `data/token-rates/v1.toml` prices only Claude Sonnet 4 (20250514) and Opus 4.1 (20250805) and their listed aliases. Newer Claude models and Codex models remain unpriced; missing prices do not imply zero cost. Heft is a relationship-graph score, not an effort estimate.
30. **Effort touchpoints.** `effort_calibration.rs` defines open, plan, implementation/ship and review capture. `aida load report` and `aida load calibration` consume available records. Calibration treats the implementer's declared ship bucket as its actual value; neither that bucket nor the reviewer's advisory assessment is a stopwatch measurement or equivalent human effort. Missing touchpoints limit the report.
31. **Cycle-time interpretation.** `metrics_cycle_time.rs` uses recorded status transitions and the local drain feed, without fetching forge timestamps. Work/wait/unknown splits and bounce counters must be read with their coverage. The older sample's bounce and unknown-time values are not current measurements and are no longer used as product figures.
32. **"No code-coverage or complexity metrics".** No `llvm-cov`, `tarpaulin` or cyclomatic-complexity code was found in `aida-cli-lib` or `aida-core`. No forecasting command was found in `aida --help` or the metrics subcommands.
33. **Air-gap claims.**
   - The store branch contents were checked with `git ls-tree aida-store`.
   - The GitLab hub is the `gitlab` remote, and GitLab ship support is the `ForgeKind::GitLab` path in `pr_ship.rs`. The train is GitHub only, per `pr train --help`.
   - The release tarballs and Linux-only status come from the README.
   - The `[contained] allowed_hosts` allowlist is in `session.rs`. It is written into the contained Claude Code settings and is opt-in.
   - The local-model evaluator is `LocalLlmEvaluator` in `evaluator.rs`: `AIDA_LOCAL_LLM_ENDPOINT`, defaulting to Ollama on loopback with model `qwen2.5:7b`. It is used only for evaluation.
   - No local-model or base-URL support for the coding agents was found. Whether a vendor CLI can be pointed at on-premises models depends on that vendor, and was not verified.
34. **Token fields (proposal 3).**
   - The keys were read from one current Claude Code transcript in `~/.claude/projects/`. Besides the four token counts, `message.usage` also holds `cache_creation`, `service_tier`, `server_tool_use`, `output_tokens_details` and other keys.
   - The Codex fields were read from a 2026-10-28 rollout file under `~/.codex/sessions/`.
   - The current ledger attributes sessions to specs only through pickup manifests and `.aida/orchestrator-handoffs` receipts (`token_ledger.rs`). Matching by working directory and branch is therefore new.
   - The 634 lease records on the store branch were counted under `coordination/leases`.
35. **Skills-section reframe (on request).** The old "what a small wrapper would have to grow into" list was replaced with "What sitting above the vendors buys you", and every size-diminishing wrapper phrasing ("small wrapper", "5,000 to 10,000-line") was removed, including the comparison-table header. The new claims were checked as follows:
   - **Data locations** were checked with `git ls-tree aida-store` and `git check-ignore`, as in note 1.
   - **Vendor-neutral skill pack:** see `aida-core/templates/skill-inventory.toml`, which says skills ship to `.agents/skills/` for Codex and Antigravity and that a unit test enforces the inventory.
   - **Vendors:** `aida agent new --help` lists claude, codex and antigravity. `HeadlessVendor` in `session.rs` has Claude, Codex and Agy.
   - **Containment is Claude-only:** `containment_applied` returns true only for Claude, and the test `containment_applied_reports_only_claude_as_contained` pins this. `require_vendor_containment` refuses with "cannot run a contained session" instead of downgrading.
   - **The 28 October refusal itself:** a report that `--vendor codex` hit this refusal on 28 October was not found in local handoffs or mail. The doc therefore cites the code behaviour, not that event.
   - **The token ledger** reads both Claude and Codex logs (note 29). The local-model hook is described in note 33.
   - **Opinion, labelled as such:** "Negotiate on price" and "survive a vendor changing terms" are judgements about the value of the design, not measured facts.
36. **Comparison table, "Guidance for the agent" row.** The row was rewritten to show the real difference. The new cell rests on four checks:
   - **Template master:** the `aida-core/templates/` master is read by the Makefile `sync-templates` target, and `check-templates` verifies the mirrors and the portable-pack hook master (`hooks/aida-sync-agent-skills.sh`).
   - **Portable pack:** the pack for Codex and Antigravity is described in note 35.
   - **Size budget:** `aida-core/templates/context-budget.toml` says "CI tests enforce these values" (for example, `skill_description_max_bytes = 240`).
   - **Enforcement:** the "backed by enforcement" claim rests on the gate code paths in notes 2–5 and 16.

   The other rows were checked for the same "identical on both sides" problem. None remained once the old "Size and setup cost: Small" cell was changed to "Smaller", and the Data ownership and Vendor choice rows were added (note 35).
37. **Section reframed to compare against standard tooling (on request).** The old "What AIDA does that simple scripts can't easily do" section was replaced by "What AIDA adds to GitHub, CI and Jira when the developers are AI agents".
   - **One shared account:** `gh pr list --search "merged:2026-10-28"` shows all 36 merged PRs authored by `joemooney`, and `git log` authors are all one identity.
   - **Branch protection on `main`:** `gh api .../branches/main/protection` shows required checks `merge-hold-gate` and `Build (ubuntu-latest)`, no required GitHub reviews, and `strict: false`.
   - **Reviews:** `gh api .../pulls/3074/reviews` returns 0 reviews. All 36 `comment_url` values in the `PR-30xx` verdict files are issue comments.
   - **Merge-hold workflow:** `.github/workflows/merge-hold-gate.yml` exists.
   - **Verdict contents:** each verdict carries `reviewed_sha` plus `recorder_attestation` (seat, `grant_id`, identity), as in `PR-3074.json`.
   - **Other AIDA claims** reuse the evidence in notes 2–8, 12 and 16 (`gate/approval.rs`, `gate/mandate.rs`, `gate/seat_authority.rs`, `gate/dispatch_auth.rs`, the (internal spec) step 9 comment, `worktree reclaim --help`).
38. **GitHub, GitLab and Jira capabilities are stated from product knowledge, not checked in this repo.** These are: the merge queue testing on top of the latest base and dropping failing PRs; GitLab merge trains; the up-to-date branch requirement; reviews recorded per commit; dismissing stale approvals; requiring approval of the most recent push; authors unable to approve their own PR; deployment-environment reviewers and wait timers; token expiry; `workflow_dispatch`; and Jira development panels and issue keys. The doc deliberately avoids claiming the details of how GitHub's merge queue bisects a failing group. Example 6 is now stated to have mostly no advantage over standard tooling, with only the agent-aware disk reclaim kept as an addition.
39. **Team framing.** "One person can supervise many agents" became "a team, from a single developer up to a larger group". A multi-human caveat was added: everything runs under one Unix user, and a same-user process forging files is outside the threat model (`aida mandate start --help`, `gate/mandate.rs` module doc). The README describes broader-team scale as still being validated. Two uses of "solo" were kept on purpose: "solo human" is the name of an AIDA review mode (note 2), and "one human, one or two agents" describes when skills alone are enough.
