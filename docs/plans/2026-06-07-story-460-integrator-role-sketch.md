# Plan: STORY-460 — Integrator role (DESIGN SKETCH, no implementation)

Date: 2026-06-07
Specs: STORY-460, EPIC-31
Status: Draft (design sketch — operator sign-off pending)
Complexity: sketch only; see "Smallest slice" for the build estimate

<!--
  This is a DESIGN SKETCH per the 2026-06-06 operator decision on STORY-460:
  "DEFER with the EPIC-31 cluster — the remaining role-scaffolding infra wants
  the same design pass as the EPIC-31 launcher/registry cluster, not a
  standalone bugs-phase ship."

  No code changes are proposed for the bugs-before-marketing phase. This sketch
  documents how the integrator role SHOULD fit EPIC-31's launcher/registry when
  that cluster is unparked. Prefer SYMBOL refs over LINE refs. trace:STORY-460
-->

## Approach

The integrator is a fourth **agent role** (alongside implementer / reviewer /
advisor) whose job is to drain the *mechanical* merge-cascade workload that
today lands on the master advisor: rebase open PRs onto a moved base, resolve
mechanical conflicts, watch CI, squash-merge clean+verdicted PRs, delete merged
branches, and run `aida pull`. It is a **load-balancing role, not a subsystem
advisor** — it has zero design authority. The moment a conflict needs judgment
(semantic overlap, missing verdict, real test failure) it **escalates to the
one master advisor**, who keeps final say.

The role *identifier* already exists and validates (`AGENT_ROLES` in
`aida-cli/src/main.rs` lists `"integrator"`; `validate_registered_agent_role`
accepts it; `--role integrator` and `aida queue add --for-role integrator`
already route). What's unbuilt is the **role-context substrate**: the discipline
the integrator loads at launch. Per the operator decision this rides the EPIC-31
launcher/registry design pass rather than shipping standalone — because the
role-context an integrator gets at `aida agent new claude --role integrator`
is produced by the *same* `render_agent_launch_context` / `role_guidance_for`
path EPIC-31 owns. Building integrator-context in isolation would fork that path.

### Diagram — where the integrator sits

```
  N implementers ──PRs──►  open-PR pool (stale-base churn = O(N^2))
                                  │
                                  ▼
                        ┌───────────────────┐
                        │   INTEGRATOR       │  load-balancing role
                        │  (mechanical loop) │  NO design authority
                        └─────────┬─────────┘
            in-scope ─────────────┤
            rebase / mech-conflict │ escalate (out-of-scope)
            CI-watch / squash-merge│        │
            branch-delete / pull   │        ▼
                                   │   ┌──────────────┐
                                   │   │ MASTER ADVISOR│ final say
                                   │   └──────┬───────┘
                                   │     routes back ▼
                                   │   reviewer queue / implementer brief
                                   ▼
                              merged + `aida pull`
```

### Launch path it rides (EPIC-31)

```
aida agent new claude --role integrator
   └─ prepare_agent_launch_dry / real launch
        └─ render_agent_launch_context()        # STORY-436
             └─ role_guidance_for(root,"integrator")   # <-- the gap (see Decisions)
                  ├─ load_role(root,"integrator")  → .aida/roles/integrator.toml
                  └─ fallback match arm            → built-in integrator guidance
```

## Decisions

- **Decision: integrator is an *agent* role, scaffolded into the agent-wired
  set — not a generic operator persona.** **Rationale**: STORY-460's whole
  point is delegation to a launchable seat. The agent-role taxonomy
  (`AGENT_ROLES`) is the right home; it already lists `integrator`. The
  *operator-persona* starter set (`STARTER_ROLES`, scaffolded by `aida init`)
  today deliberately ships only implementer/advisor/reviewer and a test
  (`starter_set_is_agent_wired_only`) guards that set. Adding integrator to
  `STARTER_ROLES` is a real change that **needs operator sign-off** because it
  flips that guarded invariant.

- **Decision: resolve the markdown-vs-TOML substrate mismatch in favor of the
  existing TOML role file + a built-in fallback, NOT a new
  `aida-core/templates/roles/*.md` tree.** **Rationale**: STORY-460's acceptance
  literally says `aida-core/templates/roles/integrator.md` and
  `.aida/roles/integrator.md`. But the *actual* role substrate today is TOML
  (`.aida/roles/<name>.toml`, `RoleState` with `purpose` / `system_prompt`
  fields read by `load_role` and surfaced by `role_guidance_for`). No
  `aida-core/templates/roles/` directory exists. Inventing a parallel markdown
  role-file format would fork the substrate. **Recommended shape**: (a) the
  durable integrator discipline lives as a discipline-pack page
  (`aida-core/templates/docs/aida/discipline/integrator-role.md`, mirroring
  `advisor-role.md` / `implementer-discipline.md`); (b) the launch-time guidance
  comes from a new `integrator` arm in `role_guidance_for` plus an optional
  scaffolded `.aida/roles/integrator.toml` whose `system_prompt` points at that
  discipline page. This keeps one role substrate (TOML) and one discipline
  substrate (the pack). The acceptance criterion's `.md` paths should be
  re-spec'd to this shape — flagged for sign-off.

- **Decision: close the `role_guidance_for` integrator gap as part of this
  work.** **Rationale**: `role_guidance_for` has arms for advisor/implementer/
  reviewer/unspecified and a generic `other =>` fallback. `integrator`
  currently hits `other =>` ("No stored role file was found…"), so an integrator
  launched today gets *no* role guidance. The minimal fix is a dedicated arm.

- **Decision: escalation is the existing file-based handshake, not a new
  channel.** **Rationale**: the autonomy/escalation architecture
  (`docs/architecture/autonomy-and-escalation.md`) already defines the
  implementer→advisor→human cascade over the brief/finding substrate. The
  integrator reuses it: a finding for the advisor's attention, a brief to route
  work to reviewer/implementer queues. No new transport.

- **Decision: defer the *active* loop (`aida-integrate` skill, `aida queue
  integrate`) to follow-up specs.** **Rationale**: STORY-460 already scopes
  those as separate. This sketch covers only the role + context + escalation,
  matching the "smallest valuable slice."

## Role definition

### Scope IN (the integrator does this autonomously)

- Rebase an open PR branch onto the moved base (default branch advanced).
- Resolve **mechanical** conflicts only:
  - whitespace / formatter drift (`cargo fmt` re-runs),
  - import-list / use-statement **unions** (both sides add distinct imports),
  - non-overlapping same-file additions (two PRs append to different regions),
  - lockfile / generated-artifact regen.
- Push the rebased branch.
- Watch CI on the rebased branch; re-trigger genuinely stale/flaky CI
  (note the `gh run rerun` vs fresh-CI distinction — a rerun re-tests the same
  SHA; a real re-test needs an empty commit or `gh workflow run --ref`).
- Squash-merge **only** when CI is green **and** a reviewer verdict is present.
- Delete the merged branch; run `aida pull` (auto-bumps Done→Completed).
- Order the rebase loop by dependency (`BlockedBy` chains, `--depends-on`
  brief order) so a base lands before its dependents rebase.

### Scope OUT (escalate — never decide)

- **Semantic conflict**: two PRs change the *same lines* with different intent →
  escalate to advisor (the advisor decides; may route a reconciliation brief).
- **Missing reviewer verdict**: PR is CI-green but unreviewed → route to the
  **reviewer** queue (`aida queue add --for-role reviewer`), do not merge.
- **Real (non-flaky) test failure**: distinguish flaky from real; a real failure
  → brief the **original implementer** (`aida brief <agent> <SPEC>`), do not
  "fix" it (that's implementation, out of role).
- **Anything needing a design call** (API shape, behavior change to resolve a
  conflict, scope question) → escalate to advisor.

The discriminator the integrator must apply: *"can I resolve this without
deciding what the code should mean?"* Yes → in-scope. No → escalate.

## Role-scaffolding & EPIC-31 launcher/registry fit

Three artifacts, all riding the EPIC-31 launcher/registry pass:

1. **Discipline page** — `aida-core/templates/docs/aida/discipline/integrator-role.md`
   (master template, embedded via `build.rs`, mirrored to
   `.aida/roles/` consumers via the discipline pack). Sibling of
   `advisor-role.md` / `implementer-discipline.md`. Add a row to the discipline
   `README.md` pointer-table. This is the *durable, read-on-demand* discipline.

2. **Launch-time guidance** — a new `integrator` arm in `role_guidance_for`
   (closes the `other =>` fallback gap) so
   `aida agent new claude --role integrator --show-context` renders real
   integrator guidance. This is the EPIC-31 touch-point: the role-context
   injection EPIC-31 Phase 5 owns is exactly `render_agent_launch_context` →
   `role_guidance_for`. Integrator guidance lands *in that path*, not beside it.

3. **Optional scaffolded role file** — `.aida/roles/integrator.toml` (TOML,
   matching the existing `RoleState` substrate), with `purpose` set and
   `system_prompt` pointing at the discipline page. Decide at sign-off whether
   to add `integrator` to `STARTER_ROLES` (auto-scaffolded by `aida init` /
   `aida role scaffold`) — this flips the `starter_set_is_agent_wired_only`
   invariant and is the single most consequential sign-off item.

Why ride EPIC-31 rather than ship standalone (the operator's framing): the
role-context an integrator receives is produced by the launcher's context
renderer and the role-file loader — both EPIC-31 surfaces. Building integrator
context now would either (a) duplicate that path or (b) bake a one-off arm that
the EPIC-31 launcher pass then has to reconcile. Deferring keeps one launch path.

## Integrator → advisor escalation handshake

Reuses the file-based handshake substrate (no new channel):

1. **Integrator detects an out-of-scope condition** (per Scope OUT) during the
   rebase/merge loop.
2. **Integrator files a finding** (advisor-attention carrier) describing: the
   PR(s), the specific blocker class (semantic-conflict / missing-verdict /
   real-test-fail / design-call), and the conflicting hunks / failing test
   names. It does **not** mutate the code or the verdict.
3. **Integrator parks that PR** and **continues the loop** on the remaining
   PRs (punt-and-continue — one blocked PR never halts the cascade; mirrors the
   EPIC-28 resilient-drain discipline and the parallel-fanout burn-down rule).
4. **Advisor triages** the finding:
   - semantic conflict → advisor resolves or files a reconciliation brief,
   - missing verdict → advisor routes to reviewer queue,
   - real test fail → advisor briefs the original implementer,
   - design call → advisor decides (final say).
5. **Re-entry**: once the advisor's routed work lands (verdict added, conflict
   reconciled, implementer pushes a fix), the PR re-enters the integrator's loop
   on the next pass. State lives in the substrate (finding status + PR state),
   so a fresh integrator session resumes correctly — no in-memory handoff.

Escalation is **always upward and one-directional from integrator's view**: the
integrator never makes the call, it routes to the seat that can. The advisor may
delegate downward (to reviewer/implementer) but the integrator never does design
work itself.

## Relationship to the one-master-advisor principle

Per `feedback_one_master_advisor_until_subsystems`:

- The integrator is a **load-balancing role**, taking *mechanical* throughput
  off the advisor — it is **not** a subsystem advisor and holds **no** design
  authority. There is still exactly **one master advisor** with final say.
- Every integrator escalation lands at that one advisor. The integrator never
  resolves a design fork, never overrides a reviewer verdict (note
  `feedback_trust_reviewer_over_dialog_intuition`: verdicts come from reviewers,
  reading code — the integrator only checks a verdict *exists*, never substitutes
  its own).
- This is consistent with the EPIC-31 framing where roles map to orchestrator
  *stages*: integrator = the integration stage's mechanical executor; advisor =
  the escalation/advisory tier above it.
- Future subsystem-scoped advisors (SPIKE-10) don't change this: integrator
  stays a single mechanical seat; if multiple integrator *instances* run (one
  per agent, via STORY-459 `--for-agent` + TASK-542 stable names), they still
  all escalate to the one master advisor until subsystem advisors exist.

## Smallest slice + what needs operator sign-off

### Smallest valuable slice (when EPIC-31 unparks)

1. `integrator` arm in `role_guidance_for` (closes the fallback gap).
2. `aida-core/templates/docs/aida/discipline/integrator-role.md` + README
   pointer-table row.
3. `.aida/roles/integrator.toml` scaffold (idempotent), `system_prompt` →
   discipline page.
4. CLAUDE.md / discipline-pack section naming the role + scope-in/out + the
   escalation handshake.
5. Test: `aida agent new claude --role integrator --show-context` renders the
   integrator guidance; `aida queue add --for-role integrator` routes (already
   works — assert it stays working).

Deliberately OUT of the smallest slice (STORY-460 already defers): the
`aida-integrate` skill, `aida queue integrate`, per-agent dispatch.

### Needs operator sign-off

1. **Substrate shape**: accept re-spec'ing STORY-460's `*.md` role-file
   acceptance to "TOML role file + discipline-pack page + `role_guidance_for`
   arm" (the actual substrate), rather than inventing an `aida-core/templates/
   roles/*.md` tree. (Recommended.)
2. **STARTER_ROLES membership**: should `integrator` auto-scaffold via
   `aida init` / `aida role scaffold` (flipping the guarded
   `starter_set_is_agent_wired_only` invariant), or stay opt-in via
   `aida role add` until multi-agent scale justifies it? Given bugs-before-
   marketing + the deferral, recommend **opt-in for now**, promote to starter
   set when the active integrator loop (`aida-integrate`) ships.
3. **Timing**: confirm this stays parked with the EPIC-31 cluster (no
   bugs-phase build), consistent with the 2026-06-06 operator decision and the
   2026-06-06 "PARKED for keyboard" note.

## Followups

- File the active-loop specs (`aida-integrate` skill, `aida queue integrate`)
  as EPIC-31-cluster children when the cluster unparks.
- When STORY-459 (`--for-agent`) + TASK-542 (stable agent names) land, integrator
  briefs can target specific agent instances.

## Related

- STORY-460 (this spec), EPIC-31 (launcher/registry cluster this rides)
- STORY-459, TASK-542, TASK-541 (compose-with: dispatch / names / ordered briefs)
- `docs/architecture/autonomy-and-escalation.md` (escalation cascade substrate)
- `aida-core/templates/docs/aida/discipline/advisor-role.md`,
  `implementer-discipline.md` (sibling discipline pages / format)
- `feedback_one_master_advisor_until_subsystems`,
  `feedback_trust_reviewer_over_dialog_intuition`,
  `feedback_parallel_implementer_fanout_burndown` (the principles this honors)
- Code touch-points (symbol refs): `AGENT_ROLES`, `AGENT_ROLE_INFOS`,
  `validate_registered_agent_role`, `role_guidance_for`,
  `render_agent_launch_context`, `scaffold_starter_roles` / `STARTER_ROLES`,
  `load_role` (all in `aida-cli/src/main.rs`)
