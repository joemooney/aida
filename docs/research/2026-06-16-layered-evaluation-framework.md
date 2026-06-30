# Evaluating AI Tooling Across the Coordination Stack

### A layered decision framework — feature requirements, risks/benefits, and lock-in safeguards for allocating money and internal resources

**Status:** Draft · **Date:** 2026-06-16 · **Companion to:** `2026-06-16-coordinating-multi-vendor-agent-fleets.md` (the theory) and `2026-06-16-research-proposal-multi-vendor-coordination.md` (the proposal) · **Audience:** decision-makers allocating budget / internal development to AI tooling · **Frame:** vendor-neutral; AIDA appears only as a worked existence-proof, never as a recommendation

> The theory paper argues *what is true* about the multi-vendor agent-coordination problem and leaves the roll-your-own verdict **open**. This document is the operational other half: a portable rubric for deciding, per layer, **build vs buy vs ride-native vs wait** — designed to (1) inform resource allocation, (2) safeguard against lock-in, and (3) build deliberately toward the one durable competitive asset (§5). It is a *living* instrument: the ground shifts, so re-score on a cadence (§7).

---

## 0. How to use this

For any candidate tool or internal build:

1. **Place it on the stack** (§1) — which layer (L0–L5) does it primarily serve? Many tools span layers; score each layer separately.
2. **Score it on the cross-cutting rubric** (§2) — the criteria that apply to *any* tool at *any* layer. This is where lock-in and substrate-ownership get caught.
3. **Run the per-layer checklist** (§3) — "what good looks like" + the layer's specific capture risk.
4. **Apply the decision lens** (§4) — build / buy / ride-native / wait, given today's ground.
5. **Test it against the asset thesis** (§5) — does this choice grow or erode the shared substrate that becomes your competitive advantage?
6. **Diary the decision** in a per-adoption decision record (Appendix A template) and re-evaluate on cadence (§7) — record *why* now, so the next review can see what changed. A worked example of the rubric in motion is Appendix B.

Scoring scale used throughout: **3** = strong/owned · **2** = partial · **1** = weak · **0** = absent/captive. A low score is not a veto; it is a *flag* that must be consciously accepted or mitigated.

---

## 1. The stack (recap)

| Layer | What it is | Commoditizing? |
|---|---|---|
| **L5** Cross-vendor fleet coordination | mailbox, roles, leases, shared intent graph across vendors | No — incentive-persistent gap |
| **L4** Autonomy & role orchestration | escalation cascade, punt-continue, drains | Partly (within-vendor) |
| **L3** Collaborative state | the durable store of specs/decisions/history | Partly |
| **L2** Enforcement & governance | gates, audit, policy | Partly (within-vendor) |
| **L1** Intent substrate | the addressable graph: IDs, relationships, traces | **The asset (§5)** |
| **L0** Model / agent capability | the models themselves | **Yes — fastest** |

The investment heuristic the theory paper lands on: **value is migrating upward.** Spend on the bottom is increasingly duplicated by vendors solving it for free; the layers worth *owning* are the ones a single vendor is structurally disincentivized to serve well (L1 as asset; L4–L5 as the open frontier).

---

## 2. Cross-cutting rubric — applies to any tool at any layer

These nine criteria catch the things that do not show up in a feature demo but dominate the five-year cost. Score each 0–3.

### 2.1 Lock-in / exit cost  *(the primary safeguard)*
- [ ] **Data portability** — can you export *everything* (not just a report) in an open, documented format, with full fidelity including history and relationships?
- [ ] **Who holds the graph** — does your intent/coordination state live in *your* git/store, or inside the vendor's database?
- [ ] **Switching cost** — if this tool vanished tomorrow, what breaks, and how many weeks to replace?
- [ ] **Pricing/contractual capture** — does cost scale with a metric the vendor controls (seats, tokens, stored objects)?
- **Red flag:** the tool is the *only* thing that can read your own data.

### 2.2 Interoperability
- [ ] Speaks open protocols (MCP / A2A / ACP / plain git / documented API), not a closed silo.
- [ ] Composes with tools above and below it on the stack.
- **Red flag:** integration requires the vendor's own ecosystem end-to-end.

### 2.3 Substrate ownership
- [ ] You can query your intent + coordination state **independent of the tool** (e.g. the data is files you own).
- [ ] The system of record is yours; the tool is a *projection* over it, not the source of truth.
- **Red flag:** "your data is safe with us" instead of "your data is yours, here."

### 2.4 Composability / synergy potential  *(feeds §5)*
- [ ] Exposes a graph/API other internal systems can read, so it can link across programs/projects.
- [ ] Identifiers are stable and shareable across teams.
- **Red flag:** a per-team island with no cross-program seam.

### 2.5 Neutrality
- [ ] Vendor-agnostic — works with whichever models/agents you mix, today and after a vendor swap.
- **Red flag:** binds your coordination to one model vendor's agents.

### 2.6 Governance / auditability
- [ ] You can see and police what agents did (who changed what, when, under what authority).
- [ ] Load-bearing invariants are enforced by a *gate* you control; low-risk procedural rules can remain instructions when field evidence shows agents follow them reliably.
- [ ] The tool exposes enough trace data to reconstruct which agent ran, what input it received, what output it produced, how long it took, what it cost, and which approval or policy boundary it crossed.

### 2.7 Reversibility / blast radius
- [ ] You can back the decision out; failure is contained, not org-wide.
- [ ] Adopting it does not silently make it load-bearing everywhere before you've validated it.

### 2.8 Total cost of ownership (incl. churn risk)
- [ ] Build + maintain + the risk a vendor obsoletes this layer for free within ~12 months (theory §12: "building on sand").
- [ ] Runtime cost controls exist: token / API-credit budgets, retry caps, loop breakers, and per-agent cost attribution.
- **Red flag:** you'd be hand-building infrastructure a platform is visibly about to ship.

### 2.9 Strategic optionality
- [ ] Adopting it **preserves** future moves rather than foreclosing them.
- **Red flag:** a one-way door taken for a short-term convenience.

**Composite read:** sum the scores, but weight 2.1, 2.3, 2.4 most heavily — they are the lock-in safeguard and the asset-builder. A tool can be feature-rich and still fail here, and that failure is the expensive kind.

---

## 3. Per-layer checklists

For each layer: *what good looks like*, the *layer-specific capture risk*, and a *default lean* (held loosely — the ground shifts).

### L0 — Model / agent capability
- **What good looks like:** provider abstraction (swap models without rewriting), an internal eval harness, cost/latency observability, no app logic coupled to one provider's quirks.
- **Capture risk:** app code wired to one vendor's API shape; prompts tuned so tightly to one model they don't port.
- **Default lean:** **Buy, multi-source.** Never build; always keep ≥2 providers swappable. This layer commoditizes fastest.

### L1 — Intent substrate  *(the asset — see §5)*
- **What good looks like:** stable identifiers that never move; typed relationships (a real graph, not tags); bidirectional code↔intent traces; an open, queryable, exportable format; the store is *yours*.
- **Capture risk:** a vendor owning your requirements/decision graph. This is the worst lock-in on the stack — it's your institutional memory.
- **Default lean:** **Own it** (build or adopt only something fully portable/open). The cost of owning is justified here even if nowhere else, because this is what becomes the competitive asset.

### L2 — Enforcement & governance
- **What good looks like:** high-risk invariants enforced as programmatic **gates** (refuse the bad action), field telemetry showing which stated rules actually fail, full audit trail, policy you control and can change.
- **Capture risk:** governance logic trapped inside a vendor's agent platform — you can't enforce what you can't reach.
- **Default lean:** **Own the load-bearing gates, ride the vendor's execution.** Place approval, merge, authority, and traceability enforcement at *your* substrate boundary; do not build gates for every prompt rule without field evidence.

### L3 — Collaborative state
- **What good looks like:** durable store with sane concurrent-edit handling (conflict-light merge, theory §5); open format; survives any single tool; history preserved.
- **Capture risk:** a proprietary database as the only home for your shared state.
- **Default lean:** **Own / open-format.** Prefer a store you can read with `git` and a text editor over a service you query only through one vendor's UI.

### L4 — Autonomy & role orchestration
- **What good looks like:** role separation (implementer / advisor / human); punt-and-continue resilience; escalation routing; resumable checkpoints; retry/fallback/circuit-breaker behavior; deep observability into what the fleet did, why, how long it ran, and what it cost.
- **Capture risk:** orchestration that only drives *one vendor's* agents; a black-box autonomy loop you can't inspect.
- **Default lean:** **Build thin or ride carefully + instrument.** This is a frontier; ride native within-vendor where exit cost is low, but keep the orchestration *logic* and observability yours.

### L5 — Cross-vendor fleet coordination  *(the open frontier)*
- **What good looks like:** vendor-neutral mailbox / roles / leases; protocol-based (any tool that speaks the protocol joins); no agent privileged over another; the coordination state is part of your owned substrate (L1/L3).
- **Capture risk:** *ironic* — the coordination tool itself becoming a new silo. A "neutral" hub that only your one vendor's agents can actually use is not neutral.
- **Default lean:** **Build or wait — and watch.** This is where a home-grown layer still uniquely buys something (theory §12), but also where a neutral incumbent may capture it instead of you (theory §9, P8b). Instrument the decision and revisit often.

---

## 4. The decision lens — build / buy / ride-native / wait

Four moves, with the test for each:

| Move | Choose when | Watch for |
|---|---|---|
| **Build** | It's the asset (L1) or the open frontier (L5) **and** no portable neutral option exists | The substrate tax (theory §12); don't build what a platform ships free in 12 months |
| **Buy** | A neutral incumbent provides it **portably** (passes §2.1/2.3) | "Neutral" tools that are really a single-vendor silo |
| **Ride-native** | You're already in a vendor's session and exit cost is low | Letting convenience become load-bearing; keep the *durable* copy in your substrate |
| **Wait** | The surface is moving fast and adopting now would foreclose options | Analysis paralysis; "wait" still means *instrument and set a revisit trigger* |

**The composing principle (theory §7):** *ride native within a vendor; own the cross-vendor and the durable.* Below the asset line, lean buy/ride and keep swappability. At the asset line and the frontier, lean own — because that is where lock-in hurts most and advantage accrues.

---

## 5. Substrate as organizational asset — the competitive-advantage thesis

This is the strategic crux, and it reframes the whole evaluation. **The durable competitive advantage is not any tool — it is the shared substrate that accumulates as AI capability grows organically across the org.**

**The mechanism.** As many teams adopt agents independently, two futures diverge:
- *Siloed:* each team's intent, decisions, and coordination live in disconnected vendor tools. The org's AI capability is the *sum* of its teams.
- *Substrate-backed:* every team's work lands in one shared, addressable intent + coordination graph (stable IDs, typed relationships, traces, history). The org's AI capability becomes the *product* of its teams, because the graph makes their work mutually visible and composable.

**What the shared substrate buys that silos cannot:**
- **Cross-program visibility** — a query spans projects; duplicated effort and conflicting decisions become *findable* instead of discovered by accident.
- **Hidden-synergy discovery** — shared requirements, reusable components, and cross-team dependencies surface from the graph; the org sees connections no single team holds in its head.
- **Institutional memory that outlives turnover** — the *why* survives staff and vendor churn, because it's in the substrate, not in a departed engineer or a deprecated SaaS.
- **The ultimate lock-in safeguard** — owning the graph means you can swap any tool above or below it. You are never captive, because the system of record is yours (§2.1, §2.3).
- **Compounding** — unlike a tool, the asset grows with use; its value is super-linear in adoption, which is exactly the network effect a competitor running silos cannot replicate by buying the same tools.

**This is a strategy advantage, not a tooling advantage.** A competitor can buy every tool you buy and still not have your substrate, because the substrate is *grown from your org's actual work* — it is non-purchasable and non-portable to them, while remaining fully portable *for you* across tools.

**The honest caveats (so this isn't hype):**
- The asset only materializes with **discipline** — consistent IDs, traces, and adoption. A half-adopted substrate is *worse than none*: it creates false confidence that the graph is complete when it isn't.
- It carries a real **cost** (the substrate tax, theory §12) and a **coordination burden** (getting independent teams to share conventions).
- It is **falsifiable:** the thesis predicts that an org owning the substrate measurably surfaces cross-program reuse and dependency/synergy that a siloed org misses. *If owning the substrate does not produce synergy a comparable siloed org lacks, the thesis is wrong.* That is the experiment worth running before betting heavily.

**Investment implication.** This is the argument for spending at L1 (and protecting L3/L5 ownership) even when the cost/benefit of any individual tool looks marginal: you are not buying a tool, you are *compounding an asset and buying optionality*. The per-tool decision (§2–4) should be weighted by the answer to one question — **does this choice grow the shared substrate, or fragment it?**

### 5.1 The synergy experiment — test the thesis before betting on it

The thesis above is falsifiable, and it should be *falsified or confirmed cheaply* before an org commits major resources. The following is a concrete, one-quarter within-org design that measures whether owning the substrate actually surfaces synergy a siloed org misses — at *your* org's synergy density, which is the variable that should drive *your* decision.

- **Arms (matched on team size, domain adjacency, current AI-tool adoption):**
  - **S — substrate:** 3–5 teams whose work all lands in one shared intent + coordination graph (stable IDs, typed relationships, traces, history).
  - **C — control / siloed:** 3–5 comparable teams keep their existing per-team tools; no shared graph.
  - **W — write-it-down:** 3–5 teams share an *unstructured* doc/wiki but no typed graph. **This arm is essential** — it separates "the graph helped" from "writing intent down at all helped" (the confound the theory paper flags in §3). Without W, a positive S result is uninterpretable.
- **Duration:** one quarter — long enough for cross-team dependencies to arise, short enough to stay cheap.
- **The probe — "synergy events."** Pre-define a synergy event as any of: (a) duplicated effort caught before both shipped; (b) a reusable component adopted across teams; (c) a cross-team dependency or conflict surfaced before it caused rework; (d) a decision in one team that changed another's plan. Count them in every arm.
- **How each arm surfaces events:** S runs standing graph queries (cross-program duplication scan, shared-dependency closure, conflicting-decision detector) on a cadence — each hit is a *candidate* event the teams validate. C surfaces events the way it does today (meetings, review, accidental discovery), logged when noticed. W surfaces them by search/reading the shared doc.
- **Primary metrics:** synergy events per team-month, and — the one that matters most — **lead time** (how early before impact each was caught). The thesis predicts S > W > C on both count and earliness; the *earliness* gap is the real value (a duplication caught in week 1 vs month 3).
- **Secondary metrics:** rework-hours avoided (attributable to early catches); and the **false-positive rate** on graph-surfaced candidates — does the query produce noise teams must wade through? A high false-positive rate is itself a finding (the graph is not yet worth the attention tax).
- **Pre-registered decision rule (write it before running):** e.g. *"proceed to org-wide substrate investment iff S surfaces ≥2× C's synergy events at ≥30 days better median lead time, false-positive rate < X, **and** S beats W (the graph adds value beyond writing-down)."* If S ≈ W, the win is discipline, not the substrate — buy the cheap version. If S ≈ C, the thesis fails at this org/scale — do not bet.
- **Threats to control:** Hawthorne effect (S knows it's watched → instrument C and W's discovery equally); selection bias (do not cherry-pick already-adjacent teams into S); and the discipline confound (handled by the W arm).
- **Cost:** one quarter, ~9–15 teams, the standing-query setup, and an analyst to validate candidate events — cheap relative to an org-wide commitment, which is the entire point: **measure before you bet.**

A single-org result does not generalize to other orgs — but it is not meant to. It tells you about *your* synergy density, and that is exactly what your allocation decision turns on.

---

## 6. Anti-patterns and failure modes

The framework fails in characteristic ways. Each is a real trap observed or predicted; the mitigation is what keeps the instrument honest.

- **Half-adoption (the worst state).** A partially-populated substrate is *worse than none*: a cross-program query that silently misses because one team isn't on the graph produces false confidence that the graph is complete. → *Adopt by complete, bounded units; mark coverage explicitly; never trust a query over a partially-covered set.*
- **The decision record as bureaucracy.** Records become a compliance ritual nobody reads, and scores get back-rationalized to justify a call already made. → *The record exists to make the lock-in / ownership question (§2.1, §2.3, §5) un-skippable, not to gate. Keep it one page; review only the "Fragments" and low-2.1/2.3/2.4 cases.*
- **False precision / score gaming.** The 0–3 numbers look objective; people anchor on the composite and stop thinking. → *The notes matter more than the number. A single 0 on lock-in or substrate-ownership can override a high composite; weights are explicit for that reason.*
- **The "neutral" layer that is secretly a silo (the L5 irony).** Building vendor-neutral coordination that in practice only your one vendor's agents use — you have paid the build cost *and* still have lock-in, now self-inflicted. → *Test neutrality empirically: actually run a second vendor's agent through it (the cross-vendor portability proof point). Neutral-on-the-roadmap is not neutral.*
- **Premature standardization.** Committing to your own coordination schema/protocol just before an industry standard converges — now you maintain a fork forever. → *Prefer the "wait" move at L5 while standards are in flux; build on the standard's primitives where they exist rather than inventing parallel ones.*
- **Substrate-tax denial.** Underbudgeting the unglamorous maintenance (theory §12) so the substrate rots — and a rotted substrate is a half-adopted one. → *Fund maintenance as a standing line item, not heroics.*
- **Owning the wrong layer.** Building at L0–L2, where vendors will commoditize the work for free, instead of the L1 asset and the L4–L5 frontier. → *Follow the per-layer lean (§3); re-test it every cadence (§7) as the commoditization line moves up.*
- **Optionality theater.** Claiming "we can swap any time" while never exercising the exit — exit cost is only real once tested. → *Periodically rehearse the swap: export + re-import the substrate, and run a second vendor's agent against it.*

---

## 7. Cadence — a living evaluation

The stack's value distribution shifts during any decision cycle, so this framework is run repeatedly, not once:

- **Re-score on a cadence** (quarterly is a reasonable default for a fast-moving field) and on trigger events: a vendor ships a layer for free, a neutral incumbent appears, a lock-in bites.
- **Keep dated, immutable snapshots** of each evaluation so the *trajectory* is visible — what moved up the stack, how fast (theory §13's "simply watch whether vendor surfaces converge on the upper layers").
- **Record the revisit trigger** with every "wait" decision, so waiting is a deliberate, time-boxed posture and not drift.

---

## Appendix A — Per-adoption decision record (template)

One record per tool-or-build decision. Copy the block, fill it in, commit it next to the thing it governs, and revisit on the trigger. The point is not bureaucracy — it is that the *reasoning* (and the revisit trigger) survives the person who made the call, and the rubric forces the lock-in / substrate questions to be answered, not skipped.

```markdown
# Decision record — <candidate name>

- **Decision ID:** DR-<n>          **Date:** <YYYY-MM-DD>
- **Owner:** <name>               **Reviewers:** <names>
- **Candidate:** <tool / internal build>
- **Layer(s):** L<0–5>            **Primary layer:** L<n>
- **Decision:** ☐ Build  ☐ Buy  ☐ Ride-native  ☐ Wait
- **Revisit trigger:** <event or date that reopens this — MANDATORY, especially for Wait>

## Rubric score (§2, each 0–3; ★ = heavily weighted)
| # | Criterion | Score | Note / red flag |
|---|-----------|:-:|-----------------|
| 2.1★ | Lock-in / exit cost        |  | <export fidelity? who holds the graph? weeks-to-replace?> |
| 2.2  | Interoperability           |  | <open protocols / silo?> |
| 2.3★ | Substrate ownership        |  | <can we query our state without this tool?> |
| 2.4★ | Composability / synergy    |  | <links across programs? stable shared IDs?> |
| 2.5  | Neutrality                 |  | <vendor-agnostic / single-vendor binding?> |
| 2.6  | Governance / auditability  |  | <gate we control, or policy we hope for?> |
| 2.7  | Reversibility / blast radius |  | <can we back out? what breaks?> |
| 2.8  | TCO incl. churn risk       |  | <will a platform ship this free in ~12mo?> |
| 2.9  | Strategic optionality      |  | <preserves or forecloses future moves?> |
|      | **Composite (weight 2.1/2.3/2.4)** |  | |

## Substrate impact (§5) — the weighting question
- **Grows or fragments the shared substrate?**  ☐ Grows  ☐ Neutral  ☐ Fragments
- **Who holds the system of record after this decision?** <us / vendor / split>
- **Exit cost / how we would back out:** <…>

## Rationale (2–3 sentences)
<why this move, this layer, now>

## Risks accepted + mitigations
- <risk> → <mitigation / who owns it>

## Sign-off
- Owner: <name / date>   ·   Reviewer: <name / date>
```

A "Fragments" answer on the substrate-impact line, or a low score on any of 2.1 / 2.3 / 2.4, is not an automatic veto — but it must be *consciously accepted in the rationale*, never silently. That single discipline is what keeps a thousand small convenient choices from quietly fragmenting the asset (§5) the org is trying to compound.

---

## Appendix B — Worked example: scoring three candidates across the stack

> **Illustrative, not authoritative.** These score *tool classes* on publicly-known characteristics as of mid-2026, to show the rubric in motion — they are not audits of named products, and a real evaluation must re-score the specific product version against the org's own weighting. The scores are arguable by design; the value is the *shape of the tradeoff* they expose, not the exact numbers. (Scale: 3 owned/strong · 2 partial · 1 weak · 0 absent/captive. ★ = heavily weighted.)
>
> *Real instances, June 2026 — **C1** ≈ Claude / GPT / Gemini model APIs; **C2** ≈ Claude Code Agent Teams, Codex sub-agents, Cursor multi-agent (each single-vendor); **C3** ≈ GNAP or Beads + Gas Town (and AIDA is one such instance). These shift fast — re-check before relying on them.*

| # | Criterion | **C1 — Frontier model API** (L0) | **C2 — Single-vendor coordination suite** (L4–L5) | **C3 — Git-canonical program-owned graph** (L1/L3) |
|---|---|:-:|:-:|:-:|
| 2.1★ | Lock-in / exit cost | 2 *(swappable if abstracted; behavior couples)* | 1 *(coordination state is the vendor's)* | 3 *(your git, open format, full export)* |
| 2.2 | Interoperability | 2 *(std API, per-vendor shape)* | 1 *(within the vendor's ecosystem)* | 3 *(git / files / open protocol)* |
| 2.3★ | Substrate ownership | 3 *(stateless to you; you keep I/O)* | 1 *(record is the vendor's)* | 3 *(system of record is yours)* |
| 2.4★ | Composability / synergy | 2 *(a capability, composes via API)* | 1 *(that vendor's agents only)* | 3 *(stable IDs, cross-program graph)* |
| 2.5 | Neutrality | 2 *(single-vendor, but multi-sourceable)* | 0 *(single-vendor by definition)* | 3 *(vendor-agnostic)* |
| 2.6 | Governance / audit | 2 *(you log around a black box)* | 1 *(audit inside the vendor's tools)* | 3 *(gates + traces you control)* |
| 2.7 | Reversibility / blast radius | 2 *(swap models with effort)* | 1 *(backing out loses the record)* | 3 *(it's files; trivially portable)* |
| 2.8 | TCO incl. churn risk | 2 *(low build; vendor-priced)* | 2 *(cheap to adopt; high churn exposure)* | **1** *(the substrate tax + maturity risk)* |
| 2.9 | Strategic optionality | 3 *(multi-sourcing preserves it)* | 1 *(deep adoption forecloses multi-vendor)* | 3 *(swap anything around it)* |
| | **Weighted read (2.1/2.3/2.4)** | solid | **weak** | **strong** |

**C1 — Frontier model API (L0).** *Decision: **Buy, multi-source.*** Capability is the point; lock-in is managed by keeping ≥2 providers swappable behind an abstraction. **Substrate impact: Neutral** — it never touches the graph. The only failure mode is coupling app logic to one provider's quirks.

**C2 — Single-vendor coordination suite (L4–L5).** *Decision: **Ride-native — but never make it the system of record.*** Its *unscored* strength is real: it is the best-integrated, lowest-friction experience, and where you are already in that vendor's session the exit cost is low. But it fails the weighted criteria hard — the coordination record is the vendor's. **Substrate impact: Fragments** (if it becomes the record). Use it for convenience; keep the durable copy in your own substrate (C3).

**C3 — Git-canonical program-owned graph (L1/L3).** *Decision: **Build / adopt-portable at the asset line — accepting the substrate tax consciously.*** It dominates ownership, neutrality, composability, and optionality — and **its one low score (2.8) is the honest one:** the substrate tax (build + maintain) and, for a home-grown or early-maturity instance, real reliability/support risk and the theory §12 churn exposure. **Substrate impact: Grows.** You own it not because it's cheap but because it's the asset and the lock-in safeguard (§5).

**The lesson from the contrast.** No candidate is "best" — they serve different layers, and a healthy portfolio uses **all three**: *buy* the model, *ride* the suite for convenience, *own* the substrate as the record. C2 wins on capability/UX and loses on ownership; C3 wins on ownership and loses on cost/maturity; the framework's job is to make that tradeoff **explicit and layer-dependent** rather than letting the best demo (usually C2) silently become the system of record — which is exactly how an org fragments the asset it is trying to compound.

---

*Working draft. Companion to the theory paper and research proposal; develop alongside them as real candidate tools are evaluated and the §5.1 experiment is run.*
