# Plan: Skill ↔ CLI symmetry audit + the slice/launcher/pure-agentic convention

Date: 2026-06-10
Specs: SPIKE-55
Status: Complete
Complexity: analysis doc only — 0 prod LOC, 0 test LOC, 1 commit, risk low

<!-- trace:SPIKE-55 -->

<!--
  SPIKE deliverable: this is an ANALYSIS DOC, not code. It enumerates and
  classifies every skill against the question "should this skill expose a
  deterministic CLI verb?" and formalizes the convention. Enacting any gap
  is a follow-up TASK requiring master sign-off — nothing here files work.
-->

## Approach

Blanket 1:1 skill↔CLI symmetry is the wrong target. Skills are **LLM
workflows** (read context, reason, write prose, make judgment calls); CLI
commands are **deterministic substrate ops** (assemble context, derive a
value, mutate the store). Most skills are irreducibly agentic and a CLI
"equivalent" would be a hollow `claude -p /skill` launcher — surface bloat for
zero determinism gain (this is the STORY-556 "don't bloat the surface"
concern). But two *specific* symmetries are genuinely valuable and already
exist inconsistently across the catalog:

1. **The deterministic SLICE** under an agentic skill — the context-assembly /
   data-derivation step the LLM does *before* it starts reasoning — should be a
   CLI verb. It is pure, testable, reusable by other agents and by CI, and it
   removes a source of LLM drift. Shipped exemplars: `aida ultraplan`
   (←/ultraplan), `aida review prompt` (←/aida-review), `aida burndown plan`
   (←/aida-burndown), `aida goal` (←/goal), `aida digest` (←/aida-digest).
2. **The LAUNCHER** symmetry — a thin `aida X run` → `claude -p /skill` — but
   *only* for skills worth running unattended (cron / CI / headless drain).
   Shipped exemplar: `aida burndown run` → `claude -p /aida-burndown`.

This audit inventories all 42 skills, classifies each into exactly one of three
buckets, marks which bucket-B slices already have their CLI verb vs which are
GAPS, formalizes the convention in five lines, and emits a prioritized
candidate-follow-up list for the operator to approve. It files nothing.

### Diagram

```
   skill = LLM workflow
        │
        ├── has a deterministic slice? ──yes──► (B) ship a CLI verb for the slice
        │        (context-assembly /                  e.g. aida ultraplan, aida goal
        │         data / derivation, pure)            │
        │                                             └─ already shipped  OR  GAP ◄── payoff
        │
        ├── run unattended (cron/CI/headless)? ──yes──► (C) thin launcher: aida X run → claude -p /skill
        │
        └── neither ───────────────────────────────► (A) pure-agentic: NO CLI verb wanted
```

## Decisions

- **Three buckets, mutually exclusive.** A skill lands in exactly one of (A)
  pure-agentic, (B) has-deterministic-slice, (C) launcher-worthy. **Rationale**:
  a single skill can in principle have *both* a slice and a launcher (burndown
  does: `aida burndown plan` + `aida burndown run`), but for the purpose of "what
  is the open gap?" we classify by the *dominant* missing/present symmetry.
  Burndown is recorded as B (its slice is the exemplar) with the C launcher
  noted inline — it is the one skill that demonstrates the full pattern.
- **"Wraps an existing read-only CLI verb" is NOT bucket B.** Skills like
  `/aida-status`, `/aida-queue`, `/aida-search`, `/aida-standup`, `/aida-insights`
  already orchestrate over data the CLI fully exposes (`aida status`, `aida queue
  list`, `aida search`, `aida usage`, …). Their deterministic slice *already
  shipped as the underlying verb* — the skill is a thin narrative wrapper. These
  are classified **A** (no *new* CLI verb wanted): the determinism is already in
  the substrate; the skill adds only LLM framing. **Rationale**: bucket B is
  specifically "a slice that *should be* a verb but isn't surfaced as its own
  composable command," not "a skill that calls verbs."
- **Mutation skills with gated lifecycle effects are agentic (A), not B.**
  `/aida-commit`, `/aida-implement`, `/aida-pr`, `/aida-review`, `/aida-release`,
  `/aida-integrate` perform judgment-laden mutation (what to commit, how to
  phrase, whether CI is green-enough, whether to merge). The *deterministic
  sub-steps* they call are already verbs (`aida pr auto-queue-review`, `aida
  review prompt`, `aida changelog refresh`, …). What remains is judgment → A.
- **Launcher-worthy (C) is reserved for the genuinely-unattended set.** A
  launcher only earns its surface slot if someone would plausibly cron/CI it
  with no human in the loop. Today that is **burndown** (shipped) and, as
  candidates, **digest** and **insights** (both read-only narrative producers a
  team might want emailed weekly). Everything else stays human-invoked.

## Skill classification table (all 42)

Legend — Bucket: **A** pure-agentic (no CLI verb wanted) · **B** has a
deterministic slice that should be a CLI verb · **C** launcher-worthy. Slice
status: **shipped** = the slice verb already exists · **GAP** = slice exists
conceptually, no CLI verb · **n/a** = bucket A/C.

| # | Skill | Bucket | Deterministic slice | Slice status |
|---|-------|--------|---------------------|--------------|
| 1 | aida-advise | A | judgment: resolve-or-escalate a punt | n/a |
| 2 | aida-architecture | A | LLM structural review | n/a |
| 3 | aida-backlog-groom | **B** | ready-set + pairwise file-overlap + risk chips | **GAP** |
| 4 | aida-burndown | **B** (+C launcher) | ready set | shipped (`aida burndown plan`); launcher `aida burndown run` |
| 5 | aida-capture | A | conversation-scan → reqs (irreducibly agentic) | n/a |
| 6 | aida-code-review | A | LLM quality review | n/a |
| 7 | aida-commit | A | (deterministic sub-steps already verbs) | n/a |
| 8 | aida-compiler-warnings | **B** | parse+categorize `cargo` warnings by risk | **GAP** |
| 9 | aida-decompose | A | LLM vertical-slice decomposition | n/a |
| 10 | aida-digest | **B** (C candidate) | windowed work digest | shipped (`aida digest`) |
| 11 | aida-doc | A | judgment-laden doc capture (`aida doc add` exists) | n/a |
| 12 | aida-docs | A | doc-sync prose work | n/a |
| 13 | aida-docs-review | A | LLM docs quality review | n/a |
| 14 | aida-doctor | **B** | drift scan (stale leases/briefs/orphans/mismatches) | shipped (`aida doctor`) — heal step agentic |
| 15 | aida-drain-queue | **B** | safe /goal prompt assembly for queue drain | partial — `aida goal` ships the condition; the queue-drain prompt-assembly slice is a **GAP** |
| 16 | aida-ecosystem-scan | A | LLM competitor classification | n/a |
| 17 | aida-evaluate | A | AI requirement evaluation (`aida evaluate` exists) | n/a |
| 18 | aida-glossary | **B** | terminology-inconsistency scan across reqs+code | **GAP** |
| 19 | aida-grill | A | LLM decision-tree interrogation | n/a |
| 20 | aida-implement | A | the canonical agentic skill | n/a |
| 21 | aida-import-plan | **B** | detect target spec + parse sections + verify refs | partial — `aida plan verify` ships ref-lint; the *detect-target-spec* + *section-parse* slice is a **GAP** |
| 22 | aida-insights | A (C candidate) | wraps `aida usage` + calibration stats (verbs exist) | n/a |
| 23 | aida-integrate | A | merge-judgment (sub-steps `aida queue integrate` exist) | n/a |
| 24 | aida-learn | A | route a lesson to the right substrate | n/a |
| 25 | aida-onboard | A | interactive narrative onboarding | n/a |
| 26 | aida-pickup | A | producer/consumer loop (`aida queue next` exists) | n/a |
| 27 | aida-plan | **B** | plan-context assembly (the ultraplan slice) | shipped (`aida ultraplan`, `aida plan helpers`) |
| 28 | aida-pr | A | PR-body judgment (`git log` derive is trivial) | n/a |
| 29 | aida-punt | A | structured-pause judgment (`aida punt` exists) | n/a |
| 30 | aida-queue | A | read-only view (`aida queue list` exists) | n/a |
| 31 | aida-rebase | **B** | classify drift (clean/ahead/behind/diverged) | shipped (`aida rebase`) |
| 32 | aida-release | A | release judgment (`aida release`/`changelog` exist) | n/a |
| 33 | aida-req | A | add+evaluate (`aida add`/`aida evaluate` exist) | n/a |
| 34 | aida-review | **B** | per-spec review checklist/prompt assembly | shipped (`aida review prompt`/`assemble`) |
| 35 | aida-search | A | unified search (`aida search`/`aida grep` exist) | n/a |
| 36 | aida-sprint | A | sprint-grouping judgment (`aida` add/edit exist) | n/a |
| 37 | aida-standup | **B** | windowed commit + status-change roll-up | **GAP** |
| 38 | aida-status | A | one-shot status (`aida status` exists) | n/a |
| 39 | aida-sync | A | template-sync (`make sync-templates`) | n/a |
| 40 | aida-techdebt | **B** | duplication/dead-trace/orphan scan | **GAP** |
| 41 | aida-test | A | LLM test generation | n/a |
| 42 | aida-triage | A | bug investigation + inbox disposition judgment | n/a |

(42 rows = all 42 skills: 41 `.md` files + the `aida-pr/` directory-form skill.
Burndown carries both its B slice and its C launcher and is counted once as B.)

## Bucket B — shipped vs GAP

**Already shipped (the slice exists as a CLI verb) — these are the exemplars
the convention generalizes from:**

- `aida ultraplan` / `aida plan helpers` ← /aida-plan, /ultraplan
- `aida review prompt` / `aida review assemble` ← /aida-review
- `aida burndown plan` ← /aida-burndown (and launcher `aida burndown run`)
- `aida goal` ← /goal (and the /aida-drain-queue condition)
- `aida digest` ← /aida-digest
- `aida rebase` ← /aida-rebase
- `aida doctor` ← /aida-doctor (scan slice; heal step stays agentic)
- `aida plan verify` ← partial slice of /aida-import-plan

**GAPS — the slice is well-defined and pure, but there is no CLI verb (this is
the payoff of the audit):**

1. **aida-techdebt** → the duplication / copy-pasted-trace / dead-trace (→Rejected) / orphan-file scan is fully deterministic. No verb.
2. **aida-compiler-warnings** → parse `cargo` warning output, categorize by risk, emit a prioritized table. Fully deterministic. No verb.
3. **aida-standup** → windowed (since-yesterday) commit list + status-change roll-up. Deterministic time-series read over commits + history. No verb.
4. **aida-glossary** → cross-corpus terminology-inconsistency scan (reqs + code). Deterministic. No verb (`aida db glossary` exists for *storage*, not the *scan*).
5. **aida-backlog-groom** → the ready-set + pairwise file-overlap + risk-chip computation. `aida burndown plan` ships the *burndown* ready-set; the *grooming* view (overlap matrix for parallel-safety) is a distinct, un-surfaced slice.
6. **aida-import-plan** → detect-target-spec + section-parse slice (ref-lint already ships as `aida plan verify`).
7. **aida-drain-queue** → the queue-drain /goal-prompt-assembly slice (the `aida goal` condition ships; the full safe-prompt assembly does not).

## The formal convention (3–5 lines)

> **Skill ↔ CLI symmetry rule.** A skill exposes a CLI verb **iff** it has a
> deterministic slice — a pure context-assembly / data / derivation op that is
> testable and reusable independent of any LLM. Agentic skills (judgment,
> NL reasoning, prose) get a CLI verb **only** as a thin launcher
> (`aida X run` → `claude -p /skill`) and **only** if they're run unattended
> (cron/CI/headless). **Never ship a hollow CLI clone** of an agentic skill —
> that is surface bloat with no determinism gain (STORY-556). This mirrors the
> established CLI↔MCP rule (STORY-82): when a deterministic surface gains a
> filter/field/op, mirror it onto the adjacent surface so the two don't drift —
> here the adjacent surface is the *skill's deterministic slice*, not the whole
> skill.

## Prioritized GAP list — candidate follow-up TASKs (operator approval required — NOT auto-filed)

Ranked by (leverage of the slice as a reusable/CI-able verb) × (how cheap the
extraction is, since the deterministic logic already exists inside the skill):

| Rank | Candidate verb | From skill | Why it earns a verb | Est. |
|------|----------------|------------|---------------------|------|
| P1 | `aida techdebt scan [--json]` | aida-techdebt | Pure scan; high CI value (gate on dup/dead-trace count); slice already fully specified in the skill body. | S |
| P2 | `aida standup [--since <window>] [--json]` | aida-standup | Deterministic commit+history time-series; trivially cron-able; reuses existing history-walk helpers. | S |
| P3 | `aida warnings scan [--json]` | aida-compiler-warnings | Parse+categorize `cargo` warnings; CI-gateable; pure transform over compiler output. | S |
| P4 | `aida glossary scan [--json]` | aida-glossary | Cross-corpus terminology-inconsistency scan; reusable by docs tooling; storage half (`aida db glossary`) already exists. | M |
| P5 | `aida backlog overlap [--json]` | aida-backlog-groom | Pairwise file-overlap matrix for parallel-safety; complements `aida burndown plan`; feeds the multi-agent dispatcher. | M |
| P6 | `aida import-plan detect <file>` | aida-import-plan | Target-spec detection + section parse; rounds out the `aida plan verify` slice already shipped. | M |
| P7 | `aida drain-queue prompt [--role R]` | aida-drain-queue | Safe /goal-prompt assembly for queue drain; extends the `aida goal` condition into a full paste-ready prompt. | M |

**Launcher candidates (bucket C, lower priority — only if unattended demand is
real):** `aida digest run` and `aida insights run` (→ `claude -p`) for teams
that want a weekly narrative emailed; burndown already ships its launcher.

These are **candidates only.** Per the SPIKE acceptance and the
one-master-advisor convention, enacting any of them is a separate follow-up
spec requiring master sign-off — this doc files nothing.

## Risks + gotchas

1. **Risk**: over-extracting verbs turns into the 1:1-clone anti-pattern the
   operator explicitly rejected. **Mitigation**: the convention's `iff
   deterministic slice` clause is the gate; every P-row above is a *pure*
   transform, not a wrapped LLM call. Re-test each against "would this verb be
   useful to a script with no LLM in the loop?" before approving.
2. **Risk**: a "slice" extraction quietly drifts from its parent skill (the
   skill keeps its own copy of the logic). **Mitigation**: when a slice becomes
   a verb, the skill must *call the verb*, not re-implement it — same discipline
   as the CLI↔MCP mirror (STORY-82).
3. **Risk**: launcher proliferation (every skill gets an `X run`).
   **Mitigation**: bucket C is gated on *demonstrated unattended demand*; today
   only burndown qualifies, with digest/insights as watch-items.

## Tests (named, not "add tests")

n/a — analysis doc, no code. Each follow-up TASK (if approved) carries its own
test obligations (a `--json` golden for the scan verbs; a snapshot for the
prompt-assembly verbs).

## Verification (executable)

```bash
# The doc exists with the required header + trace marker:
grep -q 'trace:SPIKE-55' docs/plans/2026-06-10-skill-cli-symmetry-audit.md
grep -q 'Status: Complete' docs/plans/2026-06-10-skill-cli-symmetry-audit.md
# All 42 skills are accounted for in the classification table:
ls aida-core/templates/skills | wc -l   # 42 entries = 41 .md files + aida-pr/ dir
```

## Followups

- (operator-gated) File the P1–P7 candidate TASKs above after sign-off.
- (operator-gated) Decide digest/insights launcher (bucket C) on real demand.
- When any slice verb ships, update the parent skill to *call* it (no re-impl).

## Related

- STORY-556 — don't bloat the surface (the constraint this convention encodes).
- STORY-82 — CLI↔MCP mirror (the adjacent established symmetry this generalizes).
- Shipped slice exemplars: `aida ultraplan`, `aida review prompt`,
  `aida burndown plan` (+`run`), `aida goal`, `aida digest`, `aida rebase`,
  `aida doctor`, `aida plan verify`.
