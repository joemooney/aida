# Repositioning AIDA: the human-governance layer for agent-driven development

**Date:** 2026-06-12 · **Specs:** ADR-4 (decision), EPIC-39 (umbrella), STORY-572/573/574/575/576/577, SPIKE-58/59 · **Status:** Proposed (operator review) · **Complexity:** High (many artifacts, low code risk; one new code surface)

> Commissioned by the operator from SPIKE-53's escalated decision: Beads (~18.7k stars, v1.0) + Gas Town (v1.0) shipped AIDA's substrate thesis with distribution. Direction chosen: **A (move up the stack) + B (interoperate with Beads), framed by D (regulated/enterprise as sharpest segment)**. Option C (feature-race) rejected.
> Evidence base: `docs/competitive-analysis/2026-06-12-beads-gastown-moat-rescope.md` (verified Beads parity table + surviving edges + incentive-divergence wedge).

---

## 1. Approach

### The one-paragraph thesis

Stop pitching the substrate ("git-canonical typed spec graph drained by an orchestrator") — Beads/Gas Town shipped that with 18.7k-star distribution and it is now table stakes. Reposition AIDA one altitude up, where the incentive divergence lives: **Yegge's stack is built to *unleash* agents; AIDA is built to *govern* them.** AIDA's product is the human-governance layer: approved requirements (intake → ADR-3 disposition), role-gated autonomy (product/advisor/implementer + the autonomy ladder), enforced code↔spec traces (the anti-drift loop Beads explicitly lacks), and a git-canonical audit trail of who decided what, when, and why. Beads users are not competitors' customers — they are *our funnel*: the import bridge lets a Beads-running team adopt governance without abandoning their tracker.

### What "complete overhaul" means here (scope honesty)

The moat re-scope found the *story* wrong, not the *machine*. The engine — graph, store, orchestrator, trace gate, lifecycle — **is the governance machinery**; it is re-aimed and surfaced, not rebuilt. The overhaul is total at the artifact level (every file that tells AIDA's story gets rewritten) and surgical at the code level (one new import surface + legibility features). If the operator wants more than this — rename/rebrand, engine restructuring — that is an explicit amendment to ADR-4, not an implied part of this plan (see Decisions D8).

### Diagram — position before/after

```
BEFORE (substrate-led; commoditized by Beads/Gas Town)
  "AIDA = your project's missing index: a git-canonical typed spec graph,
   stable IDs, MCP, drained by an orchestrator"
                  └── Beads: same graph, same IDs, same ready-gate, same MCP,
                      + Gas Town orchestrator, + 18.7k stars  →  parity + distribution loss

AFTER (governance-led; incentive-divergent)
  ┌─────────────────────────────────────────────────────────────┐
  │  AIDA — the human-governance layer                          │
  │  intake → approval (ADR-3) → sign-off-by-queueing →         │
  │  role-gated autonomy ladder → trace-enforced merge →        │
  │  audit trail (who/what/when/why)                            │
  ├─────────────────────────────────────────────────────────────┤
  │  mechanism: typed spec graph on git (same repo as code),    │
  │  orchestrator, MCP   ←— the floor, not the pitch            │
  ├─────────────────────────────────────────────────────────────┤
  │  interop: Beads import (funnel) · AGENTS.md generator ·     │
  │  MCP · Agent-Teams boundary (within-vendor=ride)            │
  └─────────────────────────────────────────────────────────────┘
  sharpest segment: regulated/enterprise (governance mandatory — SPIKE-59)
```

### Workstream sequencing

```
WS0 commit evidence ─► WS1 substrate (VIS/OVERVIEW/README/PRIN) ─► WS2 positioning corpus
                                   │                                       │
                                   ├──────────► WS5 messaging rollout ◄────┘   (after WS1 locks language)
                                   │
        SPIKE-58 beads feasibility ┴─► WS3b import MVP        (parallel to WS1/2 once spike lands)
        WS4 governance sharpening  ───────────────────────────  (parallel; code, independent)
        WS6 roadmap triage         ───────────────────────────  (advisor seat; parallel)
        SPIKE-59 regulated scoping ───────────────────────────  (research lane; parallel)
```

All docs work lands on branch `repositioning` (worktree `~/ai/aida-repositioning`) and merges to main as a coherent cluster once WS1+WS2+WS5 read as one story. WS3b/WS4 code can PR independently (normal lifecycle) — they don't contradict the old story, they strengthen the new one.

## 2. Decisions

- **D1 — Position**: AIDA = human-governance layer for agent-driven development. The substrate is the mechanism, never the pitch. (ADR-4; operator-selected.)
- **D2 — The line** (draft, WS1 finalizes): *"Beads gives your agents memory. AIDA gives your team governance — approved specs, enforced traces, role-gated autonomy, and an audit trail — so you can let agents run and prove what they did and why."* Anchored on incentive (they won't chase governance; it cuts against their unleash thesis), not capability (which they could copy).
- **D3 — Interop posture**: Beads is a *funnel*, not an enemy. Import first (one-way MVP, WS3b); export/bridge deferred behind a revisit-trigger (real demand or trivial cost per SPIKE-58). Same compose-don't-fight logic as AGENTS.md-generator and the Agent-Teams boundary (within-vendor-session = ride native; cross-vendor + durable = AIDA owns).
- **D4 — Engine untouched**: no rewrite of store/graph/orchestrator/lifecycle. WS4 adds *legibility* (audit view, gate explanations), not new machinery.
- **D5 — Claims discipline**: every comparative claim against Beads/Gas Town must match the verified table in the 2026-06-12 rescope doc; provenance-tagged; no "only/unsolved/nobody" overclaims. Gas Town claims stay [W]-grade until source-verified (SPIKE-58 upgrades them empirically).
- **D6 — Immutability**: dated snapshots (2026-05-31 keystone etc.) stay frozen; WS2 *supersedes* with a new dated synthesis and updates living docs/pointers only.
- **D7 — Distribution is not fixed by repositioning**: this plan sharpens *what we say*; reach is the separate bugs→stability→marketing sequencing question (Kill-shot 1 of the 2026-06-09 scan). The repositioning makes the eventual marketing tell the defensible story — it is necessary, not sufficient. Out of scope here.
- **D8 — OPEN (operator)**: rename/rebrand? Not assumed. "AIDA = AI Design Assistant" reads substrate-era; a governance-era name is a legitimate question, but high-cost and not required by ADR-4. Decide before WS5 ships if at all (cheapest moment).
- **D9 — OPEN (operator)**: does the repositioned README lead with the TUI Trojan-horse (current EPIC-26 framing) or with governance? Recommended resolution: keep the TUI as the *face*, make governance the *depth that surfaces* — the Trojan-horse pattern survives intact, the hidden depth just gets the right name. WS1 drafts both leads for the operator to pick.

## 3. Files (build order)

**WS0 — evidence (this commit)**
1. `docs/competitive-analysis/2026-06-12-beads-gastown-moat-rescope.md` — moved onto this branch (foundation).
2. `docs/plans/2026-06-12-repositioning-governance-layer.md` — this plan.

**WS1 — decision/vision substrate (STORY-572)**
3. VIS-1 spec body (store, not repo) — rewrite per D1/D2; retire "missing index" (fold STORY-551).
4. New PRIN spec — govern-vs-unleash as a constitution clause (stateless principle the roadmap triage cites).
5. `OVERVIEW.md` — vision + "Public face" section: Trojan-horse retained, hidden depth renamed to governance (D9).
6. `README.md` — lead, "moat" content, spec-lifecycle framing intro.
7. `CLAUDE.md` — strategic-positioning paragraph (the 2026-05-14 Trojan-horse note) updated to point at ADR-4.

**WS2 — positioning corpus (STORY-573)**
8. **docs/positioning/vs-beads.md** *(to create)* — NEW lead paper (verified parity table + surviving edges + funnel framing).
9. **docs/positioning/vs-gastown.md** *(to create)* — NEW (orchestrator comparison; Agent-Teams boundary cross-ref).
10. `docs/positioning/README.md` — reorder: lead pair becomes vs-beads + vs-gastown.
11. `docs/competitive-analysis/positioning.md` — rewrite around the five governance pillars.
12. **docs/competitive-analysis/2026-06-12-keystone-governance-era.md** *(to create)* — NEW dated synthesis superseding 2026-05-31-round2 on specifics; tripwires: Beads adds trace-linking / approval gates / CI-coupling; Gas City consolidation.
13. `docs/competitive-analysis/README.md` — index row + "current synthesis" pointer.

**WS3 — Beads interop (SPIKE-58 → STORY-574)**
14. Spike report → **docs/competitive-analysis/2026-06-12-beads-schema-mapping.md** *(to create)* (or spike comment) — field map, lossiness, stability verdict.
15. **aida-cli/src/import_beads.rs** *(to create)* (new) + wiring in `cli.rs`/`main.rs` — `aida import --from-beads <path>`; bd hash-IDs kept as external refs; idempotent re-import keyed on bd ID.
16. Tests alongside (see §7).

**WS4 — governance legibility (STORY-575)**
17. `aida audit <spec>` (likely **aida-cli/src/audit.rs** *(to create)* reading the YAML `history:` arrays + role activity) — the who/what/when/why view; the regulated-segment demo.
18. Gate-refusal messages (the TASK-647 refusal sites, MCP status-gate messages) — one-line governance explanations.
19. **docs/governance.md** *(to create)* (or discipline-pack chapter) — the ladder as ONE story: intake → approval → sign-off-by-queueing → gated autonomy → trace-enforced merge → audit.

**WS5 — messaging rollout (STORY-576)**
20. `docs/presentation/2026-06-management-demo.md` — repositioned moat slide + governance demo beat.
21. `aida-core/templates/` scaffolded CLAUDE.md/AGENTS.md language + init banner one-liner.

**WS6/WS7** — no repo files up front: WS6 (STORY-577) is store dispositions + an EPIC-39 comment table; SPIKE-59 produces a dated scoping doc.

## 4. Critical files

- `README.md` + `OVERVIEW.md` — the story every evaluator reads first; WS1's language locks everything downstream (WS2/WS5 quote it).
- **docs/positioning/vs-beads.md** *(to create)* — the single doc that must survive a skeptical Beads user's reading; claims discipline (D5) bites hardest here.
- **aida-cli/src/import_beads.rs** *(to create)* — the only substantial new code; the funnel. Must be idempotent and honest about lossiness.
- VIS-1 + ADR-4 + new PRIN (store) — what future sessions and the roadmap triage cite as the why.

## 5. Reusable helpers (don't reimplement)

- `aida import` machinery (tree import, `--on-conflict skip|rename|replace`) — WS3b extends the existing import path, not a new one.
- History arrays + `aida history --events` (TASK-121) — `aida audit` is a *view* over existing data.
- `aida graph` (STORY-489) + `aida why` — governance-demo plumbing already shipped.
- Existing positioning corpus structure (`docs/positioning/README.md` table) — WS2 follows the per-neighbor one-pager pattern.
- The weekly-scan brief + research lane (STORY-568 pattern) — SPIKE-59 runs as a research-lane spike.
- STORY-559 (advisor dashboard) + STORY-567 (contained posture) — already-filed governance features; WS4 relates, doesn't duplicate.

## 6. Risks + gotchas

- **Over-rotation** — the biggest: reading "Beads won the substrate" as "the substrate was worthless." It wasn't; it's the mechanism that makes the governance claims *true*. Mitigation: D4, and WS1 keeps mechanism paragraphs (demoted, not deleted).
- **Repositioning ≠ reach** (D7) — shipping this and expecting stars is the same trap as bugs-before-marketing. The plan fixes the story; distribution needs its own move after.
- **Beads velocity** — Yegge ships fast; the parity table is a 2026-06-12 snapshot. Tripwires in WS2's keystone; weekly scan watches them. If Beads adds trace-enforcement before WS2 lands, the surviving-edge list shrinks — re-check at WS2 time, don't ship stale claims.
- **Gas Town claims are [W]-grade** — web-summary, not source-verified. SPIKE-58's empirical install upgrades or corrects them before vs-gastown.md ships.
- **Tone trap in vs-beads.md** — disparaging an 18.7k-star beloved tool reads as cope. The funnel framing (D3) is also the right tone: "use both; we govern what your agents do."
- **Backtick-in-shell papercut** — spec descriptions written via CLI lose backticked text to command substitution (bit EPIC-39's first description). Use quotes or escape.
- **Shared-tree hazard** — all branch work in the worktree; push with explicit `-u origin repositioning` (the worktree's upstream currently points at origin/main).
- **Half-repositioned window** — main will briefly disagree with the branch (old README, new vs-beads). Mitigation: WS1+WS2+WS5 merge as one cluster; WS3/4 are story-neutral and can land anytime.

## 7. Tests (named)

- `import_beads::imports_issue_as_task_with_external_ref` — bd JSONL issue → typed spec, bd-ID searchable.
- `import_beads::maps_deps_to_blockedby_and_parent_child` — link-type mapping per SPIKE-58 table.
- `import_beads::reimport_is_idempotent` — second import updates, never duplicates (keyed on bd hash ID).
- `import_beads::lossy_fields_reported_not_silent` — memory-decay/replies_to lossiness surfaced in summary output.
- `audit::renders_history_who_what_when` — audit view over a spec with seeded history rows.
- Docs: `aida plan verify` green on this file; grep-gates in §8.

## 8. Verification (executable)

```bash
# this plan lints clean
aida plan verify docs/plans/2026-06-12-repositioning-governance-layer.md

# WS1/WS5 language gates (run at each merge): old pitch gone from living docs
rg -i "missing index" README.md OVERVIEW.md CLAUDE.md aida-core/templates/ && echo "FAIL: old tagline survives" || echo "OK"

# claims discipline: vs-beads claims carry provenance tags
rg -c "\[V\]|\[W\]|\[I\]" docs/positioning/vs-beads.md   # expect > 0

# WS3b end-to-end (after SPIKE-58): real bd export imports + reimports cleanly
aida import --from-beads /tmp/beads-demo/.beads/issues.jsonl --dry-run
cargo test -p aida-cli import_beads

# WS4 demo: the 10-minute governance walkthrough runs
aida audit <some-spec> && aida graph <some-spec> --blocked-by && aida why <some-spec>
```

## 9. Followups (filed or to file at Done)

- Beads **export/bridge** (WS3c) — behind revisit-trigger (D3).
- Rename/rebrand decision (D8) — operator; file only if taken.
- Distribution/marketing move once the story is coherent (D7) — pairs with `project_bugs_before_marketing_phase` re-sequencing question the 2026-06-09 scan raised.
- ReqIF build (if SPIKE-59 says pursue).
- Website/social/README-badges sweep — after WS5.

## 10. Related

- **Specs:** ADR-4 · EPIC-39 · STORY-572..577 · SPIKE-58/59 · SPIKE-53 (origin) · STORY-550/551/559/567/568 (pre-filed governance/positioning family) · VIS-1.
- **Docs:** `2026-06-12-beads-gastown-moat-rescope.md` (evidence) · `2026-06-09-weekly-scan.md` (Lane D kill-shots — this plan answers #3 and partially #2) · `2026-05-31-round2-moat-gaps-moves.md` (superseded on specifics, frozen) · `research-brief.md` (tripwire watch cadence).
- **Memories honored:** precise-claim-not-overclaim · dated-artifacts-immutable · pushback-on-overengineering (engine untouched; export deferred) · ride-native-within-vendor (Agent-Teams boundary) · capture-vs-slop (WS6 audit) · substrate-as-bouncer (WS4 surfaces it as the pitch).

## 11. Process notes

- Per-slice implementation prompts: `aida ultraplan STORY-572` (etc.) assembles the context-rich prompt when each slice is picked up — the plan above feeds it.
- Quality gate before merge: run `/panel-review` on the WS1+WS2 docs cluster (adversarial multi-agent review; lenses: overclaim, tone-toward-Beads, internal consistency, does-the-funnel-story-hold).
- Disposition: ADR-4 + EPIC-39 + children are **draft/proposed** — the operator (or master advisor) accepts ADR-4 and approves/queues slices; this plan does not self-approve anything (ADR-3).
