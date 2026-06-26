# Plan: Reliable fasttrack intake-to-implementation lane (TASK-0438)

Date: 2026-06-26
Specs: TASK-0438 (child of EPIC-0428 advisor-autopilot)
Status: Draft
Complexity: design-only; ~0 prod LOC this slice; follow-ups sized below

<!--
  DESIGN slice. The deliverable is this doc + the follow-up specs it names.
  No code lands in this PR. trace:TASK-0438
-->

## Approach

AIDA already has two halves of a fast lane but they don't compose into a
*reliable* request-to-ship pipe. `aida fasttrack` files-and-queues genuinely
**trivial** work (Approved + `batch:fasttrack` + `lifecycle:no-review`, CI the
only gate). `aida assess` (formerly `aida intake`) is the advisor's headless
**disposition** pass over open drafts. The gap TASK-0438 names is the
operator's actual want: "I have an easy bug or a small feature; I want the
advisor to *quickly* dispose of it (accept/reject/dedupe/clarify) and, if
accepted, for it to *reliably reach implementation* instead of becoming another
backlog item I have to nudge." Today nothing guarantees that last hop — an
Approved item can sit un-queued, an item that turns out non-trivial proceeds
silently, and there is no status surface telling the operator where their
request is.

This design defines **two named tiers on one lane** and the glue that makes the
hop from intake to implementation dependable:

- **Trivial fasttrack** (existing): cosmetic/doc/one-line. CI-only gate
  (`lifecycle:no-review`). No change to behavior — we just give it a name and
  fold it into the shared status surface.
- **Quick-fix express** (new tier): easy bugs / small single-purpose features
  that still get **normal CI + reviewer** but ride an **express disposition +
  guaranteed-routing** contract so they don't strand. Reviewed, not
  review-skipped.

The reliability fix is three concrete pieces: (1) an **express disposition
contract** so the advisor accept/reject/dedupe/clarify decision is bounded and
prompt; (2) a **guaranteed-routing rule** — accept *implies* queue-and-drain,
never "filed as draft"; (3) a **punt-out invariant** — if the work turns
non-trivial mid-implementation it leaves the express tier loudly
(`NeedsAttention` + finding), it never silently downgrades its own gate.

### Diagram

```
  operator request
        │
        ▼
  ┌───────────────┐   express disposition (advisor, bounded SLA)
  │  INTAKE        │   accept │ reject │ dedupe │ clarify │ escalate
  └──────┬────────┘   (each with a recorded reason)
         │ accept
   ┌─────┴───────────────────────────┐
   │ which tier?  (safety-exclusion   │
   │  litmus + blast-radius check)     │
   └──┬──────────────────────┬─────────┘
      │ trivial               │ quick-fix
      ▼                       ▼
 batch:fasttrack         batch:express
 lifecycle:no-review     (NO lifecycle skip — full gate)
      │                       │
      └───────────┬───────────┘
                  ▼  guaranteed routing: accept ⇒ queued, never draft
        aida queue work --batch <bucket> --auto-complete
                  │
     implement → CI → [review*] → merge → pull(auto-bump→Completed)
                  │
       turns non-trivial?  ──► PUNT OUT: NeedsAttention + finding
                  │            (drop the lane tags, route to normal review)
                  ▼
              shipped (Completed)        * review skipped only for trivial tier
```

## Decisions

- **Two tiers, one lane — not a third pipeline.** **Rationale**: the operator's
  ask is a *spectrum* (trivial → easy-bug → small-feature), not a new category.
  Reusing the existing `batch:` + `lifecycle:` + `aida queue work
  --auto-complete` machinery means zero new orchestrator code; the lane is a
  *convention + skill*, consistent with how `batch:fasttrack` already works.
  The two tiers differ on exactly one axis: **does it skip the human-review
  ceremony?** Trivial = yes (`lifecycle:no-review`), quick-fix = no.

- **New tier tag: `batch:express`, never a new `lifecycle:` skip.**
  **Rationale**: the quick-fix tier's whole point is that it is *reliably
  routed*, not *less gated*. It keeps full CI + reviewer. `batch:express` is a
  routing bucket only — it composes with `aida queue work --batch express
  --auto-complete` the same way `batch:fasttrack` does, but carries **no**
  `lifecycle:*` tag so all six phases run. This is the line between
  "fast-because-trusted-trivial" and "fast-because-prioritized-and-routed."

- **Accept ⇒ queued-and-drained, by policy, not by hope.** **Rationale**: the
  named failure is "filed as draft, then stranded." The express contract makes
  acceptance *mean* "on the queue with a drain owner," reusing the existing
  `[intake] on_apply = drain` knob (and `aida assess --then-drain`) so an
  accepted express item chains straight into a burndown drain. For the
  human-at-keyboard path, `aida fasttrack` already files **Approved + queued**
  in one shot; the express variant does the same plus the `batch:express` tag.

- **Punt-out is an invariant enforced by the existing shelving machinery, not a
  new code path.** **Rationale**: EPIC-28 already parks a shelvable failure to
  `NeedsAttention` and continues the drain. The express-tier rule is a
  *discipline* layered on top: an implementer who discovers the work is
  non-trivial (touches architecture/security/autonomy, needs a judgment call,
  grows multi-file/high-blast-radius) **punts** (`/aida-punt` →
  `NeedsAttention`) and the skill instructs dropping the `batch:express` tag so
  the item re-enters normal review. We do **not** invent a "silently re-gate"
  path — that would defeat the trust contract.

- **Status surface is a view over existing status + tags, not a new store.**
  **Rationale**: the lifecycle states already encode requested→shipped
  (Draft≈requested, Approved≈accepted, Planned≈queued, InProgress≈running,
  NeedsAttention≈blocked/punted, Done/Completed≈shipped, Rejected≈rejected). A
  `aida fasttrack status` (or `--lane` filter on `aida queue list`) is a
  *projection* over `batch:fasttrack`/`batch:express` rows mapped onto those
  states — no new persistence. Keeps it a Trojan-horse-depth surface, not new
  ceremony.

- **Safety exclusions are a hard fence, enforced where disposition happens.**
  **Rationale**: the express tier is only safe because risky classes can never
  enter it. We reuse the `aida assess` fence (do-not-approve classes, `risk`
  ceiling, `needs-human`/`strategic`/`keystone`/`architecture`/`security`/
  `blast-radius:high` tags) and add an **express-eligibility litmus** (below)
  the skill must pass before applying `batch:express`.

## The express-eligibility litmus (entry criteria)

A request is **express-eligible** only if ALL hold. Fail any one ⇒ it goes to
the normal flow (filed Approved/Draft for the standard burndown), not express.

1. **Bounded & single-purpose** — one bug or one small feature, describable in a
   sentence; not an epic, not "and also."
2. **Low blast radius** — small file count, no public-API/struct/signature
   change that ripples to callers (the "build combined main" hazard class).
3. **Reversible** — a revert cleanly undoes it; no migration, no data write, no
   irreversible external effect.
4. **Excluded classes are absent**: NOT architecture, NOT security, NOT autonomy
   machinery (orchestrator/drain/lease/queue/conflict), NOT cross-cutting
   lifecycle, NOT high-risk, NOT ambiguous (acceptance is clear or trivially
   inferable), NOT `vision/epic/principle/constraint/decision/term` types.
5. **Trivial-vs-quick-fix split**: if it is *also* cosmetic/doc/one-obvious-line
   ⇒ **trivial tier** (`batch:fasttrack` + `lifecycle:no-review`). Otherwise, if
   it passes 1–4 but has real behavior ⇒ **quick-fix tier** (`batch:express`,
   full gate). When in doubt between tiers, choose the *more-gated* one
   (express over trivial); when in doubt about eligibility at all, **do not use
   the lane** — file normally.

This litmus is the same risk vocabulary the `assess` fence and EPIC-0428
autopilot envelope already use, so the three stay consistent.

## The express disposition contract (advisor side)

When an express request arrives, the advisor's disposition is **bounded and
prompt** — the operator's trust is in *speed of decision*, not just speed of
build. Five verbs, each with a recorded one-line reason (all already exist in
`aida assess` / `/aida-assess`):

- **accept** — eligible per litmus + worth doing now ⇒ apply the tier tag,
  queue it, and (per policy) drain. `aida edit <ID> --status approved` +
  `--tags batch:express` (or `aida fasttrack` for the trivial tier).
- **reject** — stale / out-of-scope / not-worth-it ⇒ `aida edit <ID> --status
  rejected` + a comment naming why.
- **dedupe** — duplicate of an existing spec ⇒ reject + link to the original
  (`aida comment add` naming the superseder). (Dedupe is reject-with-a-pointer;
  there is no standalone `aida dedupe`.)
- **clarify** — acceptance is ambiguous ⇒ `/aida-clarify` / `aida questions
  ask <ID>` to get the missing criterion, then re-dispose. Ambiguous items
  **never** enter the lane un-clarified.
- **escalate** — touches an excluded class or needs an operator strategic call
  ⇒ park `NeedsAttention` / `needs-human` for the operator. This is the
  EPIC-0428 boundary: the express lane disposes the *ordinary* low-risk
  request; anything keystone/architecture/security escalates.

The headless analog is `aida assess --apply --then-drain` scoped to the lane
(`--only-tag` the lane bucket), which makes the cold-boot advisor run the same
accept/reject/park/queue dispositions and then drain — but the propose-mode
review stays load-bearing for anything the operator wants to eyeball first.

## Reliability concerns — what makes it UNreliable today + the fix

1. **Stranded-after-accept** (the headline failure). *Today*: `aida assess`
   without `--then-drain` (or a human approving but not queuing) leaves an
   Approved item that no drain owns; it sits until someone runs a burndown.
   **Fix**: the accept verb in the express contract **implies routing** — for
   the headless path bind `on_apply = drain` (or `--then-drain`); for the
   keyboard path `aida fasttrack` files Approved-*and*-queued atomically. The
   express variant must do the same. *Followup A* adds the tag/wrapper so this
   is one command, not a remember-to-also-queue step.

2. **Silent re-gating** (the trust-killer). *Today*: nothing stops an
   implementer who finds the work is bigger than billed from just... doing it
   anyway under the reduced gate. **Fix**: the punt-out invariant — discovering
   non-triviality is a **punt** (`NeedsAttention` + finding + drop the lane
   tag), making it visible. EPIC-28 already provides the park-and-continue
   mechanics; the skill encodes the discipline.

3. **No visible status** (the nudging tax). *Today*: the operator can't see
   "where is my request" without reading raw statuses. **Fix**: the
   `requested→accepted→queued→running→blocked→punted→shipped→rejected`
   projection over the lane buckets (Followup B), so a single
   `aida fasttrack status` answers it.

4. **Tier confusion** (trivial gate applied to a real change). *Today*: the
   only lane is the trivial one, so an operator wanting a "quick but real" fix
   is tempted to mis-use `lifecycle:no-review`. **Fix**: the explicit
   `batch:express` tier that is fast *without* skipping review removes the
   incentive to mis-tag.

5. **Drain-induced parking on a real spec** (don't E2E on production work).
   *Today*: an express item that hits a transient CI flake parks
   `NeedsAttention`. **Fix**: this is correct behavior, not a bug — it's the
   visible-failure contract; the status surface shows `blocked`, the operator
   (or `--max-failures`) decides. Document it so it doesn't read as a regression.

## How it composes with the existing surfaces

- **`aida fasttrack` / `batch:fasttrack`** — unchanged; becomes the *trivial
  tier* of the named lane. The express tier is its sibling.
- **`aida queue work --auto-complete`** — the shared drain engine. `--batch
  fasttrack` and `--batch express` are two filtered drains over it. Trivial
  carries `lifecycle:no-review` (phase 3 skipped); express carries no
  `lifecycle:*` (all phases run). Merge + pull never skip in either.
- **`aida assess` / advisor autopilot (EPIC-0428)** — the express disposition
  contract IS a bounded autopilot envelope: ordinary low-risk requests get
  autonomous accept/queue/drain; excluded classes escalate. This task is a
  concrete instance of the EPIC-0428 envelope, scoped to operator-requested
  quick fixes.
- **solo / zen / `--no-human` modes** — express runs the same under each:
  `--no-human=both` makes it fully headless (implementer + reviewer); zen keeps
  the advisor on standby; solo lets it ride the safe-backlog loop. The express
  *tier tag* is orthogonal to the *autonomy mode*.
- **intake (`aida assess`)** — the cold-boot intake pass can dispose express
  requests with `--only-tag express --apply --then-drain`.
- **lifecycle tags** — trivial uses `lifecycle:no-review` only; express uses
  none. Neither uses `lifecycle:no-ci-wait`/`no-build`/`trivial` (those merge
  optimistically — out of bounds for a trust lane).
- **findings / punts** — a punted-out express item surfaces in `aida findings
  list` (`from-implementer:` source) and `aida queue list`'s NeedsAttention
  slot, same as any shelved spec.
- **defer / archive** — a deferred or archived spec is fenced out of intake and
  therefore can't be express-disposed; the three view tiers (active / deferred /
  archived) are orthogonal to the lane.
- **auditability & revert** — every disposition is a recorded status edit +
  comment in the orphan-branch git log (`aida history --events`), so a
  misclassified item is fully traceable; revert is a normal PR revert (the
  reversibility litmus guarantees this is clean), and the spec re-opens via
  `aida edit --status` for re-disposition.

## Follow-up specs (file these; do NOT build here)

This is the design slice. The implied scaffolding is small and lands as
separate specs so the design can be reviewed first.

- **Followup A — `batch:express` tier + filing verb.** A thin `aida fasttrack
  --tier express` (or sibling `aida express`) that files Approved + queued +
  `batch:express` (no `lifecycle:*`), mirroring how `aida fasttrack` owns the
  trivial tier's filing convention. ~30 LOC + cli flag. (TASK/STORY-sized.)
- **Followup B — `aida fasttrack status` lane projection.** Read-only view
  mapping `batch:fasttrack`/`batch:express` rows onto
  requested→accepted→queued→running→blocked→punted→shipped→rejected. Reuses
  cache; no new store. (TASK-sized.)
- **Followup C — `/aida-fasttrack` skill: add the express tier + litmus +
  punt-out discipline.** Extend the existing skill (and its master template)
  with the two-tier split, the express-eligibility litmus, and the explicit
  punt-out step. (Docs/template-sized; glyph-lint gate.)
- **Followup D — wire express into the EPIC-0428 autopilot envelope.** Make the
  express disposition contract the canonical "ordinary low-risk request"
  authority class in the autopilot policy, so the envelope and the lane share
  one fence. (Depends on EPIC-0428 design landing.)

## Risks + gotchas

1. **Risk**: `batch:express` becomes a dumping ground for "I just want it fast"
   non-trivial work, eroding the gate. **Mitigation**: the litmus is the
   bouncer; the express tier keeps **full** review/CI, so the only thing being
   bought is *routing speed*, not *less scrutiny* — removing the incentive to
   mis-tag. Periodic slop-audit of the bucket.
2. **Risk**: accept-implies-drain fires an unsupervised headless drain on
   something subtly risky. **Mitigation**: the fence + `--risk` ceiling +
   do-not-approve classes gate entry; propose-mode (`aida assess` without
   `--apply`) stays available for anything the operator wants to eyeball.
3. **Risk**: a non-trivial discovery is "fixed in place" instead of punted,
   shipping a real change under a reduced gate (trivial tier only).
   **Mitigation**: punt-out invariant in the skill + the fact that trivial-tier
   work is by definition tiny; if it grew, that's the punt trigger.
4. **Risk**: the status projection drifts from real status. **Mitigation**:
   it's a *derived view* over the cache (no second source of truth); rebuilds
   with the cache.
5. **Risk**: scope-creep into building all four followups in this PR.
   **Mitigation**: this slice is the doc only; followups are filed, not built.

## Tests (named — for the followups, not this doc)

- (Followup A) `fasttrack_express_files_approved_queued_with_batch_express_no_lifecycle`
- (Followup A) `fasttrack_express_carries_no_lifecycle_skip_tag` — invariant.
- (Followup B) `lane_status_maps_needsattention_to_blocked_or_punted`
- (Followup C) skill lint: glyph-lint + `aida plan verify` on any plan ref.

## Verification

This slice is docs-only; verification is the lint + a plan self-check.

```bash
cd "$(git rev-parse --show-toplevel)"
bash scripts/glyph-lint.sh --block docs/plans/2026-06-26-task-0438-fasttrack-lane.md || true
aida plan verify docs/plans/2026-06-26-task-0438-fasttrack-lane.md
aida show TASK-0438 | grep -i status   # expect: Approved (design slice; spec completes on followups)
```

## Related

- Builds on: STORY-587 (`aida fasttrack` trivial lane), STORY-560 (`aida
  assess` intake pass), EPIC-28 (resilient drain / shelving), STORY-442
  (lifecycle short-circuit tags).
- Parent: EPIC-0428 (advisor autopilot bounded disposition) — this is the
  operator-requested-quick-fix instance of that envelope.
- See also: `docs/autonomous-drain.md`, `.claude/skills/aida-fasttrack.md`,
  `.claude/skills/aida-assess.md`, `aida-cli/src/auto_complete.rs`
  (`LifecycleSkip`, phase orchestrator), `aida-cli/src/intake.rs` (fence +
  policy).
