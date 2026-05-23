# AGENTS.md

Guidance for Codex and MCP-compatible coding agents working in the AIDA
repository. Read this as instructions-to-self: coordinate through AIDA,
keep git and the spec store coherent, and leave durable traces for the
next agent.

## Project Orientation

AIDA is an agent-collaboration substrate: stable spec IDs, typed
requirement relationships, code-to-spec trace comments, an autonomous
queue/orchestrator, and an MCP server that exposes the same graph to
non-Claude agents. Use `OVERVIEW.md` for product and architecture
context, `CLAUDE.md` for the broad repository guide, and
`docs/agents/cross-agent-onboarding.md` for shared MCP operating
context.

## Storage Model

The spec graph is git-canonical. The orphan `aida-store` branch stores
one YAML object per requirement; `.aida-store/` is the live worktree for
that branch; `.aida/cache.db` is a rebuildable read cache. Do not create
parallel requirement files. Use AIDA tools and MCP for durable
coordination state.

When using a sibling worktree, link the store worktree if the local CLI
needs direct git-canonical access:

```bash
ln -s /home/joe/ai/aida/.aida-store .aida-store
```

## Requirements Management

Before implementing, read the owning requirement:

```bash
aida show <SPEC-ID>
```

Prefer MCP tools for spec graph and coordination operations:
`show_requirement`, `list_requirements`, `claim_task`, `release_task`,
`file_finding`, `post_punt`, and `add_comment`. Use shell commands for
build, test, git inspection, and cross-surface verification.

When adding requirements through MCP, pass a valid lowercase `type`.
AIDA derives the canonical ID prefix from that type, for example
`type: "task"` produces `TASK-N`. Do not invent generic `SPEC-N` IDs.

## Daily-Use Commands

```bash
codex mcp add aida -- aida mcp-serve
codex --cd /home/joe/ai/aida
aida show <SPEC-ID>
aida list --status approved
aida queue work <SPEC-ID>
aida pr ship
aida brief list --for-agent codex
aida brief ack .aida/agent-briefs/codex/<brief>.md
aida --asciinema --cast-title "Demo" queue work --batch <name> --auto-complete
tests/test_mcp_stdio.sh --skip-agent-contract
tests/test_mcp_doc_consistency.sh
```

Use `aida brief list --for-agent codex` when a master/advisor session says
there is a pickup brief. Briefs live under `.aida/agent-briefs/codex/`,
embed the target spec plus setup/trailer reminders, and are local
runtime state. After reading one, run `aida brief ack <path>` so the
default list stays focused on pending work.

Use `aida --asciinema <subcommand>` for first-class terminal capture when
you need demo, training, or audit material. By default casts are written
under `~/.aida/casts/` with Windows-safe timestamps; pass `--cast-out`
and `--cast-title` to control the path and title.

## Worktree And Session Discipline

Do implementation work in a sibling worktree:

```bash
git fetch origin main:refs/remotes/origin/main
git worktree add /home/joe/ai/aida-<spec> -b <branch> origin/main
cd /home/joe/ai/aida-<spec>
ln -s /home/joe/ai/aida/.aida-store .aida-store
```

Claim the spec before editing and release the lease after shipping. Do
not edit another agent's dirty main worktree. If branch, lease, or
worktree state looks inconsistent, stop and surface it rather than
forcing git.

## Code Traceability

When code implements a spec, add a trace comment in the touched code:

```rust
// trace:TASK-123 | ai:codex
```

Keep spec IDs in developer artifacts: commits, PR titles, trace
comments, plans, and spec comments. Do not leak internal IDs into
user-facing CLI output unless that output is explicitly
developer/operator-facing.

## Commit And PR Format

Use the Codex prefix and put every shipped spec in trailing parens:

```text
[AI:codex] fix(scope): concise description (TASK-123)
[AI:codex] docs(agents): Codex setup integration (STORY-417 TASK-485 TASK-484)
```

The trailing parens are load-bearing. The auto-bump scanner reads them
when the squash commit lands on main. If one PR closes multiple specs,
include every shipped spec ID in the same trailing parens group.

## Sketch-First Protocol

Before opening a PR for architecture-class changes, post a sketch on the
owning spec and wait for master sign-off. Architecture-class means file
formats, MCP tool contracts, orchestrator semantics, lease model,
cross-cutting lifecycle vocabulary, or discipline/memory changes.
Bounded tests, docs refreshes, and acceptance-criteria implementation do
not need a sketch unless they introduce a reusable harness or new
project convention.

## MCP Notes

Trust MCP `tools/list` for tool names and argument names. Current AIDA
MCP responses are text envelopes with descriptor-level output schemas;
parse defensively until `structuredContent` ships. Restart
`aida mcp-serve` after pulling or rebuilding AIDA if you need newly
shipped MCP behavior.

Headless orchestrator reliability fixes have the same binary-staleness
risk. If a drain is already running an older `target/debug/aida` or
`target/release/aida`, newly merged headless gates such as the
AskUserQuestion denial will not apply until the binary is rebuilt and the
next drain is launched from that binary. Use `aida dev status` when the
runtime behavior contradicts current source.

## Known Codex Pitfalls

- PR-201 missed the trailing spec trailer in the squash subject; that
  incident is why trailing-parens discipline is non-optional.
- Read the `aida pr ship` arc before relying on the wrapper in a new
  environment: SPEC-410, BUG-339, BUG-344, and BUG-345 document subject
  repair, parser alignment, CI startup waiting, and stale-main-worktree
  handling.
- If an instruction from another session sounds inconsistent with branch
  contents, verify the PR contents and flag the mismatch.
