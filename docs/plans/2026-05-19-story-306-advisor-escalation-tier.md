# Plan: STORY-306 — advisor escalation tier for `--no-human`

Date: 2026-05-19
Specs: STORY-306 (absorbs BUG-241 items 4-5 — the escalation handshake)
Status: Draft
Complexity: ~650 prod LOC, ~320 test LOC, 5-6 commits, risk high

<!--
  GATING: STORY-306's core (the advisor tier) is hard-blocked on STORY-276
  (headless implementer), which is Planned but NOT merged — its own plan is
  docs/plans/2026-05-19-story-276-headless-implementer.md. This plan captures
  the architecture now (the STORY's stated purpose) but the advisor tier is
  implementable only after STORY-276 lands. The ONE exception is Slice 1, the
  reviewer escalation handshake — it needs only the shipped reviewer phase
  (STORY-263), not STORY-276, so it can land first and independently.
-->

## Approach

STORY-276 turns a headless implementer's design-fork into a *recorded punt*:
the spec flips to `NeedsAttention`, the drain advances, a human triages it in
the morning. STORY-306 inserts a middle tier — a **headless advisor** — between
the punt and the human. When phase 1 punts, the orchestrator assembles a rich,
ultraplan-grade payload, spawns a headless advisor (`claude -p`, advisor/dialog
role) with it, and the advisor either **resolves** the fork (the orchestrator
resumes the phase-1 session with the judged answer via `claude -p --resume` and
the drain continues with a *decided* call, not a default guess) or **escalates**
it (a `needs-human` finding is filed and the drain pauses or falls back to the
default per `--escalate-blocks`/`--escalate-defaults`).

The load-bearing constraint is **calibration, not capability**: a fresh advisor
applying judgment unattended is exactly where drain quality silently degrades,
so the advisor's default bias is ESCALATE. The `/aida-advise` skill prompt
enforces a three-type triage — A (recorded principle), B (recorded preference),
C (synthesized in-flight context) — resolving only A and recorded-B and
escalating everything else. Over-resolving a C produces confident-but-wrong
overnight decisions, worse than the safe-default punt. Every advisor decision is
logged to `.aida/punts.jsonl` and (on resolve) left as a spec comment, so a
human can audit overnight calls.

Five new artifacts carry the cascade, all file-based async (the proven
STORY-263/285/329 handshake pattern): a **punt-request** file (orchestrator →
advisor), a **punt-response** file (advisor → orchestrator), the extended
**punt-ledger** record, the `needs-human`-tagged spec, and — for the absorbed
BUG-241 escalation handshake — a verdict file that carries `merge:
escalated-to-human` so the reviewer phase always leaves its handshake artifact.

The advisor tier is **at most one round per spec per drain**: if the advisor's
answer spawns a fresh fork on resume, that re-punt is terminal (STORY-276's
`finish_punted`) — no advisor↔implementer conversation.

### Diagram

```
  Phase 1: run_implementer ──► ImplementerOutcome
       │                          │
   Shipped ──► phases 2-6      Punted(p)
                                   │
                       resolve_punt_via_advisor(p, escalate_mode)
                                   │
                    assemble rich payload ─► run_advisor (claude -p /aida-advise)
                                   │
                 ┌─── Resolved{answer,reasoning} ───┐    Escalated{reason,category}
                 ▼                                  │            │
   resume_implementer(answer)                       │   tag spec `needs-human`,
   (claude -p --resume <phase1 sid>)                │   advisor comment, ledger
                 │                                  │            │
        ┌── Shipped ──► phases 2-6 ✓                 │   ┌─ Blocks ──► finish_escalated
        │                                            │   │            (exit 0, skip 2-6)
        └── Punted(p2) ──► finish_punted              │   └─ Defaults ─► resume_implementer
            (re-punt, terminal, exit 0)               │                 (DEFAULT prompt)
                                                      │                  └─► phases 2-6
                                                      │
  Phase 3: run_reviewer ──► ReviewerOutcome           │   (advisor decision → punts.jsonl,
       │                       │                      │    morning audit: `aida findings`)
   Verdict(Approved) ─► merge  EscalatedToHuman ──────► finish_escalated (exit 0, skip 4-6)
   Verdict(other)    ─► phase-3 failure  (regression-guarded — still fails)
```

## Decisions

- **Decision — the orchestrator assembles the rich payload; the implementer
  just punts well.** The headless implementer's only job is a good `aida punt`
  (category + `--reason` + `--lean`). The orchestrator enriches it — spec
  description, acceptance, parent/child/sibling context, plan brief, comments,
  trace-graph helpers — by reusing the `assemble_ultraplan_prompt` machinery.
  **Rationale**: keeps the STORY-276 `/aida-pickup` skill change near-zero,
  makes the rich-payload logic deterministic Rust unit-testable without a
  session, and the spec/plan/comments are derivable from AIDA metadata anyway —
  only the fork itself needs the implementer's in-session knowledge, and
  `--reason`/`--lean` already capture that. "Recorded user prefs" are read by
  the advisor skill directly from `docs/aida-discipline/` + memory, not
  pre-assembled.
- **Decision — the advisor tier is a conditional branch off phase 1, not a 7th
  phase.** A helper `resolve_punt_via_advisor`, analogous to the existing
  `resolve_phase_failure`. **Rationale**: the six phases are linear; the advisor
  runs only on a punt. Modelling it as a phase would distort the phase index /
  exit-code contract.
- **Decision — resume directly via `claude -p --resume <phase1-session-id>`,
  not through `aida queue work`.** `RealPhaseDriver::run_implementer` (STORY-276)
  mints the phase-1 `--session-id`; `resume_implementer` reuses it. The punt
  left the lease + worktree intact (STORY-276 Risk 7), so the resumed session
  re-enters its own worktree with full context. **Rationale**: `aida queue work`
  has no headless-resume path, and resume *is* the point — the implementer
  keeps the working model it built before punting.
- **Decision — one advisor round per spec per drain.** A re-punt after a resume
  is terminal (`finish_punted`, STORY-276) — no second advisor spawn.
  **Rationale**: matches the STORY's explicit out-of-scope ("one punt, one
  advisor answer; a new fork is a fresh punt, not a conversation") and bounds
  cost + latency.
- **Decision — `--escalate-blocks` is the default; `--escalate-defaults` opts
  in.** `blocks` → leave the spec `NeedsAttention`, file the `needs-human`
  finding, advance the batch (this spec paused for the human). `defaults` →
  resume the implementer told "no judged answer — proceed with your stated lean
  / the defensible default", file a `needs-human` finding for post-hoc morning
  review. **Rationale**: a confident-but-wrong overnight default is worse than a
  paused spec (the STORY's whole thesis) — the conservative default must be
  "pause, don't guess". `defaults` exists for mechanical batches where throughput
  beats per-spec correctness.
- **Decision — `AdvisorOutcome` / `ReviewerOutcome` mirror STORY-276's
  `ImplementerOutcome`; honest non-failure stops share one `finish_escalated`.**
  An escalated design-fork and an escalated merge decision are both "exit 0,
  drain advances, human triages" — `finish_escalated` takes an `EscalationKind`
  for the epilogue wording. **Rationale**: BUG-241 item 5 ("treat
  escalated-to-human as a first-class non-failure, distinct from
  Approved/RequestChanges and from a crash") is the *same* requirement as the
  advisor-escalation stop — one code path, designed once.
- **Decision — the `needs-human` finding is the punted spec itself, tagged.**
  On escalate, the orchestrator adds the `needs-human` tag to the (already
  `NeedsAttention`) spec and appends the advisor's escalation reasoning as a
  comment — it does not file a separate draft TASK. **Rationale**: a
  `NeedsAttention` spec already surfaces in `aida findings`' "Punts awaiting
  triage" section; the `needs-human` tag distinguishes "advisor escalated this"
  from "advisor resolved-and-resumed" / "not yet triaged". A separate finding
  would duplicate the triage row.
- **Decision — extend `PuntRecord` with optional advisor fields; STORY-306
  emits, STORY-325 analyzes.** New `#[serde(default, skip_serializing_if)]`
  fields: `classification` (A/B/C), `escalation_reason`, `answer`,
  `answered_by`; `resolution_path` takes new values `advisor-resolved` /
  `escalated-to-human` / `escalate-defaulted`. **Rationale**: the STORY-325
  coupling comment is explicit — STORY-306 v1 must emit ledger records from day
  one so v1's escalation rate is measurable; the analysis layer is STORY-325.
  The fields are additive, so STORY-325 builds on them without a migration.
- **Decision — the corpus-growth loop is *prompted* in STORY-306, automated in
  STORY-325.** The advisor's escalation comment tells the human "when you answer
  this, record the answer as a memory / acceptance edit so a future advisor
  resolves it as Type A." **Rationale**: full automation (auto-suggesting the
  memory edit, measuring escalation decay) is STORY-325's analysis layer;
  STORY-306 makes the loop explicit without ballooning.
- **Decision — Slice 1 (the reviewer escalation handshake) ships first and
  independently of STORY-276.** It touches only the reviewer phase (shipped
  STORY-263). **Rationale**: it is unblocked today, it independently closes
  BUG-241 instance A's class of false-failure, and landing it first de-risks
  the `ReviewerOutcome` refactor before the advisor tier piles on.

## Files (in build-order)

Build order = slice order. Slices 1-2 are independently testable; slices 3-5
are the advisor tier and need STORY-276 merged first.

### Slice 1 — reviewer escalation handshake (independent of STORY-276)

#### `aida-cli/src/auto_complete.rs` — reviewer outcome

- `enum ReviewerOutcome` (new): `Verdict(Verdict)` | `EscalatedToHuman`.
- `trait PhaseDriver`: `fn run_reviewer` return type `Result<Verdict, …>` →
  `Result<ReviewerOutcome, PhaseFailure>`.
- `enum EscalationKind` (new): `MergeDecision` | `DesignFork` — selects the
  `finish_escalated` epilogue wording.
- `struct EscalationSummary` (new): `kind: EscalationKind`, `reason: String`.
- `struct OrchestrationResult`: add `escalation: Option<EscalationSummary>`
  (additive, alongside STORY-276's `punt` field).
- `fn finish_escalated` (new): mirrors `finish_reconciled`/`finish_punted` —
  exit 0, prints the escalation epilogue, builds the terminal result.
- `fn orchestrate`: phase-3 match arm — `ReviewerOutcome::EscalatedToHuman` →
  `finish_escalated(.., EscalationKind::MergeDecision)`; `Verdict(v) if v !=
  Approved` still routes through `resolve_phase_failure` (regression guard).

#### `aida-cli/src/reviewer_summary.rs` — verdict-file shape

- `struct VerdictFile`: add `merge: Option<String>` (`escalated-to-human` when
  the reviewer escalated the merge decision; absent otherwise).

#### `aida-cli/src/main.rs` — verdict-file read

- `fn read_verdict_file`: return a `(Verdict, Option<String> /*merge*/)` pair
  (or a small `struct ReviewerVerdict`); a `merge` field of `escalated-to-human`
  ⇒ `RealPhaseDriver::run_reviewer` returns `ReviewerOutcome::EscalatedToHuman`.
- `impl PhaseDriver for RealPhaseDriver::run_reviewer`: map the parsed verdict
  file to `ReviewerOutcome`.

#### `aida-core/templates/skills/aida-review.md` — escalation handshake (a)

- Step 7a: when the reviewer escalates the *merge* decision to a human (uncertain
  zen provenance, an irreversible call), still write the verdict file —
  `{"verdict": "Approved", "merge": "escalated-to-human", "summary": "…"}` — so
  the orchestrator's phase-3 handshake artifact always exists. Document the
  `merge` field alongside `verdict`/`mode`/`comment_url`.

### Slice 2 — punt request/response file formats + ledger extension

#### `aida-cli/src/punt.rs` — the file channel

- `struct PuntRequest` (new): the rich payload — `spec`, `category`,
  `question`, `options: Vec<String>`, `code_area: Option<String>`,
  `stakes: Option<String>`, `lean: Option<String>`, `context_markdown: String`
  (the assembled ultraplan-grade brief).
- `struct PuntResponse` (new): `resolution: PuntResolution`
  (`Resolved`|`Escalated`), `answer: Option<String>`, `reasoning: String`,
  `classification: Option<String>` (A/B/C), `escalation_reason: Option<String>`.
- `fn punt_request_path` / `fn punt_response_path`: `.aida/punts/<spec>.{request,response}.json`.
- `fn write_punt_request` / `fn read_punt_response` — write-atomic + parse.
- `struct PuntRecord`: add `classification`, `escalation_reason`, `answer`,
  `answered_by` (all `#[serde(default, skip_serializing_if = "Option::is_none")]`).

#### `aida-cli/src/main.rs` — the rich-payload assembler

- `fn assemble_punt_payload(store, req, project_root, attention_reason)
  -> PuntRequest`: wraps `assemble_ultraplan_prompt` for the `context_markdown`
  body, fills the structured fork fields from the spec's `AttentionReason`.

### Slice 3 — orchestrator advisor tier (needs STORY-276)

#### `aida-cli/src/auto_complete.rs` — advisor branch

- `enum AdvisorOutcome` (new): `Resolved { answer: String, reasoning: String }`
  | `Escalated { reason: String, category: String }`.
- `enum EscalateMode` (new): `Blocks` | `Defaults`; `fn parse` + `fn slug`.
- `trait PhaseDriver`: add `fn run_advisor(&mut self, punt: &PuntSummary)
  -> Result<AdvisorOutcome, PhaseFailure>` and `fn resume_implementer(&mut self,
  answer: &str) -> Result<ImplementerOutcome, PhaseFailure>` (both default to a
  `PhaseFailure::Internal` so the mock must opt in).
- `enum PuntResolution` (new, private): `Proceed` | `Terminal(OrchestrationResult)`.
- `fn resolve_punt_via_advisor(driver, spec, json, start, durations, punt,
  escalate_mode) -> PuntResolution`: the cascade — `run_advisor` → on `Resolved`
  `resume_implementer(answer)` (Shipped ⇒ `Proceed`, Punted ⇒
  `Terminal(finish_punted)`); on `Escalated` Blocks ⇒
  `Terminal(finish_escalated(DesignFork))`, Defaults ⇒
  `resume_implementer(<default-prompt>)`.
- `fn orchestrate`: signature gains `escalate_mode: EscalateMode`; the phase-1
  `Punted` arm routes through `resolve_punt_via_advisor` instead of STORY-276's
  direct `finish_punted`.

#### `aida-cli/src/session.rs` — headless resume launch

- `fn claude_headless_resume_args(prompt, session_id) -> Vec<String>`: SPIKE-7
  flag set with `--resume <id>` in place of `--session-id <id>`.
- `fn spawn_claude_headless_resume(prompt, session_id, log_path)` — spawn-and-wait
  counterpart for the orchestrator.

#### `aida-cli/src/main.rs` — RealPhaseDriver advisor + resume

- `RealPhaseDriver::run_advisor`: assemble the payload (`assemble_punt_payload`),
  `write_punt_request`, spawn `claude -p /aida-advise` headless with
  `AIDA_PUNT_REQUEST_FILE` + `AIDA_PUNT_RESPONSE_FILE` + `AIDA_SESSION_ROLE=dialog`
  env, wait, `read_punt_response`, append a ledger record, on escalate tag the
  spec `needs-human` + add the advisor's reasoning as a comment.
- `RealPhaseDriver::resume_implementer`: build the resume prompt embedding the
  advisor's answer, `spawn_claude_headless_resume` on the phase-1 session id,
  re-read the dispatched spec (the STORY-276 `detect_punt` logic) to classify
  the outcome `Shipped` vs `Punted`.
- `RealPhaseDriver`: store `implementer_session: Option<String>` (the minted
  phase-1 `--session-id`) so `resume_implementer` can target it.

### Slice 4 — the advisor skill

#### `aida-core/templates/skills/aida-advise.md` (new) + `aida-core/templates/commands/aida-advise.md` (new)

- The advisor/dialog-role skill: read `AIDA_PUNT_REQUEST_FILE`, apply the A/B/C
  calibration, resolve A + recorded-B, escalate C + unrecorded-B, write
  `AIDA_PUNT_RESPONSE_FILE`, on resolve `aida comment add` the answer + rationale
  to the spec, touch the TASK-329 exit sentinel last. The
  **conservative-escalation bias** ("when in doubt, escalate") is the documented
  default, enforced structurally by the type-calibration table.

#### `aida-core/templates/skills/aida-pickup.md` — resume-aware re-entry

- A headless rule: when the session is *resumed* with an advisor answer (the
  resume prompt names it), take the spec `NeedsAttention → InProgress`
  (`aida edit --status in-progress`), apply the decision, finish + open the PR.

### Slice 5 — audit surface + flags + docs

#### `aida-cli/src/cli.rs`

- `Command::Queue { Work { .. } }`: add `--escalate-blocks` / `--escalate-defaults`
  (a single `escalate: Option<String>`, `requires = "no_human"`, default
  `blocks`). `--no-human=both`-only — error if set without it.

#### `aida-cli/src/main.rs` — `aida findings` advisor-decisions footer

- `handle_findings_command`: append an "Advisor decisions (recent)" section read
  from `.aida/punts.jsonl` — resolved vs escalated counts + the escalated rows,
  so the morning-after audit lives on the existing triage surface.

#### Docs

- `docs/autonomous-drain.md` — the three-tier cascade, the `--escalate-*` flags,
  the advisor's escalate-by-default bias.
- `aida-core/templates/docs/aida-discipline/advisor-role.md` — the
  headless-advisor seat as an extension of the advisor role; the A/B/C
  calibration. (Master template — scaffolded into `docs/aida-discipline/`.)
- `CLAUDE.md`, `README.md` — `--escalate-blocks`/`--escalate-defaults`, the
  advisor tier in the autonomy taxonomy.

## Critical Files

- `aida-cli/src/auto_complete.rs`
- `aida-cli/src/punt.rs`
- `aida-cli/src/main.rs`
- `aida-cli/src/session.rs`
- `aida-cli/src/reviewer_summary.rs`
- `aida-cli/src/cli.rs`
- `aida-core/templates/skills/aida-advise.md` (new — verify flags it absent)
- `aida-core/templates/commands/aida-advise.md` (new — verify flags it absent)
- `aida-core/templates/skills/aida-pickup.md`
- `aida-core/templates/skills/aida-review.md`
- `docs/autonomous-drain.md`
- `aida-core/templates/docs/aida-discipline/advisor-role.md`

## Reusable helpers (do not reimplement)

- `assemble_ultraplan_prompt` (`aida-cli/src/main.rs`) — assembles a spec's
  description + acceptance + parent/child/sibling context + comments; reuse it
  verbatim for the punt payload's `context_markdown`.
- `build_reusable_helpers_section` (`main.rs`) — the trace-graph helper harvest;
  feeds the payload's "code refs / don't-reimplement" content.
- `punt::PuntRecord` / `punt::append_to_ledger` / `punt::ledger_path`
  (`aida-cli/src/punt.rs`) — the shipped STORY-332 ledger; extend the struct,
  reuse the append.
- `session::claude_headless_args` / `exec_claude_headless` /
  `spawn_claude_headless` (`aida-cli/src/session.rs`) — the SPIKE-7 flag set +
  `AIDA_HEADLESS=1` env; `claude_headless_resume_args` is a near-copy with
  `--resume`.
- `exit_signal::spawn_and_wait` + `sentinel_path` (`exit_signal.rs`) — the
  graceful-exit handshake; the advisor skill touches the sentinel like the
  reviewer does.
- `RealPhaseDriver::discover_orchestrated_lease` (`main.rs`) — locate the
  phase-1 session by its minted `--session-id`, for the resume.
- `read_verdict_file` / `reconcile_verdict` / `PhaseReconcile` (`main.rs`,
  `auto_complete.rs`) — the BUG-241 reconcile pattern; `ReviewerOutcome` and
  `finish_escalated` extend it, they do not replace it.
- `finish_reconciled` / `finish_punted` (`auto_complete.rs`, the latter from
  STORY-276) — `finish_escalated` mirrors their shape exactly.
- `RequirementStatus::NeedsAttention` + `AttentionReason` + `PuntCategory`
  (`aida-core`) — the STORY-332 model; no new core type is needed.
- `forbidden_attention_transition` (`aida-core/src/models.rs`) — confirms
  `NeedsAttention → InProgress` is allowed (the resume path takes it).
- `drain_state::set_phase` (`drain_state.rs`) — stamp an "advisor triage"
  sub-state so `aida drain status` shows the tier live.

## Risks + gotchas

1. **Risk**: the advisor over-resolves — answers a Type C it cannot actually
   judge — and the drain ships a confident-but-wrong overnight decision, worse
   than the safe-default punt. This is the STORY's central failure mode.
   **Mitigation**: structural, not hoped-for — the `/aida-advise` skill's A/B/C
   calibration table makes "escalate" the default for everything not provably A
   or recorded-B; `--escalate-blocks` is the *default* escalate mode (a paused
   spec, never a guessed one); every decision lands in `.aida/punts.jsonl` +
   (on resolve) a spec comment for morning audit. The bias is verified manually
   (SPIKE-7-style) against deliberately-Type-C forks.
2. **Risk**: `claude -p --resume <id>` does not cleanly re-enter the punted
   phase-1 session — wrong worktree, lost context, or the session id is
   unresumable after the punt. **Mitigation**: the punt deliberately does *not*
   run `aida session end` (STORY-276 Risk 7), so the lease + worktree persist;
   `--session-id` keeps Claude persistence on (SPIKE-7 Q9). The resume is
   verified end-to-end in the manual smoke; a resume that fails to produce a PR
   *or* a re-punt falls through to the existing `FailureKind::NoPr` failure —
   safe (no wrong code), just a stopped drain.
3. **Risk**: a re-punt after resume loops back into the advisor (cost runaway,
   infinite cascade). **Mitigation**: `resolve_punt_via_advisor` is called
   exactly once per spec from the phase-1 arm; a `Punted` from
   `resume_implementer` routes straight to `finish_punted` — terminal, no second
   `run_advisor`. A unit test asserts the mock's `run_advisor` is called once.
4. **Risk**: cost — the advisor spawn adds ~$1-3 of API spend per punt on top of
   the implementer + resume. A drain with many punts gets expensive.
   **Mitigation**: out of scope to fix here; `--max-budget-usd` is a shared
   followup with STORY-276. The advisor only runs on an actual punt, and the
   conservative-escalation bias keeps advisor *resumes* (the priciest leg) rare.
5. **Risk**: the orchestrator reads a stale spec status after the advisor /
   resumed-implementer wrote in a sibling worktree. **Mitigation**: `aida punt`,
   `aida comment add`, and `aida edit` are all write-through (git orphan branch
   + shared `.aida/cache.db`); a fresh `CachedGitBackend`/`GitBackend` open in
   the parent reads the child's write — the BUG-241/STORY-276 staleness pattern.
6. **Risk**: `ReviewerOutcome` is a breaking change to `run_reviewer`'s
   signature and STORY-276 may be mid-flight against the old `Result<Verdict,…>`.
   **Mitigation**: Slice 1 lands the `ReviewerOutcome` refactor *first* and
   independently, before the advisor tier — so STORY-276's rebase, if it
   collides, collides once on a small, already-merged change.
7. **Gotcha**: dual-copy templates — `aida-advise.md` is a *new* skill +
   command. Create the masters under `aida-core/templates/{skills,commands}/`,
   run `make sync-templates` to create the `.claude/` per-file symlinks, and
   confirm `aida init`'s skill count (currently 32) is bumped in any doc that
   states it.
8. **Gotcha**: `--escalate-defaults` resumes the implementer with a *default*
   prompt — but the implementer punted because it had no confident default. The
   resume prompt must explicitly authorize "proceed with your stated `--lean`,
   or the most defensible reading if you gave no lean" so the implementer does
   not simply re-punt. A re-punt under `--escalate-defaults` is still terminal
   (`finish_punted`) — the default mode does not guarantee a ship.
9. **Gotcha**: SPEC-IDs are developer artifacts — keep `STORY-306` out of the
   advisor skill's user-facing prose, the `--escalate-*` `--help` text, and the
   `aida findings` footer; `trace:` markers stay plain `//` comments (TASK-268).

## Tests (named, not "add tests")

`aida-cli/src/auto_complete.rs`:
- `orchestrate_punt_advisor_resolves_then_resume_runs_full_pipeline` — mock:
  phase 1 `Punted`, advisor `Resolved`, resume `Shipped` ⇒ all six phases run,
  exit 0, `result.escalation.is_none()`.
- `orchestrate_punt_advisor_escalates_blocks_skips_phases_2_to_6` — advisor
  `Escalated`, mode `Blocks` ⇒ exit 0, `escalation.is_some()`, driver calls ==
  `[Implementer, Advisor]`.
- `orchestrate_punt_advisor_escalates_defaults_resumes_with_default` — advisor
  `Escalated`, mode `Defaults`, resume `Shipped` ⇒ full pipeline, escalation
  recorded.
- `orchestrate_advisor_resume_repunt_is_terminal` — resume returns `Punted` ⇒
  `finish_punted`, exit 0, `run_advisor` called exactly once.
- `orchestrate_reviewer_escalated_to_human_skips_merge` — `run_reviewer` returns
  `ReviewerOutcome::EscalatedToHuman` ⇒ exit 0, no merge phase, no failure.
- `orchestrate_reviewer_request_changes_still_fails` — regression: a non-Approved
  `Verdict` still routes to a phase-3 failure.
- `escalate_mode_parse_defaults_to_blocks` — `parse("")`/`parse("blocks")`/
  `parse("defaults")` + the unknown-value error.

`aida-cli/src/punt.rs`:
- `punt_request_roundtrips_json` — `PuntRequest` serialize → deserialize.
- `punt_response_resolved_and_escalated_parse` — both `PuntResolution` shapes.
- `punt_record_carries_advisor_fields` — `PuntRecord` with `classification` +
  `escalation_reason` + `answer` round-trips; old records (fields absent) still
  deserialize via `#[serde(default)]`.

`aida-cli/src/main.rs`:
- `verdict_file_reads_merge_escalated_to_human` — `read_verdict_file` surfaces a
  `merge: escalated-to-human` field as `ReviewerOutcome::EscalatedToHuman`.
- `verdict_file_without_merge_field_is_plain_verdict` — regression: a STORY-263
  verdict file with no `merge` field parses as before.
- `assemble_punt_payload_includes_spec_and_fork` — the payload carries the
  spec description + the fork question/options.

`aida-cli/src/session.rs`:
- `claude_headless_resume_args_has_resume_and_no_session_id` — `--resume <id>`
  present, `--session-id` absent, SPIKE-7 mandatory flags intact.

## Verification

```bash
# --- automated ---
cargo test -p aida-cli auto_complete::      # advisor branch + reviewer outcome
cargo test -p aida-cli punt::               # request/response/ledger formats
cargo test -p aida-cli claude_headless_resume
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# --- flag surface (no API spend) ---
aida queue work --help | grep -A2 escalate          # --escalate-blocks/-defaults
aida queue work SOME-SPEC --auto-complete --escalate-defaults 2>&1 \
  | grep -i "no-human"                              # errors without --no-human=both
make sync-templates && ls .claude/skills/aida-advise.md  # new skill symlinked

# --- manual headless smoke (SPIKE-7-style, ~$6-10 API spend; needs STORY-276) ---
TMP=$(mktemp -d); cd "$TMP" && git init && aida init

# confident fork → advisor RESOLVES → implementer resumes → PR opens
aida add --title "smoke: resolvable fork" --type task --status approved \
  --description "Add a --json flag to 'aida foo'. (Fork: flag name --json vs
  --format json — AIDA's recorded convention is a bare --json bool.)"
AIDA_NO_HUMAN_ACKNOWLEDGED=1 aida queue work <SPEC> --auto-complete \
  --no-human=both --escalate-blocks
aida show <SPEC> | grep -i status            # expect: Done/Completed (resolved+shipped)
grep advisor-resolved .aida/punts.jsonl      # ledger records the resolve
aida show <SPEC> | grep -iA2 advisor         # advisor's decision comment on the spec

# genuine Type-C fork → advisor ESCALATES → needs-human, drain pauses
aida add --title "smoke: unresolvable fork" --type task --status approved \
  --description "Decide the project's overall error-reporting strategy. (No
  recorded principle — a strategy call.)"
AIDA_NO_HUMAN_ACKNOWLEDGED=1 aida queue work <SPEC2> --auto-complete \
  --no-human=both --escalate-blocks
aida show <SPEC2> | grep -i "needs attention"  # expect: NeedsAttention
aida show <SPEC2> | grep needs-human           # expect: needs-human tag
aida findings                                  # SPEC2 + advisor-escalated count
grep escalated-to-human .aida/punts.jsonl      # ledger records the escalation

# escalation handshake (independent of STORY-276 — Slice 1 only):
# a reviewer that escalates the merge writes verdict + merge:escalated-to-human;
# the orchestrator exits 0 (no merge, no failure).
```

Definition of done: under `--no-human=both`, a design-fork the advisor is
confident about is resolved + the implementer resumes + the spec ships through
the full pipeline; a fork the advisor is not confident about is escalated to a
`needs-human` finding and the drain advances without shipping a guess; every
advisor decision is in `.aida/punts.jsonl` and (resolved) on the spec; a reviewer
that escalates a merge leaves a verdict file and the orchestrator exits 0.

## Followups

- STORY-325 — the punt-ledger analysis layer: classification patterns →
  recorded principles, the escalation-rate-decay measurement.
- v2 persistent advisor — a session alive across the drain, accumulating
  context; gated behind STORY-325 ledger data showing v1 escalates too often
  for genuine lack-of-synthesis (the STORY-306 design-expansion comment).
- `--max-budget-usd` / `--model` cost knobs for the advisor + resume spawns
  (shared followup with STORY-276's headless implementer).
- Automate the corpus-growth loop: on a human triage-resolving a `needs-human`
  punt, prompt to record the answer as a memory / acceptance edit.
- Robust worktree cleanup for an escalate-blocks spec whose phase-1 lease is
  never consumed by a resume (extends STORY-276's lingering-worktree followup).
- `aida drain status` live "advisor triage" sub-state while `run_advisor` runs.

## Related

- Builds on: STORY-276 (headless implementer — the tier to punt FROM; **must
  merge before slices 3-5**), STORY-285 (implementer findings — Completed),
  STORY-332 (`/aida-punt` + `NeedsAttention` + the punt ledger — Completed),
  TASK-329 (sentinel-file channel — Completed), STORY-263 (headless reviewer).
- Absorbs: BUG-241 items 4-5 (the escalation handshake — Slice 1).
- Composes with: STORY-287 (three-mode autonomy taxonomy), STORY-278 (headless
  reviewer findings → advisor — this STORY is its deferred "auto-triage" piece),
  STORY-301 (drain status — the morning-after audit), STORY-325 (punt ledger).
- See also: `docs/autonomous-drain.md`, `docs/spikes/2026-05-16-claude-headless.md`,
  `feedback_three_mode_autonomy_taxonomy.md`.
