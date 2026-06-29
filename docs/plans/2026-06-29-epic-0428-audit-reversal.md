# Plan: TASK-0430 — Advisor autopilot audit and reversal surface

Date: 2026-06-29
Specs: TASK-0430 (parent EPIC-0428) — depends on TASK-0429 (the envelope)
Status: Draft — **design only, needs master-advisor sign-off before any code**
Complexity: ~150 prod LOC + ~120 test LOC when built, 0 commits now, risk medium (audit must be durable + reversal must be safe)

<!-- Depends on the Decision/Outcome shapes from TASK-0429. Do NOT implement. -->

## Approach

Every autopilot `Execute` outcome (TASK-0429) must leave a **durable, vendor-independent
trail** and be **reversible with one command**. The key constraint, surfaced by
the machinery map: dispositions today are produced by a headless `claude -p`
agent and **no structured per-disposition record exists** — only git-commit
authorship + tags. And the obvious place to log (`.aida/`) is *gitignored,
per-clone runtime state* (the punt ledger `.aida/punts.jsonl` lives there) — so
it **cannot** be the durable trail that "survives agent/vendor changes"
(TASK-0430 acceptance).

The design therefore splits audit into two layers, mirroring AIDA's own
git-canonical-store + rebuildable-cache pattern:

1. **Durable source of truth — the orphan-branch git store.** Every autopilot
   action already produces a targeted commit (`update SPEC-ID`) when it edits a
   spec. Autopilot *additionally* writes a **structured comment** on the
   affected spec recording `actor=autopilot, mode, action, reason, evidence,
   timestamp, decision-id`. Comments live in the spec's YAML → the `aida-store`
   orphan branch → durable, diffable, and independent of which agent/vendor ran
   the disposition. This is the trail that survives.

2. **Fast index — a rebuildable ledger.** A `.aida/autopilot-log.jsonl` (one
   line per decision, the same shape as `PuntRecord`) is the *queryable index*
   for "list recent autopilot decisions" — fast, but **rebuildable** from the
   git-store comments exactly like `.aida/cache.db` rebuilds from YAML. Losing it
   loses nothing.

Reversal is a pure inverse-action table: each `ActionClass` has a deterministic
undo (approve→draft, reject→reopen, queue→remove, tag→untag, dedupe→unlink,
park→unpark, route→move-back). `aida autopilot undo <decision-id>` reads the
ledger, replays the inverse, and records the reversal *as its own audited
event* (reversals are themselves auditable). "Challenge" is a lighter verb —
flag a decision for human review without reverting it yet.

### Diagram — two-layer audit + reversal

```
 autopilot Execute(spec, action)
        │
        ├─► aida edit/queue/tag …  ──► targeted commit "update SPEC-ID"  ┐
        │                                                                 │ DURABLE
        ├─► aida comment add SPEC "autopilot: <action> — <reason>         │ (orphan
        │        [evidence …] [mode …] [id:<decision-id>]"  ──► YAML ─────┘  branch)
        │
        └─► append .aida/autopilot-log.jsonl (decision-id, actor, mode,    ┐ FAST INDEX
                 action, spec, reason, evidence, risk, grounding, ts)      ┘ (rebuildable)

 aida autopilot list                ◄── reads index (rebuilds from comments if stale)
 aida autopilot undo <decision-id>  ◄── inverse-action table → replays + audits the reversal
 aida autopilot challenge <id>      ◄── flags needs-human, does NOT revert
```

## Decisions

- **Decision: durable trail = git-canonical spec comments, NOT `.aida/`.**
  **Rationale**: TASK-0430 demands the trail "survive agent/vendor changes."
  `.aida/` is gitignored per-clone runtime (the `.gitignore` deny-by-default
  convention). The orphan-branch YAML is the only substrate that is replicated,
  diffable, and vendor-independent. Comments are already first-class on every
  requirement and already render in `aida show`. Reuse them.

- **Decision: the JSONL ledger is a rebuildable index, modeled on
  `PuntRecord`.** **Rationale**: the punt ledger (`.aida/punts.jsonl`,
  `PuntRecord` with `raised_by`/`answered_by`/`resolution_path`/`classification`/
  `escalation_reason`) is the *existing* structured actor/reason/mode substrate.
  Autopilot's `AutopilotDecision` record is its sibling. Keeping it rebuildable
  (a `aida autopilot reindex` that re-scans `autopilot:` comments) means the fast
  surface never becomes a second source of truth that can drift from the store.

- **Decision: one structured comment format, machine-parseable.**
  **Rationale**: so the index can rebuild from comments. Format:
  `autopilot: <action> <spec-state-delta> — <reason> · evidence: <a>, <b> · mode: <mode> · grounding: <A|B|C> · risk: <level> · id: <decision-id> · <ISO-8601>`.
  A single regex anchors `id:` and `action` for the reindex parse. Human-readable
  *and* machine-recoverable.

- **Decision: reversal is a deterministic inverse-action table, not an LLM call.**
  **Rationale**: undo must be safe and predictable. Each `ActionClass` maps to
  exactly one inverse CLI operation. No judgment, no agent. `undo` is pure
  replay.

- **Decision: reversals and challenges are themselves audited events.**
  **Rationale**: the audit trail must show not just what autopilot did but what
  a human did about it. `undo` writes its own `autopilot: reverted <decision-id>
  by <user>` comment + ledger line; `challenge` writes a `needs-human` finding +
  comment without changing spec state.

- **Decision: reuse `aida findings list` grouping + `aida history --events`
  semantics for the list surface; do not build a third audit viewer.**
  **Rationale**: `findings list` already groups by source/severity and is the
  morning-triage surface; autopilot decisions are a natural `--source autopilot`
  extension (new `FindingSource::Autopilot`). `aida autopilot list` is a thin
  filtered view over the same machinery. MCP gets a `list_autopilot_decisions`
  tool mirroring the CLI (parity rule, STORY-82).

- **Decision: irreversible-in-principle actions are blocked from autopilot
  upstream, not "undone" downstream.** **Rationale**: undo is best-effort for
  reversible state (status, tags, queue membership). Anything *not* cleanly
  reversible (a merge, a release tag, a deletion) is already `never`/fenced in
  the TASK-0429 envelope — autopilot must never take an action it cannot undo.
  Undo's contract is therefore total over the action set autopilot is *allowed*
  to take.

## Files (in build-order)

### `aida-cli/src/autopilot_log.rs` (new) — the decision record + ledger

- `struct AutopilotDecision { decision_id, ts, actor, mode, action: ActionClass, spec_id, state_before, state_after, reason, evidence: Vec<String>, grounding, risk, reverted: Option<Reversal> }` — sibling of `PuntRecord`.
- `fn build_decision_id(...)` — stable id (mirror `calibration::build_punt_id`).
- `fn append_to_ledger(...)` / `fn read_ledger(...)` — JSONL append/read (mirror `punt::append_to_ledger`/`read_ledger`, `punt.rs:150`/`:181`).
- `fn audit_comment(d: &AutopilotDecision) -> String` — the machine-parseable comment format.
- `fn parse_audit_comment(s: &str) -> Option<AutopilotDecision>` — the reindex parse (regex-anchored on `id:`/`action`). Round-trips `audit_comment`.
- `fn reindex(store) -> Vec<AutopilotDecision>` — rebuild the ledger from `autopilot:` comments across the store. **Pure-ish, unit-testable.**

### `aida-cli/src/autopilot.rs` (new) — emit audit on Execute (created by TASK-0429)

- After each `Outcome::Execute`, call: the spec edit/queue op, then `aida comment add` with `audit_comment(...)`, then `append_to_ledger(...)`. All three in one logical step; if the comment write fails, the *whole* action fails (audit is not optional).

### `aida-cli/src/autopilot_undo.rs` (new) — inverse-action table

- `fn inverse(action: ActionClass) -> InverseOp` — the deterministic undo map.
- `fn undo(decision_id, store, user) -> Result<Reversal>` — read ledger → replay inverse → write `reverted` comment + ledger line.
- `fn challenge(decision_id, store, user, note) -> Result<()>` — file a `needs-human` finding + comment, no state change.

### `aida-cli/src/findings.rs` — new source

- Add `FindingSource::Autopilot` + `FROM_AUTOPILOT_PREFIX` (mirror the `Advisor` variant at `findings.rs:126`) so autopilot decisions/challenges thread into `aida findings list --source autopilot`.

### `aida-cli/src/cli.rs` + `main.rs` — surface

- `aida autopilot list [--since <window>] [--risky] [--escalated] [--from-product] [--json]` — filters per acceptance (#4: risky/escalated/product-sourced).
- `aida autopilot undo <decision-id>` / `aida autopilot challenge <decision-id> [--note]` / `aida autopilot reindex`.
- TUI: an "Autopilot" panel/overlay listing recent decisions with an inline undo keybinding (compose with `docs/tui/README.md`).

### `aida-cli/src/mcp.rs` — parity

- `list_autopilot_decisions`, `undo_autopilot_decision`, `challenge_autopilot_decision` tools mirroring the CLI (STORY-82 parity rule).

### `aida-core/templates/skills/aida-assess.md` — record-the-evidence discipline

- The autopilot section must instruct the agent to emit, per Execute, the `reason` + `evidence` (substrate citations) + `grounding` so `audit_comment` has real content (not "auto-approved" with no why).

## Critical Files

- `aida-cli/src/autopilot_log.rs` (new)
- `aida-cli/src/autopilot_undo.rs` (new)
- `aida-cli/src/autopilot.rs` (new, TASK-0429)
- `aida-cli/src/findings.rs`
- `aida-cli/src/cli.rs`, `aida-cli/src/main.rs`, `aida-cli/src/mcp.rs`
- `aida-core/templates/skills/aida-assess.md`

## Reusable helpers (do not reimplement)

- `punt::PuntRecord` / `punt::append_to_ledger` / `punt::read_ledger` (`aida-cli/src/punt.rs`) — the ledger pattern to copy for `AutopilotDecision`.
- `calibration::build_punt_id` (`aida-cli/src/calibration.rs`) — stable id derivation.
- `findings::FindingSource` + `findings::FROM_ADVISOR_PREFIX` (`aida-cli/src/findings.rs`) — extend with an `Autopilot` variant; reuse the whole `findings list` grouping/severity machinery.
- `handle_findings_add` (`aida-cli/src/main.rs`) — the file-a-finding path `challenge` reuses.
- The git-canonical comment path — `aida comment add` → `GitBackend::update_requirement` → `auto_commit_paths` (`aida-core/src/db/git_backend.rs`) — the durable write. Comments already render in `aida show`.
- `aida history --events` / `resolve_history_id_filter` (`aida-cli/src/main.rs`) — the existing per-spec event timeline; `aida autopilot list` should *complement*, not replace it (history shows the field deltas; autopilot list shows the actor/reason/evidence).
- `current_user_id` (`aida-cli/src/main.rs:119036`) — the reverting/challenging user's identity.

## Risks + gotchas

1. **Risk: ledger and store drift (the fast index lies).** **Mitigation**: the
   ledger is *derived*; `aida autopilot list` checks a recorded store-HEAD like
   the cache does and triggers `reindex` on mismatch. The git-store comment is
   always authoritative. A `reindex` round-trip test (`audit_comment` →
   `parse_audit_comment`) guards the format contract.

2. **Risk: undo of a stale decision — the spec moved since autopilot touched
   it** (e.g. a human already re-approved, or it merged). **Mitigation**: `undo`
   checks `state_after` still matches current state before replaying; on
   mismatch it refuses and prints what changed (no blind revert). For a
   *merged/completed* spec, undo is rejected outright (autopilot should never
   have queued something that could merge before review — but defense in depth).

3. **Risk: comment-format brittleness — a hand-edited comment breaks reindex.**
   **Mitigation**: reindex parse is tolerant — an unparseable `autopilot:`
   comment is logged-and-skipped, never fatal; the ledger keeps the parseable
   ones. The `id:` anchor is the only hard requirement.

4. **Risk: audit write failure leaves an un-audited action.** **Mitigation**:
   the action is not "done" until the comment lands (decision: audit is not
   optional). If the comment write fails, the action is rolled back or the run
   aborts — never a silent un-audited edit. (Same posture as
   `feedback_verify_edits_landed_before_claiming_done`.)

5. **Risk: PII / vendor-name leakage into a PUBLIC repo's audit comments**
   (`feedback_public_repo_scrub_employer_content`). **Mitigation**: evidence
   citations are *substrate references* (memory names, doc paths, spec IDs), not
   free text — bounded by construction. Document the rule in the skill.

## Tests (named)

- `audit_comment_round_trips_through_parse` — format contract.
- `reindex_rebuilds_ledger_from_store_comments` — durability/recovery.
- `reindex_skips_unparseable_comment_non_fatally` — tolerance.
- `undo_approve_returns_spec_to_draft` — inverse table.
- `undo_queue_removes_from_queue` — inverse table.
- `undo_tag_removes_only_the_added_tag` — surgical inverse.
- `undo_refuses_when_state_changed_since_decision` — stale-undo guard.
- `undo_refuses_merged_spec` — irreversible guard.
- `undo_is_itself_audited` — reversal leaves a trail.
- `challenge_files_needs_human_finding_without_state_change` — challenge ≠ revert.
- `list_filters_risky_and_escalated_and_product_sourced` — acceptance #4.

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && "$AIDA_BIN" init >/dev/null
printf '\n[autopilot]\ntag = "auto"\napprove = "auto"\n' >> .aida/config.toml

"$AIDA_BIN" add --title "tidy a log line" --type task --status draft --tags "risk:low"
"$AIDA_BIN" groom --autopilot --apply

# Durable trail: the spec carries an autopilot comment in its YAML.
"$AIDA_BIN" show TASK-... --verbose | grep -i 'autopilot:'        # actor/reason/evidence present
# Fast index lists it.
DID=$("$AIDA_BIN" autopilot list --json | jq -r '.[0].decision_id')
# Recovery: nuke the index, reindex from the store, still there.
rm -f .aida/autopilot-log.jsonl
"$AIDA_BIN" autopilot reindex && "$AIDA_BIN" autopilot list | grep -i 'TASK-'
# One-command reversal returns the spec to draft and audits the reversal.
"$AIDA_BIN" autopilot undo "$DID"
"$AIDA_BIN" show TASK-... | grep -i 'status: *draft'
"$AIDA_BIN" show TASK-... --verbose | grep -i 'reverted'
```

## Followups

- TASK-0431 — product-sourced decisions must be filterable (`--from-product`); the evidence field records the product handoff (this plan reserves the filter, TASK-0431 fills the source).
- TASK-0432 — the `mode` field records which composition mode produced the decision (autopilot / zen+autopilot / solo+autopilot).
- Followup TASK (file at sign-off): TUI autopilot panel + inline undo (`docs/tui/README.md`).

## Related

- TASK-0429 (envelope), `.aida/punts.jsonl` / `PuntRecord`, `aida findings`, `aida history --events`, `docs/aida/discipline/substrate-as-bouncer.md`.

## Recommendation + smallest first slice

**Recommendation**: make the **git-canonical spec comment** the durable trail
and the JSONL ledger a rebuildable index — never let the fast surface become a
second source of truth. Reuse the punt-ledger pattern, the findings grouping,
and the comment/commit path wholesale; the only genuinely new code is the
`AutopilotDecision` shape, the comment round-trip parse, and the inverse-action
table. Make audit non-optional (an un-audited action is a failed action) and
make reversal deterministic (a table, never an agent).

**Smallest first slice**: ship `autopilot_log.rs` — the `AutopilotDecision`
record, `audit_comment`/`parse_audit_comment` round-trip, and `reindex` — plus
`aida autopilot list` reading the ledger, **wired to the TASK-0429 first slice's
`evaluate` outputs in a dry-run harness** (no live disposition yet). This proves
the durable-trail + recovery story end-to-end before `undo` or the live launcher
exist. `undo`/`challenge` and the TUI/MCP surfaces are the second slice.
