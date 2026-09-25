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

### Which queue a drain consumes

When `aida queue work --auto-complete` is launched without an explicit spec,
batch, or `nextN` target, it consumes the head of the selected role queue. By
default that selected queue is the active shell role (`AIDA_SESSION_ROLE`);
`--role <NAME>` overrides only the queue-pull role, so an advisor can start a
drain that consumes work routed with `aida queue add <SPEC> --for implementer`.
For example:

```bash
aida queue work --auto-complete --role implementer --no-human=both
```

The drain-start permission check still uses the caller's real authority. The
role override does not turn an implementer shell into an advisor; it only says
which role queue to read. If the selected queue is empty while sibling role
queues hold work, AIDA names those queues and prints the exact rerun command.
trace:BUG-795

### Getting told when it stalls

`aida notify check` can send one operator alert when the drain reaches a state
that needs attention: idle with open work, shelved work, or an advisor
escalation. It is off unless `[notify].command` is set in `.aida/config.toml`.
The command receives the alert body on stdin; `{title}` and `{rule}` are the
only placeholders substituted into the command string.

```toml
[notify]
command = "msmtp joe@example.com"
min_interval = "30m"
quiet_hours = "23:00-07:00"

[notify.rules]
idle_with_work = true
shelve = true
escalation = true
```

For `msmtp`, use your normal msmtp account configuration and let AIDA provide
the plain-text message body. For a desktop alert, wrap `notify-send` so stdin
becomes the body:

```toml
[notify]
command = "notify-send 'aida: {rule}' \"$(cat)\""
min_interval = "15m"
```

`aida notify test` sends a sample message and reports whether the command
exited successfully. `aida notify status` shows enabled rules, last fire time,
and suppression counts. `aida notify check` is safe from cron; the drain exit
path, `aida session reap`, and `aida watch` also run the same best-effort check.
Command failures are written to `.aida/notify.log` and never fail the caller.
trace:STORY-1029

### Transient self-retries

Before a phase failure parks a spec in `NeedsAttention`, the drain retries a
closed set of transient causes once by default:

```toml
[drain]
retry_transient = 1
```

The value is a retry budget, not total attempts, and is capped at `3`. Retried
causes are `watchdog`, `no-verdict`, `no-pr`, `tool-exit`, and `cache-locked`.
Non-transient decisions are never retried: `verdict:request-changes`,
`verdict:reject`, `ci-red`, `environmental`, and `internal`.

A retry re-enters only the failed phase. Reviewer retries do not rerun the
implementer; an implementer retry repeats phase 1 against the same branch
state. `aida drain status` shows the current retry as `attempt 2/2`, and the
drain appends a `SpecRetried` event to `.aida/events.jsonl`.
trace:STORY-975

**`PhaseEntered` is duplicate-free and carries its own attempt number
(BUG-1290).** Every phase entry emits exactly one `PhaseEntered` — a consumer
does not need to deduplicate. A retry that re-enters the same phase still
emits its own `PhaseEntered`, whose `attempt` field is one greater than the
attempt that preceded it, so a legitimate re-entry is distinguishable BY VALUE
rather than by whether a `SpecRetried` event happens to sit next to it in the
feed — a consumer reading a filtered or partial stream should key on
`attempt`, not on adjacency to `SpecRetried`. `seat` on `PhaseEntered` is
`Option`al and legitimately absent for phase 1 (the implementer seat is not
yet resolved that early); its absence is not a signal of anything.

### Pipelining

### Which red checks gate a merge (`[ci]`)

A PR's checks collapse to one coarse pass/fail for the drain's CI phase and for
`aida pr ship`. Two reds are *not* failures and are re-classified before the
drain shelves `ci-red` or `pr ship` aborts:

- **The supervised merge-hold itself.** `merge-hold-gate` is designed to be red
  while `.aida/merge-holds/PR-<n>` exists. `aida pr ship` keeps the hold through
  the CI watch, releases it at the merge step, waits (up to 180s) for the gate
  to re-run green, then merges. A red gate with **no** local marker is a
  lingering label — `aida merge-hold clear <n>` re-syncs it.
- **Informational checks.** A red check is ignored only when it is *not* a
  branch-protection-required check **and** matches the allow-list below.
  Everything else red is a real failure, so a repo with no branch protection
  keeps "any red fails".

```toml
[ci]
informational_workflows = ["Cross-platform*"]  # default: the path-filtered Windows/macOS matrix
informational_checks = []                      # e.g. ["Build (windows*"] — `*` wildcards, case-insensitive
```

`informational_workflows` matches the *workflow* name (the Windows/macOS jobs
share PR CI's `Build (…)` job name, so the workflow is the discriminator);
`informational_checks` matches the check name. An explicit `[]` disables that
list. The rows come from the forge: GitHub `gh pr checks --json` (with the
branch-protection required set), GitLab the newest pipeline's jobs (no
required-check concept, so only the allow-list applies); pure-git has no rows
and keeps the coarse verdict.

### Advisory harvest in the drain (`[harvest] gate`)

After the review gates pass and before the merge, the drain runs `aida harvest`
in **propose-only** mode on the PR diff: the agent proposes the observable,
non-obvious facts the change established, the `[harvest]` filter applies, and
whatever survives is written to `.aida/harvest/<spec>-drain-<id>.json` with an
`[aida:harvest]` ledger comment on the spec and a brief for the advisor (so it
shows in `aida awaiting`). Nothing lands on the spec unattended — a human or
the advisor confirms later with `aida harvest <SPEC> --from <file>`. The step
is never a phase and never fails the run: an agent error is logged and the
drain merges as usual.

```toml
[harvest]
gate = "advisory"   # default; "off" disables the step in drains
docs_only_mode = "strict" # default; "off" keeps doc restatements eligible
```

With `docs_only_mode = "strict"`, a diff whose changed paths are all Markdown
documentation still runs the advisory harvest step, but facts merely restated
from its added prose (or copied verbatim from an existing spec's acceptance
criteria) are filtered as `restated from docs` and recorded that way in the
ledger. A genuinely new decision introduced by documentation remains eligible.

Per spec, the `lifecycle:no-harvest` tag skips it (the other short-circuit tags
do not imply it; `lifecycle:trivial` still harvests).

Batch and `nextN` drains have a small in-flight window:

```toml
[drain]
pipeline_depth = 1
```

The default is depth `1`, sourced from `default_pipeline_depth()` in
`aida-cli-lib/src/drain_state.rs`, and keeps the historical strictly sequential
behavior. Set depth `2` to opt in: once spec A has opened a PR and moved into
CI/review/merge, the drain may start spec B's implementer in a separate
worktree instead of idling through A's remote waits. Values above `3` are
clamped to `3`; wider fan-out remains the job of burndown-style concurrency,
not the single-drain loop.

ADR-27 records the original depth `2` decision from when pipeline depth was
state only and did not control scheduling. STORY-1091 made the setting live and
changed the safe default to `1`; ADR-27 remains the historical decision rather
than the current default. trace:BUG-1585

The merge side stays serial and ordered. Even when two specs are in flight,
phase 4-6 for the later spec waits behind any earlier spec's merge/pull/build
window, so main receives one PR at a time and the Done → Completed auto-bump
observes one landed change at a time. `aida drain status` reports this as
`N merged, K in flight` instead of the older single `spec N of M` line.
trace:STORY-1041

### Tuning model per seat

Headless launches resolve model and reasoning effort from the vendor plus the
AIDA seat. The seat names are the role names AIDA already uses:
`implementer`, `reviewer`, `advisor`, `integrator`, `product`, and `trivial`.

```toml
[agents.claude]
model = "opus"        # default for any seat not listed; unset = vendor default
tiers = ["haiku", "sonnet", "opus"]

[agents.claude.seats]
implementer = { model = "sonnet", effort = "medium" }
reviewer    = { model = "opus",   effort = "high" }
advisor     = { model = "opus",   effort = "high" }

[agents.codex]
model = "gpt-5-codex"

[agents.codex.seats]
implementer = { model = "gpt-5-codex", effort = "medium" }
reviewer    = { model = "gpt-5.1",     effort = "high" }
```

Resolution order is:

1. `AIDA_AGENT_MODEL` / `AIDA_AGENT_EFFORT` for the current process.
2. Project `.aida/agents.toml`.
3. Project `.aida/config.toml`.
4. User `~/.aida/agents.toml`.
5. Vendor default, by omitting the native flag.

Within the selected file, `[agents.<vendor>.seats].<seat>.model` overrides
`[agents.<vendor>].model`; `effort` is seat-scoped. Empty strings deliberately
select the vendor default for that field. Claude and AGY receive
`--effort <level>`; Codex receives `-c model_reasoning_effort=<level>`.

Transient retry attempt 2 escalates to the next configured
`[agents.<vendor>] tiers` entry by default. For example, an implementer seated
on `sonnet` moves to `opus` when the tier list is `["haiku", "sonnet",
"opus"]`. If the current model is missing from the list, or already the last
tier, AIDA records the retry without changing the model. Disable this with:

```toml
[drain]
retry_escalate_model = false
```
trace:STORY-1033 | ai:codex

> **This serial engine is the vendor-agnostic drain.** `aida queue work
> --auto-complete` drives one spec at a time and the **orchestrator** owns the
> drive, so it runs under any vendor (Codex, Cursor, Amp, a bare `claude -p`
> loop), not just Claude Code. The *parallel* alternative — `/aida-burndown`,
> which fans out worktree-isolated implementer subagents over a whole ready set
> — is **Claude-Code-harness-only**: it depends on the harness's native
> subagent fan-out (`Agent(isolation: "worktree")`), a primitive no other
> vendor exposes. Non-Claude vendors therefore drain the *serial* way described
> here. See `docs/aida/discipline/autonomous-burndown.md` for the fan-out path
> and the Claude-only caveat in full.

### Why a shelved spec can start again after re-queue

`aida queue add` does not launch work by itself. It only makes a spec visible
to the queue again. A relaunch happens when an already-running drain runner
polls the queue and starts a new pass.

The built-in resilient runner is `scripts/drain-loop.sh`: its main loop
(`scripts/drain-loop.sh:57`) checks whether the active role has queued work,
then runs `aida queue work "next${CHUNK}" --auto-complete …`
(`scripts/drain-loop.sh:82`). Inside AIDA, that enters the `nextN` path
(`aida-cli-lib/src/lib.rs:73579`, `handle_auto_complete_next_n`), whose
`RealNextNDriver::next_head` (`aida-cli-lib/src/lib.rs:73523`) re-resolves the
queue head on each iteration and whose `RealNextNDriver::run_spec`
(`aida-cli-lib/src/lib.rs:73536`) calls `run_auto_complete` for the selected
spec. That is why a re-added shelved item can appear seconds later as a fresh
single-spec `aida queue work SPEC --auto-complete` run in process listings,
events, and `~/.aida/auto-complete.jsonl`.

If no drain runner or wrapper is active, a re-added shelved spec stays queued
until someone explicitly runs `aida queue work`, `aida queue work nextN
--auto-complete`, a batch drain, or an equivalent wrapper.
trace:TASK-1201 | ai:codex

## The three autonomy modes

For the short map across all autonomy surfaces — headless cron, live
`/aida-solo`, `--zen`, and the `[intake]` / `[autopilot]` / risk knobs — run
`aida autonomy` or see [`docs/aida-power-features.md`](aida-power-features.md#the-autonomy-ladder-in-one-command).

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

Before an unattended window, run `aida burndown readiness --hours 20 --lanes 4 --specs 20`.
It reports the repo/build/temp filesystems, available memory, and the exact
caller binary (embedded build SHA and mtime), and refuses when disk headroom is
below the workload-scaled threshold. `aida burndown run` performs the same hard
preflight automatically immediately before launch.
<!-- trace:TASK-1298 | ai:codex -->

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
| `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=<ms>` | AIDA → every headless child | the bounded turn-end background-wait ceiling, set by the LAUNCHER so a hand-typed launch behaves identically to a daemonized one (TASK-1169) |

### Checking whether a drain is alive

Use `aida ps` or `aida burndown status`; for low-level diagnosis, inspect
`.aida/drain.lock`. The lock records both the holder PID and its kernel process
start time, and AIDA validates the pair. Do not use `pgrep` or command-name
matching: wrapper command lines can contain the search text, launch paths can
change `argv[0]`, and a PID can be recycled. None of those name-based signals
prove that the process which acquired the lease is still running.

### Integration waits belong to the launcher, not to an agent turn (TASK-1169)

A headless `claude -p` session reaps its background tasks when the turn ends,
so a "background merge watch" a drain promises dies with the session — that was
BUG-755, which stranded three PRs open and unwatched. The obvious repair —
block in the *foreground* instead — does not hold either: a foreground tool
call is capped at about ten minutes, and a cross-platform CI run is longer than
that. **Neither agent-side shape can hold a long integration wait.**

So the wait moved out of the agent entirely. After each headless session exits,
`aida burndown run` — the long-lived process that already holds the drain lock
— probes the open wave PRs and integrates them **itself, in Rust**:

1. Blocks on each PR's CI using the same re-arming idle timer + absolute ceiling
   the orchestrator uses. Configure durable bounds in `.aida/config.toml`;
   the environment variables override them for one launch:

   ```toml
   [drain]
   ci_idle = "20m"      # default; above the measured 705s GitHub CI p95
   ci_absolute = "90m"  # hard backstop even while a check remains in flight
   ```

   `AIDA_WORKER_CI_IDLE` and `AIDA_WORKER_CI_ABSOLUTE` are the corresponding
   per-process overrides. A queued or running required check re-arms the idle
   timer on every poll; the absolute ceiling remains the bound for a hung run.
   trace:BUG-1275 | ai:codex
2. Squash-merges the clean ones and runs `aida pull` so the Done → Completed
   auto-bump fires. Every existing gate still applies: a `RequestChanges`
   review, red CI, or a merge conflict is never merged over, and a spec whose
   `execution_mode` is not `drain` is **held** for a human (BUG-727).
3. Parks anything else `NeedsAttention` with a recorded reason and files a
   finding — the PR is left **open** and untouched. Nothing is terminated and
   no work is orphaned without a record.
4. Only then relaunches an agent turn, and only for work that genuinely needs
   one: blessed specs still unstarted.

Two consequences worth knowing. The harness background-wait ceiling is now
irrelevant to whether integration completes — it is defense-in-depth, not the
mechanism. And waiting costs **zero tokens**: a Rust poll loop replaced an LLM
turn spent watching CI.

The ceiling AIDA sets is bounded and generous (45 min by default), never the
old `=0` wait-forever stopgap: a real ceiling that rarely fires beats no
ceiling, because wait-forever means a genuinely wedged task hangs the drain
until someone notices. Tune it with `[burndown] bg_wait_ceiling_ms` or
`AIDA_BURNDOWN_BG_WAIT_CEILING_MS`; values below the floor clamp up.

Triage what parked with `aida findings list` (the rows are tagged
`kind:IntegrationWaitParked`).

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
  events: 412 seen · 397 benign-absorbed (96%) · 15 actionable
  findings to triage: 1 — `aida findings list`
```

The **events** line is the empirical read-out of the event-driven supervision
lever (STORY-712): how many state-change events the drain emitted to
`.aida/events.jsonl` during its window, how many of those the cheap classifier
**absorbed silently** (costing a supervising LLM nothing), and how many it
would have **surfaced as a wake**. The split is computed against the same
`is_actionable` predicate `aida watch` classifies each line with, so the ratio
means exactly what the wake behaviour does. Two honesty guards: a drain with no
readable stream prints `events: none recorded for this drain` rather than a
fabricated `0%`, and a window the stream rotated inside is labelled
`partial window` so the ratio is never read as covering the whole drain.

**Watching a live drain by hand.** `aida watch` is silent by design — it prints
only the wake lines a machine consumer needs. When you want to *see the drain
think*, run `aida watch --verbose`: it implies `--all` (every event, not just
the actionable ones) and stamps each line with the event's local wall-clock
time, the run correlation id, and the event's raw payload — the fields the
one-line hint elides (a shelve's phase and failure kind, a bake-off's
per-candidate scores, a PR number):

```
17:04:31.128 [9f21c0aa]      phase-entered STORY-1 — phase 2 (ci)  {"event":"PhaseEntered","idx":2,"slug":"ci"}
17:07:58.902 [9f21c0aa] WAKE spec-shelved STORY-1 — shelved at ci (ci-red)  {"event":"SpecShelved","phase":"ci","kind":"ci-red"}
```

The `WAKE` marker keeps its place between the prefix and the hint, so
`aida watch --verbose | grep WAKE` still selects exactly the actionable lines,
and the header goes to stderr — piping the feed stays clean. Pair it with
`--backlog` (replay the history first) or `--once` (classify what is already
there and exit) when you are debugging a drain after the fact.

**Determining whether a spec finished.** Treat terminality as a property of
the event kind, never of the event's position in `events.jsonl`. Select the
newest event for the spec whose kind is terminal (`PrMerged`, `SpecCompleted`,
or a genuine parked/failed terminal); later `PhaseEntered` and `PhaseDonePr`
records are expected because pull and build continue after merge. A
`SpecCompleted` record carries the merge commit, PR number when known, and the
path that closed it. `RunCompleted` separately records that the trailing pull
and build phases finished. Events written before the BUG-1286 change were not
backfilled, so absence of a terminal record in an older stream means
"unknown", not "still in flight".

### Product-role nudge loop

Until the permanent re-drive supervisor lands, run the nudge from a shell-side
scheduler that does not wake a model when nothing needs judgment:

```bash
while sleep 1800; do aida supervise nudge; done
```

Do not wrap this command in model-side `CronCreate`, `/loop`, or
`ScheduleWakeup`; the command is cheap, but reloading a long-lived seat context
is not. <!-- trace:BUG-1589 | ai:codex -->

`aida supervise nudge` reads `.aida/events.jsonl`, finds transiently parked
specs, and sends one urgent mailbox request to a live advisor naming each spec
and its move command (`aida queue work <id> --resume`). It is seat-safe: it does
not take leases, drive queue work, open PRs, or merge. If no advisor is live, it
falls back to the operator-notification lane (`aida notify check`, STORY-1029)
instead of sending mailbox messages into an empty room. Repeated nudges are
deduped until a stuck spec changes state or the interval elapses.

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

Token totals in records written before 2026-09-22 used `0` both for a genuine
measured zero and for failed or absent collection. Historical sums are therefore
lower bounds, not totals. New records carry `token_measurement`, `vendor`, and
`run_key`; when collection cannot establish an exact total,
`token_measurement` is `unknown` and the token fields are JSON `null`.
// trace:BUG-1418 | ai:codex

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

### Garbage-collecting stale drain orders

A hand-staged overnight plan that never ran (or only partially ran) leaves
`drain <SPEC-ID> …` lines behind after the target specs ship — a latent
landmine: the next `aida-worker` run would fire unattended drains against
finished work. Sweep them:

```bash
aida worker gc --dry-run           # preview: which drain lines target an
                                   # archived / Completed / Rejected spec
aida worker gc                     # prune them; everything else survives
```

Only spec-targeted drain lines with a finished target are pruned. Bare
drains, batch-scoped drains, comments, blank lines, and control directives
(`pause` / `exit`) are kept verbatim, in order. Sibling of `aida queue gc`
(which prunes dead routed queue entries).

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

## Recovery: when a drive dies after pushing

Sometimes a headless drive gets the important part done — commits exist on a
spec branch — but exits before opening or handing off the PR. That state used
to be easy to miss because there may be no live lease left and no PR for the
review queue to see.

Run:

```bash
aida awaiting
```

Look for **Unshipped work** rows. Each row names the spec, branch, commit count,
age, and one recovery command. The same detector is included in:

```bash
aida awaiting --notice
aida session reap
```

The notice path is local/cache-only and skips the forge check, so it reports a
compact `unshipped:N` count when it sees branch-shaped work. The full
`aida awaiting` path checks for open PRs and suppresses branches that already
have one. `aida session reap` reports the same set before cleanup and never
removes a worktree/branch that still carries unshipped work.

Typical recovery:

```bash
aida pr ship story-1043
```

If the work is remote-only, first recreate a local branch from the
remote-tracking ref, then ship it:

```bash
git switch -c story-1043 origin/story-1043
aida pr ship story-1043
```

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

A shelved spec carries the same stored `NeedsAttention` status as a punt, plus
a **`FailureReason`** sibling to `AttentionReason`. Human-facing surfaces render
these as **Shelved (`kind`)** rather than **Needs Decision**, so mechanical
rework does not count as a decision escalation:

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
each batch. The same cap applies to a queue-wide `--drain` with no batch.

The drain stops as soon as the Nth shelve lands, before it starts another
spec: with `--max-failures 1`, the first shelved failure ends the drain
(exit `3`) and nothing else is dispatched. A member already running in a
pipelined drain when the budget runs out is not interrupted.

The cap counts shelve **events**, not distinct specs. If a spec shelves, is
requeued (by a human, an advisor, or an automatic in-drain retry), and shelves
again in the same drain, it spends the budget twice. A second failure after
triage is more evidence that something is wrong, and the cap is the drain's
only automatic circuit breaker: an advisor agent can requeue a shelve that was
never escalated, so a spec counted only once could otherwise fail without
limit. The drain summary still lists each spec once, under its final
disposition. Requeueing a spec never refunds or resets the budget of a
running drain, and a drain that already stopped on a spent budget stays
stopped. <!-- trace:STORY-1429 | ai:claude -->

### Dependency-aware skip

The skip falls out of the pickability gate (STORY-333). When B is
shelved, B is no longer `Completed`, so any member with `BlockedBy → B`
is reported as `UnsatisfiedBlocker` by `pickability` and dropped on the
next head pickup. Both the batch resolver and the queue-wide `--drain`
head resolver apply this gate, and they re-read the store on every pick,
so a blocker shelved by the previous member is seen immediately. Only
`Completed` satisfies a `BlockedBy` edge: `Done`, `Needs Attention`, or a
pushed branch do not. An edge whose target cannot be resolved also counts
as blocked. The summary surfaces both the skipped member and why
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
aida rework                              # triage every parked spec, one key each
aida rework TASK-99                      # requeue one spec (to Approved, back on the queue)
aida findings list                       # list punts and failures, with the requeue hint
aida show TASK-99                        # detail on a shelved spec
aida edit TASK-99 --status rejected      # drop (was wrong direction)
```

Bare `aida rework` at a terminal walks the parked specs. Each one shows what
a requeue would do before you take it: the resulting status, the queue it
lands on, any dependency it will still wait on, and whether a `needs-human`
escalation keeps it parked. Then it takes one key: `[r]` requeue, `[s]` skip,
`[o]` show, `[q]` quit. When the spec has an open decision, `[r]` is not
offered; `[d]` hands that one spec to `aida decide` and then comes back to it.
The resulting status is never a flag you pass, so it cannot be wrong. Without
a terminal, `aida rework` prints the requeue command for each parked spec and
exits 0. `aida findings list` never prompts; bare `aida findings` offers the
loop when specs are parked.

The drain does not requeue triaged specs on its own. The decision to resume
belongs to someone who can see the findings: a human at a terminal, or the
advisor seat. Only a human at a terminal can resume a spec an advisor
escalated with `needs-human`. A requeued spec is Approved and back on its
queue route. A running drain picks it up on its next head pick, and the
requeue says whether a drain is running (it never starts one). Requeue does
not bypass the dependency gate.

Every way out of `NeedsAttention` (`aida rework`, `aida edit --status`, the
`queue_rework` MCP tool, the re-drive supervisor) goes through one
transition, applied to that one spec. The status is read again just before
the single-spec write, so a spec that moved in the meantime is left alone. It clears `attention_reason`,
`failure_reason` and the drain's parking tag, writes one audit note that
carries the triage reason, and records a `SpecRequeued` event. `aida rework`,
the MCP tool and `aida edit --status` refuse while another session holds a
live claim on the spec, `--force` included, and they refuse when a claim that
could be on the spec cannot be read. The punt
ledger entry stays; it's history. <!-- trace:STORY-1429 | ai:claude -->

## Seat jobs — periodic duties per seat (STORY-1226)

The `[schedule]` registry (`aida schedule`, alias `aida cron`) gives every seat
its recurring duties without depending on any one session staying alive.
A **substrate** job (`command = "session reap"`, `every = "30m"`) needs no LLM
and is run by `aida schedule tick`; a **seat** job (`prompt = "triage the
mailbox"`, `seats = ["advisor"]`, `every` / `on = ["MailReceived"]` /
`when = "…"`) is *never executed by the scheduler* — it becomes due and is
delivered to whoever holds the seat: the `aida awaiting --notice` per-turn
line, a `DUE JOBS` block leading the pickup prompt, and the `## Due Jobs`
section of `aida agent new`'s launch context. That delivery is identical for
Claude, Codex and Antigravity (one registry, one `aida schedule due` view);
the seat reports each run with `aida schedule done <job>`, which ledgers who
ran it and when on the `aida-store` branch. A seat job is advice to the seat,
not an automatic action: it never launches a drain or merges. Unattended
execution, `aida schedule install`, and cold-booting a headless seat for an
overdue seat job (at most once per hour per seat) are the scheduler tick's
business — STORY-1218 — which consults this same registry. Full reference:
`docs/cli/03-work-autonomy.md` (`aida schedule`).

## Night shift — `aida shift` (STORY-1218)

The night shift keeps drain waves moving when no seat is awake. It is a
deterministic, LLM-free check (`aida shift tick`) registered as the
`night-shift` substrate job of `aida schedule tick`, so it runs on whichever
scheduler driver this repo has: a systemd user timer (`aida shift install
--systemd-user`, Linux; fires 1 minute after it is started, 2 minutes after
boot, then 10 minutes after each tick finishes) or a crontab entry (`aida
shift install --cron`; every 15 minutes, which caps the job's own
`every = "10m"`). It is **off unless enabled for this clone**, and the switch
never lives in committed config.

### Scheduler driver: systemd timer or crontab

Each repo should have exactly one driver. `aida shift install --systemd-user`
(or `aida schedule install-systemd`) writes
`~/.config/systemd/user/aida-tick-<hash>.{service,timer}`, enables the timer
and checks `systemctl --user is-enabled`; only after that check passes does it
remove this repo's crontab lines, including old entries written before the
marker comment existed. It prints each removed line. Commented-out lines and
other repos' lines are never touched. `--cron` (or `aida schedule
install-cron`) works the other way round: it writes the crontab entry, reads
it back, and only then disables this repo's timer. If a step fails after the
new driver is in place, both drivers remain. That is harmless, because the
tick lock stops two ticks overlapping, and `aida doctor` reports it. The
install never leaves the repo with no driver.

Unit details worth knowing:

- `KillMode=process`: the wave the tick launches stays in the tick service's
  cgroup, and without this setting systemd would kill the wave when the tick
  exits. With it, the journal logs a line like `Unit process <pid> (aida)
  remains running after unit stopped` or `Found left-over process <pid>
  (aida) in control group while starting unit` for a running wave. That is
  expected and not an error.
- No memory or CPU limits are set, because they would also limit the wave.
  `TimeoutStartSec=15min` stops a stuck tick.
- Removing the systemd driver only disables the timer. A tick that is running
  finishes, and a wave is never killed. A unit file is deleted only if it
  contains aida's marker comment, and a file with the same name that aida did
  not write is never overwritten.
- If linger is off for your user, the timer stops when you log out. The
  installer tells you to run `loginctl enable-linger`; it never uses sudo.
- Output goes to the journal:
  `journalctl --user -u aida-tick-<hash>.service`. See the next run with
  `systemctl --user list-timers`.
- Installing either driver needs a person at a terminal who answers yes, as
  `aida shift enable` does.

What one tick does, in order:

1. Takes `.aida/shift.lock` (a second concurrent tick is a no-op).
2. Reaps finished sessions (the `aida session reap --yes` predicate; a live
   process is never touched).
3. Settles the previous shift wave: when its pid is dead, a `QueueDrained`
   after the launch with shipped + shelved > 0 is progress; anything else,
   including no `QueueDrained` at all, is zero progress.
   <!-- trace:TASK-1492 | ai:claude -->
   A breaker trip is also sent to the operator through `aida notify` (rule
   `shift-breaker`).
4. **Re-drive (opt-in, off by default).** Only when this clone's local layer
   sets `redrive = true` (see "Re-drive" below). Re-queued specs go to the
   head of the queue and join this tick's wave.
5. Evaluates every guard (see `aida shift tick --dry-run`); any failure means
   no launch and exit 0.
6. Records the launch intent in `.aida/shift-state.json`, tags the next
   explicit-`drain` queue slice `batch:shift-YYYYMMDD-HHMM` (replacing any
   older shift tag on those specs), spawns one detached
   `aida queue work --batch … --auto-complete --no-human=both --escalate-blocks
   --role implementer --max 6 --max-iterations 6 --max-failures 2
   --max-tokens T --max-runtime 3h` in its own session with its output in
   `.aida/shift-wave-<stamp>.log`, and records the pid. A tick killed between
   tagging and spawning leaves an intent the next tick reuses.
7. **Mail latency.** For each mail recipient, the age of the oldest unread
   message (the same local + canonical mailbox read and read-watermarks as the
   `mail.oldest_unread_age` schedule predicate). Above `[shift] mail_latency`
   (default `30m`) the operator is notified through `aida notify` (rule
   `mail-latency`), once per episode per recipient; the episode re-arms when
   that recipient's oldest unread age drops back under the threshold. It
   never writes to the mailbox or to a chat. `[notify]`'s own `min_interval`
   and quiet hours still apply, and with no `[notify] command` configured
   nothing is sent and no episode is opened. Skipped past the tick deadline.
8. Emits one `ShiftTick` event only when it acted or its refusing-guard set
   changed; it names re-queued specs (`redriven`), capped parks
   (`reclassified`) and mail escalations (`mail_escalated`). It wakes a
   supervisor only for a breaker trip or an escalation (a capped park also
   emits its own `ReclassifiedNeedsHuman`, which does wake one).

### Re-drive (opt-in)
<!-- trace:TASK-1492 | ai:claude -->

The tick can re-drive transient parks itself, the same decision `aida
supervise` makes, but it is **off by default** (ADR-26): enabling the shift
does not enable it, and `aida shift tick --dry-run` prints `re-drive: off
(ADR-26 default)`. Turn it on for this clone only after watching real parks,
by adding `redrive = true` next to `enabled = true` under this repo's
`[repo."<path>"]` table in `~/.aida/shift-local.toml`. A `redrive` key in the
committed `.aida/config.toml` is ignored.

When on, and only while no drain is running (no live lock in this clone, no
other clone draining, no shift wave still alive) and the no-progress breaker
is closed:

- It considers only parks whose spec has an explicit `execution_mode =
  drain`, is not keystone-class and has no live merge hold. A park with an
  attention reason, a `needs-human` tag, or a non-transient failure
  (`ci-red`, `request-changes`, …) is left for a human.
- It keeps the ADR-26 cap: at most 3 supervised re-drives per spec, waiting
  2m / 8m / 30m before each, counted from the `SpecReDriven` events in
  `.aida/events.jsonl` **and** its rotated archive `.aida/events.jsonl.1`,
  so a rotation never resets the count. At most `[shift]
  max_redrives_per_tick` (default 3) per tick.
- A re-driven spec goes back to Approved (`SpecReDriven`, `SpecRequeued`)
  and is put at the **head** of the implementer queue, oldest-parked first,
  so the wave actually sees it. It is never force-claimed; a spec that cannot
  be claimed without force parks again for a human.
- A spec at the cap is tagged `needs-human`, `ReclassifiedNeedsHuman` is
  emitted and a cap finding is filed, so it never sits silently parked.
- `redrive-evidence` fails closed — no re-drive, no reclassification that
  tick — when `AIDA_EVENTS_DISABLE` is set in the tick's environment or the
  event stream cannot be read. A held re-drive never blocks the wave launch.

`aida shift tick --dry-run` lists every transient park with its attempt count
and what the tick would do with it.

Safety floors that hold in every configuration:

- Never launches over a live drain lock, in this clone or another one (the
  shared drain claim on `aida-store`). A stale lock is reported and left for
  the wave's own acquire to reclaim.
- Only explicit `execution_mode = drain` specs are selected; drive, guided,
  operator and decide specs, specs with no mode, keystone-class specs and
  specs under a merge hold are never launched. A configured `[shift] batch`
  with any such member refuses, naming it.
- Never passes `--force-claim`, `--steal` or `--force`; the wave's environment
  drops `AIDA_DRAIN_FORCE`, `AIDA_DRAIN_BORROW`, `AIDA_DRAIN_LOCK_STALE_SECS`,
  `AIDA_EVENTS_DISABLE` and `AIDA_SCHEDULE_CHILD`. The tick never sets
  `AIDA_NO_HUMAN_ACKNOWLEDGED`; it refuses until `aida no-human acknowledge`
  has been run.
- Budget: refuses at or above 80% of the daily token budget (the runaway-seat
  watchdog's, 6B by default), and refuses when the watchdog's aggregates are
  missing, more than an hour old, degraded or unknown, or blind to the wave's
  vendor (Codex spend is not measured; a local
  `allow_uncovered_vendors = ["codex"]` overrides that one case). Each wave is
  also capped by `--max-tokens`, `--max-iterations` and `--max-runtime`.
- Circuit breakers in `.aida/shift-state.json`: at most 8 waves per 24h; two
  consecutive zero-progress waves stop launches until `aida shift resume` or a
  change to the queue; a spec that rode two shift waves in 24h without
  finishing is excluded and reported once. An unreadable state file blocks
  launches and fails the job.
- Load, memory and disk headroom must be readable and within bounds.

`max_failures` defaults to 2 for shift waves. That is a deliberate choice,
not a measured one: the SPIKE-82 window never ended a wave by exhausting its
failure cap. Re-derive it from `ShiftTick` / `QueueDrained` outcomes after a
few shift nights.

### Runbook

- [ ] Groom the implementer queue: set `execution_mode = drain` explicitly on
  every spec that may run unattended; leave everything else in another mode.
- [ ] `aida no-human acknowledge` (once per machine).
- [ ] Confirm the `watchdog` job is enabled and has run in the last hour
  (`aida schedule status`); the shift refuses without its evidence.
- [ ] `aida shift install --systemd-user` (Linux) or `aida shift install
  --cron` at your own terminal. It enables the shift and installs that
  driver in one step, and removes this repo's other driver after the new one
  is verified. It refuses in an agent session or without a TTY, and asks y/N.
  It writes `~/.aida/shift-local.toml` for this clone and registers the
  `night-shift` job. If a driver is already installed, `aida shift enable`
  alone does the enabling part. `aida shift status` names the driver, and
  says "both installed" if there are two.
- [ ] **Preflight:** `aida shift tick --dry-run`. Read every `FAIL` line, the
  exact wave command and the specs it would include. It writes nothing.
- [ ] Optional: configure `[notify] command` (`aida notify test`) so mail
  latency and a breaker trip reach you. Optional, and only after watching real
  parks: `redrive = true` in the local layer (see "Re-drive").
- [ ] In the morning: `aida shift status`, `aida history events --kind
  shift-tick`, then triage shelves and escalations as usual. The tick never
  merges, never clears a merge hold and never approves work — held PRs wait
  for the morning gate.
- [ ] `aida shift disable` to stop. A wave already running finishes its
  current spec; stop it the usual way if it must end now.

Not in this cut: headless cold-boot of overdue seat jobs.

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
