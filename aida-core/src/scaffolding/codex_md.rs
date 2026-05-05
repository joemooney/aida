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

Guidance for AI coding agents (Codex CLI, MCP-compatible agents, etc.)
working in this repository. The block delimited by HTML comment markers
below is auto-generated from `.claude/AIDA.md` on each
`aida scaffold apply` — edit that file (the markers themselves are
intentionally machine-readable, so leave them in place). Anything
outside the marked block is yours to tailor.

## Project overview

{project_name}{description}

{aida_block}

## Codex / MCP-specific notes

### MCP integration

If AIDA is configured as an MCP server (`.mcp.json` is auto-scaffolded),
these tools are available:

| Tool | Purpose |
|------|---------|
| `list_requirements` | List requirements with optional status/type filters |
| `show_requirement` | Show full details by SPEC-ID |
| `search_requirements` | Search by keyword across titles + descriptions |
| `add_requirement` | Create a new requirement |
| `update_requirement` | Update status / priority / owner |
| `add_comment` | Add an implementation note |
| `list_features` | List feature categories |

To configure for Codex CLI:

```bash
codex mcp add aida -- aida mcp-serve
```

### Non-interactive workflows (codex exec)

```bash
# Implement a specific requirement
codex exec "Implement FR-042. Use 'aida show FR-042' to see the details first."

# Sprint standup
codex exec "Run 'aida list --status in-progress' and 'git log --since=yesterday'. Generate a standup report."

# Capture untraced work
codex exec "Review today's git commits. For each, check if trace comments exist. Create requirements for untraced code."
```

### Commit attribution

When committing on behalf of Codex, use the `[AI:codex]` prefix per the
commit format spec in the AIDA-AUTOGEN block above.
"#
        )
    }
}
