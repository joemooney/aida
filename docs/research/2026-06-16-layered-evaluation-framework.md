# Evaluating AI Tooling Across the Coordination Stack

### A layered decision framework — feature requirements, risks/benefits, and lock-in safeguards for allocating money and internal resources

**Status:** Draft · **Date:** 2026-06-16 · **Companion to:** `2026-06-16-coordinating-multi-vendor-agent-fleets.md` (the theory) · **Audience:** decision-makers allocating budget / internal development to AI tooling · **Frame:** vendor-neutral; AIDA appears only as a worked existence-proof, never as a recommendation

> The theory paper argues *what is true* about the multi-vendor agent-coordination problem and leaves the roll-your-own verdict **open**. This document is the operational other half: a portable rubric for deciding, per layer, **build vs buy vs ride-native vs wait** — designed to (1) inform resource allocation, (2) safeguard against lock-in, and (3) build deliberately toward the one durable competitive asset (§5). It is a *living* instrument: the ground shifts, so re-score on a cadence (§6).

---

## 0. How to use this

For any candidate tool or internal build:

1. **Place it on the stack** (§1) — which layer (L0–L5) does it primarily serve? Many tools span layers; score each layer separately.
2. **Score it on the cross-cutting rubric** (§2) — the criteria that apply to *any* tool at *any* layer. This is where lock-in and substrate-ownership get caught.
3. **Run the per-layer checklist** (§3) — "what good looks like" + the layer's specific capture risk.
4. **Apply the decision lens** (§4) — build / buy / ride-native / wait, given today's ground.
5. **Test it against the asset thesis** (§5) — does this choice grow or erode the shared substrate that becomes your competitive advantage?
6. **Diary the decision** and re-evaluate on cadence (§6) — record *why* now, so the next review can see what changed.

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
- [ ] Enforcement is a *gate* you control, not a *policy* you hope is honored (theory §4, P1).

### 2.7 Reversibility / blast radius
- [ ] You can back the decision out; failure is contained, not org-wide.
- [ ] Adopting it does not silently make it load-bearing everywhere before you've validated it.

### 2.8 Total cost of ownership (incl. churn risk)
- [ ] Build + maintain + the risk a vendor obsoletes this layer for free within ~12 months (theory §12: "building on sand").
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
- **What good looks like:** invariants enforced as programmatic **gates** (refuse the bad action), not documented rules; full audit trail; policy you control and can change.
- **Capture risk:** governance logic trapped inside a vendor's agent platform — you can't enforce what you can't reach.
- **Default lean:** **Own the gates, ride the vendor's execution.** Place enforcement at *your* substrate boundary so it holds regardless of which agent acts.

### L3 — Collaborative state
- **What good looks like:** durable store with sane concurrent-edit handling (conflict-light merge, theory §5); open format; survives any single tool; history preserved.
- **Capture risk:** a proprietary database as the only home for your shared state.
- **Default lean:** **Own / open-format.** Prefer a store you can read with `git` and a text editor over a service you query only through one vendor's UI.

### L4 — Autonomy & role orchestration
- **What good looks like:** role separation (implementer / advisor / human); punt-and-continue resilience; escalation routing; deep observability into what the fleet did and why.
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

---

## 6. Cadence — a living evaluation

The stack's value distribution shifts during any decision cycle, so this framework is run repeatedly, not once:

- **Re-score on a cadence** (quarterly is a reasonable default for a fast-moving field) and on trigger events: a vendor ships a layer for free, a neutral incumbent appears, a lock-in bites.
- **Keep dated, immutable snapshots** of each evaluation so the *trajectory* is visible — what moved up the stack, how fast (theory §13's "simply watch whether vendor surfaces converge on the upper layers").
- **Record the revisit trigger** with every "wait" decision, so waiting is a deliberate, time-boxed posture and not drift.

---

*Working draft. To develop with: a worked scoring of 2–3 real candidate tools per layer, the §5 synergy experiment design, and a one-page decision-record template teams can fill per adoption.*
