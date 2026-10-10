# SPIKE-58 — governance-depth verification: does the ADR-4 repositioning survive a skeptic?

<!-- trace:SPIKE-58 | ai:claude -->

**Dated 2026-06-12. Provenance: agent-verified via live web/repo research (sourced below); NOT advisor- or operator-re-verified. The hands-on items in "Needs operator" are explicitly SPIKE-58's job and are NOT settled here.** Companion to [2026-06-12-beads-gastown-vs-aida.md](2026-06-12-beads-gastown-vs-aida.md), which it **corrects** on governance.

## Verdict (blunt)

The ADR-4 headline — *"AIDA is the human-governance layer Beads/Gas Town lack"* — is **PARTLY-TRUE, and overclaim-risk if shipped as written.** It will NOT survive a skeptic who reads the Gas Town docs: GT has a real, tiered, **design-fork-aware** escalation protocol (Deacon→Mayor→Overseer), and Beads ships actor-attributed audit logging (v1.0.1). Neither is governance-naive.

**The precise defensible slice (narrow the copy to THIS):**

> AIDA governs at the **front** of the pipeline — a programmatic pre-work **approval-authority gate** (Draft→Approved is advisor-only; a cold-boot agent cannot self-bless work into the ready pool) — and **binds every action to a requirement graph** (enforced code↔spec trace + structured per-spec field-delta history). Beads and Gas Town govern at the **back and side** — reactive escalation and merge gates — with **no pre-work approval gate and no spec↔code binding.**

That claim is true, sourced, and narrow. The blanket version is not.

## The 3 genuinely AIDA-only governance edges (verified — headline these)

1. **Pre-work approval-authority gate as a programmatic invariant.** Draft→Approved is advisor-only; a cold-boot agent can't self-bless work in. Both Beads (self-claim) and Gas Town (GUPP: *"If there is work on your Hook, YOU MUST RUN IT… No confirmation. No questions. No waiting."*) are **autonomous-by-design at the execution boundary.** **The single cleanest, best-sourced edge.** (High confidence)
2. **Enforced code↔spec traceability + structured per-spec field-delta history.** Neither tool links *code* to a spec. Beads' audit is event/actor-level (`interactions.jsonl`); AIDA's `history:` is a per-field who/when time-series **plus** `trace:` comments tying commits to specs. Value: a human-reviewable "what changed in the code, against which approved requirement, by which agent." (High)
3. **The advisor as a *calibrated AI escalation tier below the human*.** GT escalates worker→Deacon→Mayor→human; AIDA inserts an advisor seat that resolves-or-escalates and records calibration (predicted vs actual). Novelty is the calibrated AI middle-tier, not escalation per se. (Med — architecture difference, not a capability GT wholly lacks.)

## Corrections to the prior snapshot (it was too generous to AIDA on governance)

- ❌ **DROP "Beads/GT have no audit trail."** Beads v1.0.1 ships audit logging + actor (human-vs-agent) attribution (`BEADS_ACTOR` → `interactions.jsonl`). Gas Town tracks all changes in git. → Claim *code↔spec trace + structured field-delta history*, not "audit trail."
- ❌ **DROP "agents just decide, no escalation."** Gas Town escalates **design forks** ("multiple valid paths, need choice" / "architectural choices that need human judgment") to a human by severity. → Claim the *calibrated advisor middle-tier*, not the existence of escalation.
- ❌ **DO NOT headline containment/blast-radius or merge-queue mechanics.** GT is at parity or ahead — worktree isolation + Refinery merge-queue with bisect + Witness/Deacon watchdog recovery.

## Honest counter (don't strawman them)

Beads/GT's lighter touch is a **deliberate, defensible design for throughput at scale.** GUPP's "no confirmation, ever" is *the point* at 20-30 agents — a pre-work approval gate would bottleneck the fleet. For a solo dev firehosing agents, AIDA's gates are friction. The repositioning must own this: AIDA's wedge is **governed/auditable agent development where the human stays the authority** — valuable to teams/regulated contexts, *cost* to a throughput-maximizing soloist. (This is why the SPIKE-59 regulated/enterprise beachhead is the natural buyer.)

## Needs operator hands-on (SPIKE-58 proper — NOT settled here)

1. Does Gas Town have approval-preference settings ("always auto") / `gt escalate ack`? (A search snippet claimed yes; the authoritative escalation doc doesn't mention them. `gt --help` / live-doc check.)
2. **Does an escalated GT agent BLOCK or proceed?** Load-bearing for any "AIDA pauses, GT doesn't" framing. Docs don't say. Hands-on only.
3. Exact `.beads/issues.jsonl` field schema + cross-version stability (required before any sidecar/import — diff a live `bd export` across two versions).
4. Does Beads' actor attribution distinguish a *named* AI tool (claude vs codex) or just agent-vs-human?
5. **Is GT's governance *good* or merely *present*?** Does the Overseer get a usable human-review surface or a flood of P2 beads? Only your hands-on can judge.

## Sources

[Beads README](https://github.com/steveyegge/beads/blob/main/README.md) · [FAQ](https://raw.githubusercontent.com/steveyegge/beads/main/docs/FAQ.md) · [CHANGELOG (audit logging v1.0.1)](https://raw.githubusercontent.com/steveyegge/beads/main/CHANGELOG.md) · [DeepWiki: Beads AI integration](https://deepwiki.com/steveyegge/beads/8-ai-agent-integration) · [Gas Town README](https://github.com/gastownhall/gastown/blob/main/README.md) · [Escalation Protocol](https://docs.gastownhall.ai/design/escalation/) · [DeepWiki: Gas Town GUPP](https://deepwiki.com/steveyegge/gastown/1.2-quick-start-guide)

## Recommendation (for the operator)

Ship the repositioning, but **narrow the headline** from "the governance layer they lack" to **"front-of-pipeline approval authority + spec↔code binding"** — the two edges that survive a sourced skeptic. Gate ALL repositioning copy (WS1–WS9) on this slice + the corrections above. The blanket claim does not survive.
