# Skill-catalog slop audit — 2026-05-29

**Author**: master advisor session
**Trigger**: operator steer — the article→SPIKE→ship firehose risks adding overlapping/ambiguous surface. The 575/577/481 skill trio merge was held pending this audit.
**Scope**: 39 skills in `aida-core/templates/skills/` + the 3 pending trio branches.

## Headline

The trio is **not egregious slop** — both new skills are read-only (report + recommend, no mutation) and have genuinely substrate-aware cores generic tools can't replicate. The real slop is **pre-existing**: `aida-doctor` ≈ `aida-recover` are ~80% the same skill. The catalog is also simply *large* (39), so the bar for net-new should stay high even for defensible additions.

## Trio verdict

| Spec | Skill | Verdict | Rationale |
|---|---|---|---|
| TASK-575 | (frontmatter hardening on ~10 destructive skills) | **SHIP** | Safety, not new surface. `disable-model-invocation` on destructive shapes. Template-side only (no symlink-duality trap). Lowest-risk of the three. |
| STORY-481 | `/aida-techdebt` | **SHIP (trim generic overlap)** | Read-only. Its substrate-aware detection — dead trace paths (trace→Rejected spec), spec-graph dups, copy-pasted trace comments, orphan files — is *novel*, not covered by `/aida-code-review` (generic reuse) or `/aida-architecture` (structure). Keep the substrate-specific scan; the generic-duplication part nominally overlaps code-review but is cheap to keep. |
| TASK-577 | `/aida-insights` | **BORDERLINE → operator's call** | Read-only; combines 3 existing signals (`aida usage`, drain reliability, `findings calibration --stats`) into a monthly "where is attention going" view + maps to follow-ups. Real interpretation value, but it's an interpretation-layer over existing CLI — the kind that, multiplied, becomes catalog-selection noise. Defensible to ship; equally defensible to defer until there's pull-demand. |

**Net delta recommended: +2 skills (575 hardening + 481 techdebt), 577 deferred** — unless the operator wants the insights surface now.

## The sharper finding — pre-existing overlap

**`aida-doctor` ≈ `aida-recover`** — both walk a state-divergence diagnostic battery over leases / worktrees / branches:

- `aida-doctor`: "Diagnose and heal multi-agent AIDA state drift: stale leases, obsolete briefs, orphan worktrees, orphan branches" (+ heal)
- `aida-recover`: "structured diagnostic battery … session leases, git worktrees, GitHub PRs, AIDA spec graph, orchestrator/queue" (+ walk-the-operator)

~80% scope overlap. The functional difference is doctor *heals* (mutating) and recover *walks the operator* (advisory) — which is a `--heal` flag, not two skills. **Consolidation candidate**: merge into one `aida-recover` (or `aida-doctor`) with an advisory default + `--heal`. Filed as a finding for recurrence-gated action.

## Other near-neighbors checked (no action)

- `aida-insights` vs `aida-digest`: distinct inputs (telemetry/usage vs narrative work/commits). Not a dup.
- `aida-techdebt` vs `aida-code-review` / `aida-architecture`: generic overlap only; substrate-aware core is distinct.
- `aida-capture` vs `aida-learn`: capture = missed requirements; learn = rule-from-mistake. Distinct triggers.

## Standing guidance (from this audit)

1. Read-only advisory skills are cheap (low blast radius) — the slop bar for them is catalog-selection-budget noise, not behavioral risk.
2. Net-new *mutating* commands/skills carry a higher bar.
3. At 39 skills, the catalog is large enough that "we already have N" is a reason to scrutinize N+1, not to wave it through.
4. The doctor/recover overlap shows slop accumulates silently — periodic audits (this one) are worth repeating per `feedback_memory_pack_hygiene`-style triggers.

trace:from-strategic-recompose-round-3 | ai:claude
