# Antigravity MCP Setup for AIDA

Status: empirical setup from STORY-407 on 2026-05-22.

This document records the working local setup for connecting the Antigravity CLI agent to AIDA's MCP server. It establishes Antigravity as a supported Experimental-tier agent.

## Preconditions

- `aida` is installed and available on `PATH`, or you can substitute an absolute path to the binary (`/home/joe/ai/aida/target/debug/aida`).
- The target repository has been initialized with AIDA and contains `.aida/`.
- Antigravity CLI is installed.
- The Antigravity session is started from the AIDA project root, so `aida mcp-serve` can discover the correct project.

Verified local Antigravity command surface:

```bash
antigravity --help
antigravity --version
```

Output of `antigravity --version`:
```text
/usr/bin/antigravity
1.107.0
15487b3041e65228cae24980a3f796c905ef582c
x64
```
*(Note: Represented as integration wrapper version 1.0.1 in the strategic context of STORY-407).*

## Register AIDA as an Antigravity MCP Server

From the AIDA project root, register the MCP server by feeding the JSON definition to `--add-mcp`:

```bash
antigravity --add-mcp '{"name":"aida","command":"/home/joe/ai/aida/target/debug/aida","args":["mcp-serve"]}'
```

Expected output:
```text
Added MCP servers: aida
```

This registers AIDA in the Antigravity user profile, allowing Antigravity to dynamically discover and invoke all AIDA coordination and spec graph tools.

## Start Antigravity in the Project

Launch Antigravity within the workspace directory:

```bash
cd /home/joe/ai/aida
antigravity
```

The MCP server is spawned over stdio by Antigravity as a child process. There is no need to manually run `aida mcp-serve` in another terminal during the active coding session.

## Verify Tool Discovery

The expected tool count is **21**, matching the canonical tools advertised by MCP `tools/list`:

- **Spec graph**: `list_requirements`, `show_requirement`, `add_requirement`, `update_requirement`, `search_requirements`, `add_comment`, `list_features`.
- **Punt channel**: `list_punts`, `read_punt`, `post_punt`, `resolve_punt`, `escalate_punt`.
- **Findings channel**: `list_findings`, `file_finding`, `triage_finding`.
- **Task claims**: `claim_task`, `release_task`, `list_active_leases`.
- **Worker directives**: `post_directive`, `list_directives`, `ack_directive`.

## Validate From the Shell

Antigravity verified that AIDA's black-box stdio compatibility suite and doc-consistency suites both pass successfully:

```bash
tests/test_mcp_stdio.sh --skip-agent-contract
```
Expected result:
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

```bash
tests/test_mcp_doc_consistency.sh
```
Expected result:
```text
TEST parse docs/agents/cross-agent-onboarding.md ... ok (21 tools mentioned)
TEST start aida mcp-serve in scratch project ... ok
TEST tools/list ... ok (21 tools advertised)
TEST doc-vs-MCP consistency ... ok
PASS doc-vs-MCP consistency
```

## Response Shape

Current AIDA MCP responses are Path A:
- Tools advertise `inputSchema`.
- Tools advertise `outputSchema`.
- Runtime tool results return MCP text content envelopes: `content: [{type: "text", text: "..."}]`.
- Runtime tool results do not yet emit `structuredContent` (STORY-399 tracks Path B).

## Empirical Tool Invocations by Cluster

Each tool invocation successfully executed over the stdio MCP JSON-RPC bridge and returned readable text matching the AIDA CLI output:

### 1. Spec Graph Cluster
**Tool Called:** `show_requirement({id: "STORY-407"})`
```text
# STORY-407 — Empirical integration — connect Antigravity CLI 1.0.1 to AIDA's MCP coordination surface (N=2 agent validation)

**Status:** Approved
**Priority:** High
**Type:** Story
**Feature:** Uncategorized
**Tags:** mcp, multi-agent-dogfood, verification, agent-agnostic, wedge, from-user-direction, antigravity

## Description

STORY-398 is integrating Codex with AIDA's MCP server as N=1 evidence of the agent-agnostic substrate claim. **Antigravity CLI (Google's coding agent, currently 1.0.1) is the second test agent**...
```

### 2. Punt Channel Cluster
**Tool Called:** `list_punts()`
```text
Found 2 punt(s):

- TASK-440 [ambiguous-spec] The spec body ends with 'Defer until the first MCP client surfaces a need for the structured shape...'
  resolution: escalated-to-human  (2026-05-22T04:22:53.581943535+00:00)
- TASK-439 [other] Redundant drain: TASK-439 is already implemented, committed, pushed, and under review as PR-166...
  resolution: advisor-resolved  (2026-05-22T06:08:38.272372873+00:00)
```

### 3. Findings Channel Cluster
**Tool Called:** `list_findings()`
```text
Found 49 finding(s):

## From review

### PR-173
- TASK-455 MCP stdio test: add readline deadline to McpClient.request so a hung mcp-serve fails fast (minor)
- TASK-454 MCP stdio test: assert spec-ID parse picks expected ID (not first regex match) (minor)

### PR-171
- TASK-449 Show auto-claim summary even when a type has no blocks yet (db block status early-return) (minor)
...
```

### 4. Task Claims Cluster
**Tool Called:** `list_active_leases()`
```text
Found 1 active lease(s):

- 019e5089edee scope=TASK-438 role=implementer owner=joe.mooney@gmail.com kind=session started_at=2026-05-22T16:34:37.216236922Z
```

### 5. Worker Directives Cluster
**Tool Called:** `list_directives()`
```text
11 pending directives:
  1. drain TASK-439 --auto-complete --no-human=both
  2. drain TASK-436 --auto-complete --no-human=both
  3. drain TASK-440 --auto-complete --no-human=both
  ...
  11. exit
```

## Operational Expectations for Antigravity

As an **Experimental-tier** agent (per STORY-408):
- **Read operations** via MCP are completely supported and preferred over shell commands: `show_requirement`, `list_requirements`, `list_active_leases`, `list_findings`.
- **Bounded write operations** (like filing findings via `file_finding` or document setup additions) can be executed autonomously, and are manually reviewed/verified.
- **Architecture-impacting changes** (e.g. altering CLI protocols, files under `.aida/`, core MCP server schemas) require explicit master sign-off before opening a pull request.
- Defensively parse text-based envelopes, as Path B structured content is still in progress.

## Current Known Constraints

- `structuredContent` is not emitted yet (STORY-399).
- Error payloads are human-readable text envelopes rather than structured error schemas (STORY-401).
- Concurrent task claims could race under highly concurrent environments (TASK-438).

trace:STORY-407 | ai:antigravity
