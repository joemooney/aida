# Autonomous drain — `--no-human`

`aida queue work --auto-complete` drives a spec through the full
implementer → CI → reviewer → merge → pull → build lifecycle in one command.
By default each Claude phase is **interactive**: the orchestrator launches
`claude`, you watch it work, and you press **Ctrl+D** when a phase is done so
the orchestrator advances. That is the right mode for the everyday "watch a
spec ship in ~25 min" pattern.

`--no-human` removes the Ctrl+D. It launches the phase's Claude session
**headless** (`claude -p`) — a single-turn run that exits on its own when the
work is finished. That is what makes an unattended overnight drain possible:

```bash
aida queue work --batch nightly --auto-complete --no-human
```

## The three autonomy modes

`--no-human` is the far end of a three-mode ladder. The middle rung,
`--zen`, exists because "is a human present" and "what does the human want
to be asked" are two different axes — a user can be *at the keyboard* and
still not want to click-yes through thirty mechanical prompts.

| Mode | Flag | Persona | Mechanical prompts | Design-fork prompts |
|---|---|---|---|---|
| **Default** | *(none)* | "Driving" — approves each step | Pause + ask | Pause + ask |
| **Zen** | `--zen` / `AIDA_ZEN=1` | "Advisor on standby" — consulted on real questions only | **Auto-resolve** | Pause + ask |
| **No-human** | `--no-human` / `AIDA_HEADLESS=1` | "Absent" — nobody reachable | Auto-resolve | *Punt* (follow-up slice) |

Each rung is strictly more autonomy than the one above. Precedence is
**`--no-human` > `--zen` > default** — setting both `--zen` and
`--no-human` resolves to `--no-human` (it wins, with a warning).

**When to use each:**

- **Default** — you are shaping the work as it goes, or the spec has open
  design questions you want to decide live.
- **`--zen`** — you are watching the drain and want to stay in the loop on
  real decisions, but the mechanical "open PR? / merge? / grab next?"
  prompts are noise. You are still there to answer a design fork.
- **`--no-human`** — nobody is watching (overnight, a long batch).
  `--no-human=both` runs both Claude phases headless; the implementer punts a
  design-fork to Needs Attention (STORY-276) rather than guess past it.

`--zen` works today without any headless machinery — it is pure
prompt-classification. Skill templates tag each prompt with a `kind:`
annotation (`confirmation` vs `design-fork`); under `$AIDA_ZEN` the skill
auto-resolves the `confirmation` prompts and still surfaces the
`design-fork` ones. The classification rules live in
`docs/aida-discipline/skill-prompt-kinds.md`.

### Exiting a `--zen` session — the graceful-exit sentinel (TASK-329)

Auto-resolving a `confirmation` prompt is easy when the answer is "open the
PR" or "merge". It is harder when the answer is **"exit the session"**: a
skill cannot synthesize the Ctrl+D it would press interactively (BUG-230),
so under `--zen` the end-of-drain annotation used to print while the Claude
Code REPL sat open at `❯`, blocking the orchestrator.

`--zen` closes that gap with a one-way file signal. The orchestrator exports
an `AIDA_EXIT_SENTINEL` path to each phase and, instead of blocking on the
child, polls for it. The skill's absolute last action is
`touch "$AIDA_EXIT_SENTINEL"`; the orchestrator sees the file and reaps the
idle REPL (SIGTERM, a 2s grace window, then SIGKILL).

**Friction after a `--zen` step:** near zero. In interactive mode the user
still presses Ctrl+D; under `--zen` the orchestrator reaps the REPL within
~100ms of the skill touching the sentinel — no keystroke, no hang. The
polling and grace windows are tunable via `AIDA_EXIT_POLL_MS` /
`AIDA_EXIT_GRACE_MS`. Full protocol:
`docs/aida-discipline/skill-prompt-kinds.md`.

## What runs headless

Of the two Claude phases, `--no-human` runs one or both headless depending on
the MODE:

| Phase | `reviewer-only` | `both` |
|-------|-----------------|--------|
| 1 — implementer | **Interactive** — pauses for you at each phase-1 completion. | **Headless** — `/aida-pickup` runs `claude -p`; on a design-fork it cannot resolve it punts the spec to Needs Attention (STORY-276) instead of guessing. |
| 3 — reviewer | **Headless** — `/aida-review` runs `claude -p`, writes its verdict file, and exits. No Ctrl+D. | **Headless** — same. |

The reviewer was the safe phase to run headless first: `/aida-review` already
writes its verdict to a file the orchestrator reads (the `--auto-complete`
handshake) and stops before any merge — the orchestrator owns phase 4. The
headless implementer (STORY-276) is the riskier phase, and its safety net is
the **punt**: a headless implementer that hits a design-fork it cannot safely
resolve runs `/aida-punt` rather than commit a silent wrong guess. The spec
parks in Needs Attention, the orchestrator records the punt and advances to
the next item, and the advisor triages it later (`aida findings list`).

### MODE selector

- `--no-human` / `--no-human=reviewer-only` — headless reviewer; the
  implementer phase stays interactive. **Bare `--no-human` resolves here** —
  the conservative default (TASK-306). Use it when you want to review each
  spec's implementation yourself but let the reviewer phase run unattended.
- `--no-human=both` — fully headless drain, implementer included (STORY-276).
  The implementer punts design-forks rather than guessing. Use it for an
  unattended overnight drain of low-ambiguity work.

`--unattended` and `--headless` are accepted as aliases of `--no-human`.

### Scope clarity at kickoff and in the statusline (TASK-306)

`--no-human` covers different ground per MODE, so the scope is stated
**loudly** in three places:

- **Pre-launch banner** — `aida queue work --auto-complete --no-human` prints
  a scope banner — for `reviewer-only`, that phase 1 stays interactive and the
  drain pauses there; for `both`, that phase 1 runs headless with the
  design-fork punt as its safety net — and requires a one-time
  acknowledgement before launch. It shows once per kickoff. Skip the prompt
  for an unattended run by exporting `AIDA_NO_HUMAN_ACKNOWLEDGED=1`; a
  non-terminal stdin without it errors rather than blocking on an
  unanswerable prompt.
- **`--help` text** — `aida queue work --help` spells out the per-MODE scope.
- **Statusline** — an interactive phase running inside an `--auto-complete`
  orchestrator shows `auto:N/6 <phase>`, the `no-human:<mode>` scope, and a
  loud `pause-here` cue, so a user who expected to walk away sees that
  phase 1 still needs them (`reviewer-only`).

### How the headless launch is configured

Headless sessions launch with the flag set verified empirically in SPIKE-7
(`docs/spikes/2026-05-16-claude-headless.md`):

- `--permission-mode bypassPermissions` — **mandatory**. `acceptEdits` leaves
  `Bash` gated and `default` auto-denies tools silently; either way the work
  does not happen. `--no-human` forces `bypassPermissions`.
- `--output-format stream-json --verbose` — newline-delimited JSON events,
  streamed to `<project>/.aida/headless-logs/<branch>-<session>.jsonl`.
- `--session-id <uuid>` — persistence stays on, so a killed run is resumable.
- Never `--bare` — it breaks OAuth/keychain auth.

## The orchestrator → child env contract

A `--auto-complete` orchestrator launches each phase as a child `aida queue
work` subprocess. The child needs to know it is a *genuine* phase child — so
it suppresses interactive menus, takes the clean orchestrator hand-off, and
keys orchestrator-aware skill behavior correctly. The env contract that
carries this (BUG-233):

| Variable | Set by | Meaning |
|----------|--------|---------|
| `AIDA_AUTO_COMPLETE=1` | orchestrator → every phase child | "this session belongs to an `--auto-complete` run" — **not trusted on its own** |
| `AIDA_AUTO_COMPLETE_TOKEN=<run-uuid>` | orchestrator → every phase child | the corroboration token: a per-run UUID naming a marker file |
| `AIDA_AUTO_COMPLETE_PHASE=<1..6>` | orchestrator → each Claude-launching phase child (1 implementer, 3 reviewer) | the 1-based phase index, so the child's statusline can show `auto:N/6` (TASK-306) |
| `AIDA_NO_HUMAN_MODE=<slug>` | orchestrator → phase children, when `--no-human` is set | the `--no-human` scope (`reviewer-only` / `both`), shown in the statusline (TASK-306) |
| `AIDA_HEADLESS=1` | the headless `claude -p` launch → its session | "this Claude session is headless" — skills key findings-filing and the design-fork punt off it |
| `AIDA_REVIEW_VERDICT_FILE=<path>` | orchestrator → reviewer child only | absolute path the reviewer writes its verdict JSON to |
| `AIDA_PUNT_SIGNAL_FILE=<path>` | orchestrator → implementer child only | absolute path `aida punt` drops a signal file at, so the orchestrator detects a punt and parks the spec instead of reporting a phantom no-PR failure (STORY-276) |
| `AIDA_EXIT_SENTINEL=<path>` | orchestrator → every phase child | file the skill `touch`es as its last action so the orchestrator reaps the idle REPL (TASK-329) |

**Why a token, not a bare flag.** `AIDA_AUTO_COMPLETE=1` alone is
unverifiable: a child cannot tell a legitimate orchestrator parent from a
stale inherited value. Guessing wrong does real harm — an orchestrated child
that thinks it is standalone runs menus that break the chain; a standalone
session that thinks it is orchestrated stalls waiting for an orchestrator
that does not exist (BUG-233's two misfires).

**How corroboration works.** For the lifetime of each run the orchestrator
holds a marker file `.aida/orchestrator-runs/<run-uuid>` (under the *main*
worktree root) recording its own PID. A child trusts orchestrator-mode only
when `AIDA_AUTO_COMPLETE=1` **and** `AIDA_AUTO_COMPLETE_TOKEN` names a marker
whose PID is alive. The marker is RAII-cleaned when the run ends.

**How to check it.** `aida orchestrator status` prints `orchestrated` (the
corroborated verdict) or `interactive`; `--json` adds `corroborated` +
`reason`. Skills (`/aida-pickup`, `/aida-pr`, `/aida-review`) branch off that
word, never the bare env var. A bare `AIDA_AUTO_COMPLETE` with no live token
is treated as interactive plus a one-line informational note — there is no
"env leak" to chase (BUG-233 corrected its own original misdiagnosis).

## When `--no-human` is appropriate

Good candidates:

- **Small specs with clear, checkable acceptance criteria.** The implementer
  has no real decision to make; the reviewer has a concrete checklist.
- **Batches of similar, low-ambiguity work** — a `batch:NAME` of mechanical
  TASKs is the canonical overnight drain.

Use interactive (omit `--no-human`) for:

- **Specs whose description mentions a design fork / open question.** A
  headless `--no-human=both` implementer will *punt* such a spec to Needs
  Attention rather than guess — safe, but it spends a session and a punt to
  reach a decision you could have made up front. Decide known-forky specs at
  the keyboard; let the punt catch the forks you did not see coming.
- **Anything where you want to shape the approach as it goes.**

The trade-off is simple: **interactive = better decisions, autonomous =
better throughput.** Pick per session, not once per project.

## Cost

A full implementer→reviewer lifecycle is roughly **$3 of API spend per spec**
at current Opus prices (SPIKE-7 measured ~$3.80 across its test suite). An
overnight drain of a 20-item batch is a ~$60 run. Size your batches
accordingly.

## Findings reach the advisor (STORY-278, STORY-285)

A headless drain phase surfaces things a human would normally hand to the
`dialog`/advisor role to file as follow-up TASKs. With no human in the loop
those follow-ups would be lost when the drain moves on. So each headless
phase files them itself, and the advisor triages the lot on its next session
through one shared surface — `aida findings`.

AIDA exports `AIDA_HEADLESS=1` for every headless `claude -p` launch; both
phases below key on it and skip filing entirely in an interactive session.

**Reviewer side (phase 3).** A headless reviewer surfaces non-blocking
findings — clippy noise, drifted refs, small bugs. `/aida-review` step 7b,
after posting the consolidated PR comment, files each as a draft TASK tagged
`from-review:PR-N,severity:<cosmetic|minor|major>`. Idempotent — a re-review
of the same PR skips filing if `from-review:PR-N` TASKs already exist.

**Implementer side (phase 1).** A headless implementer (`--no-human=both`)
raises conversational flags at the end of a spec — a deviation from the
acceptance criteria, a non-obvious design call, a pre-existing bug spotted in
passing, a follow-up suggestion. `/aida-pickup` step 5b files each as a draft
TASK tagged
`from-implementer:SPEC-ID,kind:<deviation|design-choice|bug-spotted|followup-suggestion>,severity:<level>`.
Idempotent — keyed on the `from-implementer:SPEC-ID` tag. A *design-fork* the
implementer cannot resolve is not a finding — it is a **punt** (`/aida-pickup`
step 3d → `/aida-punt`), which parks the spec in Needs Attention.

**Triage.** `aida findings list` shows pending findings grouped by source
(*From review* / *From implementer*), then origin, severity-sorted;
`--source review|implementer` and `--kind bug-spotted` narrow it. The
`dialog`-role SessionStart hook and `/aida-pickup` surface a one-line pending
count. `aida findings promote <ID>` sends one to the work queue;
`aida findings dismiss <ID>` rejects it with an audit comment.

A finding is always a `task` — the advisor re-types it to `bug` on promote
if warranted. "Findings" is a query (a `from-review:` / `from-implementer:`
tag), not a new requirement type.

## `aida-worker` — the autonomous-drain loop (TASK-294)

`--auto-complete` finishes one drain and exits. The `aida-worker` bash
function (emitted by `aida dev shell-init`) wraps it in a loop so an
overnight session can chain multiple drains, pause itself when something
breaks, and stop cleanly on demand — all driven by a single per-project
control file.

### Quick start

```bash
aida dev shell-init --install        # one-time: lands the function in your rc
source ~/.aida/shell-init.sh         # or open a new shell

# Drain whatever the queue head is until it bails.
aida-worker
```

### The directive file: `.aida/worker.cmd`

A FIFO — one directive per line — that the worker reads at the top of each
loop iteration. Blank lines and `#`-comment lines are skipped, so you can
annotate the plan.

| Directive       | Effect                                                                                                  |
|-----------------|---------------------------------------------------------------------------------------------------------|
| _absent file_   | Same as `drain` — pick the queue head and run a full `--auto-complete` lifecycle.                       |
| `drain`         | Same as the absent-file case (the line persists; bare drain has no args to pop).                        |
| `drain <args>`  | Run `aida queue work <args> --auto-complete`. **Line is consumed on success** so a FIFO drains itself.  |
| `pause`         | Log and sleep 30s; line persists. Worker stays paused until you edit the file.                          |
| `exit`          | Return 0. Worker stops cleanly.                                                                         |
| _anything else_ | Defensively treated as `pause`.                                                                         |

The directive line *is* the configuration channel: every `aida queue work`
flag rides on it. `drain batch:autonomy-modes --zen` runs the batch in zen
mode; `drain --no-human=both` goes fully headless; `drain next3 --zen`
drains the next three queue items.

### An overnight plan

```bash
printf 'drain batch:autonomy-modes --zen\ndrain batch:cleanup --no-human=both\nexit\n' \
    > .aida/worker.cmd
aida-worker
```

Three lines, three drains, then stop. Each `drain` line is popped from the
FIFO when its lifecycle ships green, so the file shrinks as you go. Append
new lines mid-night (`>>`) and they survive into the next iteration;
overwriting (`>`) is racey only while a drain is mid-pop.

### Inspecting the directive queue

```bash
aida worker directives             # human view: counts + verbs + args
aida worker directives --json      # machine view: [{verb, args, raw}, …]
```

The same pending-directive summary surfaces in `aida status` and `aida drain
status` so a glance at either tells you what the worker will do next.
Silent when the file is empty (quiet projects stay quiet).

### Watchdog — `AIDA_WORKER_SPEC_TIMEOUT`

Each drain is wrapped in `timeout` (default **1800s**; configurable via
`AIDA_WORKER_SPEC_TIMEOUT`). Exit 124 → log `TIMED OUT` and auto-pause; any
other non-zero exit → log `halted` and auto-pause. The output substring
`nothing to drive` is treated specially: the worker sleeps 30s and
re-checks (the queue may fill from another session) rather than pausing.

A `--auto-complete` drain that blocks on CI for >30 min wants the timeout
raised — set `AIDA_WORKER_SPEC_TIMEOUT=5400` (90 min) in your shell or
project-local env file. Watch for `TIMED OUT` in the worker log when
calibrating; that's the signal you set it too low.

### What the watchdog reaches — and what it doesn't

`timeout --kill-after=5s` is what the worker uses. It puts the drain in a
new process group and signals the whole group on expiry, then SIGKILLs 5s
later if the SIGTERM was ignored. Verified to reach exec-chained children,
backgrounded children, and intermediate scripts. **It does not reach
descendants that `setsid()` out of the process group** — those orphans
survive the watchdog. If you see leaked processes after a `TIMED OUT`
event, that's the case; the heartbeat followup (`.aida/worker.heartbeat`)
addresses it.

### Stopping the worker

Three escalating choices:

1. Cleanest: append `exit` to `.aida/worker.cmd`. The worker finishes the
   in-flight drain (if any), reads `exit` next iteration, returns 0.
2. Quicker: append `pause`. The worker finishes the in-flight drain and
   then sits in the pause loop; Ctrl+C the parent shell when convenient.
3. Hard stop: Ctrl+C the worker. Kills the in-flight `aida queue work`
   with it; the directive file is left intact, so re-running `aida-worker`
   picks up where you stopped.

### Where the file lives, and the gitignore convention

`.aida/worker.cmd` lives next to `.aida/drain-state.json` (STORY-301) and
all other per-clone runtime state. The deny-by-default `.aida/*` rule
already gitignores it — no `.gitignore` change is needed when you create
the file.
## The advisor escalation tier (STORY-306)

Under `--no-human=both` a design-fork no longer goes straight from the
implementer's punt to the morning human queue. STORY-306 inserts a middle
tier — a **headless advisor** — turning the flat punt into a three-tier
escalation cascade:

```
  implementer (punts) ──► headless advisor ──► human
                          │
                  resolves │ or │ escalates
```

1. The headless implementer hits a design-fork it cannot safely resolve and
   punts (`/aida-punt`) — exactly as before.
2. The orchestrator assembles a rich, ultraplan-grade payload (the spec, its
   acceptance, graph context, trace-graph helpers) and spawns a **headless
   advisor** — `claude -p /aida-advise`, in the advisor role.
3. The advisor does one of two things:
   - **Resolves** the fork — it writes the judged answer, and the
     orchestrator resumes the *exact* punted implementer session
     (`claude -p --resume`) with that answer. The drain continues with a
     decided call, not a default guess.
   - **Escalates** the fork — it judges the fork genuinely needs a human and
     writes that decision instead.

**The advisor's default bias is ESCALATE.** A headless advisor applying
judgment unattended is exactly where drain quality silently degrades: an
over-resolved fork ships a confident-but-wrong overnight decision, which is
worse than the safe-default punt. The `/aida-advise` skill enforces an A/B/C
calibration — it resolves only a fork grounded in a **recorded principle**
(type A) or a **recorded user preference** (type B); anything turning on
strategy, irreversibility, un-recorded context, or taste (type C) is
escalated. When in doubt, escalate.

**`--escalate-blocks` (default) vs `--escalate-defaults`.** When the advisor
escalates, what the drain does next is a flag:

- `--escalate-blocks` (the default) — leave the spec parked in Needs
  Attention and advance the drain. The spec waits for a human; a paused spec
  beats a guessed one.
- `--escalate-defaults` — resume the implementer told to ship the defensible
  default, and file a `needs-human` finding for post-hoc review. For
  mechanical batches where throughput beats per-spec correctness.

Both flags are `--no-human=both`-only and mutually exclusive.

**One advisor round per spec.** If the resumed implementer's work surfaces a
fresh fork, that re-punt is terminal — the drain stops the spec; there is no
advisor↔implementer conversation.

**Every advisor decision is auditable.** Each resolve / escalate appends a
record to `.aida/punts.jsonl` (with the A/B/C classification), a resolved
fork leaves the advisor's answer + rationale as a spec comment, and an
escalated fork is tagged `needs-human`. `aida findings list` prints an
**Advisor decisions (recent)** footer — resolved-vs-escalated counts and the
escalated rows — so the morning triage sees what the overnight advisor did.

The escalation handshake also covers the **reviewer**: a headless reviewer
that will not auto-merge a PR writes its verdict file with
`merge: escalated-to-human`, and the orchestrator treats that as a
first-class non-failure outcome (exit 0, no merge, the PR left for a human) —
distinct from a non-Approved verdict and from a crashed phase.

## Limits of this cut

- There is no liveness watchdog yet: a genuinely stuck headless run is not
  auto-detected. The `stream-json` log is written so the watchdog can be
  added (TASK-298). Until then, treat a drain that has not progressed for a
  long time as needing a look.
- The advisor tier is v1 — a *fresh* advisor per punt with a rich payload. A
  persistent advisor session (one alive across the drain, accumulating
  context) is gated behind ledger evidence that v1 escalates too often for
  genuine lack of synthesis (STORY-325).
- An `--escalate-blocks` spec's lingering phase-1 worktree now cleans up
  automatically: the orchestrator stamps the lease `escalated_to_human` on
  the Blocks path, and `aida edit --status` (out of Needs Attention) — the
  morning-triage step — removes the marked worktree + lease + manifest as
  part of the same edit. `aida session prune --escalations` is the explicit
  recovery surface for cases where the auto-clean didn't fire (older
  triages that pre-date this code, a sibling-worktree edit). The
  `--escalate-defaults` resume path deliberately leaves the marker absent
  so its worktree is preserved for the resume. trace:TASK-358
- Skill templates have not been audited for interactive prompts beyond the
  reviewer/merge path (TASK-297).

## Related

- `docs/spikes/2026-05-16-claude-headless.md` — the empirical basis for every
  flag and caveat above.
- STORY-246 — the `--auto-complete` orchestrator.
- TASK-285 — `--batch --auto-complete`; `--no-human` composes with it.
- STORY-306 — the advisor escalation tier + `--escalate-blocks` /
  `--escalate-defaults`; `docs/plans/2026-05-19-story-306-advisor-escalation-tier.md`.
