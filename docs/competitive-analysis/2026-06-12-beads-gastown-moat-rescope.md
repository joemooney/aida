# Beads + Gas Town: substrate-competitor deep-dive + moat re-scope (2026-06-12)

**Spec:** SPIKE-53 · **Status:** living (dated snapshot) · **Mode:** advisor-run research; decision escalated to operator
**Provenance:** Beads architecture **[V]** = verified against the GitHub README this run; Gas Town **[W]** = web-summary level (multiple consistent sources, not source-verified). Frozen at time T — supersede with a new dated file.

> ⚠️ This re-scopes the moat. The prior keystone (`2026-05-31-round2-moat-gaps-moves.md`) anchored on **Spec Kit** and concluded "all five AIDA differentiators are absent in competitors → the moat holds, only distribution is the problem." **That anchored on the wrong competitor.** Beads is the right one, and it has four of the five — plus distribution, plus an orchestrator.

## What they are

- **Beads (`bd`)** — Steve Yegge, ~18.7k stars, **v1.0** (Apr 2026). Positioned as **"persistent, structured memory for coding agents… replaces messy markdown plans with a dependency-aware graph."** Explicitly **"not a requirements specification system."** **[V]**
- **Gas Town** — Steve Yegge, **v1.0** (Apr 2026). "Kubernetes for AI coding agents": a **Mayor** (lead Claude Code instance) orchestrating **20–30 parallel worker agents** with persistent identity / ephemeral sessions. **All orchestration state lives in Beads** (data plane + control plane). **[W]**

Together: **a typed dependency graph (Beads) drained by a parallel orchestrator (Gas Town) over MCP** — which is, almost exactly, the architecture the keystone called AIDA's "durable, un-commoditized core."

## Verified Beads architecture **[V]** (from the README)

| Capability | Beads | AIDA |
|---|---|---|
| Source of truth | **Dolt** (embedded, `.beads/embeddeddolt/`); `.beads/issues.jsonl` is an export, *explicitly not the source* | **git** (YAML in orphan `aida-store` branch), *same repo as code* |
| Read cache | Dolt is the store; cell-level merge | SQLite `.aida/cache.db` (rebuildable) |
| Stable IDs | hash-based `bd-a1b2` + hierarchical `bd-a3f8.1.1` (collision-safe, opaque) | typed semantic `STORY-549` / `BUG-492` (human-legible) + agreed-ids |
| Typed relationships | `relates_to`, `duplicates`, `supersedes`, `replies_to`, parent-child, blocks | BlockedBy/blocks, related-to, parent, supersedes, … |
| Ready/unblocked gate | `bd ready` (dependency-aware) | `aida burndown plan` (pickability gate) |
| MCP | yes | yes |
| Parallel orchestrator | **Gas Town** (Mayor + 20–30 workers) | `queue work --auto-complete` / `/aida-burndown` |
| **Code↔spec trace enforcement** | **NOT mentioned — no code-to-issue linking** | **`// trace:SPEC-ID`, commit-gated** |
| Positioning | **agent memory / issue tracker** | **requirements / product-intent + governance** |
| Distribution | **~18.7k stars, Yegge's reach, Amp + Claude Code adoption, v1.0** | pre-distribution |

## The honest re-scope

**Commoditized / now at PARITY (was claimed as AIDA's moat):** typed relationship graph · stable IDs · dependency-aware ready gate · MCP graph · version-controlled substrate · an orchestrator that drains the graph in parallel. Beads/Gas Town has all of these, shipped, at v1.0. **The "git-canonical typed graph drained by an orchestrator over MCP" is no longer a differentiator.** And on distribution, Beads doesn't just match — it dominates (18.7k stars + Yegge's megaphone vs AIDA's ~0).

**What survives — verified, narrower, and different in KIND:**

1. **Code↔spec trace enforcement** — Beads has *no* code-to-issue linking. AIDA's `// trace:SPEC-ID` commit-gated loop is a real, confirmed edge. (But Beads *could* add it.)
2. **Requirements/product-intent + human governance** — Beads is explicitly *"not a requirements specification system"*; it's the **agent's memory** (bottom-up). AIDA is the **human's requirement graph** (top-down) with disposition gates (ADR-3 approval), roles (product/advisor/implementer), the advisor seat, and a lifecycle tied to product intent. **Different altitude, not a feature gap.**
3. **Truly git-canonical** — AIDA's specs are literally git YAML in the *same repo as the code*; Beads' truth is Dolt with a git *export*. Narrow but real for "your specs branch and diff *with* your code." (Counter: Dolt's cell-level merge is arguably better for multi-agent conflict.)
4. **Lifecycle coupled to PR/CI/merge** with Done→Completed auto-bump + reviewer phase. (Uncertain edge — Gas Town orchestrates too; depth-of-coupling unverified.)

## The durable wedge is an INCENTIVE divergence, not a feature list

Yegge's Beads/Gas Town philosophy is **agent autonomy** — "give the agents memory and let them run" (the Mayor + 20–30 workers, "50 First Dates" framing). AIDA's philosophy is **human governance of agents** — approval gates, roles, the advisor seat, trace enforcement, supervised lifecycle. **Beads structurally won't pursue heavy human-governance/requirements-discipline because it cuts against its "unleash the agents" thesis.** That incentive divergence ages better than any capability claim (which Beads could copy).

## The moat re-scope — OPTIONS (operator decision)

- **A — Move up the stack.** Concede the substrate (Beads won it on parity + distribution). Reposition AIDA as **requirements-governance + trace-enforcement for *supervised* agent development** — explicitly the anti-thesis to Yegge's agent-autonomy.
- **B — Interoperate.** Build Beads import/export; position AIDA as the governance/requirements/trace layer *on top of or alongside* Beads. Ride the 18.7k-star wave (the "ride the standard" move, like AGENTS.md) instead of fighting the substrate war.
- **C — Differentiate-and-race** on trace-enforcement + lifecycle + multi-vendor governance. **Not recommended** — racing a 18.7k-star, Yegge-backed project on features it can copy is the losing game the keystone already warned about.
- **D — Niche to regulated/enterprise ALM.** Where governance + audit + requirements discipline are *mandatory* and "let agents run free" is a non-starter. Beads' agent-autonomy framing becomes a *liability*; AIDA's governance is the feature. (Pairs with the ReqIF option from the keystone.)

## Recommendation (escalated, not decided)

**Lean A + B, framed by D.** Stop pitching "git-canonical typed agent graph" as the moat — Beads commoditized it and won distribution. Reposition AIDA as the **human-governance + requirements-discipline + trace-enforcement layer for supervised agent development**, seriously evaluate **interoperating with Beads** rather than competing on substrate, and treat **regulated/enterprise** (where governance is mandatory) as the segment where the incentive divergence is sharpest. The honest one-liner candidate: *"Beads is your agent's memory; AIDA is your product's requirement graph with the human in the loop — approvals, roles, traced code, and an audit trail Beads doesn't keep because it's built to let agents run, not to govern them."*

## Tripwires

- **Beads adds code-to-issue trace linking** → erodes surviving edge #1.
- **Beads/Gas Town adds approval gates / requirements framing / human-governance** → erodes the incentive-divergence wedge (watch for it; unlikely given their thesis).
- **Gas Town adds CI/PR-lifecycle coupling** → erodes edge #4.
- **A Beads MCP/registry standard** consolidates the substrate layer → makes "ride Beads" (Option B) more urgent.

## Honest meta-read

This is a harder finding than the keystone's confident "moat holds." A famous engineer shipped AIDA's architecture with 18.7k-star distribution. **The substrate is no longer the moat — and pretending otherwise is the dangerous path.** AIDA's defensible position is narrower, higher up the stack (governance/requirements/trace), and anchored on an incentive divergence (govern vs unleash) rather than a feature. The decision among A/B/C/D is the operator's. **Confidence:** Beads architecture high (README-verified); Gas Town medium (web-summary); the incentive-divergence thesis is analysis, not fact.
