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

> **This orchestrator drain is the vendor-neutral engine.** It is a CLI verb, so
> it runs on any agent that can invoke `aida` — Claude, Codex, Gemini, or a plain
> shell loop. The parallel **fan-out burndown** (`/aida-burndown`) is the
> faster hands-off path, but it is **Claude-harness-only**: it spawns its wave
> with the Claude Code harness's native subagent Task tool, which non-Claude
> vendors don't expose. On those vendors, drive this per-spec drain **serially**
> over the ready set instead. See `docs/aida/discipline/autonomous-burndown.md`
> ("Relationship to the orchestrator drain") for the full contrast.

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
`docs/aida/discipline/skill-prompt-kinds.md`.

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
`docs/aida/discipline/skill-prompt-kinds.md`.

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
  unanswerable prompt. **For a recurring overnight loop, run
  `aida no-human acknowledge` once** (TASK-394) — it persists the ack as a
  marker (`~/.aida/no-human-acknowledged` machine-wide, or `--project` for
  `.aida/no-human-acknowledged`) so each loop iteration's fresh process skips
  the prompt without re-exporting the env var. `aida no-human status` shows the
  current state; `aida no-human revoke` removes the marker.
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

**How corroboration works.** For the lifetime of each spec's orchestration
the orchestrator records its run-UUID + PID into the live drain-state file
`.aida/drain-state.json` (under the *main* worktree root). A child trusts
orchestrator-mode only when `AIDA_AUTO_COMPLETE=1` **and**
`AIDA_AUTO_COMPLETE_TOKEN` matches the file's recorded `run_uuid` and its
`orchestrator_pid` is alive. The drain-state file is removed on clean exit;
between batch members the run-UUID is cleared so a sibling member's token can
no longer corroborate. (Folded together by TASK-336 — before that, a sidecar
`.aida/orchestrator-runs/<run-uuid>` marker file owned the corroboration.)

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

### The drain exit summary (TASK-967)

Every `--auto-complete` drain (`nextN`, `--batch`, `--batches`) prints a
permanent **exit summary** when it finishes — drained, cap-hit, shelved, or
failed:

```
─ drain summary ─
  batch:autonomy-modes (drained-with-shelved) · 4 shipped · 1 shelved · 0 skipped · 5 iterations
  tokens: 1,234,567 cumulative · ~246,913/spec
  diff: +4,210 -820 across 37 files
  findings to triage: 1 — `aida findings list`
```

The token figures are the **cumulative reported tokens** across the drain's
headless phases (input + output + cache), summed from each phase's
`stream-json` log under `.aida/headless-logs/` — the same accounting the
`--max-tokens` budget cap uses (TASK-966). The diff stats are
`git diff --numstat` between the drain's start HEAD and its end HEAD on the
integration branch (each shipped spec merges + `aida pull`s, advancing main).

The same numbers are appended to `~/.aida/usage.jsonl` as a structured
`drain_summary` event (distinct from the per-invocation `UsageEvent` rows), so
the cost-per-drain history is queryable for the calibration + budget-dispatching
loop. Telemetry opt-out (`AIDA_TELEMETRY=0` / `[telemetry] enabled = false`)
suppresses the persisted record; the on-screen summary still prints.

## Findings reach the advisor (STORY-278, STORY-285)

A headless drain phase surfaces things a human would normally hand to the
`advisor`/advisor role to file as follow-up TASKs. With no human in the loop
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
`advisor`-role SessionStart hook and `/aida-pickup` surface a one-line pending
count. `aida findings promote <ID>` sends one to the work queue;
`aida findings dismiss <ID>` rejects it with an audit comment. Both accept
`--reason "<text>"` to record the *why* in that audit comment in one command
instead of two (TASK-404).

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

### Fork-from-live: full in-flight context for the headless advisor (STORY-360)

By default the headless advisor cold-boots: a fresh `claude -p /aida-advise`
loads only the persistent substrate (memories, discipline docs, the spec
graph). It has no access to the in-flight context that lives in your running
advisor session — the thread of conversation, the half-formed mental model,
the design choices you have been talking through that have not yet hardened
into a recorded principle.

STORY-360 adds an opt-in **fork-from-live** path. When a live advisor is
registered, the orchestrator copies that session's JSONL transcript to a new
UUID under the spec's worktree project slug and `claude --resume`s the copy
with the punt prompt. The forked advisor boots with the full transcript — the
same context the live session had at fork time — minus exactly one in-flight
turn at worst (the JSONL is append-only, so a fork is never more than the
current turn behind).

**Register once per session:**

```bash
# In your live advisor session — Claude Code sets CLAUDE_CODE_SESSION_ID
aida advisor register
```

That writes `~/.aida/advisor.toml`. `aida advisor status` shows what is
registered plus an estimated $/fork at the current transcript size.
`aida advisor unregister` reverts to cold-boot.

**The trade-off is cost vs context.** Fork pays a cache-creation tax on the
first fork (~$4 for a 1.3 MB / 225K-token transcript on Opus 4.7), then
~$0.03 for each additional fork within the 5-minute cache TTL. Cold-boot
runs at ~$0.50-$1.00 per advise. So fork-from-live is 4-8× more expensive
per invocation in exchange for full in-flight context. SPIKE-11 has the
full empirical writeup at `docs/spikes/2026-05-20-spike-11-session-forking.md`.

**Discovery cascade** (highest-confidence first):

1. `~/.aida/advisor.toml` — what `aida advisor register` wrote.
2. `AIDA_ADVISOR_SESSION_UUID` env var.
3. Latest-session-by-mtime under the spec's project slug — **only when
   `[advisor] allow_mtime_fallback = true`**. Off by default because every
   Claude session on the project updates mtime, so the heuristic mis-fires.

If discovery returns nothing — or the registered session looks dead (JSONL
mtime older than the freshness window AND no live `claude` PID), or the
transcript exceeds `max_source_size_mb` — the orchestrator falls through to
the cold-boot path. The fork is opt-in and graceful: drains that ignore it
behave exactly as they did before STORY-360.

**Config keys (`.aida/config.toml`)**:

```toml
[advisor]
fork_mode = "auto"            # auto (default) | always | never
allow_mtime_fallback = false  # opt-in heuristic; rare false positives
keep_fork_jsonls = true       # default: keep for audit trail
max_source_size_mb = 10       # soft cost ceiling
```

`fork_mode = "never"` short-circuits the entire fork path — useful for
parity testing against cold-boot. `keep_fork_jsonls = false` deletes the
fork JSONL after the advisor exits (the source transcript is untouched
either way; the fork is project-slug-isolated).

The fork JSONL lands under `~/.claude/projects/<spec-worktree-slug>/<fork-uuid>.jsonl`
— **not** the source's slug. Two consequences: the source transcript is
byte-clean (the fork writes only to its own UUID), and a debugger can read
every advisor decision back via `aida headless tail` from inside the spec's
worktree.

### Calibration mode: cold-boot vs fork-from-live ledger (STORY-347)

Substrate-enrichment is **open-loop** by default — the advisor writes memories
they think will close context gaps, but there is no empirical signal on
whether those memories actually narrow the cold-boot-vs-live-advisor reasoning
gap. Calibration mode closes that loop. When ON, every punt the advisor tier
sees produces **two** verdicts side-by-side:

1. The **cold-boot** advisor (the existing `claude -p` path). This drives
   the drain — calibration never changes drain behaviour.
2. The **fork-from-live** advisor (per STORY-360, when registered). This is
   shadow-only — the verdict is recorded but discarded for drain purposes.

Both verdicts land in `.aida/punts/<punt-id>/calibration.yaml`. Each
disagreement is interpretable: same model, same prompt, only the context
differs, so a disagreement names exactly one of three failure modes — a
substrate gap (a memory that should exist but doesn't), an inherently
in-flight framing (something a live session can hold that file-substrate
can't), or the rare case where the cold-boot's fresh read of the substrate
was actually cleaner than the live advisor's potentially-stale context.

**Toggle**:

```toml
[advisor]
calibration_mode = "off"   # off (default) | on
```

**Per-drain override**:

```bash
aida queue work --auto-complete --no-human=both --calibrate    # force ON
aida queue work --auto-complete --no-human=both --no-calibrate # force OFF
```

**Cost**: with calibration mode ON, every punt pays for **both** advisor
runs — roughly `cold-boot + fork` (~$0.50-1.00 + ~$4 first / ~$0.03 cached).
The right time to turn it on is when you want to **mine substrate gaps**:
run a batch overnight with calibration on, then triage the disagreements
in the morning to find what to write into memory. Turn it back off once you
trust the substrate again.

**Review surface**:

```bash
aida findings calibration                  # disagreements (the triage signal)
aida findings calibration --all            # disagreements + agreements
aida findings calibration --agreement      # only the agreements
aida findings calibration --since 7d       # window the view
aida findings calibration --stats          # rolling agreement-rate metric
aida findings calibration annotate <punt-id> "gap → wrote memory feedback_x"
```

The annotation categories the `--stats` histogram recognises (suggested,
not enforced):

- `gap → wrote memory <name>` — the disagreement named a substrate gap and
  you wrote the memory that closes it.
- `inherently in-flight, accept` — the disagreement names a framing that
  cannot easily be externalised; accept the gap as a property of the
  substrate.
- `cold-boot was actually correct` — the live-advisor's verdict was
  drift; the cold-boot's fresh read was the right call.

**Graceful skip**: calibration ON with no live advisor registered logs a
single line ("calibration: no live advisor registered, skipping fork") and
proceeds with cold-boot only. The calibration record still gets written so
the review surface can see "how many punts found no live advisor."

## Recovery: merging a drained spec's PR by hand (TASK-406)

When a drain bails before phase 4 (CI hung, the orchestrator was interrupted,
the reviewer escalated, you stopped to inspect something), you finish the work
by hand. The natural sequence `gh pr merge <N> --squash --delete-branch` trips
on a confusing-but-cosmetic error when the spec's worktree is still around:

```
failed to run git: fatal: 'main' is already used by worktree at '/home/joe/ai/aida'
```

or (when run from inside the spec's worktree):

```
failed to run git: cannot delete branch 'task-XXX' used by worktree at '/home/joe/ai/aida-task-XXX'
```

`gh --delete-branch` ends with a local `git branch -d`. Run from inside a
worktree where the main branch is checked out elsewhere — or where the
to-be-deleted branch's own worktree is still live — that final step fails.
**The remote merge and remote-branch delete both succeeded**; only the local
branch cleanup did. Easy to misread as a real failure (and recurring enough
that it bumped TASK-406 to High after five hits in 24h).

Two worktree-aware sequences side-step it. Pick by where your shell is:

**From the main worktree (recommended):**

```bash
cd /home/joe/ai/aida
aida session end <lease-id>      # removes the spec's worktree first so
                                 # gh's local --delete-branch step succeeds
gh pr merge <N> --squash --delete-branch
aida pull                        # auto-bumps Done → Completed
```

Order matters: `git branch -d` (which `gh --delete-branch` runs locally
last) refuses to delete a branch that any worktree has checked out, no
matter the cwd. Ending the session first removes the spec's worktree so
`--delete-branch` cleans both remote and local in one shot.

**From inside the spec's worktree:**

```bash
# cwd is /home/joe/ai/aida-task-NNN
gh pr merge <N> --squash         # NO --delete-branch
cd /home/joe/ai/aida
aida pull
aida session end <lease-id>      # cwd-back via the shell wrapper; the
                                 # branch lingers harmlessly until next prune
```

`aida session end` never tries to delete the local branch itself — it removes
the worktree and prints `branch <name> retained — merge or git branch -D
<name> when ready`. Recipe 1 lets `gh --delete-branch` do the local cleanup
after the worktree is gone; recipe 2 skips `--delete-branch` entirely and
the lingering local branch is harmless. Either way, the lease + worktree get
cleaned and `aida pull` auto-bumps the spec's status from Done → Completed
when it sees the merge SHA on main.

> An `aida pr merge <N>` wrapper that detects cwd vs worktree and picks the
> right flag set is a tracked-but-deferred follow-up — TASK-406 acceptance
> calls it out as optional. Until then, the two recipes above plus muscle
> memory cover the ground.

## Shelving on failure (EPIC-28)

Before EPIC-28, the first phase failure in a `--batch --auto-complete` drain
halted the whole batch — the rest of the members sat queued until morning.
EPIC-28 changes that for the recoverable kinds of failure (CI red, reviewer
RequestChanges, build failed, …): the orchestrator **shelves** the failed
spec, **skips its dependents**, and **continues** with the independent
members.

```
batch:nightly = [A, B, C, D, E]   (D declared blocked-by B via `aida rel add D B --type blocked-by`)

  head=A → ship                       shipped=[A]
  head=B → ✗ CI red → shelve(B)       shelved=[B]   (B → NeedsAttention + FailureReason)
  head=C → ship                       shipped=[A,C]
  head=D → pickability=blocked-by B   skipped=[D]   (NeedsAttention is not Completed)
  head=E → ship                       shipped=[A,C,E]
  ───────────────────────────────────────────────────────
  outcome: DrainedWithShelved   exit 2
  → triage shelved with `aida findings list`
```

### What's recorded on a shelve

A shelved spec carries the same `NeedsAttention` status as a punt, plus a
**`FailureReason`** sibling to `AttentionReason`:

| field | shape | example |
|---|---|---|
| `phase` | slug | `ci` / `review` / `merge` / `pull` / `build` |
| `phase_index` | 1..=6 | `2` |
| `kind` | slug | `ci-red` / `request-changes` / `merge-conflict` |
| `detail` | one-liner | "Linux CI red — 3 tests panicked" |
| `recovery_hint` | optional one-liner | "`gh run view 12345`" |
| `shelved_by` | role / `None` | `implementer` |
| `shelved_at` | UTC timestamp | … |

`aida findings list` renders these under a **"Failures awaiting triage"**
section next to the existing **"Punts awaiting triage"** — the column
shape differs (the failure section leads with `failure:<phase>` + the
recovery hint), so the triage action is immediately obvious. The same
ledger file (`.aida/punts.jsonl`) records each shelving with
`resolution_path: "shelved-by-failure"` and `decision: "failure:<phase>"`
so STORY-325 punt analysis can filter shelvings in or out by that
discriminator.

### Which failures shelve

Shelvable (the drain continues):

- `no-pr` — phase 1 finished cleanly but opened no PR
- `ci-red` — phase 2 CI failed
- `ci-timeout` — phase 2 CI never reached a terminal state
- `no-verdict` — phase 3 reviewer produced no usable verdict
- `failed` — the spawned work ran and reported failure (the phase default)

Not shelvable (the drain stops, as it did pre-EPIC-28):

- `spawn` — subprocess could not even start (PATH problem)
- `missing-tool` — `gh` / `cargo` / etc. is absent
- `internal` — an orchestrator invariant was violated

The split is intentional: those last three describe a broken **environment**.
Parking an entire batch of innocent specs because the env is broken would
be worse than stopping; the env needs to be fixed before any spec can ship.

### The `--max-failures` safety cap

`aida queue work --auto-complete --batch X --max-failures N` stops the
drain after N shelves in a single batch — the assumption being that if N
specs in a row fail in a recoverable way, the assumption that they're
"independent local failures" is probably wrong (more likely: an upstream
broke, or every spec depends on something missing). The cap **defaults
to 5**; pass `--max-failures 0` to fall back to the historical "first
failure stops the batch" semantics. The cap is **per-batch**, not
per-chain — a `--batches A,B,C` chain has its own independent budget for
each batch.

### Dependency-aware skip

The skip is **free**: it falls out of the existing pickability gate
(STORY-333). When B is shelved, B is no longer `Completed`, so any
member with `BlockedBy → B` is reported as `UnsatisfiedBlocker` by
`pickability` and silently dropped by `resolve_batch_members` on the next
head-pickup call. The summary surfaces both the skipped member and why
("D (blocked-by B (Needs Attention))") so the operator can see the
cascade at a glance. Today's declaration path is
`aida rel add D B --type blocked-by`; STORY-1 of EPIC-28 will add an
`aida add --blocked-by` / `aida edit --blocked-by` flag for the same
relationship at file-time.

### Exit code grid

| Outcome | Exit code |
|---|---|
| Clean drain — every member shipped (or `--max` cap reached, more queued) | `0` |
| Empty batch, or stalled — head did not advance after a successful run | `1` |
| **Drained with shelved members (EPIC-28)** | **`2`** |
| **Hard failure — un-shelvable phase fail, build / env / internal (TASK-1054)** | **`3`** |
| `--max-tokens` / `--max-iterations` / `--max-runtime` cap stop | `7` |

Exit `2` is the EPIC-28 signal: "the drain did its job — independents shipped,
failures parked — but you have triage to do." Scripts that wrap a batch
drain should treat exit `2` as non-failure but actionable.

**TASK-1054: exit `3` is the distinct hard-failure code.** Before TASK-1054 a
single-spec drive exited the *failed-phase index* (so a CI failure exited `2`)
— colliding with the EPIC-28 `2 = shelved` sentinel, so a wrapping script could
not tell "the drive parked a spec and moved on" (recoverable) from "the drive
hit a wall" (un-shelvable phase fail, build break, OOM, internal error). The two
are now split: **`2` = shelved / parked-and-advanced (recoverable, re-drivable),
`3` = hard unrecoverable failure.** This holds for *both* the single-spec drive
and the batch drain. The `2` contract is preserved exactly; `3` is the new code.
The same table is the doc-comment on `DRIVE_EXIT_CLEAN` / `DRIVE_EXIT_SHELVED` /
`DRIVE_EXIT_HARD_FAIL` in `aida-cli/src/auto_complete.rs`.

### Triage path

```bash
aida findings list                       # show both punts and failures
aida show TASK-99                        # detail on a shelved spec
aida edit TASK-99 --status approved      # fix-and-re-queue
aida edit TASK-99 --status rejected      # drop (was wrong direction)
```

Triaging a spec out of `NeedsAttention` clears both `attention_reason`
and `failure_reason`. The punt ledger entry stays — it's history.

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
