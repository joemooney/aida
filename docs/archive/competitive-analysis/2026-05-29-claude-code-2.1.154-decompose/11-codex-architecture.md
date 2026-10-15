# Codex architecture: CLI substrate, AGENTS.md scaffolding, and AIDA integration

Date: 2026-06-04

Spec: SPIKE-24

## Sources

- Local observation: `codex --version` reported `codex-cli 0.135.0`.
- Local observation: `codex --help`, `codex mcp --help`, `codex mcp list`.
- OpenAI Codex product page: <https://openai.com/codex/>
- OpenAI Codex CLI docs: <https://developers.openai.com/codex/cli>
- OpenAI AGENTS.md guide: <https://developers.openai.com/codex/guides/agents-md>
- OpenAI Codex MCP docs: <https://developers.openai.com/codex/mcp>
- OpenAI Codex subagents docs: <https://developers.openai.com/codex/subagents>
- OpenAI Codex GitHub repository: <https://github.com/openai/codex>
- AIDA local docs: `AGENTS.md`, `docs/agents/codex-mcp-setup.md`,
  `docs/agents/codex-brief-pickup.md`,
  `aida-core/templates/docs/agents/codex-mcp-setup.md`.

## Executive verdict

Codex's substrate model is best described as a **local terminal agent with
layered project instructions, configurable execution policy, persisted
conversation sessions, MCP extension, and optional app/cloud/subagent surfaces**.
It is not, by itself, a durable project-management substrate. It can resume and
fork conversations, load repository guidance from `AGENTS.md`, invoke external
MCP tools, run non-interactive jobs, and bridge to cloud/app workflows. It does
not natively provide AIDA's stable spec graph, leases, routed briefs, queue,
audit history, or cross-agent status model.

AIDA's Codex integration is therefore correctly shaped as a **bilingual
scaffolding + MCP substrate**:

- `AGENTS.md` gives Codex the same project discipline role that `CLAUDE.md`
  gives Claude Code.
- `docs/agents/codex-mcp-setup.md` and
  `docs/agents/codex-brief-pickup.md` teach Codex how to attach to AIDA's MCP
  server and pick up work.
- `aida agent new codex` wraps Codex launch with leases, worktrees, agent
  registry entries, and context snapshots.
- AIDA MCP exposes the durable coordination surface that Codex lacks natively.

The main gap is not "Codex cannot work in AIDA." It can. The gap is that AIDA
still relies on a manually maintained Codex-facing instruction layer and a
text-envelope MCP runtime. The integration should harden around automatic
scaffold freshness, MCP smoke tests, and clearer separation between Codex's
conversation state and AIDA's project state.

## Codex CLI surface observed locally

`codex --help` in this repository exposes these relevant clusters:

- Interactive terminal agent: bare `codex [PROMPT]`.
- Non-interactive automation: `codex exec`, including JSON output and final
  message file options.
- Review agent: `codex review`.
- Session continuation: `codex resume` and `codex fork`.
- MCP client management: `codex mcp list|get|add|remove|login|logout`.
- Codex-as-server: `codex mcp-server`.
- App/cloud bridge: `app-server`, `remote-control`, `cloud`, and `apply`.
- Local health and configuration: `doctor`, `features`, `plugin`, `sandbox`,
  `debug`, `completion`, `update`.

The command-level architecture is significant for AIDA because it shows Codex is
not just a one-shot code generator. It has enough local substrate to be launched,
resumed, reviewed, extended through MCP, and supervised. AIDA should use those
native primitives rather than emulate them.

## Execution policy and trust model

Local `codex --help` exposes:

- Config layering through `~/.codex/config.toml`, profiles, and ad-hoc
  `-c key=value` overrides.
- Workspace selection through `--cd` and extra writable roots through
  `--add-dir`.
- Sandbox modes: `read-only`, `workspace-write`, and `danger-full-access`.
- Approval policies: `untrusted`, `on-request`, `never`, plus deprecated
  `on-failure`.
- A dangerous bypass flag that disables both approvals and sandboxing.
- Optional web search and image inputs.

AIDA already maps this reasonably: `aida agent new codex` keeps the dangerous
autonomous mode explicit through `--bypass-sandbox`, rather than making it the
default. That is the right default for a substrate that expects multiple agents
to work in adjacent branches.

## AGENTS.md as Codex's repo-native instruction layer

OpenAI's AGENTS.md guide describes a layered discovery model: Codex reads global
guidance under `CODEX_HOME`/`~/.codex`, then project guidance from the repository
root down to the current directory, with override files taking precedence at a
given level. The docs also describe fallback filenames and a default combined
instruction size limit.

AIDA's `AGENTS.md` is therefore not a cosmetic compatibility file. It is the
Codex-native entrance to AIDA's discipline:

- Work through AIDA specs and briefs.
- Use sibling worktrees.
- Prefer MCP for spec/coordination operations.
- Preserve spec IDs in trace comments and commit trailers.
- Sketch architecture-class changes before implementation.
- Treat `aida pr ship` as the direct-ship path only when the brief permits it.

This is "bilingual scaffolding": `CLAUDE.md` remains the broader Claude Code
repository guide, while `AGENTS.md` gives Codex and other AGENTS.md-compatible
clients a native instruction file with the same project invariants. The two
files should not be mechanically identical. `AGENTS.md` should stay phrased as
Codex instructions-to-self and link to shared docs for cross-agent semantics.

## Native multi-agent surface

OpenAI's Codex product page now presents Codex as a multi-surface coding
partner: app, terminal, IDE, GitHub/cloud, and parallel task workflows. The
subagents docs say Codex can spawn specialized agents, collect their results,
and expose thread management in the CLI/app. The local CLI also exposes `fork`,
`resume`, cloud browsing/apply, and app-server/remote-control commands.

Those are real multi-agent primitives, but they are mostly **conversation and
execution primitives**, not **project coordination primitives**:

- Codex can fork or resume a conversation. AIDA can say which spec owns the
  work, who holds the lease, and whether the spec reached Completed.
- Codex can spawn subagents. AIDA can coordinate sibling vendor agents and
  surface active agents in `aida status`.
- Codex can apply a cloud task diff. AIDA can attach the result to a stable spec
  ID and route findings/punts.
- Codex can load AGENTS.md. AIDA can keep the cross-agent rules durable and
  project-specific.

The integration stance should be additive: let Codex handle local agent
execution and conversation continuity; let AIDA handle durable project state.

## AIDA Codex integration gap analysis

| Area | Current state | Gap | Recommendation |
| --- | --- | --- | --- |
| Instruction scaffolding | `AGENTS.md` and Codex setup docs exist and are propagated by AIDA templates. | Drift risk between `CLAUDE.md`, `AGENTS.md`, and generated templates. | Keep `AGENTS.md` short and durable; move volatile workflows into `docs/agents/*`; add doc-consistency checks for Codex-specific setup claims. |
| MCP attachment | `codex mcp list` shows `aida` configured as `aida mcp-serve`; local docs explain setup. | This session's MCP tool transport was unavailable, requiring CLI fallback for briefs. | Preserve CLI fallback docs, but add a lightweight "Codex MCP health" check that verifies tools/list against AIDA's expected tools. |
| Tool response shape | AIDA MCP uses descriptor-level schemas and text-envelope responses. | Codex can consume the tools, but strict structured clients cannot rely on `structuredContent`. | Keep text parsing defensive until STORY-399; document this explicitly in Codex setup and competitive-analysis notes. |
| Launch supervision | `aida agent new codex` registers process metadata and context snapshots. | Codex native session ids and AIDA registry ids are separate. | Record Codex session ids when launch output exposes them; make `aida status` link agent registry entries to AIDA leases and Codex session metadata when available. |
| Brief pickup | AIDA has CLI and MCP brief list/read/ack. | Brief pickup can still fail when MCP transport is closed. | Treat MCP brief pickup as preferred, CLI as supported fallback; keep both paths in `AGENTS.md`. |
| Cross-agent memory | Codex has persisted conversations, local history, and optionally memories/plugins/skills. | Native persistence is session/user-local, not a shared project graph. | Do not rely on Codex memory for project truth. Use AIDA comments, specs, findings, and punts for shared durable knowledge. |
| Multi-agent workflows | Codex has native subagents. | Native subagents do not automatically become AIDA-visible agents/spec owners. | If Codex subagents work on AIDA specs, route them through AIDA-visible launch/brief/lease conventions or explicitly mark them as ephemeral exploration. |

## Named substrate model

Use this name for future docs:

**Codex local-agent substrate over AIDA durable-coordination substrate.**

Codex owns:

- local execution,
- sandbox/approval policy,
- interactive and non-interactive sessions,
- conversation resume/fork,
- MCP client configuration,
- optional subagents/cloud/app surfaces.

AIDA owns:

- stable spec IDs,
- git-canonical requirement graph,
- leases/worktrees,
- briefs,
- punts/findings/comments,
- queue/orchestrator,
- cross-agent status,
- commit/PR traceability.

That split is coherent. AIDA should not try to become Codex's session manager,
and Codex should not be asked to infer AIDA's project state from conversation
history.

## Concrete next steps

1. Add a Codex MCP health command or doc-consistency test that runs
   `codex mcp list`, starts `aida mcp-serve`, and verifies AIDA tools/list from
   the Codex-facing configuration.
2. Keep `AGENTS.md` as the universal Codex entry point, but avoid stuffing every
   historical pitfall into it. Link to `docs/agents/session-communication.md`
   and Codex-specific setup docs for details.
3. Update template docs whenever live docs change. The live
   `docs/agents/codex-mcp-setup.md` currently says 26 AIDA MCP tools; the
   template copy observed during this spike still said 25 in one section. That
   is exactly the drift class AIDA should gate.
4. Consider a Codex plugin or packaged profile only after the AGENTS.md + MCP
   route is stable. The current repo-native setup is simpler and easier to
   audit.
5. Treat native Codex subagents as useful internal parallelism, but not as
   AIDA-visible sibling agents unless they are launched or registered through
   AIDA.
