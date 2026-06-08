use super::*;

impl Scaffolder {
    /// Generate AGENTS.md content for Codex-compatible coding agents.
    ///
    /// AGENTS.md is seed-class for the user-owned framing (project intro,
    /// agent-specific notes) but contains a delimited AIDA-AUTOGEN block
    /// where the conventions are inlined. `scaffold status` extracts and
    /// checksums just the block — user content outside is freely
    /// editable, AIDA-owned content inside auto-upgrades.
    ///
    /// Codex / generic MCP agents don't expand `@` imports the way Claude
    /// Code does, so we inline rather than reference. Trade: drift detection
    /// is delimiter-based instead of single-file checksum.
    /// trace:FR-1-035 | ai:claude
    pub(super) fn generate_agents_md(&self, store: &RequirementsStore) -> String {
        let project_name = if !store.title.is_empty() {
            &store.title
        } else if !store.name.is_empty() {
            &store.name
        } else {
            "Project"
        };

        let description = if !store.description.is_empty() {
            format!("\n\n{}", store.description)
        } else {
            String::new()
        };

        let aida_block = self.generate_aida_md_for_agents(store);

        format!(
            r#"# AGENTS.md

Guidance for Codex and MCP-compatible coding agents working in this
repository. Read this as instructions-to-self: when you implement work,
coordinate through AIDA, keep the git/aida-store state coherent, and
leave durable traces for the next agent.

The block delimited by HTML comment markers below is auto-generated from
`.claude/AIDA.md` on each `aida scaffold apply`. Leave the markers in
place. Content outside the marked block is project-owned guidance.

## Project Orientation

{project_name}{description}

Use `OVERVIEW.md` for product/architecture context and
`docs/agents/cross-agent-onboarding.md` for the shared MCP operating
model. Use `docs/agents/codex-mcp-setup.md` when configuring Codex
against AIDA's MCP server. Use `docs/agents/session-communication.md`
for agent pause/abort/defer semantics.

{aida_block}

## Codex Operating Discipline

### Storage Model

AIDA's source of truth is the git-canonical spec store, not an ad hoc
notes file. Use MCP tools for spec graph and coordination operations
when available; use shell commands for build, test, git inspection, and
cross-surface verification.

### Requirements Management

Before implementing, make sure a requirement exists and read it with
`show_requirement` or `aida show <ID>`. If you file new requirements via
MCP, pass a valid lowercase `type`; AIDA derives the canonical ID prefix
from that type. Do not invent `SPEC-N` IDs.

### Daily-Use Commands

```bash
codex mcp add aida -- aida mcp-serve
aida show <SPEC-ID>
aida list --status approved
aida queue work <SPEC-ID>
aida pr ship
aida brief list --for-agent codex
aida brief ack .aida/agent-briefs/codex/<brief>.md
tests/test_mcp_stdio.sh --skip-agent-contract
tests/test_mcp_doc_consistency.sh
```

### MCP Coordination

Use AIDA MCP for substrate operations: `show_requirement`,
`list_active_leases`, `claim_task`, `release_task`, `file_finding`,
`post_punt`, `list_briefs`, `read_brief`, `ack_brief`, `add_comment`,
and directive tools. Trust MCP `tools/list` for argument names. Current
responses are text envelopes; parse defensively until structuredContent
ships.

For cross-agent communication semantics, especially Claude Code
`PreToolUse` / `PostToolUse`, `continue: false`, `ask`, and `defer`, use
`docs/agents/session-communication.md`. Do not assume a later hook can ask
whether to continue after an earlier hook has halted the run.

### Worktree And Session Discipline

Do implementation work in a sibling worktree. No `.aida-store` symlink is
needed — a sibling worktree resolves the canonical store at the main
worktree automatically (BUG-331). Do not edit another agent's dirty main
worktree. If a branch, lease, or worktree state looks inconsistent, stop
and surface it instead of forcing git.

### Direct Assignment: Implement BUG/TASK-N

When the operator says "implement BUG-N / TASK-N" and there is no queued
brief, follow this path (it's the same one used for TASK-132 and BUG-406):

1. `aida show <SPEC>` — read the spec, acceptance criteria, and any owning plan.
2. If it is Draft and the operator explicitly assigned it, promote it: `aida edit <SPEC> --status approved`.
3. Start an isolated session: `aida session start --owns <SPEC> --role implementer --base origin/main`.
4. Work in the sibling worktree — no `.aida-store` symlink (the store resolves automatically, per Worktree discipline above).
5. Implement; add `// trace:<SPEC> | ai:codex` comments; run targeted tests + `cargo fmt --all -- --check`.
6. Commit `[AI:codex] type(scope): description (<SPEC>)`.
7. `aida pr ship` — watches CI, squash-merges, pulls, and auto-bumps the spec to Completed.
8. End the session; verify the spec reached Completed.
9. Architecture-class work → sketch first and wait for master sign-off (see Sketch-First Protocol).

### Code Traceability

When code implements a spec, add a trace comment in the touched code:

```rust
// trace:TASK-123 | ai:codex
```

Keep spec IDs in developer artifacts: commits, PR titles, trace
comments, and plans. Do not leak internal IDs into user-facing CLI text
unless that output is explicitly developer/operator-facing.

### Commit And PR Format

Use the Codex prefix and put every shipped spec in trailing parens:

```text
[AI:codex] fix(scope): concise description (TASK-123)
[AI:codex] docs(agents): Codex setup integration (STORY-417 TASK-485 TASK-484)
```

The auto-bump scanner reads the trailing parens. If one PR closes
multiple specs, include every spec ID in that group.

### Sketch-First Protocol

Before opening a PR for architecture-class changes, post a sketch on the
owning spec and wait for master sign-off. Architecture-class means file
formats, MCP tool contracts, orchestrator semantics, lease model,
cross-cutting lifecycle vocabulary, or discipline/memory changes.
Bounded tests, docs refreshes, and acceptance-criteria implementation do
not need a sketch unless they introduce a reusable harness or new
project convention.

### Known Codex Pitfalls

- PR-201 missed the trailing spec trailer in the squash subject; that
  incident is why trailing-parens discipline is non-optional.
- Read the `aida pr ship` arc before relying on the wrapper in a new
  environment: SPEC-410, BUG-339, BUG-344, and BUG-345 document subject
  repair, parser alignment, CI startup waiting, and stale-main-worktree
  handling.
- `aida mcp-serve` self-respawns after handled requests when the on-disk
  `aida --version` reports a newer package version or different build
  SHA. If MCP still appears stale, kill that agent's server process and
  let the client respawn it.
- If an instruction from another session sounds inconsistent with the
  branch contents, verify the PR contents and flag the mismatch.
"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_agents_md_has_codex_operating_sections() {
        let scaffolder = Scaffolder::new(
            std::path::PathBuf::from("/tmp/aida-story-417-test"),
            ScaffoldConfig::default(),
        );
        let store = RequirementsStore::default();
        let md = scaffolder.generate_agents_md(&store);

        assert!(md.contains("## Codex Operating Discipline"));
        assert!(md.contains("docs/agents/codex-mcp-setup.md"));
        assert!(md.contains("docs/agents/session-communication.md"));
        assert!(md.contains("[AI:codex]"));
        assert!(md.contains("SPEC-410"));
        assert!(md.contains("BUG-345"));
        assert!(md.contains("<!-- AIDA-AUTOGEN-BEGIN -->"));
    }
}
