# STORY-560 — Headless advisor INTAKE pass (`aida intake` + `/aida-intake`)

- **Date:** 2026-06-12
- **Specs:** STORY-560 (composes with STORY-558, STORY-554, STORY-557, STORY-306)
- **Status:** Implemented (CLI + config + skill + tests green)
- **Complexity:** Medium-high (new launcher + config block + judgment skill; reuses existing gates)

## Approach

The advisor-side analog of `aida burndown run`. Where `burndown run` fires headless
IMPLEMENTER agents over the blessed ready set, `aida intake` fires a headless ADVISOR
agent that reads all open specs, applies worth-doing judgment, and proposes
approve/reject/park/queue dispositions — propose-by-default, `--apply` executes.

Two layers, mirroring the burndown launcher/skill split:

```
aida intake (CLI launcher, deterministic + testable)
  ├─ load [intake] config (P1 bias, P2 do-not-approve classes, P3 on_apply) + flag overrides
  ├─ self-load store → compute the BOUNDED candidate set:
  │     (Drafts + Approved-not-queued)
  │       MINUS do_not_approve_classes types   ← P2 HARD bound, fenced HERE (substrate-as-bouncer)
  │       MINUS needs-human / strategic tags    ← P2 always-on
  │       MINUS --exclude-tag / not --only-tag
  │       MINUS risk above --risk ceiling
  ├─ --dry-run → print command + candidate set, exit
  └─ launch headless `claude -p /aida-intake [--apply]` with policy + fence via env

/aida-intake (skill — the judgment, what Claude is good at)
  ├─ read the bounded candidate set (env AIDA_INTAKE_CANDIDATES)
  ├─ apply disposition_bias posture (P1)
  ├─ PROPOSE per spec WITH one-line reasoning (acceptance #1 — the ultimate gate)
  └─ under --apply: aida edit --status approved/rejected (within fence, ≤max-approvals,
        NEVER a spec it flags needs-human → leave draft / aida questions),
        then `aida backlog groom --pickable --apply --risk <ceiling>` (STORY-554, the queue step),
        then on_apply=drain / --then-drain → `aida burndown run`
```

The fence is the key design call: the P2 do-not-approve classes are a HARD authority
bound, so they are excluded by the launcher (programmatic gate) — the agent never sees
them as actionable. The agent's judgment operates strictly inside the fence. This honors
`feedback_substrate_as_bouncer_not_rules` rather than trusting a CLAUDE.md/prompt rule.

## Decisions

- **D0 (binding):** new `/aida-intake` skill (batch read-all-propose), not an extension of
  `/aida-advise` (per-punt). CLI launcher `aida intake`. Reuse `backlog groom --pickable`
  for the queue step.
- **Headless cold-boot `claude -p`** (mirror `handle_burndown_run`), `AIDA_SESSION_ROLE=advisor`,
  default `bypassPermissions` with a loud notice, `--permission-mode` override.
- **Fence the P2 classes in the launcher** (not just the prompt) — substrate enforcement of the
  HARD bound.
- **`[intake]` parsed with the hand-rolled section scanner** (mirror `AdvisorConfig`), no serde-toml dep.
- **Defaults work with zero config:** approve-eligible bias + tight do-not-approve classes +
  stop-at-queue + propose-mode.

## Files (build order)

1. `aida-cli/src/intake.rs` (NEW) — `IntakeConfig` (DispositionBias, do_not_approve_classes, OnApply)
   + Default + load/scanner/apply_key; pure `select_intake_candidates(...)`; `intake_skill_prompt(apply)`;
   unit tests.
2. `aida-cli/src/main.rs` — `mod intake;`; early dispatch `Command::Intake`; `handle_intake_command(...)`.
3. `aida-cli/src/cli.rs` — `Intake { apply, max_approvals, only_tag, exclude_tag, risk, then_drain, dry_run, permission_mode }`.
4. `aida-core/templates/skills/aida-intake.md` (NEW) — the judgment workflow.
5. `aida-core/templates/commands/aida-intake.md` (NEW) — the command stub.
6. `.claude/skills/aida-intake.md` + `.claude/commands/aida-intake.md` — per-file symlinks (dogfood).

## Critical files

- `aida-cli/src/main.rs::handle_burndown_run` — the launcher template.
- `aida-cli/src/burndown.rs::{classify,parking_tag}` — the pickability gate (reused by groom).
- `aida-cli/src/backlog.rs::{handle_groom_pickable,select_pickable,RiskLevel}` — the queue step.
- `aida-cli/src/advisor.rs::{AdvisorConfig,scan_advisor_section,strip_inline_comment}` — config pattern.

## Reusable helpers (don't reimplement)

- `load_store_for_lookup`, `all_queued_requirement_ids`, `find_project_root` (main.rs).
- `RiskLevel::parse`, `classify_risk`, `select_pickable` (backlog.rs).
- `burndown::classify` / `parking_tag` (the parking-tag predicate).

## Risks + gotchas

- `backlog groom --pickable` only considers Approved specs — newly-approved drafts must be
  approved BEFORE the groom step (skill ordering).
- The fence excludes do-not-approve classes from BOTH approve and reject (rejecting a vision is
  also a strategic call).
- max-approvals is an agent-enforced throughput cap (propose-mode is the real gate), not a fence.

## Tests (named)

- `intake::config_defaults_are_safe` — zero-config = approve-eligible + tight classes + queue.
- `intake::config_parses_intake_block` — `[intake]` knobs override defaults.
- `intake::select_excludes_do_not_approve_classes` — vision/epic/etc fenced out.
- `intake::select_excludes_needs_human_and_strategic_tags`.
- `intake::select_applies_only_and_exclude_tag`.
- `intake::select_applies_risk_ceiling`.
- `intake::skill_prompt_propose_vs_apply`.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli intake::
aida intake --help          # knobs + defaults documented
aida intake --dry-run       # prints candidate fence + command, launches nothing
```

## Followups

- Optional `[intake]` example block in the `aida init` config template (defaults already work).
- Substrate-grade gate on `aida edit --status approved` by type (broader than this story).

## Related

- STORY-558 (human-guided sibling), STORY-554 (groom half), STORY-306 (/aida-advise headless tier),
  STORY-545 (burndown run launcher template).
