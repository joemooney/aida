# AI Agent Coordination — Market Landscape Refresh (2026-06-26)

**Status:** Dated snapshot (immutable once landed) · **Snapshot date:** 2026-06-26 · **Supersedes-by-cadence:** `2026-06-17-agent-coordination-market-landscape.md` (the prior roster; that file stays frozen at its date — this is a *new* point-in-time observation, not an edit of it) · **Feeds:** EPIC-48 finding **P8a** ("cross-vendor portable coordination is incentive-blocked, not capability-blocked") · **Method:** multi-modal discovery per `[[feedback_competitive_discovery_multimodal]]` — awesome-list sweep by category, known-builder watch (Yegge), the-problem's-many-names search, star-velocity / new-entrant scan, snowball from each found tool. Not a single-vocabulary search.

> **Frame (EPIC-48).** AIDA is the *probe*, not the subject. This snapshot exists to keep the dated observations under P8a honest as the field moves — including observations that *weaken* AIDA's wedges. Confidence: ✅ verified from a primary source · 🟡 likely · ⚠️ uncertain / single-source. Funding and star counts are point-in-time and directional only. Re-verify the 🟡/⚠️ items before any external citation.

---

## 0. What changed since 2026-06-17 (the delta, up front)

The nine-day-old baseline still holds in its broad shape — multi-agent *execution* is commoditized; a durable, queryable, **cross-vendor** coordination record is still the scarce thing. But three movements since then are load-bearing for P8a, and one of them **partially erodes a wedge the prior snapshot leaned on**:

1. **Gas Town OSS went cross-vendor** (✅, the consequential one). The 2026-06-17 baseline said Gas Town's OSS was "~Claude-only; cross-vendor via the paid Kilo cloud." That is now **stale**: the open-source Gas Town (gastownhall org, **v1.2.1, June 2026**) drives Claude Code, **Codex, Gemini, and Copilot** through its tmux worker roles, and the **Wasteland** trust network (a DoltHub-federated cross-town reputation/work-exchange fabric, introduced ~Mar 2026) lets many Gas Towns claim each other's work. So the durable *and* cross-vendor *and* free combination — which the baseline called "unoccupied white space" — now has a serious occupant. The wedge narrows from "nobody ships durable+cross-vendor+free" to a precise, incentive-anchored slice (see §6).
2. **A new minimalist *cross-vendor file-based messaging* lane appeared and is crowding** (✅). Three small entrants — **agmsg** (bash + SQLite, no daemon, WAL single-writer; Claude/Codex/Gemini/Copilot/Antigravity/OpenCode), **HUA-Labs/tap** (file-based P2P, Claude/Codex/Gemini-experimental), **shinpr/sub-agents-skills** (cross-LLM sub-agent routing as an Agent Skill) — all ship the *agent↔agent message-in-the-repo* idea that overlaps AIDA's mailbox. None ships a typed requirement graph or code traces; they are the messaging *substrate*, not the coordination *record*. But they validate the lane and they ship in days, not quarters.
3. **Frontier-lab churn continued** (✅): **Google Antigravity 2.0** relaunched at I/O as a multi-agent suite with a new Go **Antigravity CLI** (replacing Gemini CLI for consumers **June 18**) plus a **Managed Agents API + self-host SDK**; **GitHub Copilot** moved to usage-based **AI Credits (June 1)**; **Cursor** restructured Teams pricing (Standard/Premium seats). Antigravity's self-host SDK is the move to watch — a frontier lab shipping a *self-hostable* cross-vendor agent runtime is exactly the incentive shift P8a predicts would *not* happen at the coordination-record layer (it shipped at the runtime layer instead — see §6).

Two smaller corrections to the baseline's numbers: **spec-kit** is now cited at **~93k★** (baseline said ~113k — likely the baseline over-counted or a correction; treat the lower as current ⚠️), and **OpenSpec** at **~52k★** (baseline ~55k). Star drift inside a single quarter is noise; flagged only so the next refresh doesn't read it as a trend.

---

## 1. The durable-coordination tier (the rare, contested layer) — refreshed

This is the tier P8a is about. The fan-out launchers, frontier agents, and SaaS trackers are mapped in the 2026-06-17 baseline and have not moved materially in nine days; this refresh re-verifies only the *durable, queryable, cross-vendor coordination record* tier, where the change happened.

| Player | X-vendor (now) | State substrate | What changed / honest take | Conf |
|---|:-:|---|---|:-:|
| **Beads** (`bd`, Yegge / gastownhall) | ✅ (Claude/Codex/Factory/Cursor via AGENTS.md) | **Dolt** (embedded, default since v1.0) | No category move since baseline. Still the **richest typed-link vocab**; still positioned by its author as an *execution* tool that *defers planning/requirements to external tools* — the incentive split that ages well. Storage is a versioned **SQL DB**, not plain git (the "Beads is git-native" line stays retired). | ✅ |
| **Gas Town / Gas City** (Yegge) | ✅ **now in OSS** (Claude/Codex/Gemini/Copilot) | Beads (Dolt) + **Wasteland** (DoltHub federation) | **The change.** OSS cross-vendor is real now, not paid-cloud-only. Wasteland adds *cross-town* work-exchange + portable reputation stamps — a genuinely novel coordination primitive (towns post wanted-work, claim each other's, submit evidence). Yegge's own caveats persist: "cash guzzler ~$100/hr", "fixes get lost", needs a power-operator. AIDA's edge moves off "we're cross-vendor and they aren't" onto the *front* approval-gate + advisor-escalation + plain-git store. | ✅ |
| **farol-team/GNAP** | ✅ (OpenClaw/Codex/Claude/human) | **pure files in git** (`agents.json` + `board/{todo,doing,done}/`) | **Re-graded from "stalled" to "actively promoted RFC."** The baseline called GNAP quiet since Mar 2026; it is now framed as an **open RFC Draft** ("Git-Native Agent Protocol, zero servers") and is being *snowball-pitched into many ecosystems* (filed as integration issues against Letta, Roo Code, Playwright-MCP, OpenViking, RagaAI, awesome-lists). Coordination is 4 JSON files + git history as audit trail. Still the **nearest file-based fleet-coordination neighbor** to AIDA's shape; still no typed depends-on edges, still no code traces. The promotion push is a distribution signal worth tracking. | ✅ |
| **Claude Flow** (→ Ruflo, ruvnet) | ❌ Claude-centric | SQLite (`.swarm/memory.db`) | Unchanged. Durable-state orchestrator, Claude-only; "84.8% SWE-bench" remains **self-reported — discount.** | 🟡 |
| **agmsg** (fujibee) — *new* | ✅ (Claude/Codex/Gemini/Copilot/Antigravity/OpenCode) | single **SQLite** file (WAL, one writer) | New minimalist cross-vendor **messaging** layer — *not* a coordination record (no tasks/specs/graph; messages survive sessions). Closest in *shape* to AIDA's mailbox; deliberately does the *message bus* and nothing above it. ~days old. | ✅ |
| **HUA-Labs/tap** — *new* | ✅ (Claude/Codex; Gemini experimental, poll-only) | files-in-repo (P2P messages) | New cross-vendor **file-based P2P messaging**; "turn your repo into a shared workspace." Same lane as agmsg; reviews/handoffs as repo files. No graph, no traces. | ✅ |

**Read of the tier:** the durable-coordination layer is no longer near-empty. Beads (DB) + Gas Town/Wasteland (DB + federation, now OSS cross-vendor) hold the *richest* end; GNAP holds the *plainest-git* end and is actively recruiting; a fresh crop of minimalist *messaging* tools (agmsg/tap) fills the substrate beneath. What **none** of them ships is the combination AIDA probes: a **typed requirement graph with machine-checkable code-to-spec traces, a pre-work approval gate, and a plain-git (not SQL-DB) source of truth** — held together by one tool. That precise slice is the open question, not a blanket "cross-vendor durable coordination is unoccupied" (which is now false).

---

## 2. Honest pressure on AIDA's wedges (what this refresh weakens)

Per the EPIC-48 frame, the probe's *failures and erosions* are first-class data. Three:

- **"cross-vendor + free + self-hosted + durable" is no longer white space.** Gas Town OSS now occupies it. The defensible claim must drop the blanket form and narrow to the *combination with traces + a pre-work gate + plain-git*. State the narrower truth or don't claim it.
- **The code↔spec trace wedge is still a clean *product* wedge, but it's now claimed in the *literature*.** A 2026 academic paper, **ReqToCode** (arXiv:2603.13999), explicitly "embeds requirements traceability as a structural property of the codebase, making mismatches between code changes and requirement updates visible." Still no shipping competitor does machine-checkable `trace:ID` links — so the product wedge holds — but the *idea* is being formalized by others. Watch for a tool implementing it.
- **The messaging-substrate idea commoditized in a quarter.** agmsg/tap/sub-agents-skills shipped the cross-vendor agent-message-in-the-repo idea as weekend-scale tools. AIDA's mailbox is no longer novel *as a message bus*; its value has to live in being *part of the same record that carries the graph, the lease, and the trace* — i.e. the integration, not the channel.

---

## 3. Why the gap persists — anchored on incentive, not capability (P8a)

The central P8a finding is *strengthened*, not weakened, by this refresh — and crucially, anchored on **incentive**, the part that ages well:

- **Frontier labs shipped the cross-vendor move at the *runtime* layer, not the *coordination-record* layer.** Antigravity 2.0's self-host SDK + Managed Agents API, and Copilot's earlier multi-model shelf, prove the labs *can* and *will* ship cross-vendor when it sells more model tokens. They have not — and are structurally disinclined to — ship a *vendor-neutral, portable coordination record*, because such a record is precisely the thing that makes their model interchangeable. The capability is plainly present (they built harder things); the **incentive runs the other way**. This is the cleanest evidence yet for P8a's "incentive-blocked, not capability-blocked."
- **The one OSS player who *did* ship cross-vendor durable coordination (Gas Town) routes monetization through DoltHub/Kilo cloud and a power-operator model**, and its author still reports it as a "cash guzzler" that "loses fixes." The free+durable+cross-vendor *open* combination exists but is, by the builder's own account, not yet a stable or cheap thing to operate — which is itself the P8a point: the layer is hard to sustain precisely because nobody is incentivized to fund it as a portable commons.
- **Standards still refuse the slice.** MCP (Tasks primitive = single-requestor), A2A (explicitly stateless — "without sharing internal memory, state, or tools"), ACP (merged into A2A) — none standardizes a durable, multi-vendor-readable *coordination record*. KILLSWITCH.md (an emergency-stop spec) and the AXP "durable coordination" protocol surfaced in the awesome-list sweep as *governance/safety* primitives, not as a shared task/spec graph. The standards bodies are building the floor and the guardrails, not the record.

The honest verdict for P8a: the gap persists because the parties with the capability (frontier labs) are **disincentivized** to commoditize their own lock-in, and the parties with the will (OSS builders) lack the **funding model** to sustain a portable commons. That is an incentive story, and incentive stories age better than capability stories.

---

## 4. Discovery-method notes (so the next refresh stays honest)

What the multi-modal pass caught that a single-vocabulary search would have missed:
- **Known-builder watch (Yegge)** caught the Gas-Town-goes-cross-vendor reversal — the single most consequential delta — which an "AIDA-vocabulary" search would not have surfaced as a *change*.
- **Awesome-list category sweep** (`caramaschiHG/awesome-ai-agents-2026`, `andyrewlee/awesome-agent-orchestrators`) surfaced AXP / KILLSWITCH.md and the deterministic-orchestrator subcategory.
- **The-problem's-many-names** ("cross-vendor messaging", "agent-to-agent file-based", "issue-driven development") surfaced agmsg / tap / sub-agents-skills — none of which name themselves "coordination record."
- **Snowball** from GNAP's repo surfaced its multi-ecosystem RFC-promotion push (the re-grade from "stalled").
- **Star-velocity** flagged the spec-kit count discrepancy (re-verify next pass).

Carry-forward watch items: Antigravity self-host SDK adoption; whether any tool *implements* ReqToCode-style structural traceability; whether Wasteland federation gets real cross-org traction or stays demo-grade; agmsg/tap star velocity (do the minimalist messaging tools consolidate or fragment?).

---

## 5. Confidence / uncertainty log

- Gas Town OSS cross-vendor: ✅ from multiple secondary sources (The New Stack, Kilo, builder posts) + the gastownhall repo description; not hands-on verified. The *quality* of non-Claude support (functional vs nominal) is ⚠️ — verify with a real Codex/Gemini run before quoting depth.
- Wasteland federation: ✅ exists / described; real-world cross-org traction ⚠️ unverified (may be demo-grade).
- spec-kit ~93k★ vs the baseline's ~113k: ⚠️ one of the two is wrong; re-verify against the live repo next pass before citing either.
- ReqToCode: ✅ the paper exists (arXiv); it is *research*, not a shipping product — do not list it as a competitor, only as evidence the trace idea is being formalized.
- agmsg/tap/sub-agents-skills are days-to-weeks old; treat adoption as unproven. Their *existence* is the signal, not their traction.
- Self-reported benchmarks (Claude Flow) and funding/valuation figures remain point-in-time and are intentionally minimized.

---

## 6. The slice, stated precisely (for P8a)

The open slice is **not** "cross-vendor durable coordination" — Gas Town now occupies that. The precise open slice, as of this snapshot, is:

> a **single, plain-git (not SQL-DB) source of truth** that carries a **typed requirement graph** *and* **machine-checkable code-to-spec traces** *and* a **pre-work approval/authority gate**, readable by **any vendor's agent** with no server and no hosted-cloud dependency.

Each clause is doing work: *plain-git* excludes Beads/Gas Town (Dolt) and the SaaS trackers; *typed graph* excludes agmsg/tap (messaging only) and GNAP (no depends-on edges); *code traces* excludes everyone shipping today (only ReqToCode formalizes the idea, in research); *pre-work gate* excludes the merge-time-only verifiers; *any-vendor, no server* excludes the Claude-only and the cloud-coupled. The gap persists because no party is **incentivized** to assemble all five at once — frontier labs are disincentivized to commoditize lock-in, and OSS builders lack the funding model to sustain a portable commons. AIDA tests whether one tool *can* hold all five and at what cost; the honest answer the probe may return is still "yes, but the cost/distribution makes roll-your-own the wrong call for most teams."

---

## Sources (point-in-time, 2026-06-26)

Gas Town / Wasteland: The New Stack ("Gas Town comes to the cloud"), `github.com/gastownhall/gastown`, `kilo.ai/gastown`, Yegge's Gas City / Gas Town Medium posts. Beads: `github.com/steveyegge/beads` (README, FAQ, CHANGELOG), `steveyegge.github.io/beads`. GNAP: `github.com/farol-team/gnap` + its cross-ecosystem integration issues. Messaging lane: `github.com/fujibee/agmsg` (+ design.md, agmsg.cc), `github.com/HUA-Labs/tap`, `github.com/shinpr/sub-agents-skills`. Frontier churn: Antigravity 2.0 / CLI (I/O coverage, havoptic tracker), Copilot AI-Credits + Cursor pricing (developersdigest, lushbinary). Spec lane: `github.com/github/spec-kit`, OpenSpec, Augment Code SDD comparisons. Awesome-lists: `caramaschiHG/awesome-ai-agents-2026`, `andyrewlee/awesome-agent-orchestrators`. Standards/governance: killswitch.md; AXP via the awesome-list. Research: ReqToCode (arXiv:2603.13999). GitHub repo descriptions and star counts read 2026-06-26; treat as directional.

*This is an immutable dated snapshot. The next refresh lands as a new dated file; do not edit this one — the observations are frozen at 2026-06-26.*
