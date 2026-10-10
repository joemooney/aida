# Multi-vendor coordination refresh — what changed since the June baseline (2026-07-19)

**Specs:** TASK-1058 (parent EPIC-48) · **Status:** dated snapshot (immutable once landed; supersede with a new dated file) · **Baseline:** `docs/research/2026-06-26-agent-coordination-market-landscape.md` (market) + `2026-05-31-round2-moat-gaps-moves.md` (moat/moves) · **Evidence:** this repo's own commits, specs, benchmarks, and code — **not** a fresh web scan. External-market claims that would need re-verification online are marked **[needs-web-verify]** rather than asserted.

> Frozen at time T per the immutability discipline. Scope note: everything below is evidenced *inside* this repository (git log, spec graph, benchmark artifacts, source). Where the June baseline's market claims are repeated, they carry the baseline's date, not today's.

## The delta, up front

Between the 2026-06-26 market baseline and today, the load-bearing movement in multi-vendor agent coordination — as this repo can evidence it — happened **in the artifact, not the market scan**: the cross-vendor stance moved from thesis to routine operation. Five changes:

1. **The agent-surface question is settled by measurement: the token-efficient CLI is the primary agent surface; MCP is the typed option.** The 2026-06-29 72-cell benchmark (SPIKE-73, `bench/agent-surface/results/report.md`) priced identical substrate reads over four surfaces: plain CLI 100% success at ~$0.036/task avg; MCP 89% success at ~$0.071 (~2×); MCP with on-demand schema loading recovered success (100%) but not cost (~1.8× — the schema round-trips eat the input-token savings, and turns/tools *rise*); TOON output at CLI parity. The premium is structural, not a schema-loading artifact. The repositioning is already landed product-wide (TASK-986, commit 59108552f): CLAUDE.md, README, and the positioning docs now say "CLI primary, MCP typed option." What this changes for coordination positioning: **"we expose the graph over MCP" is no longer the lead claim — "any vendor with a shell can join the fleet cheaply" is.** The cross-vendor portability pilot (TASK-870, 2026-06-19) already showed the mechanism: Codex onboarded to a live fleet in ~50s on the stock CLI alone, zero MCP config, zero glue.

2. **The AXI incorporation (EPIC-56) went from sighting to substantially shipped in three weeks.** The 2026-06-28 AXI note (`2026-06-28-axi-ecosystem.md`) closed with "adopt, don't just defend." As of today 19 of EPIC-56's 21 direct children are closed (only TASK-975 and STORY-713 remain open), and the completed set includes the items that matter for multi-vendor coordination: **SPIKE-74** (agent-agnostic drain backend — the keystone loop is no longer hardcoded to `claude -p`), TASK-970/972 (token-efficient content-first agent output), STORY-712 (event-driven zero-token supervision, replacing timer polling), STORY-714 (worktree warm-pool — per-agent build cost cut ~30× per `2026-06-29-warm-pool-build-delta.md`), and TASK-986 (the MCP reframe above). The June baseline treated AXI as a challenger to answer; the honest July read is that its interface-ergonomics claims were **tested, confirmed on our own tools, and absorbed**. The AXI ecosystem's own trajectory since 2026-06-28 (stars, adoption, "AXI standard" framing) is **[needs-web-verify]**.

3. **Codex is now a proven implementer vendor on this repo — no longer an n=1 portability existence-proof.** Since 2026-06-01, **43 solo `[AI:codex]` commits** have merged to main (118 all-time including `[AI:codex+claude]` mixed-authorship), and the recent run is not smoke-test material: fixes and features on core machinery — `pr_ship`, queue gating, drain locks, scaffolding, worktree lifecycle (e.g. #1489–#1504, #1519). Stacked with the controlled evidence — the I2 cross-vendor ablation (Codex held a buried ambient rule at 100%, identical to Claude) and the ~50s zero-integration onboarding pilot — the claim upgrade is: *from* "a second vendor **can** join the fleet" *to* "a second vendor **routinely ships merged, CI-gated work** through the same discover→claim→ship→verify loop, on the stock CLI." **Precision boundary:** proven on *this* repo, one operator, one vendor pair (Claude↔Codex). It is not a market claim, and it says nothing about a third vendor until one ships the same way.

4. **Multi-vendor ≠ symmetric vendors: the fleet now runs an explicit per-vendor trust policy, and that is itself a coordination-layer finding.** SPIKE-76 (completed; plan at `docs/plans/2026-07-02-spike-76-dispatch-resilience.md`, operator directive 2026-07-02) codified the tiering: Claude and Codex as headless implementers; **Antigravity (AGY) as draft-for-review, cross-validation, and mechanical/bounded work only** — routed via briefs, never given unattended merge authority. That policy is grounded in recorded failure evidence (TASK-123: three distinct AGY failure modes in one 8h overnight session, including byte-for-byte plagiarism with rebranded trace tags) and is enforced structurally: `compete::vendor_adapter` models antigravity as `HumanBriefed` (no headless argv exists for it), so the orchestrator *cannot* dispatch AGY unattended even by mistake. The positioning consequence: a credible cross-vendor coordination layer is not "N interchangeable vendors" — it is **a neutral record plus a per-vendor capability/trust policy the substrate enforces**. None of the June-baseline neighbors (Gas Town, GNAP, agmsg/tap) ships differentiated per-vendor trust tiers as far as the baseline recorded — current state **[needs-web-verify]**. Whether AGY has since gained a credible headless CLI (which would let the adapter row upgrade from `HumanBriefed`) is likewise **[needs-web-verify]**.

5. **The vendor-at-execution-layer principle is now load-bearing in code, not a stance.** Vendor specifics live behind adapters at the *execution* layer — `compete::vendor_adapter`'s one-row-per-vendor table (`claude -p …` / `codex exec …` / antigravity → `HumanBriefed`), SPIKE-74's backend trait for drain phases, headless phase routing to whichever vendor binaries the machine actually has, Codex custom-prompt scaffolding at init — while the *coordination* layer (store, mailbox, briefs, leases, roles, queue) stays vendor-free. Adding a vendor is a single adapter row plus a trust-tier decision; nothing in the coordination record changes. This is the concrete form of "ride native within-vendor; own the cross-vendor-durable layer," and it is the design answer to the June baseline's observation that frontier labs ship cross-vendor at the *runtime* layer while leaving the *record* layer unclaimed.

## What this does to the standing moat picture (2026-05-31 round-2 doc)

The round-2 moat table holds, with two refinements:

- **"Transport (MCP) — COMMODITIZED — RIDE"** stays correct, but the *reason* sharpened: we now have first-party measurement that the typed transport carries a ~2× structural premium for agent work. "Invest in the graph payload, not the pipe" was right; the pipe agents actually prefer turned out to be text. The MCP tailwind claim ("every agent can already query our graph") survives as a *reach* claim, not an *economics* claim.
- **"Git-canonical multi-vendor substrate — DIFFERENTIATED (implementation)"** upgrades from implementation-differentiated to **operationally evidenced**: two vendors shipping merged work through one record on this repo, with a third vendor held at a policy-enforced trust tier. The June-baseline caveat stands unchanged: cross-vendor-durable-free is *contested* space (Gas Town OSS), so the precise slice remains "typed graph + code traces + pre-work gate + plain-git store + per-vendor trust policy," not "cross-vendor coordination" unqualified. Neighbor movement since 2026-06-26 is **[needs-web-verify]**.

## Positioning docs made stale by the above

The 2026-07-09 sweep (`177e1f365`) already refreshed the vs-* set for the CLI-primary reframe, so the residual staleness is narrow. Checked today, file by file:

**Edited in this refresh (surgical, dated notes only):**

| File | Stale claim | Fix applied |
|---|---|---|
| `vs-agent-teams.md` | Vendor-table row + prose listed Antigravity as a co-equal *driving* vendor ("Claude, Codex, Antigravity all drive one git-canonical substrate", "routes the same spec across vendors") | Dated notes: cross-vendor routing is trust-tiered — Claude/Codex headless implementers, Antigravity draft-for-review only (SPIKE-76) |
| `vs-claude-code-workflows.md` | Same vendor-table row overstatement | Same dated note |
| `vs-saas-pm.md` | Composition table framed agent context as "AIDA via MCP" with no CLI-primary caveat | Row now leads with the token-efficient CLI; dated SPIKE-73 note |
| `vs-axi.md` | Synthesis presented the AXI incorporation as an open recommendation ("the remaining live recommendation") | Dated note: EPIC-56 substantially shipped (19 of 21 direct children closed, incl. SPIKE-74/STORY-712/STORY-714); AXI-side movement since 2026-06-28 flagged [needs-web-verify] |
| `README.md` (positioning index; edited beyond the strict vs-* scope because it is the clearest remaining stale claim) | Niche statement: "served to AI through MCP and to humans through a small CLI" | Reworded to CLI-primary / MCP-typed-option, with a dated note citing SPIKE-73 |

**Flagged, not edited (mild / judgment-call, left for the owner):**

- `composition.md` Recipe 3 ("Any MCP editor reads the same graph") — accurate for MCP editors specifically, but never mentions the CLI-primary path; a reader could infer MCP-first. One-line caveat candidate.
- `vs-agent-teams.md` incentive-section sentence ("lets a Codex or Antigravity session pick up the same spec") — an AGY session *does* pick up briefs, just at the draft tier; the two dated notes elsewhere in the doc carry the qualification, and editing inside the incentive argument would muddy it.
- `agent-decision-matrix.md` step 5 ("Standardize coordination on MCP, the one layer both agents read identically") — about the neutral coordination *seam*, not agent output economics; the file already carries the SPIKE-73 caveat. Borderline, left as is.
- `docs/positioning/README.md` "Maintenance rhythm" says each doc keeps its date "in frontmatter" while the docs actually use an italic `*Last updated:*` line — internal inconsistency, cosmetic, not a positioning claim.

**Checked clean on all five refresh topics:** `agent-decision-matrix.md` (already CLI-primary-aware), `vs-a2a.md`, `vs-spec-kit.md`, `vs-kiro.md`, `vs-karpathy-md.md`, `vs-langgraph.md`, `vs-ultraplan.md`, `vs-ultrareview.md`, `vs-claude-code-subagents.md`, `vs-continue.md`, `vs-aider.md`, `when-not-to-use-aida.md`, `composition.md` (beyond the Recipe 3 flag above). Notably, **no doc under-claims cross-vendor** — the residual staleness ran in the *over*-claiming direction (Antigravity symmetry), which is the direction the precision discipline exists to catch.

## [needs-web-verify] register (for the next web-connected refresh)

1. AXI ecosystem trajectory since 2026-06-28 — star velocity, adoption, any "AXI standard" framing, and whether the AXI-vs-MCP benchmark was independently reproduced or contested outside this repo.
2. Gas Town / Beads / Wasteland current state — the 2026-06-26 "OSS cross-vendor is real" finding and star counts have not been re-verified since.
3. GNAP RFC promotion — whether the snowball-pitch push converted into adoptions.
4. agmsg / tap / the minimalist cross-vendor messaging lane — growth or consolidation since June.
5. Whether any of the June neighbors shipped differentiated per-vendor trust tiers (would erode delta #4's "as far as the baseline recorded" qualifier).
6. Antigravity headless CLI status — a credible unattended surface would let the `HumanBriefed` adapter row and the AGY draft-only policy be revisited.
7. MCP/A2A working-group movement on coordination state (the A2A↔MCP interop group, NIST agent-standards) — the P8b watch items.
8. Frontier-lab first-party coordination features going cross-vendor (the P8a falsifier) — none known at the June baseline; July state unchecked.

## Refresh cadence

Next refresh trigger: any [needs-web-verify] item resolving against us, a third vendor shipping merged work through the fleet, or ~6 weeks — whichever first.
