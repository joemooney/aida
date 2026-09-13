# Entry "memory lane": AIDA under the hood, no machinery

- **Date:** 2026-09-12
- **Specs:** EPIC-<tbd> (entry memory lane) + 5 children
- **Status:** Approved (shape signed off by operator 2026-09-12)
- **Complexity:** Medium — packaging + reflex scaffolding + positioning over existing primitives; NOT engine work.

## Approach

Ship a supported way to use AIDA as an invisible requirements-store / memory /
notepad that a normal Claude Code or Codex chat consults and updates on its own,
with the user knowing nothing about AIDA (like codegraph under the hood).

The store, the agent query surface (`list`/`search`/`show`/`add`/`edit`/`comment`/
`graph`/`why`/`history`), trace comments, the `(SPEC-ID)` trailer, MCP tools, and a
`--footprint minimal` init already exist. This is ~80% built. The remaining work is
a curated init profile, the agent *reflex* scaffolding, a cheap default query path,
a light hygiene floor, and positioning.

```
Existing primitives (store, query CLI, trace, trailer, MCP)
        │
        ├── NEW: lane init profile (store + reflex skills, NO machinery)
        ├── NEW: reflex scaffolding (CLAUDE.md/AGENTS.md block + 2 slim skills)
        ├── ensure cheap default query path (CLI/TOON, not MCP)
        ├── hygiene floor (capture-as-you-go + staleness signal)
        └── positioning + discoverable-but-quiet footprint
```

**Explicitly out of scope:** queue, drain, orchestrator, roles, multi-agent, and
the codegraph → requirements-graph auto-population (deferred; see Followups).

## Decisions (signed off 2026-09-12)

- **A named footprint tier**, not a `--lane` flag — footprint is already the axis
  (full/minimal); add the memory lane as a third tier.
- **Inject the reflex block into an existing CLAUDE.md/AGENTS.md**, marker-guarded
  (same edit-preserving mechanism as the AGENTS.md injection work), rather than
  only writing our own.
- **CLI-only default; MCP behind a flag.** The invisible-memory reflex must use the
  token-cheap CLI/TOON surface (MCP ~2× cost); MCP stays opt-in.

## The one hard problem

Everything hinges on the agent's *reflex* to (a) read relevant specs before acting
and (b) record decisions/observations as it goes — unprompted. The lane succeeds or
fails on scaffolding that makes that reflex automatic and cheap. This is where the
real work and the real risk are.

## Files / work items (build order)

1. **Lane init profile** — a third footprint tier that installs store + config +
   cache + gitignore + the two reflex skills + the reflex CLAUDE.md/AGENTS.md block,
   and nothing queue/drain/orchestrator/role-related. (`init_cmd.rs`,
   `scaffolding/*`, the footprint enum.)
2. **Reflex scaffolding (core)** — a lane CLAUDE.md/AGENTS.md section + a slim
   query-first + capture-as-you-go skill pair, CLI/TOON-pointed, zero machinery
   vocabulary. Reuse `aida-req`/`aida-search`/`aida-capture` stripped down.
   (`aida-core/templates/skills/*`, the scaffolded discipline block.)
3. **Cheap default query path** — confirm/enforce the reflex uses
   `AIDA_AGENT_OUTPUT=toon` over the CLI; MCP not required.
4. **Hygiene floor** — capture-as-you-go + a light staleness signal (e.g. surface
   last-touched age in the reflex query) so authored memory doesn't silently rot.
5. **Positioning + discoverability** — one doc naming this as the entry lane; make
   the `.aida/` footprint discoverable-but-quiet.

## Critical files

- `aida-cli-lib/src/init_cmd.rs` + `aida-core/src/scaffolding/` — footprint tier.
- `aida-core/templates/` (skills + CLAUDE.md/AGENTS.md blocks) — reflex scaffolding.

## Reusable helpers

- Existing footprint scaffolding (`--footprint minimal` path) — extend, don't rebuild.
- The marker-guarded AGENTS.md/CLAUDE.md injection + refresh mechanism.
- `aida-req` / `aida-search` / `aida-capture` skills — strip to lane vocabulary.

## Risks & gotchas

- **Reflex doesn't fire for a cold agent** (biggest) — only provable on a *fresh*
  repo, never this one.
- **Authored-memory rot** — unlike codegraph (derived, self-fresh), this needs
  capture + staleness or it misleads.
- **Footprint surprise** — keep `.aida/` discoverable-but-quiet.

## Tests

- Unit: the new footprint tier scaffolds exactly the lane fileset (store + reflex
  skills + block; NO machinery skills/hooks/roles).
- Scaffold-injection test: reflex block lands in an existing CLAUDE.md/AGENTS.md,
  marker-guarded, edit-preserving.

## Verification (the real test)

A fresh throwaway repo, `aida init` in lane mode, then a *plain* chat where the
agent is told nothing about AIDA — confirm it spontaneously queries before acting
and records as it goes. That is the pass condition; nothing else proves "invisible."

## Followups

- Codegraph → requirements-graph auto-population as an interactive hygiene session
  (deferred, longer-term).

## Related

- STORY-830 (`--footprint minimal`), the AGENTS.md injection work, the agent-surface
  benchmark (CLI vs MCP token cost), `docs/positioning/`.
