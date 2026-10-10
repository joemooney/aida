# Codex MCP and tool model: protocol parity with Claude Code and AIDA

Date: 2026-06-04

Spec: SPIKE-25

## Sources

- Local observation: `codex --version` reported `codex-cli 0.135.0`.
- Local observation: `codex mcp --help` and `codex mcp list`.
- Local source check: `aida-cli/src/mcp.rs`, `tests/test_mcp_stdio.py`,
  `docs/agents/codex-mcp-setup.md`,
  `docs/agents/codex-mcp-roundtrip-verdict.md`,
  `docs/agents/cross-agent-onboarding.md`.
- OpenAI Codex MCP docs: <https://developers.openai.com/codex/mcp>
- OpenAI Docs MCP guide: <https://developers.openai.com/learn/docs-mcp>
- OpenAI Codex CLI docs: <https://developers.openai.com/codex/cli>
- OpenAI AGENTS.md guide: <https://developers.openai.com/codex/guides/agents-md>

## Executive verdict

Codex is MCP-native enough for AIDA's current integration model. It manages MCP
servers through `codex mcp`, stores configuration in Codex config files, supports
local stdio servers and streamable HTTP servers, and shares MCP configuration
between the CLI and IDE extension. AIDA's `aida mcp-serve` fits the local stdio
case cleanly.

The parity gap is not protocol discovery. Discovery is fine. The gap is runtime
shape and operational robustness:

- AIDA currently advertises schemas but returns text-envelope tool results, not
  `structuredContent`.
- Codex can list the configured AIDA server, but this session's direct MCP tool
  transport was closed, so brief pickup had to fall back to the AIDA CLI.
- AIDA's live docs and template docs can drift on tool count; source and
  tools/list must remain canonical.

## Codex MCP model observed locally

`codex mcp --help` exposes management commands:

- `list`
- `get`
- `add`
- `remove`
- `login`
- `logout`

The local AIDA configuration is:

```text
Name  Command  Args       Env  Cwd  Status   Auth
aida  aida     mcp-serve  -    -    enabled  Unsupported
```

For AIDA, `Auth: Unsupported` is expected. The server is a local stdio process,
not an authenticated remote HTTP service.

The OpenAI Codex MCP docs describe two relevant transport families:

- Local stdio servers started by command.
- Streamable HTTP servers, with bearer-token and OAuth authentication options.

AIDA is currently in the first bucket. That is the right default for
single-machine development because it avoids a long-running HTTP service,
credential distribution, and cross-machine trust questions.

## AIDA MCP surface verified from source

The current `aida-cli/src/mcp.rs` tools/list descriptors advertise 29 tools:

- Spec graph: `list_requirements`, `show_requirement`, `add_requirement`,
  `update_requirement`, `search_requirements`, `add_comment`,
  `add_relationship`, `query_graph`, `list_features`, `history`.
- Mailbox: `send_message`, `read_inbox`.
- Punt channel: `list_punts`, `read_punt`, `post_punt`, `resolve_punt`,
  `escalate_punt`.
- Findings: `list_findings`, `file_finding`, `triage_finding`.
- Task claims: `claim_task`, `release_task`, `list_active_leases`.
- Worker directives: `post_directive`, `list_directives`, `ack_directive`.
- Briefs: `list_briefs`, `read_brief`, `ack_brief`.

This aligns with `docs/agents/cross-agent-onboarding.md` and the live
`docs/agents/codex-mcp-setup.md`. One template copy observed during this spike
still said 25 tools in a section; that is a scaffold drift finding, not a
protocol limitation.

## Protocol parity with Claude Code

At the AIDA layer, Codex and Claude Code should be treated as equivalent MCP
clients:

- Both discover tools from MCP `tools/list`.
- Both should trust tool descriptors over stale docs.
- Both receive AIDA's current text-envelope responses.
- Both need a fallback path when a long-running MCP server is stale or closed.

The differences are client-specific launch and instruction conventions:

| Area | Codex | Claude Code | AIDA implication |
| --- | --- | --- | --- |
| Project instructions | `AGENTS.md` hierarchy and `CODEX_HOME` config. | `CLAUDE.md`, skills, hooks, and Claude-specific slash commands. | Keep `AGENTS.md` and `CLAUDE.md` bilingual but link to shared docs for invariant rules. |
| MCP setup | `codex mcp add aida -- aida mcp-serve`. | Claude-specific MCP registration. | AIDA docs need per-client setup snippets, not one universal command. |
| Tool response handling | Codex session receives MCP text envelopes today. | Claude Code also consumes text envelopes today. | AIDA can defer `structuredContent` if docs warn consumers to parse defensively. |
| Approval/headless semantics | Codex uses sandbox and approval policy flags. | Claude Code has hook/permission semantics such as `ask`, `defer`, and `continue: false`. | AIDA must not project Claude hook behavior onto Codex. Keep session-communication docs client-specific. |
| Persistent work state | Conversation resume/fork, local config/history. | Claude session/transcript and hooks. | AIDA specs/leases remain the project source of truth for both. |

## Tool registration and schema shape

AIDA's MCP server follows the useful half of structured tooling:

- Every tool has an `inputSchema`.
- Every tool has descriptor-level `outputSchema`.
- Runtime output remains text content.

That is enough for human-supervised Codex work and for the current MCP brief
pickup/read/write loop. It is weaker for programmatic clients because a caller
cannot validate `structuredContent` yet. The operational rule should be:

> Treat `tools/list` as canonical for names and input arguments, but treat
> runtime bodies as human-readable text until STORY-399 changes the response
> shape.

## Environment and authentication conventions

Codex MCP configuration can be global under `~/.codex/config.toml` or scoped to
a trusted project config. The CLI supports environment variables for stdio
servers and authentication flows for HTTP/OAuth servers.

AIDA's current local setup has no MCP auth layer. That is acceptable for the
repo-local stdio case. It becomes insufficient for cross-machine or remote
agent scenarios. The moment AIDA offers remote MCP, it needs:

- explicit project identity,
- bearer/OAuth or another trust boundary,
- path/worktree scoping,
- audit logging for remote write tools,
- clear separation between read-only and write-capable tool clusters.

## AIDA Codex-MCP scaffolding recommendations

1. Keep `codex mcp add aida -- aida mcp-serve` as the primary local setup path.
2. Add a Codex-facing health check that verifies:
   - `codex mcp list` contains enabled `aida`;
   - `aida mcp-serve` tools/list has the expected 29 tools;
   - the brief trio `list_briefs`, `read_brief`, `ack_brief` is present;
   - the write tools `add_requirement`, `add_comment`, `add_relationship`,
     `update_requirement`, `post_punt`, `file_finding`, `claim_task`, and
     `post_directive` are present.
3. Gate template drift: the generated
   `aida-core/templates/docs/agents/codex-mcp-setup.md` should not disagree
   with live docs about tool count or canonical tool names.
4. Keep CLI fallback documented. During this spike, the MCP tool transport in
   the Codex session was closed, but the AIDA CLI brief path worked. That is a
   real reliability requirement, not just a convenience.
5. Do not overfit Codex docs to Claude Code hook semantics. Link to
   `docs/agents/session-communication.md` for client-specific pause/abort
   behavior.
6. For future remote MCP, split docs and possibly tools into read-only and
   write-capable operational profiles. AIDA's current local stdio setup assumes
   same-user trust.

## Bottom line

Codex MCP parity with AIDA is good enough for local, single-machine
coordination. The remaining work is not basic MCP support; it is hardening:
structured runtime output, scaffold drift prevention, client health checks, and
remote/auth planning.
