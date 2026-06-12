# Beads + Gas Town vs AIDA — competitive snapshot (SPIKE-53)

<!-- trace:SPIKE-53 | ai:claude -->

**Dated 2026-06-12. Provenance: agent-verified via live web/GitHub-API research (sourced below); not yet advisor-re-verified. Load-bearing claims flagged.**

## Bottom line (strategic)

Beads (Steve Yegge, ~24.5k stars in ~8 months) is a fast-moving, well-distributed adjacent ecosystem — and the most important verified fact is that **its author explicitly positions it *out* of AIDA's space.** Yegge's own words: *"Everyone is focused on making planning tools, and Beads is an **execution tool**"* — he tells users to *"use your favorite planning tool… then translate it into Beads."* ([Yegge, Beads Best Practices](https://steve-yegge.medium.com/beads-best-practices-2db636b9760c))

So Beads is **adjacent, not head-on**: it's agent *task-memory / execution* ("the 50 First Dates problem"); AIDA is *requirements modeling + enforced traceability*. The 2026-05-31 keystone worry ("typed multi-vendor durable graph is AIDA-unique") is **partly stale** (Beads has a typed git-backed graph) — but the *more durable* framing is the category split, which Beads' author chose deliberately.

**The real threat is distribution, not features.** Beads has ~24.5k stars + Yegge's name; AIDA is tiny. That gap is the thing to worry about — not the feature matrix.

## Verified AIDA edges (only what the research confirmed Beads *lacks*)

1. **Code↔spec trace comments + enforcement.** Beads has *no* code-to-task linkage at all (verified across README, FAQ, docs, Yegge's article). Its `discovered-from` links issues to issues, not code to issues. **Cleanest, unambiguous differentiator.**
2. **Rich lifecycle + per-spec history + merge-driven auto-bump.** Beads has **3 unenforced** states (`open`/`in_progress`/`closed`; `in_progress` is convention, "the bd tool does not enforce this"), no transition-history time-series, no merge auto-promotion. AIDA has the 7-state machine + structured `history:` + auto-bump.
3. **Requirements-modeling category** — Beads is, *by its author's explicit design*, execution/task-memory and defers planning/requirements to external tools. This is an **incentive moat** (it ages well): the space is AIDA's not because Beads *can't* build it, but because its author has publicly chosen not to.
4. **Plain-git/YAML source of truth** — but **state this precisely.** Beads *migrated to Dolt* (a git-for-data SQL DB): SQLite removed v0.58, **embedded Dolt the default at v1.0**. AIDA's source of truth is plain-git YAML; Beads' is a SQL database. The correct claim is *"plain-git substrate vs a SQL-DB substrate"* — **NOT** "they need a database server" (embedded Dolt needs none).

## Corrections to prior moat claims (the headline edits)

- ❌ "the typed, durable, multi-vendor graph is absent from agents / AIDA-unique" → **stale.** Beads is a typed, git-backed, vendor-neutral graph. Drop this as a headline.
- ⚠️ "plain-git, zero-infra" → **true but narrower than implied.** Keep it, but as "plain-git/YAML source of truth vs SQL-DB," not "no server needed" (embedded Dolt also needs no server).
- ✅ Headline instead on the three *verified* edges: **trace enforcement · lifecycle+history · the requirements-modeling category Beads abdicates by design.**

## Near-parity / where Beads is ahead (don't claim these)

- **Typed relationships** — near-parity. Beads: `blocks` / `parent-child` / `discovered-from` / `related` (core set; docs are internally inconsistent — README mentions more). `discovered-from` (agent files a discovered bug linked to its origin) is a nice agent-native edge AIDA lacks.
- **MCP** — near-parity; both ship official MCP servers. (Both ecosystems independently concluded CLI+hooks is token-cheaper than MCP — worth noting.)
- **Orchestrator** — **Gas Town is broader/more mature** than AIDA's burndown: a 20-30+ agent fleet manager (Mayor/Deacon/Witness watchdogs, Refinery merge-queue, Convoys). AIDA's edge there is the advisor-escalation layer, not scale. *(Caveat: only the documented architecture was verified, not real-world reliability at scale.)*
- **Distribution** — Beads/Gas Town ahead by orders of magnitude (~24.5k + ~15.9k stars).

## The bridge (Trojan horse) — viable, low cost

Beads' `.beads/issues.jsonl` is an intentionally-stable, git-tracked interchange format (`bd export`/import) — **no Dolt coupling to read/write it.** An AIDA import (JSONL → AIDA spec types + relationship graph) is a **small-EPIC effort** (reader + writer + field/type/status mapping + tests). Strongest as a **one-way import**: *"bring your Beads graph into AIDA, gain trace + lifecycle."* Two-way is possible but lossy on AIDA's richer fields. Main design choice: preserve Beads `bd-` IDs as an alias vs re-allocate. *(Caveat: confirm the live `issues.jsonl` schema before building — Beads versions churn fast.)*

## Couldn't verify / needs a human check

1. Beads' authoritative current relationship taxonomy (docs inconsistent — 4-type core set is likely current; `relates_to/duplicates/supersedes/replies_to` may be deprecated/aspirational).
2. Yegge's "tens of thousands of daily users" — qualitative, unverifiable.
3. Gas Town's real-world reliability at 20-30+ agents (only the documented design was checked).
4. Exact current `issues.jsonl` field schema (verify against a live `bd export` before any bridge).
5. That embedded Dolt is truly zero-setup on all platforms (v1.0 CHANGELOG says yes; v0.56 needed a server — worth a hands-on `bd init` smoke test).

## Sources

[Beads repo](https://github.com/steveyegge/beads) · [README](https://github.com/steveyegge/beads/blob/main/README.md) · [FAQ](https://github.com/steveyegge/beads/blob/main/docs/FAQ.md) · [core-concepts/issues](https://gastownhall.github.io/beads/core-concepts/issues) · [CHANGELOG](https://raw.githubusercontent.com/steveyegge/beads/main/CHANGELOG.md) · [beads-mcp](https://raw.githubusercontent.com/steveyegge/beads/main/integrations/beads-mcp/README.md) · [Yegge — Beads Best Practices](https://steve-yegge.medium.com/beads-best-practices-2db636b9760c) · [ianbull independent review](https://ianbull.com/posts/beads/) · [Gas Town repo](https://github.com/gastownhall/gastown) · [glossary](https://github.com/gastownhall/gastown/blob/main/docs/glossary.md) · GitHub API (live stats, 2026-06-12).

## Recommendation (for the operator's positioning call — see SPIKE-53)

1. **Headline the three verified edges** (trace enforcement · lifecycle+history · requirements category), anchored on the **incentive** argument (Beads' author abdicates the space) — not the capability argument.
2. **Correct the moat line**: drop "typed graph is unique"; keep "plain-git substrate" but precisely.
3. **Build the one-way Beads→AIDA import bridge** as a distribution Trojan horse — file as a STORY when the bug backlog clears.
4. **Treat distribution, not features, as the real gap.**
