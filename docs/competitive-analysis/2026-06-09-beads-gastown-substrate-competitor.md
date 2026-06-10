# Beads / Gas Town — the first substrate-axis competitor (2026-06-09)

**Specs:** SPIKE-53 (Lane B scan finding B2) · **Status:** living (dated snapshot) · **Evidence:** web-current 9 Jun 2026
**Sources:** [github.com/gastownhall/beads](https://github.com/gastownhall/beads), [github.com/gastownhall/gastown](https://github.com/gastownhall/beads), [beads DEPENDENCIES.md](https://github.com/gastownhall/beads/blob/main/docs/DEPENDENCIES.md), [Beads docs site](https://gastownhall.github.io/beads/core-concepts/issues)

> Frozen at time T per the immutability discipline. Supersede with a new dated file. Does **not** rewrite the 2026-05-31 keystone — the corrected positioning line is *proposed* in §5 for the next dated paper.

---

## Why this snapshot exists

The 2026-05-31 round-2 synthesis graded **"typed inter-spec relationship graph"** as DIFFERENTIATED with the parenthetical *"only ReqIF has the model; absent from agents"* (`2026-05-31-round2-moat-gaps-moves.md`, the commoditized-vs-differentiated table). That parenthetical is now **stale**.

`gastownhall/beads` (~24.4k stars, MIT, by the org behind Steve-Yegge-associated Gas Town) is a **git-distributed, vendor-neutral, dependency-aware issue graph with a typed relationship model and hash-based stable IDs**, drained by the **Gas Town** orchestrator (~15.8k stars). It is the **first competitor that lands on the substrate axis** rather than the mechanical (worktree/swarm) axis. Earlier AIDA notes treated "Gastown/Beads" as a worktree multiplexer whose work units were "raw scripts or commits" (`2026-05-16-market-snapshot.md` §3) — that framing under-read Beads. Beads is a *typed graph substrate*, and that is exactly AIDA's claimed territory.

This dents one specific clause. It does **not** collapse the moat. The job of this doc is to say precisely which clause falls, which edges survive, and why.

---

## 1. AIDA-vs-Beads feature matrix (verified)

Provenance tags: **[V]** = verified from Beads' own README / docs / DEPENDENCIES.md on 9 Jun 2026 · **[I]** = inferred (consistent with sources but not explicitly stated) · **[A]** = absent from sources (Beads README/docs make no claim either way).

| Dimension | AIDA | Beads | Provenance | Verdict |
|---|---|---|---|---|
| **Vendor-neutral / multi-agent** | Yes — MCP + CLI, Claude/Codex/Cursor/… | **Yes** — `bd setup codex/claude/factory/mux/cursor`; "A memory upgrade for your coding agent" | [V] Beads | **PARITY** — Beads is genuinely cross-vendor |
| **Typed relationship graph** | parent/child, blocks/blocked-by, subsumes, depends-on, relates | **Yes** — blocking: `blocks`, `parent-child`, `conditional-blocks`, `waits-for`; non-blocking: `related`, `tracks`, `discovered-from`, `caused-by`, `validates`, `supersedes` | [V] DEPENDENCIES.md | **PARITY** (Beads' graph is, if anything, *richer* on the blocking-semantics axis) |
| **Stable IDs (identity ≠ position)** | `SPEC-ID` (`TASK-94`), merge-gate short IDs | **Yes** — hash-based `bd-a1b2`, hierarchical `bd-a3f8.1.1`; "prevent merge collisions in multi-agent/multi-branch workflows" | [V] Beads | **PARITY** (different scheme — hash vs assigned-sequential — same property) |
| **Orchestrator drains the graph** | `aida queue work --auto-complete` + escalation/shelving cascade | **Yes** — Gas Town Mayor/polecats drain Beads; `bd ready` = auto-ready detection | [V] Gas Town + Beads | **PARITY on existence**; AIDA's edge is the *spec-grounded escalation/shelving cascade*, not the drain itself |
| **Plain-git, zero-infra storage** | YAML on an orphan `aida-store` branch + rebuildable SQLite cache; **git is the only dependency** | **No** — *"powered by Dolt"*; Dolt is the source of truth (embedded in-process by default). `issues.jsonl` is *"an export … not the source of truth"* | [V] Beads README | **AIDA EDGE** — Beads requires a Dolt runtime; AIDA needs only git |
| **Rich lifecycle state machine** | Draft→Approved→Planned→In Progress→Done→Completed→Released, gated transitions, `done`≠`completed` | **Partial** — states exist (`open`, `in_progress`, `blocked`, `deferred`, `closed`, + `tombstone`/`pinned`/`hooked`); basic `open→in_progress→closed` flow | [V] docs site / [A] no gated state-machine | **AIDA EDGE** — Beads has *status values*; no documented multi-stage gated lifecycle |
| **Merge-driven auto-bump** | `(SPEC-ID)` commit trailer auto-bumps `done→completed` on merge to main | **No such mechanism documented** | [A] Beads | **AIDA EDGE** |
| **Structured per-item history/audit** | `history:` array inside each YAML (every field change, authored + timestamped) | **Partial** — `bd show <id>` surfaces "audit trail"; depth/structure not documented | [V] partial / [A] structure | **AIDA EDGE (soft)** — Beads has *an* audit trail; granularity unverified |
| **Code↔spec trace enforcement** | `// trace:SPEC-ID` comments, `aida show` git-linkage harvest, commit-gated `(SPEC-ID)` | **Absent** — README/docs make **no** claim of code-comment/commit linkage or trace enforcement | [A] Beads | **AIDA EDGE — the cleanest one** |
| **Requirements modeling** (functional/non-functional, ADR, vision, constraint, principle, glossary) | 19 requirement types incl. ADR/knowledge-graph family | **No** — Beads is an *issue/task* tracker ("issue", "task", "epic"); not a requirements model | [I] from issue-centric framing | **AIDA EDGE — framing difference** (see §4) |
| **Distribution / reach** | small | **~24.4k ⭐ Beads + ~15.8k ⭐ Gas Town**, 91 releases, active (v1.0.4 / v1.2.1, Jun 2026) | [V] | **BEADS EDGE — by ~2 orders of magnitude** |

### Provenance honesty notes

- **Relationship-type list discrepancy.** Beads' top-level README names `relates_to, duplicates, supersedes, replies_to` "for knowledge graphs"; `docs/DEPENDENCIES.md` enumerates a *different and larger* set (`blocks`, `parent-child`, `conditional-blocks`, `waits-for`, `related`, `tracks`, `discovered-from`, `caused-by`, `validates`, `supersedes`) and does **not** list `duplicates` or `replies_to`. The two surfaces disagree; I did not reconcile them against the running binary. Treat the *union* as "approximately what Beads models" and the *exact* set as **[I]**. Either way the headline holds: Beads has a real typed relationship model.
- **Lifecycle states** come from the docs site / a web index, not a single authoritative README block I could quote verbatim — graded **[V]** for *existence of the states*, **[A]** for any *gated state machine*.
- **"Code↔spec trace enforcement absent"** is the strongest negative claim here, so state its basis precisely: it is **absent-from-sources [A]**, i.e. Beads' README, FAQ, ARCHITECTURE.md, and DEPENDENCIES.md make no traceability claim. That is strong evidence of absence for a feature a project would normally headline, but it is not the same as having read the source and confirmed no hook exists. Do not over-claim it as "verified impossible."

---

## 2. What Beads genuinely does well (no strawman)

Honesty is load-bearing in competitive analysis, so state Beads' real strengths plainly:

1. **It is already the thing round-1 feared.** A git-distributed, typed, dependency-aware, vendor-neutral task graph that an orchestrator drains — shipped, popular, maintained. The "no agent tool has the typed multi-vendor graph" line is empirically false as of now.
2. **Hash-based IDs are a legitimately elegant answer** to the multi-agent merge-collision problem — arguably cleaner than AIDA's assigned-then-merge-gated short IDs for the *pure concurrency* case (no central dispenser needed).
3. **Richer blocking semantics than AIDA today** — `conditional-blocks` ("B runs only if A fails") and `waits-for` ("B waits for all of A's children") are graph edges AIDA does not currently model.
4. **`bd ready` auto-unblock** is a clean, legible drain primitive (an issue is ready when all blocking deps are closed) — the same idea as AIDA's BlockedBy-aware queue, well-executed.
5. **Distribution.** ~24k stars and Steve-Yegge-adjacent mindshare is the asset AIDA most lacks.

The matrix above is not "Beads is weak." It is "Beads and AIDA overlap heavily on the *graph substrate*, and diverge on *enforcement, lifecycle depth, zero-infra, and requirements-vs-issues framing*."

---

## 3. The surviving edges — what to HEADLINE next

Four edges survive contact with Beads. Ordered by how cleanly they survive:

1. **Code↔spec trace enforcement (the cleanest).** Beads links *issues to issues*; AIDA links *issues to the code that satisfies them* and gates the commit on it (`// trace:SPEC-ID`, `(SPEC-ID)` trailer, `aida show` git-linkage). This is the anti-drift answer and it is **absent from Beads' surface**. Headline it.
2. **Plain-git / zero-infra.** Beads is *"powered by Dolt"* — a real runtime dependency (a MySQL-compatible versioned database), even in embedded mode, with `issues.jsonl` explicitly *not* the source of truth. AIDA's substrate is YAML on a git branch; the only dependency is git, and the SQLite cache is a rebuildable projection. "Your issue graph needs a database; AIDA's needs only `git`" is a true, sharp, checkable line.
3. **Rich gated lifecycle + merge-driven auto-bump + structured history.** Beads has status *values* and an audit trail; AIDA has a *gated multi-stage state machine* (`done`≠`completed`, transitions gated by role/merge) with a structured per-item `history:` ledger and merge-driven promotion. This is "issue states" vs "a lifecycle that stays true on its own."
4. **Requirements modeling, not issue tracking (the framing wedge).** Beads' nouns are *issue / task / epic*. AIDA's are *functional / non-functional / constraint / principle / vision / decision (ADR) / term / story / bug …* — a requirements model with a knowledge-graph family, not a backlog. This is the §4 incentive anchor and the line that ages best.

**Drop / demote from the headline:** "typed relationship graph" and "stable IDs" as *standalone* differentiators — Beads has both. They remain *true* and part of the stack, but they are no longer the wedge; pitching them as unique now invites the "Beads does that too" rebuttal. Lead with **enforcement + plain-git + lifecycle + requirements-framing**; let IDs/graph be *table stakes we also have*, not the headline.

---

## 4. Incentive anchor — why the gap persists (and why it's thinner here)

Round-2's discipline: anchor "why won't they close the gap?" on **incentive**, not capability — incentive ages well, capability ages badly.

**Beads' center of gravity is agent task-memory, not enforced requirement traceability.** Its own tagline is *"a memory upgrade for your coding agent"* — the job is *don't lose the plan between agent restarts*. For that job:
- **Trace enforcement is off-mission.** Binding code to issues serves *requirement integrity / audit*, not *agent memory continuity*. Beads has no incentive to build the trace loop because its users aren't asking "did the code actually satisfy the spec?" — they're asking "what was I doing?".
- **A deep gated lifecycle is over-engineering for task-memory.** `open→in_progress→closed` is enough for "what's next?". The Draft→Approved→…→Released ladder pays off for *requirements governance*, which is a different buyer.
- **Dolt is a deliberate choice for *their* job** (concurrent multi-agent writes, SQL queryability) — not an accident they'll reverse. So "plain-git" isn't a gap they'll close; it's a divergent design center.

**But be precise: the incentive moat is thinner than the provider case.** The provider argument (single-vendor runtimes won't make memory portable) rests on a *structural* conflict of interest. Beads has **no such conflict** — it is *already* cross-vendor and *already* typed. Nothing stops Beads from adding a trace-comment convention or a richer lifecycle tomorrow except that it's off their current mission. That is a **mission-priority moat, not a structural one** — softer, and worth watching as a tripwire (§6). The honest claim is *"Beads could add enforcement; it hasn't, because its job is memory not governance"* — not *"Beads structurally can't."*

---

## 5. Proposed positioning-line edit (for the next dated paper — do NOT edit the 2026-05-31 file)

The keystone line in `2026-05-31-round2-moat-gaps-moves.md` (§"The positioning line that survives the convergence") reads:

> *"AGENTS.md and Spec Kit standardized how agents read your project and your specs. AIDA is the graph underneath — stable IDs, typed relationships, enforced traces, and a lifecycle that keeps them all true — and it writes those standard files for you. It's the only one where an orchestrator drains that graph through a spec-grounded escalation cascade, **and the only one portable across every vendor because it lives in git.**"*

The bolded clause is now wrong: **Beads is also portable across every vendor and also lives in (Dolt-on-)git.** "Stable IDs, typed relationships" as leading differentiators are also dented. Proposed correction:

> *"AGENTS.md and Spec Kit standardized how agents read your project and your specs; Beads gave agents a portable, typed task graph to remember their work. AIDA is the layer those stop at: it binds the graph to the **code that satisfies it** (enforced `trace:` comments + merge-gated trailers), runs a **gated requirement lifecycle** that keeps `done` honest against `completed`, models **requirements — not just issues** (constraints, decisions, principles, glossary), and needs **only git** — no database runtime. It's the only one where an orchestrator drains that graph through a spec-grounded escalation cascade. Portability across vendors is now table stakes; **enforced traceability over a governed requirements graph is the wedge.**"*

Key moves in the edit: (a) **name Beads** as the proof that portable-typed-graph is solved — competitor existence as validation, not threat; (b) **retire "only … portable across every vendor"** — replace with "table stakes"; (c) **re-headline** to enforcement + lifecycle + requirements-modeling + plain-git (the §3 four); (d) keep the orchestrator/escalation-cascade clause — still genuinely un-matched.

---

## 6. Trojan-horse: a Beads import/export bridge

**Idea:** meet Beads' ~24k-star distribution where it is — `aida import --from-beads .beads/issues.jsonl` and `aida export --to-beads`, mapping Beads' typed edges onto AIDA's relationship model.

**Assessment: PURSUE as a SPIKE, gated behind the bugs→stability phase (`project_bugs_before_marketing_phase`). Not now, but the right shape.**

- **Feasibility is high.** Beads ships `issues.jsonl` as an explicit interchange format — *"an export for viewers and interchange"* — so we import their *documented* boundary, not their Dolt internals. The typed-edge mapping is mostly mechanical (`blocks`→blocks/blocked-by, `parent-child`→parent/child, `supersedes`→supersedes; the soft links → AIDA `relates`).
- **The pitch writes itself:** *"Keep using Beads for agent task-memory. Point AIDA at your `issues.jsonl` and get enforced code↔spec traces, a governed lifecycle, and a requirements layer on top — no migration, no database."* That is the Trojan horse: AIDA as the *governance/enforcement layer over* the substrate they already adopted, not a rip-and-replace.
- **Strategic caution (the real risk):** a *bidirectional* bridge legitimizes Beads as the default substrate and risks positioning AIDA as "a plugin for Beads." Mitigation: ship **import-first** (one-way, "graduate from issue-memory to enforced requirements"), make export a deliberate second step, and never frame AIDA as downstream of Beads in the marketing.
- **Lower bound:** even a *read-only* `aida import --from-beads` that lands a Beads graph as AIDA specs (one-shot, no live sync) is a cheap, high-signal first slice — file it as the SPIKE's smallest-valuable-increment.

This also generalizes: the same interchange-import pattern is the answer to *any* future substrate-axis competitor with a documented export — it makes AIDA the *enforcement layer over portable substrates* rather than one more silo. (Cf. the ReqIF-import option already flagged in round-2.)

---

## 7. Net read

The 2026-05-31 keystone over-claimed on *one* clause: typed-multi-vendor-graph is no longer AIDA-unique — **Beads got there, at scale.** That is a genuine correction, and the kind the immutability discipline exists to capture: leave the dated file frozen, supersede with this.

But the moat did not collapse — it **re-centered**. Strip away the now-commoditized "typed graph + stable IDs + cross-vendor + git-distributed" (Beads has all four) and what AIDA still holds alone is the **enforcement + governance layer**: code↔spec trace enforcement, a gated requirement lifecycle with merge-driven auto-bump and structured history, requirements-modeling (not issue-tracking) breadth, plain-git zero-infra, and an orchestrator that drains the graph through a *spec-grounded escalation/shelving cascade*. That is a narrower, sharper, more defensible claim than "we have the typed graph" — and Beads' 24k stars are the **validation** that the substrate thesis was right, not the **refutation** of AIDA's wedge.

The honest open slice: Beads' incentive moat against closing the gap is **mission-priority, not structural** — softer than the provider case. Watch it (§ tripwires). The right competitive move is not to out-graph Beads; it's to be the **enforcement layer over portable substrates**, and the import bridge (§6) is how AIDA meets the distribution where it already is.

### Tripwires (refresh ~6 weeks)

- **Beads adds any code↔commit/trace linkage** — closes the cleanest surviving edge; highest-priority watch.
- **Beads adds a multi-stage gated lifecycle** or merge-driven status promotion — erodes edge §3.3.
- **Beads / Gas Town drop or abstract the Dolt requirement** (e.g. a pure-git mode beyond the `issues.jsonl` export) — erodes the plain-git edge §3.2.
- **Gas Town's drain grows an escalation/shelving cascade** — erodes the last orchestrator clause.
- **Beads reframes from "agent memory" toward "requirements/governance"** — that's the mission-priority moat (§4) eroding; the structural conflict-of-interest that protects the provider case does *not* protect us here.

## Related

- [`2026-05-31-round2-moat-gaps-moves.md`](2026-05-31-round2-moat-gaps-moves.md) — the keystone this snapshot corrects (§5). Frozen; superseded on the typed-graph clause.
- [`2026-05-31-git-canonical-substrate-thesis.md`](2026-05-31-git-canonical-substrate-thesis.md) — round-1 substrate thesis; Beads is the strongest test of it to date.
- [`2026-05-16-market-snapshot.md`](2026-05-16-market-snapshot.md) §3 — the earlier (under-reading) Gastown/Beads-as-worktree-multiplexer entry this snapshot upgrades.
- [`2026-05-26-agent-memory-libraries.md`](2026-05-26-agent-memory-libraries.md) — Beads' self-description ("a memory upgrade for your coding agent") puts it adjacent to this category too; the trace/lifecycle/requirements wedge is the same.
