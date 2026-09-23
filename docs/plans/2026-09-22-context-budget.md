# Context baseline reduction — 2026-09-22

Claude Code version: `2.1.280`. Measurements use a fresh `claude -p` session,
the same account/configuration, and the JSON usage object's input plus cache
creation/read tokens.

## Before and after

| Surface | Before | After |
|---|---:|---:|
| Repository `CLAUDE.md` | 62,730 bytes | 2,572 bytes |
| Repository memory index | 20,722 bytes | 2,870 bytes |
| Scaffold skill descriptions | 15,325 bytes | 5,711 bytes |
| Scaffold command descriptions | 7,366 bytes | 1,062 bytes |
| Generated always-loaded scaffold proxy | not previously gated | 15,461 bytes |
| Fresh AIDA repository first call | 60k minimum / ~80k typical (task baseline) | 27,788 input + cache tokens |
| Fresh `aida init --agent claude` scratch first call | ~45k (task baseline) | 31,145 input + cache tokens |

The fresh repository call is below the 45k acceptance ceiling. Against the
task's ~22k harness floor, the fresh scaffold adds about 9.1k tokens and meets
the floor-plus-10k target. The generated proxy is about 3.9k tokens at four
bytes/token and provides a deterministic CI signal alongside the live measure.

## Moved-content map

No repository guidance was deleted. The complete former `CLAUDE.md` moved
byte-for-byte to `docs/agents/aida-repository-guide.md`; the new root file is a
short routing index.

| Former always-loaded topic | Destination |
|---|---|
| Product, architecture, storage | `OVERVIEW.md`; full preserved guide |
| Initialization, demo, daily commands | preserved guide; `docs/cli/` |
| Discipline and lifecycle detail | `.aida/discipline/README.md` |
| Skills and MCP catalogue | on-demand skill bodies; MCP install matrix |
| Template internals and CLI reference | preserved guide |
| Connector-disable guidance | `docs/agents/context-economy.md` |

Memory bodies were not changed. Their oversized index was replaced with grouped
filename/pattern pointers, preserving on-demand discoverability without loading
every lesson summary on each call.

## Guard

`aida-core/templates/context-budget.toml` owns the limits. The
`context_budget` integration test generates a default scaffold and fails when
the root guide, generated imports, or skill/command descriptions exceed them;
it also verifies headless-only prompts stay out of the interactive catalogue.

<!-- trace:TASK-1441 | ai:codex -->
