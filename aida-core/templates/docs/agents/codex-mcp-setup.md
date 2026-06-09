# Codex MCP Setup for AIDA

Status: empirical setup from STORY-398, refreshed by STORY-417 on 2026-05-22.

This document records the working local setup for connecting Codex CLI to AIDA's MCP server. It supersedes the Codex-specific placeholder in `docs/agents/cross-agent-onboarding.md`.

## Preconditions

- `aida` is installed and available on `PATH`, or you can substitute an absolute path to the binary.
- The target repository has been initialized with AIDA and contains `.aida/`.
- Codex CLI is installed.
- The Codex session is started from the AIDA project root, or with `codex --cd /path/to/project`, so `aida mcp-serve` can discover the correct project.

Verified local Codex command surface:

```bash
codex mcp add --help
codex mcp list
```

The relevant Codex command shape is:

```bash
codex mcp add <name> -- <command>...
```

## Register AIDA as a Codex MCP Server

From the AIDA project root:

```bash
codex mcp add aida -- aida mcp-serve
```

If `aida` is not on `PATH`, use the absolute binary:

```bash
codex mcp add aida -- /absolute/path/to/aida mcp-serve
```

Check registration:

```bash
codex mcp list
```

Expected shape:

```text
Name  Command  Args       Env  Cwd  Status   Auth
aida  aida     mcp-serve  -    -    enabled  Unsupported
```

`Auth: Unsupported` is expected for this local stdio setup. There is no HTTP server or bearer token in the current local-machine transport.

## Start Codex in the Project

Use one of:

```bash
cd /path/to/aida-project
codex
```

or:

```bash
codex --cd /path/to/aida-project
```

The MCP server is launched by Codex over stdio. You do not need to run `aida mcp-serve` in a separate terminal for the Codex integration. Running it manually is still useful for debugging JSON-RPC framing or the black-box stdio tests.

## Optional AIDA-Aware Status Lines

AIDA is meant to bootstrap other projects, so status-line setup should stay
optional. Some teams want a quiet default; others want the visible AIDA context
right away.

The AIDA-aware renderer is:

```bash
aida statusline --color=always
```

It prints the project, role, active spec/session hint, queue depth, and cache
freshness. Use it wherever your shell, terminal multiplexer, or agent client
can run a command-backed status line.

Claude Code can run that command through
`.claude/settings.json`:

```json
{"statusLine": {"type": "command", "command": "aida statusline --color=always 2>/dev/null || printf '%s' \"$(pwd)\""}}
```

Codex CLI has a footer concept too, but its current `status_line` setting
chooses from Codex built-in item IDs. It does not execute arbitrary commands
like `aida statusline`, so treat Codex's footer as a companion to the AIDA-aware
shell/statusline command rather than as a replacement.

For Codex, use the built-in footer fields that best complement AIDA's session
and worktree model:

```toml
[tui]
status_line = ["model-with-reasoning", "context-remaining", "git-branch", "current-dir"]
```

Put this in `~/.codex/config.toml` for your personal Codex default. For a
trusted AIDA project, the same snippet can live in `.codex/config.toml`.
Continue using `aida statusline` directly in the shell when you need the
AIDA-specific role, queue, cache, and session summary.

trace:TASK-0413 | ai:claude

## Launch Codex Through AIDA

For AIDA projects, prefer the supervised launcher:

```bash
aida agent new codex --role implementer
aida agent new codex --spec STORY-433 --role implementer
```

The launcher runs Codex from the project root, sets `AIDA_AGENT_TYPE=codex`, propagates `AIDA_SESSION_ROLE` and `AIDA_SESSION_SCOPE`, registers the process in `.aida/agents/`, and deregisters it on exit. When `--spec` is supplied, it first creates the standard sibling worktree + lease and launches Codex from that worktree.

By default it also writes a point-in-time launch-context snapshot under
`.aida/agents/context/` and passes the path as `AIDA_AGENT_CONTEXT_FILE`.
The snapshot includes role guidance, active lease/spec details, pending
brief paths with one-line titles, and queue-head hints. Use
`--show-context` to print the generated snapshot before Codex starts, or
`--no-context` to launch without it. The snapshot is not live-updating;
continue polling briefs/MCP for work filed after startup.

Keep the unsafe autonomous flag explicit:

```bash
aida agent new codex --spec STORY-433 --role implementer --bypass-sandbox
```

`--bypass-sandbox` passes Codex's `--dangerously-bypass-approvals-and-sandbox`; it is not the interactive default.

## Verify Tool Discovery

Inside a Codex session with the MCP server connected, the AIDA tools are exposed as MCP tools. In this environment they were available under the `mcp__aida__` namespace.

The expected tool count is 25:

- Spec graph: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`, `history`.
- Punt channel: `list_punts`, `read_punt`, `post_punt`, `resolve_punt`, `escalate_punt`.
- Findings channel: `list_findings`, `file_finding`, `triage_finding`.
- Task claims: `claim_task`, `release_task`, `list_active_leases`.
- Worker directives: `post_directive`, `list_directives`, `ack_directive`.
- Agent briefs: `list_briefs`, `read_brief`, `ack_brief`.

The canonical argument names are the names advertised by MCP `tools/list`. If a document disagrees with `tools/list`, trust `tools/list` and file a doc-drift finding.

## Validate From the Shell

AIDA has a black-box stdio compatibility suite that launches `aida mcp-serve` and speaks JSON-RPC like Codex does:

```bash
tests/test_mcp_stdio.sh --skip-agent-contract
```

Expected result on 2026-05-22:

```text
TEST initialize ... ok
TEST tools/list descriptors ... ok
TEST CLI-created spec visible through MCP ... ok
TEST MCP-created spec visible through CLI ... ok
TEST spec graph round trips ... ok
TEST coordination tools round trips ... ok
TEST findings round trip ... ok
PASS MCP stdio compatibility suite
```

The doc consistency gate should also pass:

```bash
tests/test_mcp_doc_consistency.sh
```

Expected result:

```text
TEST parse docs/agents/cross-agent-onboarding.md ... ok (25 tools mentioned)
TEST start aida mcp-serve in scratch project ... ok
TEST tools/list ... ok (25 tools advertised)
TEST doc-vs-MCP consistency ... ok
PASS doc-vs-MCP consistency
```

## Response Shape

AIDA MCP responses ship both Path A and Path B (STORY-399):

- Tools advertise `inputSchema`.
- Tools advertise `outputSchema` (Path A, TASK-440).
- Successful tool results return MCP text content envelopes: `content: [{type: "text", text: "..."}]` (preserved for back-compat).
- Successful tool results also emit `structuredContent` matching the declared `outputSchema` (Path B, STORY-399) — a machine-readable mirror of the text envelope.
- Error results keep the text envelope plus a structured `structuredError` object (STORY-401); they do not carry `structuredContent`.

Strict structured-output validation passes:

```bash
tests/test_mcp_stdio.sh --require-structured-content
```

## Operational Expectations for Codex

Use MCP for AIDA substrate operations instead of shelling out when the tool exists. Shell commands are still appropriate for build/test/git work and for independent verification.

Recommended pattern:

- Read via MCP first: `show_requirement`, `list_requirements`, `list_active_leases`, `list_findings`, `list_briefs`, `read_brief`.
- Write via MCP when coordinating: `post_punt`, `file_finding`, `claim_task`, `post_directive`, `ack_brief`.
- Verify cross-surface state with CLI when testing the substrate: MCP write, then CLI read.
- Parse tool output defensively. Error and success bodies are human-readable text envelopes today.
- Avoid concurrent claims on the same spec until TASK-438 closes the `claim_task` TOCTOU race.
- Punt design forks instead of guessing. Use `post_punt` when a spec is ambiguous and `file_finding` for rough edges.

## Codex Discipline In AIDA Projects

Use a sibling worktree for implementation:

```bash
git worktree add /path/to/project-<spec> -b <branch> origin/main
cd /path/to/project-<spec>
ln -s /path/to/project/.aida-store .aida-store
```

Claim work through MCP when available:

```text
claim_task({spec_id: "TASK-123", role: "implementer"})
```

Before implementing architecture-class changes, post a sketch comment on the owning spec and wait for master sign-off. Architecture-class changes include MCP tool contracts, file formats, orchestrator behavior, lease semantics, lifecycle vocabulary, and discipline or memory pack edits.

Commit and PR subjects should use the Codex prefix and a trailing spec trailer:

```text
[AI:codex] fix(scope): concise description (TASK-123)
[AI:codex] docs(agents): Codex setup integration (STORY-417 TASK-485 TASK-484)
```

The trailing parens are load-bearing. The auto-bump scanner promotes referenced specs when the squash commit lands on main. If a PR closes multiple specs, include every shipped spec ID in the same trailing parens group.

When adding a requirement through MCP, pass a valid lowercase `type`. AIDA now derives the canonical ID prefix from the type (for example, `type: "task"` returns `TASK-N`). Do not invent `SPEC-N` IDs.

Use `aida pr ship` as the finish line for bounded direct-publish work. Current wrapper behavior includes:

- Squash-subject spec ID repair from PR title, branch, or body.
- Parser alignment with the auto-bump scanner's trailing-parens convention.
- A CI startup wait so GitHub's initial "no checks reported" window is not misclassified as failure.
- Main-worktree preparation before post-merge `aida pull`.

Before relying on the wrapper in a new environment, read the five-bug arc that hardened it: SPEC-410, BUG-339, BUG-344, BUG-345, plus TASK-458 for the original wrapper.

## Current Known Constraints

- `structuredContent` is emitted on success (Path B, STORY-399) alongside the text envelope; parse either.
- Error bodies carry both a text envelope and a structured `structuredError` object (STORY-401).
- `claim_task` has a known race under concurrent claims. TASK-438 tracks atomicity.
- Cross-machine MCP and auth are out of scope for this local stdio setup.
- Project-local automatic Codex registration is not scaffolded by `aida init` yet. Manual `codex mcp add aida -- aida mcp-serve` is the working path.
- `aida mcp-serve` self-respawns after handled requests when the on-disk `aida --version` reports a newer package version or a different build SHA. If MCP still appears stale, kill that agent's server process and let the client respawn it.
- Headless drains have the same binary-staleness caveat. If an existing `target/debug/aida` or `target/release/aida` binary predates a merged orchestrator reliability fix, that running binary will not enforce the new behavior. Rebuild/relaunch and use `aida dev status` when runtime behavior disagrees with current source.

trace:STORY-398 | ai:codex
