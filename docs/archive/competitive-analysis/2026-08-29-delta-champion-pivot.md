# Market delta — the champion-product pivot (2026-08-29)

**Specs:** EPIC-60 (codex daily-driver parity) · **Status:** dated snapshot (immutable once landed; supersede with a new dated file) · **Baseline:** `2026-07-19` delta + `2026-06-26` market landscape · **Evidence:** one fresh web scan (2026-08-29, sources linked) plus this repo's own code and specs. Web claims are cited; everything else is repo-evidenced.

> Context for this snapshot: the project's governing register changed on 2026-08-29 from
> research-probe to **product intended to win**. This delta is deliberately one page — the
> full teardown of the new entrants below is follow-up work, not this file.

## What changed outside since 2026-07-19

1. **The harness layer is now the declared battleground.** Trade coverage frames the
   competition explicitly: every major lab ships a model portfolio *plus* an agent harness,
   competing on approval modes, resumable work, MCP auth, artifacts, and review surfaces —
   not raw model quality ([Developers Digest, July 2026](https://www.developersdigest.tech/blog/codex-claude-code-july-agent-controls)).
   AIDA's neighbourhood, now crowded from above.

2. **Portability is commoditizing into conventions.** `AGENTS.md`, `SKILL.md`, and MCP are
   being described as a portable stack "any compliant runtime can consume"
   ([MindStudio](https://www.mindstudio.ai/blog/portable-ai-agent-stack-avoid-vendor-lock-in));
   opencode markets model-swap without workflow change. Consequence: **"we are
   cross-vendor" stops being a differentiator** — it becomes table stakes AIDA merely
   *also* has. The June moat table's "RIDE the commoditized transport" verdict now applies
   to the portability *conventions* too.

3. **Org-scope coordination platforms have arrived in the adjacent seat.**
   [Augment Cosmos](https://www.augmentcode.com/tools/best-spec-driven-development-tools)
   (launched May 2026) is positioned as the runtime/memory/coordination layer at
   *organizational* scope where most SDD tools are workspace-scope. And an
   agent-management-tool category now exists that "manages Claude Code and Codex sessions
   from one interface" ([Nimbalyst](https://nimbalyst.com/blog/best-agent-management-tools-2026/)) —
   AIDA's TUI/fleet surface, shipped by others. Depth of both: **[needs-web-verify]** —
   a proper teardown is follow-up.

4. **Vendor-standardization mandates are a live market force.** Organizations are picking
   one coding-agent vendor and excluding others. This converts cross-vendor *durability of
   the record* — surviving a mandated vendor switch with graph, IDs, and traces intact —
   from research finding into **purchase reason**. (Generic market observation; see
   EPIC-60 for the engineering response.)

## What changed inside since 2026-07-19

- **Codex parity moved from readiness to daily-driver grade** (EPIC-60): hooks now
  scaffold for Codex with a build-time parity guard against `settings.json`
  (TASK-1181), and BUG-793 found that AIDA's own scaffolded `.codex/config.toml`
  carried a key that made Codex **silently discard the whole file** — meaning the Codex
  MCP registration had likely never worked on any project. Found, fixed, verified live
  in a real `codex exec` session.
- **BUG-793 is also the positioning story**: the substrate caught a silently-inert
  integration that no vendor surface reported. A checkable record beats a harness
  feature — that is the argument, made by the artifact.
- Two stale papercuts from the July validation run turned out already fixed
  (BUG-706/703); filed-then-rejected with duplicate links, which is the audit trail
  working.

## Consequence for positioning (the one-paragraph version)

The defensible claim is **not** "cross-vendor" (commoditizing) and **not** "spec-driven"
(crowded: Kiro, Spec Kit, OpenSpec, Cosmos). It is the layer none of them own: **a
vendor-neutral, git-canonical decision record with stable IDs, typed relationships, and
code→decision linkage (`aida why <file:line>`) that survives vendor switches, machine
switches, and years.** Cosmos-class platforms offer coordination *inside their walls*;
a mandate-driven vendor switch is precisely when walls become the problem. Build and
sell the record, ride everything else.

## Follow-ups this snapshot creates

- Full teardown of Augment Cosmos + the agent-manager category (where would a buyer
  choose them; what exactly is their record's portability story).
- Extend `aida why` coverage from trace-comments to **git-blame trailer fallback** —
  widen the differentiator from annotated lines to every conventionally-trailered line.
- Refresh `docs/positioning/` lead pair once the teardown lands (do not touch before —
  precise claims only).
