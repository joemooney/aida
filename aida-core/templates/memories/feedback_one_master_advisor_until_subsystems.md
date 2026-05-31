---
name: feedback_one_master_advisor_until_subsystems
description: "Until subsystem-scoped advisors exist (SPIKE-10 territory), there is exactly ONE master advisor (the live advisor-role session). Sibling agents and implementers contribute substrate freely BUT must seek permission before merging architecture-impacting changes. As subsystems emerge, master delegates ownership; until then, the master's gate is the project's gate."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
A multi-agent AIDA project (sibling Claude + Codex + Cursor + …) is dogfood for both the substrate AND the coordination model. As more agents participate, two governance questions surface:

1. **What can each agent decide autonomously?**
2. **What requires the master advisor's permission before merging?**

The current model: **one master advisor (the live advisor-role session); permission required for architecture-impacting changes; subsystem-level delegation arrives later (SPIKE-10's multi-advisor coordination work).**

## What sibling agents (implementer Codex, reviewer Claude, …) decide autonomously

- **Capture observations.** File specs, findings, comments, plan files. Always autonomous. *(See [[feedback_capture_over_concentration]].)*
- **Implement against approved acceptance.** When a spec's acceptance criteria are clear and the implementation path is bounded, ship without permission. Punt if uncertain.
- **Add tests.** New test files, test infrastructure improvements, regression fixtures. Autonomous.
- **Bug fixes** that don't change file formats, schemas, tool contracts, or how OTHER agents/subsystems interact. Autonomous.
- **Refinements to acceptance criteria** on specs they own (per [[feedback_refinements_must_be_acceptance_criteria]]).
- **Documentation contributions** that don't change documented architecture or contracts.

## What requires master-advisor permission before merging

- **File formats / on-disk schemas** — `.aida/sessions/*.toml`, `.aida-store/objects/*.yaml`, `.aida/punts.jsonl`, etc. Changing the shape of a file breaks every other agent.
- **MCP tool contracts** — adding/removing/renaming tools, changing input/output schemas, changing the response envelope. Affects all MCP consumers.
- **Orchestrator behavior** — phase semantics, lease management, drain-state model. Affects every drain.
- **EPIC-shaped work** — anything bigger than a single STORY's natural scope. EPICs by definition span subsystems.
- **Convention changes** — commit message format, trace-comment format, role taxonomy, lifecycle vocabulary. Cross-cutting.
- **The master memory pack / discipline docs** themselves — those define HOW agents should coordinate; changes need cross-agent buy-in.

## Why the asymmetry

A sibling agent (Codex) attaching to AIDA only sees a slice of the project's history + intent. The master advisor (the live advisor-role session) holds the long-term context, the cross-spec relationships, the strategic positioning that hasn't yet hit substrate. **Architecture-impacting changes propagate to every future drain + every future agent**; getting them wrong has compounding cost. The master's veto isn't authority; it's continuity.

## What changes when subsystems exist (SPIKE-10 future)

Per SPIKE-10's strategic doc + STORY-362/363/364/365 implementation arc:

- Memory pack gains `subsystem:` tagging
- Advisor sessions can `--focus <subsystem>` to load only relevant context
- Master advisor delegates ownership of subsystems (orchestrator, MCP server, TUI, CLI, web-dashboard) to subsystem advisors
- Each subsystem advisor becomes the "master" for that scope; the project's master coordinates strategic decisions across subsystems

Until that lands, ALL architecture decisions route through the single master.

## How to operationalize this with sibling agents

When briefing a sibling (Codex, Cursor, etc.) on an AIDA project, include:

> **Architecture-impacting changes require master-advisor sign-off before merging.** That includes: file format changes, MCP tool contract changes, orchestrator behavior, EPIC-shaped work, convention changes, memory pack / discipline doc changes. Capture observations and ship bounded acceptance-criteria-driven specs autonomously; **flag architecture proposals before opening a PR** rather than at review time.

Pre-PR-flag pattern: post a finding via `file_finding` (or comment on the spec) saying *"proposing architecture change: <one-line>. Sketch: <link to plan file>. Awaiting master sign-off."* Master responds with approve / revise / decline. Then the PR opens.

## Composes with

- [[feedback_advocate_not_be_passive]] — the master ADVOCATES for the project's strategic interests; that advocacy includes saying "no, not yet" or "yes but with these constraints" on architecture proposals.
- [[feedback_self_test_via_dogfood_merge]] — multi-agent dogfood exposes coordination + conflict patterns. Each merge is also a coordination test.
- [[feedback_pushback_on_overengineering]] — sibling agents may propose EPIC-shaped solutions; master pushes back to smallest-valuable-slice + revisit triggers.
- SPIKE-10 + STORY-362/363/364/365 — the future state where this asymmetry decomposes into subsystem-scoped masters.
- 2026-05-22 user direction: *"there is only one master advisor unless we create subsystems and delegate responsibility ... So with one master permission must be sought before merging changes that impact the system architecture."*
