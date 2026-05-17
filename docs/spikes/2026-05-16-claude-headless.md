# Spike: Headless Claude (`claude -p`) behaviour with AIDA skills

| | |
|---|---|
| **Spec** | SPIKE-7 |
| **Date** | 2026-05-16 |
| **Status** | Done |
| **Time-box** | 2–4 h (actual: ~1.5 h, full empirical suite) |
| **Gates** | STORY-263 (`aida queue work --auto-complete --no-human`) |
| **Environment** | Claude Code `2.1.143`, default model `claude-opus-4-7` (`[1m]` when AIDA `CLAUDE.md` is in scope), Linux |
| **Empirical cost** | ~$3.80 across ~16 headless invocations |

## TL;DR — verdict

**Headless Claude works for AIDA's skill patterns — but with hard caveats.** Maps to the spike's
decision tree as *"works but with caveats"*: **STORY-263 proceeds, scoped, with a watchdog.**

Three findings dominate:

1. **Headless never hangs on a pause-for-input.** When a skill would normally stop to ask the
   human (`AskUserQuestion`), headless Claude fails *fast and clean* (~10 s, exit 0) — it does
   **not** hang and does **not** silently pick a default. Good news for autonomy: no 30-minute
   timeout is needed to catch a stuck-on-a-question session.
2. **Exit code 0 is a liar.** A run that bailed at a human gate still exits `0` with
   `is_error:false`. The real "did the work actually happen" signal is **`permission_denials`** in
   the final JSON result. A watchdog that trusts the exit code alone will mark abandoned work as
   shipped.
3. **`bypassPermissions` is mandatory.** Under the default permission mode, every approval-gated
   tool (`Write`, `Edit`, `Bash`) is auto-denied headless — the run "succeeds" while doing nothing.

## How to read this doc

Each investigation question (Q1–Q9 from SPIKE-7) has: **finding**, **evidence**, and a
**reproduction command**. The synthesis — the detection model and the STORY-263 recommendation —
is at the end. All test artifacts (PR-62, branch `spike7-headless-test`, TASK-296, STORY-274) were
created and then cleaned up; nothing from this spike remains in the repo except this file.

---

## Q1 — Basic invocation

**Finding:** `claude -p "<prompt>"` runs fully headless, exits `0`, ~2–4 s for a trivial prompt.
Output is the response text only — no banner, no preamble.

```bash
claude -p "Reply with exactly the word: PONG" --permission-mode bypassPermissions --no-session-persistence
# -> PONG   (exit 0, ~4 s)
```

**Gotcha — `--bare` breaks auth.** `--bare` makes Anthropic auth *strictly* `ANTHROPIC_API_KEY` /
`apiKeyHelper` — OAuth and keychain are never read. With only an OAuth login (no `ANTHROPIC_API_KEY`
exported) a `--bare` run fails with `Not logged in · Please run /login`. **Do not use `--bare` for
headless orchestration** unless the orchestrator exports an API key.

---

## Q2 — Tool permissions

**Finding:** Permission mode decides everything; pick wrong and the work silently does not happen.

| `--permission-mode` | `Write`/`Edit` headless | `Bash` headless | `permission_denials` |
|---------------------|-------------------------|-----------------|----------------------|
| `bypassPermissions` | ✅ works | ✅ works | empty |
| `acceptEdits` | ✅ works | ❌ still gated | empty (edits) |
| `default` | ❌ **auto-denied** | ❌ auto-denied | **populated** |

Under `default`, the run still exits `0` with `is_error:false`; Claude politely reports *"the write
needs your permission and it hasn't been granted"* — but the file is never created. The denied call
is recorded in `permission_denials`.

```bash
# default mode: file is NOT created, yet exit 0 / is_error:false
claude -p "Create probe.txt containing OK" --permission-mode default --output-format json
#   -> permission_denials: [{tool_name: "Write", ...}]
# bypassPermissions: file IS created
claude -p "Create probe.txt containing OK" --permission-mode bypassPermissions --output-format json
```

**Implication:** STORY-263's headless launches **must** pass `--permission-mode bypassPermissions`.
`acceptEdits` is insufficient — AIDA skills shell out to `aida`/`git`/`gh` via `Bash`.

---

## Q3 — Skill output

**Finding:** Skills load and execute headless. Embedded `aida` commands run, multi-step workflows
complete, and markdown output (tables, `▶ ⏵ 🚪` glyphs, next-steps) renders into the result text.

- `/aida-status` ran headless: skill resolved, `aida` probes executed, full formatted status report
  produced — `is_error:false`, 1 turn, $0.15.
- `/aida-pickup` and `/aida-pr` (see Test 2) drove their full multi-step workflows and emitted their
  next-steps tables.

**One genuine difference — TTY-gated banners are suppressed.** The `/aida-pr` skill gates its ASCII
banner on `[ -t 1 ]`. Headless stdout is not a TTY, so the **banner does not print**. This is *by
design*, not a bug — the next-steps table (plain markdown) still renders. Any skill output guarded
by a TTY check will be absent headless; AIDA skills already do the right thing by keeping the
load-bearing content out of the TTY-gated block.

```bash
claude -p "/aida-status" --permission-mode bypassPermissions --output-format json
```

---

## Q4 — Mid-task pauses (the design-fork question)

**Finding — the most important one for STORY-263.** When headless Claude wants to ask the human
(`AskUserQuestion`), it does **not** hang and does **not** silently choose:

- The `AskUserQuestion` tool call returns *no selection*.
- It is recorded in `permission_denials` with `tool_name: "AskUserQuestion"`.
- Claude **stops cleanly** and reports it is blocked (*"the question wasn't answered yet… I won't
  proceed"*).
- Wall time: **~10 s** — fast, not a timeout. `is_error:false`, exit `0`, `terminal_reason:completed`.

This held in the real reviewer skill too: headless `/aida-review` reviewed PR-62 fully, posted its
verdict, then hit the merge-decision `AskUserQuestion`, recorded the denial, and **refused to merge**
— exactly the safe behaviour. The "never merge without confirmation" guard is honoured headless.

```bash
claude -p "...use the AskUserQuestion tool to ask me which approach... do not proceed until I answer." \
  --permission-mode bypassPermissions --output-format json
#   -> permission_denials: [{tool_name: "AskUserQuestion", tool_input:{questions:[...]}}]
#   -> exit 0, is_error:false, ~10 s, work NOT done
```

**Consequence:** A pause-for-design-input (per `feedback_pause_for_design_input.md`) becomes, in
headless mode, a *clean early stop with the work undone*. The orchestrator must treat a non-empty
`permission_denials` as **"needs a human" → re-queue / escalate**, because the exit code says
success.

---

## Q5 — Long-running operations

**Finding:** `claude -p` has **no built-in wall-clock timeout.** It runs until the task completes,
the model stops, or `--max-budget-usd` trips. Observed completed runs:

| Run | Wall time | Turns | Outcome |
|-----|-----------|-------|---------|
| `/aida-status` | 15 s | 1 | ✅ completed |
| `/aida-review --pr 62` | 230 s | 18 | ✅ completed |
| `/aida-pickup` (Test 2) | 153 s | 20 | ✅ completed |
| `/aida-pr` (Test 2) | **302 s** | 23 | ✅ completed |

A 5-minute, 23-turn skill workflow completed cleanly with no degradation. There is no `--max-turns`
or wall-clock flag in `claude --help`; the only model-side cap is `--max-budget-usd`.

> **Scoping note (honest):** a deliberate 15–20 min burn was *not* run. The question — "does
> headless time out?" — is already answered: there is no built-in timeout, and a 5-min run proves
> it runs well past any short threshold. A longer run would cost ~$3–5 and yield no new signal.
> **The caller must impose its own timeout** (see watchdog recommendations).

---

## Q6 — Streaming output

**Finding:** Output behaviour depends entirely on `--output-format`:

| Format | Behaviour |
|--------|-----------|
| `text` (default) | Buffers — prints the final result as a single block at exit. **Not monitorable.** |
| `json` | Single JSON object at exit. Rich metadata, but no progress. |
| `stream-json` (`--verbose`) | **Streams** newline-delimited JSON events as they happen — monitorable. |

`stream-json` event arrival is genuinely incremental — observed timestamps spread across the run.
Event types seen: `rate_limit_event`, `system`(`subtype:init` / `subtype:task_notification`),
`assistant`, `user` (tool results), and a final `result`. The `init` event lists `tools`, `model`,
`permissionMode`, `skills`, `mcp_servers` — useful for a watchdog to assert the session started as
intended.

**Implication:** STORY-263's watchdog must launch with `--output-format stream-json --verbose` and
tail the event stream. Plain `-p` gives no liveness signal until exit.

---

## Q7 — Exit codes

**Finding:** Exit codes are consistent and usable, but **insufficient alone** (see Q2/Q4).

| Exit | Cause | JSON `is_error` | JSON `subtype` |
|------|-------|-----------------|----------------|
| `0` | success | `false` | `success` |
| `0` | **bailed at a human gate** (denied tool / unanswered question) | `false` ⚠️ | `success` ⚠️ |
| `1` | unknown CLI flag | — (no JSON) | — |
| `1` | invalid/unauthorised model | `true` | `success` ⚠️ |
| `1` | not logged in (auth) | `true` | `success` ⚠️ |
| `1` | `--max-budget-usd` exceeded | `true` | **`error_max_budget_usd`** |
| `124` | killed by the caller's `timeout` (SIGTERM) | — | — |
| `137` | SIGKILL'd child process (seen on a tool subprocess at kill time) | — | — |

Key traps:

- `subtype` is **not** a reliable success/failure discriminator — auth failures still report
  `subtype:success` with `is_error:true`. It *does* carry a specific code for some errors
  (`error_max_budget_usd`).
- A clean exit `0` does **not** mean the work happened — inspect `permission_denials`.

```bash
# budget cap: distinct, machine-readable failure
claude -p "<multi-step task>" --max-budget-usd 0.05 --output-format json
#   -> exit 1, is_error:true, subtype:"error_max_budget_usd"  (overshoots ~1 turn — checked between turns)
```

---

## Q8 — Verdict file (`/aida-review` headless)

**Finding:** Headless `/aida-review` writes its verdict artifacts correctly:

- Worksheet `.aida/review-prompt-pr-62.md` (gitignored audit record) — written, 2 KB.
- Consolidated verdict comment — **posted to the PR** (`PR-62#issuecomment-…`), with the
  per-spec verdict table (TASK-296 → ✅ PASS, CI green).
- It then stopped at the merge gate (the `AskUserQuestion` of Q4) and **did not merge** — correct.

So the reviewer skill is headless-safe *up to the merge decision*. STORY-263's "headless reviewer"
gets a real verdict + PR comment for free; the merge step needs an explicit mechanism (it cannot be
an `AskUserQuestion`). Note `aida queue work --auto-complete` already owns an autonomous-merge path
— STORY-263's headless reviewer should compose with that rather than re-ask.

---

## Q9 — Resume after a kill

**Finding:** A SIGTERM-killed headless session **is resumable**, and partial work persists.

- Spawned a 5-step file-creation task with `--session-id <uuid>` (persistence on); SIGTERM'd it at
  45 s. `step1.txt` and `step2.txt` (created before the kill) **survived** on disk. The tool call
  in flight at kill time (a `sleep`) was SIGKILL'd (exit 137).
- `claude -p --resume <uuid> "continue"` picked up with **full context** — it correctly reported
  *"step1/step2 created earlier this session before the interruption"* and finished step3–5.

```bash
SID=$(uuidgen)
timeout --signal=TERM 45 claude -p "<multi-step task>" --session-id "$SID" \
  --permission-mode bypassPermissions --output-format stream-json --verbose   # killed mid-run
claude -p --resume "$SID" "Continue where you left off."                       # resumes, exit 0
```

**Nuances:**
- Persistence must be **on** (do *not* pass `--no-session-persistence`) and a fixed `--session-id`
  must be supplied so the orchestrator knows what to resume.
- The step in flight at kill time is **re-run** on resume — resumed steps must be idempotent (a
  half-written file / a partial commit could be re-attempted).

---

## Test 2 — Real `/aida-pickup` → `/aida-pr` chain

A throwaway TASK-296 was created, picked up headless, and shipped through to a real PR — then
cleaned up. It worked, and surfaced two AIDA-specific constraints.

- **`/aida-pickup TASK-296 --auto-first`** — headless: created the file, committed
  (`f35c63f6`), `aida edit --status in-progress` → `aida queue done --yes`. ✅ Done. 153 s, 20 turns.
- **`/aida-pr`** — headless: pushed the branch, opened **PR-62**, auto-filed review story
  STORY-274, posted to the reviewer queue. ✅ 302 s, 23 turns.

**Constraint A — fresh worktrees are not AIDA-functional.** The test ran in a `git worktree` that
had `.aida/config.toml` (tracked) but **no `.aida/cache.db` and no `.aida-store` worktree**. The
`aida-store` orphan branch can only be checked out by one worktree at a time (held by the primary
clone). Every store-touching `aida` command failed there with *"no aida store found"*. The headless
Claude *adapted* — it ran `aida show`/`edit`/`queue done` from the primary clone (the orphan branch
is shared) — but an orchestrator spawning worktrees per task **must provision the store** (or AIDA
needs robust `--project`/store-discovery on every mutating command).

**Constraint B — interactive `aida` prompts stall headless.** `aida queue done` prompts for
confirmation; headless it reads EOF on stdin and **cancels the action** (does not hang forever, but
does not complete). The headless Claude worked around it with `--yes`. Any `aida` command with an
interactive prompt needs a non-interactive flag on the orchestrator's call path.

---

## Synthesis — the detection model

The single most important output of this spike. **A headless run's true outcome is not the exit
code.** Parse the final `result` event (`--output-format stream-json`, last line; or
`--output-format json`) and classify:

```
exit 0  ∧  is_error:false  ∧  permission_denials == []   → SUCCESS — work done
exit 0  ∧  is_error:false  ∧  permission_denials != []   → STALLED — hit a human gate,
                                                            work INCOMPLETE, needs a human
exit 1  ∧  is_error:true   ∧  subtype:error_max_budget_usd → BUDGET — cost cap tripped
exit 1  ∧  is_error:true                                  → ERROR — auth / bad model / API
exit 124                                                  → TIMED OUT — caller's timeout fired
exit 137 / 143                                            → KILLED — SIGKILL / SIGTERM
no `result` event ever emitted                            → CRASHED / STUCK before completion
```

"Stuck" detection: there is **no hang on pause-for-input** (those resolve in ~10 s). A genuinely
stuck run is one that exceeds its expected wall-time *without exiting*. Detect it by tailing the
`stream-json` event stream — **if no event arrives for N minutes, the run is stuck** → SIGTERM it,
then `--resume` or escalate.

## Watchdog / timeout recommendations (for STORY-263 + TASK-294)

1. **Launch flags (mandatory):**
   `--permission-mode bypassPermissions --output-format stream-json --verbose --session-id <uuid>`
   (persistence on — do not disable). Do **not** use `--bare`.
2. **Caller-side timeout.** `claude -p` has no built-in wall-clock limit — wrap every launch in
   `timeout --signal=TERM <budget>`. Size the budget generously (skill workflows run 2–5 min;
   real implementation work can run longer) and treat exit `124` as "timed out → investigate".
3. **Cost circuit-breaker.** `--max-budget-usd <cap>` per launch gives a hard cost ceiling with a
   distinct `subtype:error_max_budget_usd`. Costs are material — see below.
4. **Liveness watchdog.** Tail the `stream-json` stream; no event for N minutes ⇒ stuck ⇒
   SIGTERM + resume/escalate.
5. **Outcome parsing.** Classify with the detection model above. **Never trust exit 0 alone** —
   a non-empty `permission_denials` means the work did not finish; re-queue or escalate to a human.
6. **Resume on kill.** Killed sessions resume cleanly with `--resume <session-id>`; partial work
   persists. Ensure resumed steps are idempotent.

## Cost analysis

| Run | Cost |
|-----|------|
| Trivial prompt (with AIDA `CLAUDE.md` cached as context) | ~$0.12 |
| `/aida-status` (read-only skill) | $0.15 |
| `/aida-pickup` (Test 2) | $0.82 |
| `/aida-pr` (Test 2) | $1.17 |
| `/aida-review --pr 62` | $1.06 |
| Full spike (~16 invocations) | **~$3.80** |

**This is a STORY-263 design input.** A full implementer→reviewer lifecycle is **~$3 in API spend
per spec** at Opus-4.7 prices, before any retries. An overnight drain of a 20-item queue is a
~$60 run. The `--max-budget-usd` cap (per launch) and a model choice knob (`--model sonnet` for
cheaper phases) should be first-class in STORY-263's design, not afterthoughts. Even a trivial
"hi" costs ~$0.12 because the full AIDA `CLAUDE.md` is loaded as cached context every launch.

## Recommendation for STORY-263

**Decision-tree outcome: "works but with caveats" → STORY-263 proceeds, scoped, with a watchdog.**

`--no-human` is *viable* — headless Claude degrades gracefully (fast clean stop, never an infinite
hang) and skills execute correctly. But STORY-263's design must absorb these caveats:

1. **`permission_denials` is the completion signal**, not the exit code. The orchestrator re-queues
   / escalates any run with non-empty denials.
2. **Human gates must be designed out, not relied on.** The reviewer merge gate and any
   `AskUserQuestion`-based skill pause cannot be answered headless — they become clean no-ops.
   STORY-263's headless reviewer composes with the existing `--auto-complete` autonomous-merge
   path instead of asking.
3. **`bypassPermissions` is non-negotiable** for headless launches.
4. **AIDA worktree/store provisioning** (Constraint A) and **non-interactive `aida` flags**
   (Constraint B) must be solved before a worktree-per-task headless drain works unattended.
5. **Cost caps + model selection** belong in the design — ~$3/spec is real money at scale.

A "reduced human interaction" framing (the decision-tree's third branch) is **not** needed —
headless is reliable enough for the full `--no-human` mode, *provided* the orchestrator implements
the detection model. The watchdog is for genuinely-stuck long runs, not for pauses.

## Followups

- STORY-263: scope to the watchdog + detection model above; compose the headless reviewer with
  `--auto-complete`'s autonomous-merge rather than an `AskUserQuestion` merge gate.
- STORY-263 / TASK-294: AIDA worktree store-provisioning (Constraint A) — a headless drain that
  spawns a worktree per task needs the `.aida-store` worktree + `.aida/cache.db` wired up, or
  `aida` mutating commands need reliable store discovery / `--project`.
- STORY-263 / TASK-294: ensure every `aida` command on the orchestrator's headless call path is
  invoked non-interactively (`--yes` etc.) — interactive prompts cancel silently headless
  (Constraint B).
- Consider a `--model` / `--max-budget-usd` knob on the headless orchestrator for cost control.

## Related

- **STORY-263** — `aida queue work --auto-complete --no-human`; this spike gates its acceptance.
- **TASK-294** — `aida-worker` bash MVP; shares the watchdog/timeout need.
- **`feedback_pause_for_design_input.md`** — headless mode converts a design-fork pause into a
  clean early stop with the work undone; the orchestrator must detect and escalate it.
