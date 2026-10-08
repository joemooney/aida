<!-- trace:TASK-0422 -->

# Codex defer/resume replacement for AIDA's escalation loop

Date: 2026-06-25
Specs: TASK-0422 (child of EPIC-0419, Claude-to-Codex migration readiness)
Status: Design -- for operator review (DESIGN-ONLY; no production code in this change)
Complexity: Medium (one trait method + one launch builder; the substrate already exists)

> Generic, cross-vendor framing. "The agent" below means any headless coding
> CLI AIDA drives (Claude, Codex, or a future vendor). The design keeps the
> approval/escalation invariant in AIDA's durable substrate and treats each
> vendor's resume primitive as a pluggable accelerator, not a dependency.

## 1. The problem, stated against the real surface

AIDA's `--no-human=both` autonomous drain has a three-tier escalation cascade
(STORY-306, `docs/autonomous-drain.md`): a headless implementer that hits a
design-fork it cannot safely resolve **punts** (parks the spec
`NeedsAttention`, drops a durable record), the orchestrator spawns a headless
**advisor** that either resolves the fork or escalates it to a human, and on a
resolve the orchestrator **resumes the exact punted implementer session** with
the advisor's answer so the drain continues with a decided call rather than a
cold re-run.

Only the last step is vendor-coupled. For Claude that resume is
`claude -p --resume <session-id>` (the `resume_implementer` trait method in
`aida-cli/src/auto_complete.rs`, default impl errors; `RealPhaseDriver`
overrides it to drive a real `claude --resume`). The reference doc
`docs/agents/session-communication.md` documents Claude's
`permissionDecision: "defer"` -> `stop_reason: "tool_deferred"` ->
`claude -p --resume` primitive as Pattern 3.

Codex (verified 2026-06-24, OpenAI Codex CLI docs) has **no** tool-call
`defer`: there is no way to pause one pending tool call and resume that exact
call. It does have **session-level resume** (`codex exec resume <SESSION_ID>`
/ `codex exec resume --last [PROMPT]`, sessions persisted as JSONL under
`~/.codex/sessions/YYYY/MM/DD/`). So the question this design answers: how does
the orchestrator's punt -> advise -> resume loop work for a Codex implementer
that cannot be `defer`-resumed at the tool-call level?

## 2. The key finding: AIDA never used `defer`

Read against the shipped hooks (confirmed in
`docs/agents/porting-claude-code-to-codex.md`, "What AIDA Actually Depends On"):
**none of AIDA's shipped hooks emit `permissionDecision`, `defer`, or
`continue: false`.** The `defer` pattern lives in `session-communication.md`
purely as a reference for what Claude *can* do. AIDA's actual escalation is
substrate-based and already vendor-neutral:

- The punt itself is a status flip (`NeedsAttention`) plus a durable ledger
  append (`.aida/punts.jsonl`, `aida-cli/src/punt.rs::append_to_ledger`) plus
  a signal file (`AIDA_PUNT_SIGNAL_FILE`, `punt::write_signal`). None of that
  is Claude-specific.
- The advisor handshake is **already file-based and agent-agnostic**: the
  orchestrator writes `.aida/punts/<spec>.request.json` (`PuntRequest` -- the
  ultraplan-grade brief), the advisor writes `.aida/punts/<spec>.response.json`
  (`PuntResponse` -- `Resolved { answer }` or `Escalated { reason }`). See
  `punt::write_punt_request` / `read_punt_response`.
- The resolved `answer` is plain text. Nothing about delivering it to the
  implementer requires a tool-call-level resume.

So the migration cost is small and bounded: AIDA loses a capability it
**deliberately never used.** The design below is "make the resume step
pluggable per vendor," not "rebuild the escalation loop."

## 3. Recommended substitute

**Map AIDA's punt -> advise -> resume loop onto Codex's session-level resume,
and make the resume step a vendor-dispatched launch builder.** Concretely:

1. **Punt signalling (unchanged, already portable).** A Codex implementer
   signals a punt the same way a Claude one does: it runs `aida punt`
   (CLI / MCP `post_punt`), which flips the spec to `NeedsAttention`, appends
   the ledger record, and drops the `AIDA_PUNT_SIGNAL_FILE` signal the
   orchestrator polls. The vendor is irrelevant -- the substrate is the
   contract. The one prerequisite is that the Codex implementer's brief tells
   it to call `aida punt` on an unresolvable fork (the same instruction
   `/aida-pickup` carries for Claude); for Codex this rides in the brief /
   `AGENTS.md`, not a Claude skill.

2. **Advisor tier (unchanged).** The orchestrator's `run_advisor` already
   writes a `PuntRequest` file and reads a `PuntResponse` file. The *advisor*
   can itself be any vendor -- already true via the cross-vendor judge work in
   `compete.rs` (`JudgeVendor::Codex`, `codex exec --dangerously-bypass-...`).
   No change needed for the request/response contract.

3. **Resume (the one vendor-coupled step -- make it pluggable).** Today
   `resume_implementer(answer)` is hard-wired to `claude -p --resume`. The
   recommendation:

   - **Codex resolve path: `codex exec resume <session-id> "<answer-prompt>"`.**
     Codex persists the implementer's session, so a true resume is available --
     it is NOT a cold re-run. The advisor's `answer` becomes the follow-up
     prompt, wrapped in a short directive ("Continue the punted work on
     <SPEC>. The design fork you punted was: <question>. The decision is:
     <answer>. Proceed and open the PR."). The implementer's prior turn context
     is preserved by Codex's own session JSONL.
   - **Caveat vs Claude's `defer`:** Codex resumes the *session/turn*, not the
     *exact pending tool call*. Practically this is fine for AIDA's loop --
     the punt already *stopped* the implementer (it parked the spec and exited);
     there was never a live pending tool call to replay. AIDA's resume has
     always been "park, decide out of band, resume with the decision in
     context," which is exactly the session-level shape. The tool-call-level
     `defer` would only matter if AIDA paused *mid-tool-call without exiting* --
     which it does not.

4. **Capture the Codex session id at launch.** The resume needs the
   implementer's session id. For Claude the orchestrator mints `--session-id`
   (a caller-minted UUID, see `session::claude_headless_args`). For Codex the
   orchestrator must record the session id Codex assigns. Two options (open
   question Q1): (a) parse it from `codex exec --json` event output and persist
   it onto the lease / a sidecar; or (b) use `codex exec resume --last` from
   the spec's worktree, relying on "last session in this cwd" rather than an
   explicit id. Option (a) is more robust under concurrent worktrees; option
   (b) is simpler but races if two Codex runs share a session store. Lean: (a).

## 4. Control flow

```
  headless implementer (Codex: `codex exec ...`)
        | hits unresolvable design-fork
        v
  runs `aida punt` -> NeedsAttention + .aida/punts.jsonl + AIDA_PUNT_SIGNAL_FILE
        |                                  (durable, vendor-neutral)
        v
  orchestrator detects punt signal, writes .aida/punts/<spec>.request.json
        |
        v
  headless advisor (any vendor) reads request, writes .aida/punts/<spec>.response.json
        |
   +----+--------------------+
   | Resolved { answer }      | Escalated { reason }
   v                          v
  resume_implementer(answer)  --escalate-blocks (default): park, advance drain
   |  vendor dispatch:        --escalate-defaults: resume with defensible default
   |    claude -> claude -p --resume <sid>
   |    codex  -> codex exec resume <sid> "<answer-prompt>"
   v
  implementer continues, opens PR  (a fresh re-punt is terminal: one round/spec)
```

The boxes that change for Codex are exactly two: the implementer launch (already
exists in `compete.rs`'s adapter table) and the `resume_implementer` dispatch.
Everything between the two dashed substrate files is unchanged.

## 5. What changes in the orchestrator

- **`auto_complete.rs` -- make `resume_implementer` vendor-aware.** Today the
  trait method assumes Claude. Introduce a small `ImplementerVendor` notion on
  the phase context (claude | codex), and have `RealPhaseDriver::resume_implementer`
  branch: Claude keeps `claude -p --resume`; Codex builds
  `codex exec resume <sid> "<answer>"`. The pure argv builder belongs next to
  the existing adapter table in `compete.rs` (extend `vendor_adapter` with a
  `resume_argv(session_id, prompt)` helper) so it is unit-testable without
  spawning.
- **Session-id capture.** Add a field to the phase context / lease for the
  implementer's vendor session id, populated at implementer launch. For Claude
  it is the minted `--session-id`; for Codex it is parsed from `codex exec
  --json` (Q1). `auto_complete.rs::PhaseContext` already carries
  `implementer_session: Option<String>` -- reuse it; only the *producer* differs
  per vendor.
- **Brief / instruction surface.** The "on an unresolvable fork, run
  `aida punt`" instruction must reach a Codex implementer. For Claude it is in
  `/aida-pickup`; for Codex mirror it into the brief body and `AGENTS.md`
  (the Codex-native instruction surface, per the porting doc). No orchestrator
  code change -- a content/scaffolding change.
- **No change** to: the punt ledger, the request/response handshake, the
  `--escalate-blocks` / `--escalate-defaults` flags, the finding-filing path,
  the lease-escalation marker (TASK-358), or the exit-code grid.

## 6. Notification as a side effect of the stopping component

TASK-0422 acceptance calls for documenting notification as a side effect of the
component that stops the run. AIDA already follows this (Pattern 2 in
`session-communication.md`): the component that *makes the stop decision* emits
the alert, not a later hook. Concretely, for Codex this is unchanged because the
stop decision lives in AIDA, not the agent:

- The **punt** is the stop: `aida punt` flips status and writes the ledger; the
  ledger append IS the durable notification. `aida findings list` surfaces it.
- An **escalation** is the stop: the advisor writes `Escalated` and (under
  `--escalate-blocks`) the orchestrator parks the spec and tags it
  `needs-human`; that tag + the `aida findings list` "Advisor decisions" footer
  IS the notification.

There is no Codex-specific notification gap: AIDA never relied on a Claude hook
to notify. Whatever stops the run (punt, escalation, shelve-on-failure) records
durable evidence as its own side effect, and the morning triage reads it from
`aida findings`.

## 7. The honest residual gap

Codex's session resume is real but **coarser** than Claude's `defer`: it cannot
pause and resume a *single in-flight tool call*. AIDA does not need that, because
its punt always *exits* the run rather than freezing a live call. The only place
this would bite is a hypothetical future "approve this exact `git push` mid-turn
without exiting" gate -- which AIDA has deliberately never built (the invariant is
a substrate gate / lease / pre-commit hook, not a live tool-call pause). If such
a gate is ever wanted, it is a Claude-only optimization with a Codex fallback of
"park + resume session," not a portability blocker.

## 8. Open questions for the operator

- **Q1 -- Codex session-id capture.** Parse the id from `codex exec --json`
  events and persist it (robust under concurrent worktrees), or rely on
  `codex exec resume --last` scoped to the spec's worktree cwd (simpler, races
  if the session store is shared)? The `--json` event schema's session-id field
  was flagged uncertain by the Codex-docs probe -- worth a one-command spike
  (`codex exec --json` on a throwaway prompt, inspect the events) before
  committing. Lean: parse-and-persist.
- **Q2 -- Should Codex be a first-class implementer vendor for `queue work
  --no-human`, or stay confined to `aida compete`?** Today `compete.rs` runs
  Codex headless for the bake-off; the drain orchestrator launches only Claude.
  Promoting Codex to a drain implementer is the bulk of STORY-683's surface and
  implies the session-id plumbing above. Confirm scope: full drain-implementer
  parity, or resume-loop design only for now.
- **Q3 -- `--escalate-defaults` on Codex.** The resume-with-default path is
  identical in shape; confirm the operator wants it enabled for Codex drains
  (it ships a defensible default unattended -- the throughput-over-correctness
  knob) or wants Codex drains pinned to `--escalate-blocks` until the resume
  path has soak time.
- **Q4 -- One-round-per-spec rule.** Keep STORY-306's "a fresh re-punt is
  terminal" for Codex (recommended -- same recursive-failure guard), confirmed.

## 9. Related

- `docs/autonomous-drain.md` -- the escalation cascade this design slots into.
- `docs/agents/session-communication.md` -- Claude `ask`/`continue:false`/`defer`
  reference; update its "Codex And Antigravity" section to point here once this
  lands.
- `docs/agents/porting-claude-code-to-codex.md` -- the dependency-surface audit
  that establishes AIDA never used `defer`.
- `aida-cli/src/punt.rs` -- `PuntRequest` / `PuntResponse` handshake (agent-neutral).
- `aida-cli/src/auto_complete.rs` -- `resume_implementer` (the one vendor-coupled
  method).
- `aida-cli/src/compete.rs` -- the working `codex exec` adapter + cross-vendor
  judge; the natural home for a pure `resume_argv` builder.
