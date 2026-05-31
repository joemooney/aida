# Assumptions I'm making this session — please challenge

**Date filed**: 2026-05-29
**Target reader**: any independent agent (Codex, Antigravity, web Claude)
**What's being requested**: identify which of these assumptions are most likely to be wrong and what evidence would catch it.
**Time budget**: 20–30 minutes. Pick the 3 weakest, ignore the rest.

---

## Context

This is the explicit-assumptions sibling of two other briefs I wrote this session:
- `2026-05-29-spike-32-workflow-compiler-thesis.md` (workflow compiler architecture)
- `2026-05-29-code-review-compose-moves.md` (Code Review delegation strategy)

Those briefs ASK for second opinions on specific proposals. This brief lists the **assumptions underneath those proposals** that I'd rather not defend if I'm wrong about them. Strategic + tactical mixed; ordered roughly by stakes (highest first).

For background: AIDA = git-canonical spec graph + Rust orchestrator that drains a queue of specs across multiple agent CLIs. Today I shipped 4 SPIKEs (30, 31, 33, 34) that compose AIDA with Claude Code's 2.1.154 native surfaces, then 5 more SPIKEs (35-39) from the Code Review / Actions / GitLab surfaces.

---

## Strategic assumptions (stake = years of direction)

### 1. The compose pattern is right

**Assumption:** AIDA's right architectural move is to compose WITH Claude Code's expanding native surfaces (workflows, agent view, code review, /goal) rather than to compete by building parallel substrate.

**What could be wrong:** maybe the compose pattern just makes AIDA a thin wrapper around Anthropic's product, leaking value to them. Maybe a competitive runtime is the only durable moat — let Anthropic add features; AIDA stays sovereign.

**Evidence that would settle it:** a 12-month projection of how much "AIDA-specific" surface remains after each Claude Code release. If the curve trends to zero (every new Anthropic feature absorbs an AIDA primitive), compose loses.

### 2. AIDA's substrate moat is durable

**Assumption:** The four moat dimensions (cross-machine, cross-tool, cross-time, spec-graph-grounded) are strictly more capable than any single-vendor solution, and Anthropic specifically will not absorb them natively.

**What could be wrong:**
- Anthropic could add cross-machine session sync (they're already managing supervisors per-user; per-org is one config flag)
- Anthropic could add Codex/AGY integration via their MCP — though they have business reasons not to
- The "spec graph" might just be a structured CLAUDE.md from a sufficiently advanced model's POV

**Evidence:** check if Claude Code 2.2 or 2.3 ships any of cross-machine sync / cross-tool dispatch / structured-spec primitives.

### 3. Anthropic won't absorb AIDA's spec-graph layer

**Assumption:** Claude Code's auto-memory + skills + CLAUDE.md will NOT converge on structured requirement graphs with stable IDs + lifecycle + relationships within 12 months.

**What could be wrong:** Auto-memory IS already starting to look like a graph (related-notes links, topic clustering). One Anthropic eng-week away from adding `@id:` stable identifiers + `relationships:` frontmatter to CLAUDE.md files.

**Evidence:** the trajectory of `/memory` + auto-memory features. If they add stable IDs in any release, this assumption is broken.

### 4. AIDA users actually care about cross-tool dispatch

**Assumption:** AIDA's Codex / AGY / web-Claude routing matters enough to AIDA users that they'd give up Claude Code-only features to keep it.

**What could be wrong:** maybe in practice every AIDA user is 95% Claude Code with occasional Codex sessions. If so, Claude-Code-native features (everything Anthropic ships) win on usability and AIDA's "we route to anything" pitch is theoretical.

**Evidence:** look at actual agent type distribution in `aida-cli/src/global_queue.rs` lease data. Run `aida usage --json | jq '.agent_type'` if telemetry captures it.

---

## SPIKE-set assumptions (stake = months of work)

### 5. Delegation to managed Code Review is right (vs. native reviewer)

**Assumption:** AIDA should divest reviewer-as-Claude-session in favor of triggering Anthropic's managed Code Review and parsing its severity tally. Spec-grounded behavior survives via REVIEW.md injection.

**What could be wrong:**
- REVIEW.md is a "highest-priority instruction" but Claude doesn't always follow instructions. Substrate-via-prose ≠ substrate-via-code
- Per-PR cost ($15-25) compounds for orgs running 50+ PRs/week — could be $1000+/month vs $0 today
- ZDR holdouts force a permanent two-mode architecture (managed + native fallback)

**Evidence:** ship SPIKE-35 + SPIKE-36 on this repo for 2 weeks; measure (a) whether Code Review actually follows REVIEW.md severity-recalibration, (b) cost trajectory, (c) reliability of severity parse.

### 6. The "saved-script" framing is right for SPIKE-32

**Assumption:** AIDA's "compile spec → workflow.js" thesis lives in the saved-script lane (build artifact checked in alongside spec, replayed deterministically by Claude Code's runtime).

**What could be wrong:** the failure-routing logic that AIDA's orchestrator handles today (shelve on CI red, escalate to advisor on punt, pause on human-ack) might not fit into a static workflow.js. If it doesn't, AIDA either:
- Recompiles workflow.js dynamically on each failure (back to dynamic-generation lane — contradicts the thesis)
- Keeps failure-routing in AIDA's supervisor outside the workflow.js (then what's the point of the compile?)

**Evidence:** prototype emitting workflow.js for one simple spec; see if you can express the punt → advisor → resume cascade in pure workflow.js.

### 7. Gitignoring SPIKE-31's generated rules is right

**Assumption:** `.claude/rules/aida-specs/` is per-clone derived state, gitignored, regenerated on `aida rules sync`.

**What could be wrong:** if a team member doesn't run `aida rules sync`, they don't get the substrate-as-bouncer behavior. The whole point is that the rules ARE the substrate; making them per-clone means the substrate is uneven across the team.

**Evidence:** propose to a teammate "you need to run `aida rules sync` to load spec scope into Claude" — see if they'd actually do it consistently. If not, committed > gitignored.

### 8. REVIEW.md should be per-PR not per-spec

**Assumption:** SPIKE-35 emits a single REVIEW.md (root-level, regenerated on spec changes or PR opens) rather than per-spec REVIEW-SPEC-N.md files (Code Review doesn't support multiple REVIEW.md files anyway).

**What could be wrong:** one REVIEW.md means N specs' rules collide. Each spec's "skip-rule" applies to every other spec's review. The selectivity that path-gated rules give us (SPIKE-31) doesn't exist for REVIEW.md.

**Evidence:** read what Code Review does when REVIEW.md has competing instructions across sections.

---

## Tactical assumptions (stake = hours of work)

### 9. `aida queue work --auto-complete` will recover the 9 stalled batch items

**Assumption:** Re-kicking the drain on batch:scaffolding-2026-05-28 will advance the 9 Done-awaiting-merge items through phase 4 (merge) → 5 (pull) → 6 (build) without intervention.

**What could be wrong:** the 12-hour stall earlier was on phase 2 (CI). Phase 4+ are different code paths. Some of those 9 items may have PRs that never opened (phase 3 never ran). The drain might not be able to recover from arbitrary mid-pipeline state.

**Evidence:** test it. Worst case: drain shelves whatever it can't recover, operator triages via `aida findings list`.

### 10. The `backgrounded · <id>` format SPIKE-34 parses is stable

**Assumption:** `claude --bg` will continue printing `backgrounded · <short-id>` as its first content line for at least the next 6 Claude Code releases.

**What could be wrong:** trivially fragile. Anthropic changes the separator or wording and AIDA silently fails to link sessions to leases. Should be using a structured JSON output channel (does claude --bg support --json?).

**Evidence:** `claude --bg --help | grep -i json` — if there's a structured-output flag, prefer it.

### 11. Five SPIKEs in 3 hours is a healthy ratio

**Assumption:** Filing 5 new SPIKEs from one URL paste-bomb (the Code Review / Actions / GitLab triple) is right-sized capture, not over-eager.

**What could be wrong:** some of these are duplicative or shouldn't exist. E.g.:
- SPIKE-38 (publish a GitHub Action) might be premature — no AIDA users yet
- SPIKE-39 (gh/glab abstraction) is the right SHAPE but maybe should be a STORY not a SPIKE since no design discovery needed
- SPIKE-37 (`@claude review once` trigger) might be a TASK under SPIKE-36 not a peer

**Evidence:** review the SPIKE list in 7 days. If any are still "Approved" with no activity, the filing was premature.

---

## What I'm NOT asking about

- The Round 1 / Round 2 synthesis architecture verdicts — those have their own briefs
- AIDA's existing 100+-spec backlog — those are decided
- Whether AIDA exists as a project (it does; this is meta-level)

---

## Desired reply shape

Pick the 3 weakest assumptions from the list above. For each:

1. **Why it's likely wrong** — 1-2 sentences
2. **What evidence would settle it** — 1 sentence
3. **What I should do instead if it IS wrong** — 1 sentence

Under 400 words total. Markdown.

If none of them are weak enough to flag, say so — that's useful signal too.

---

*Generated by AIDA master advisor session 2026-05-29. The user (Joe Mooney) will hand this to you outside AIDA's substrate.*
