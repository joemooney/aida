# Codex MCP Roundtrip Verdict

Status: viable with conditions.

Date: 2026-05-22.

Story: STORY-398.

## Verdict

Codex can use AIDA's MCP server as a real coordination surface for local, single-machine work. The substrate is mature enough for empirical dogfooding and sibling-agent coordination, provided clients treat `tools/list` as canonical and consume the current text-envelope response shape.

This is not yet a fully mature schema-native MCP API. It is an alpha-quality but operational local transport over AIDA's canonical filesystem/git substrate.

## What "Roundtrip" Means Here

A roundtrip is cross-surface agreement:

1. Codex writes or reads through MCP.
2. A different surface reads or verifies the same state, usually the `aida` CLI or another MCP list/read tool.
3. The two surfaces agree on the result.

The purpose is to prove AIDA's substrate is shared across agents. A successful roundtrip means Codex is not writing to a private shadow state; it is operating on the same spec graph, punt ledger, finding queue, lease files, and directive channel as the CLI, Claude Code, and other agents.

## Evidence

The following commands were run locally from the AIDA project root on 2026-05-22.

Codex MCP registration exists:

```text
$ codex mcp list
Name  Command  Args       Env  Cwd  Status   Auth
aida  aida     mcp-serve  -    -    enabled  Unsupported
```

The black-box stdio suite passes when it follows the canonical `tools/list` contract:

```text
$ tests/test_mcp_stdio.sh --skip-agent-contract
TEST initialize ... ok
TEST tools/list descriptors ... ok
TEST CLI-created spec visible through MCP ... ok
TEST MCP-created spec visible through CLI ... ok
TEST spec graph round trips ... ok
TEST coordination tools round trips ... ok
TEST findings round trip ... ok
PASS MCP stdio compatibility suite
```

This covers:

- Codex-style JSON-RPC stdio startup through `initialize`.
- Discovery of the 21 MCP tools through `tools/list`.
- `inputSchema` and `outputSchema` presence.
- CLI-created spec visible through MCP.
- MCP-created spec visible through CLI.
- Spec graph write/read roundtrip through comments, search, and status update.
- Punt channel write/read/resolve roundtrip.
- Task claim lease create/list/release roundtrip.
- Worker directive post/list/ack roundtrip.
- Findings file/list/triage roundtrip.

The separate doc consistency gate also passes:

```text
$ tests/test_mcp_doc_consistency.sh
TEST parse docs/agents/cross-agent-onboarding.md ... ok (21 tools mentioned)
TEST start aida mcp-serve in scratch project ... ok
TEST tools/list ... ok (21 tools advertised)
TEST doc-vs-MCP consistency ... ok
PASS doc-vs-MCP consistency
```

Strict Path B structured-output mode fails as expected:

```text
$ tests/test_mcp_stdio.sh --skip-agent-contract --require-structured-content
TEST initialize ... ok
TEST tools/list descriptors ... ok
TEST CLI-created spec visible through MCP ...
FAIL show_requirement missing structuredContent in strict mode
```

That failure confirms the current implementation is descriptor-level Path A: `outputSchema` exists, but tool results still use the text `content` envelope.

## Maturity Assessment

Current stage: alpha, operational with documented constraints.

What works:

- Local stdio MCP server startup through `aida mcp-serve`.
- 21 MCP tools advertised.
- `inputSchema` and descriptor-level `outputSchema` on the tool descriptors.
- Text-envelope tool results.
- CLI to MCP read consistency.
- MCP to CLI write consistency after BUG-310.
- Punt response atomic writes after TASK-439.
- Review tag canonicalization after TASK-437.
- Directive verb validation after TASK-436.
- Headless AskUserQuestion structural disablement after BUG-327.
- Black-box MCP stdio test infrastructure after TASK-451.
- Doc-vs-MCP consistency gate after TASK-452.

Known constraints:

- `structuredContent` is not emitted. STORY-399 tracks this.
- Error bodies are text-first, not structured machine-readable error objects. STORY-401 tracks this.
- `claim_task` still has a TOCTOU race under concurrent same-spec claims. TASK-438 tracks this.
- Original spec-graph tools work but are thinner than the newer coordination cluster. STORY-82 and EPIC-27 track modernization.
- Cross-machine MCP transport and auth are not implemented.
- Codex registration is manual today; `aida init` does not yet run `codex mcp add`.

## Expectations for Codex

Codex should use AIDA MCP as the primary substrate surface when a matching tool exists. This means using MCP to inspect specs, file findings, punt ambiguous specs, claim tasks, and post directives instead of bypassing through ad hoc files.

Codex should still use shell commands for normal development work: build, test, git, local inspection, and cross-surface verification. The valuable empirical pattern is MCP write followed by CLI read, or CLI write followed by MCP read.

Codex should not assume response bodies are schema-native yet. The safe parser strategy is:

- Trust `tools/list` for tool names and argument names.
- Expect text-envelope `content`.
- Treat `outputSchema` as descriptor metadata, not proof that `structuredContent` will be present.
- Parse success and error text defensively.
- File findings for any doc/tool/schema mismatch.

## Governance Boundary

Multi-agent dogfooding is useful and should continue. It has already surfaced real substrate issues.

The current project governance is single-master-advisor mode. Codex can autonomously:

- File specs, findings, and comments.
- Add tests and docs.
- Implement bounded acceptance-criteria work.
- Produce plan files and observations.
- Punt when a design fork is not safely resolvable.

Codex should seek master sign-off before opening PRs that change architecture:

- File formats or on-disk schemas.
- MCP tool names, schemas, or response envelopes.
- Orchestrator phase semantics, lease model, or drain-state behavior.
- Cross-cutting conventions such as trace format, lifecycle vocabulary, role taxonomy, or memory/discipline docs.
- EPIC-shaped subsystem changes.

For architecture proposals, file a finding or spec comment first and wait for approve/revise/decline before implementation.

## Conclusion

STORY-398 validates AIDA's core agent-agnostic claim for local MCP: Codex can operate against the same AIDA substrate as the CLI and Claude Code. The right headline is:

`Operational for local Codex dogfooding, viable with conditions, not yet schema-native production MCP.`

The next practical moves are:

- Keep `tests/test_mcp_stdio.sh --skip-agent-contract` and `tests/test_mcp_doc_consistency.sh` green.
- Decide whether the stdio suite's duplicated agent-contract alias gate should be removed or regenerated from the doc consistency test, because the canonical doc consistency gate already passes.
- Implement TASK-438 before stress-testing concurrent claims.
- Implement STORY-399 when a schema-native MCP client requires `structuredContent`.
- Keep architectural changes behind master-advisor sign-off.

trace:STORY-398 | ai:codex
