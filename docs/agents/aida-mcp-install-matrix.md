# AIDA MCP Install and Agent-Client Adaptation Matrix

**Last verified**: 2026-05-26  
**Owner**: AIDA ecosystem watch / Codex  
**Purpose**: track how AIDA should be exposed to each major coding-agent client without relying on memory or stale setup snippets.

This matrix is operational, not marketing. It records how to connect each client to `aida mcp-serve`, what instruction file or marketplace surface the client expects, and which AIDA MCP profile should be safe by default.

## Tool profiles (STORY-474)

`aida mcp-serve` now gates its tool surface behind a named **profile** — a capability tier where each higher tier is a strict superset of the one below:

| Profile | Exposes | Use for |
|---|---|---|
| `read-only` | pure reads only (`list_*`/`show_*`/`search_*`/`query_graph`/`history`/`read_*`) — **no write tools at all** | untrusted / marketplace / exploratory / remote clients (the recommended safe default) |
| `coordination` | read-only + drain-coordination writes (punts, findings, task claims, directives, brief acks, comments, relationships, messages) — no spec create/edit | trusted agents participating in a drain |
| `operator` | coordination + spec-graph writes (`add_requirement`, `update_requirement`) | trusted local day-to-day work |
| `admin` / `full` | every tool | fully-trusted local operator (the built-in backwards-compatible default) |

Select the profile (first match wins):

1. `AIDA_MCP_PROFILE=read-only` in the MCP server's environment (set it in the client's `.mcp.json`/`mcpServers` env block), or
2. `[mcp]\nprofile = "read-only"` in `.aida/config.toml`.

The default is `full` so existing trusted local installs are unchanged. **Marketplace and any non-local/untrusted install should set `read-only` (or `coordination`) explicitly.** The profile is a real boundary: an out-of-profile tool is both hidden from `tools/list` and rejected at `tools/call` with a `permission_denied` envelope, even if the client calls it by name. The active profile is printed on the server's stderr startup banner.

## Recommended Defaults

- Default to local stdio for solo/local agents.
- Default to `read-only` or `coordination`-scoped MCP for any untrusted/remote/marketplace context (set `AIDA_MCP_PROFILE` / `[mcp] profile`).
- Keep write-capable tools off for cloud/remote agents until auth, project scoping, and audit logs exist.
- Prefer client-native package/marketplace channels for discovery, but keep `aida init` as the source of repo-local scaffolding.
- Keep a "last verified" date on every row; do not silently assume client setup stays stable.

## Matrix

| Client | Instruction / agent context file | MCP config surface | Marketplace / package surface | AIDA setup recommendation | Write-tool stance | Last verified | Source |
|---|---|---|---|---|---|---|---|
| Claude Code | `CLAUDE.md`; `.claude/skills/`; slash commands and hooks | Project `.mcp.json` scaffolded by `aida init`; plugin packages may also bundle MCP servers | Claude Code plugin marketplaces package commands, agents, hooks, MCP servers, and skills via `.claude-plugin/marketplace.json`; AIDA package skeleton lives at `plugins/aida/` | Keep `aida init` for repo scaffolding; use the AIDA Claude plugin for discovery and repeatable install | Trusted local write tools are acceptable after project trust; marketplace package must document permissions | 2026-05-26 | https://code.claude.com/docs/en/plugin-marketplaces |
| Codex CLI | `AGENTS.md`; Codex skills when scaffolded | `codex mcp add <name> -- <command>...`; local setup verified by `docs/agents/codex-mcp-setup.md` | Codex has CLI MCP management; broader marketplace/plugin behavior is still changing, so keep install docs conservative | Prefer `aida agent new codex --spec <SPEC>` for supervised launches; register MCP as `codex mcp add aida -- aida mcp-serve` | Trusted local coordination profile; structuredContent ships on success (STORY-399), text envelope preserved for back-compat | 2026-05-26 | https://platform.openai.com/docs/docs-mcp |
| Cursor | Cursor rules files; project-specific guidance should be mirrored from AGENTS/CLAUDE where useful | Project `.cursor/mcp.json` for project tools; user/global MCP config also exists | Cursor extension ecosystem, not a stable AIDA-specific package target yet | Document `.cursor/mcp.json` snippet once profiles exist; use read-only by default for exploratory Cursor sessions | Read-only first; coordination only after explicit trust because Cursor may run tools inside editor workflows | 2026-05-26 | https://docs.cursor.com/context/model-context-protocol |
| Windsurf / Cascade | Windsurf rules/customizations; repo guidance should point back to AGENTS.md | `~/.codeium/windsurf/mcp_config.json`; supports command/args/env/server URL fields and marketplace/deeplink flows | Windsurf MCP marketplace, custom registries, team admin controls, whitelists | Provide a global config snippet and a marketplace-ready metadata block; warn that global config can leak tools into unrelated workspaces | Read-only or coordination-scoped only; respect team whitelists/admin registry controls | 2026-05-26 | https://docs.windsurf.com/windsurf/cascade/mcp |
| Continue | Rules/tools blocks and Continue config; can import JSON MCP snippets | `.continue/mcpServers/` can ingest JSON-style MCP server configs from Claude Desktop/Cursor/Cline-style files | Continue Hub / Mission Control for shared models, rules, tools, and secrets | Provide a copied JSON config under `.continue/mcpServers/aida.json` once profile support exists | Read-only first; coordination only in trusted project workspaces | 2026-05-26 | https://docs.continue.dev/customize/deep-dives/mcp |
| Cline | `.clinerules` for project behavior | Cline MCP settings JSON via MCP Servers UI; stdio and remote server configs | Cline MCP Marketplace supports one-click installs; enterprise controls govern marketplace/MCP usage | Publish AIDA server metadata to Cline-compatible marketplaces only after safe profile + security checklist exist | Read-only default for marketplace; local trusted implementers can use coordination profile | 2026-05-26 | https://docs.cline.bot/mcp/configuring-mcp-servers |
| Roo Code | `.roo/rules` / mode-specific instructions, depending on project setup | Roo Code uses MCP-capable extension settings and marketplace install flows | Roo Code Marketplace includes MCP-related install surface | Track as a secondary marketplace target after Claude/Cline/Windsurf; verify exact config before publishing snippets | Read-only until verified | 2026-05-26 | https://roocodeinc.github.io/Roo-Code/features/marketplace/ |
| GitHub Copilot coding agent | Repository/custom-agent instructions in GitHub | Repository or custom-agent MCP configuration for Copilot coding agent tasks | GitHub-hosted cloud-agent configuration; not a local plugin package | Do not expose local stdio AIDA directly. Requires remote/auth-capable AIDA MCP first, plus least-privilege profile and audit log | No write tools until remote auth/audit/project scoping ships | 2026-05-26 | https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-coding-agent |
| VS Code Copilot Chat / agent mode | `.github/copilot-instructions.md` or VS Code/project instructions depending on setup | VS Code MCP config surfaces vary between chat, IDE, and coding-agent flows | VS Code extension marketplace; not an AIDA-specific target yet | Treat as separate from GitHub Copilot cloud agent; verify local MCP config before adding snippets | Read-only first | 2026-05-26 | https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-coding-agent |
| Sourcegraph / Amp | Amp project guidance and Sourcegraph context | Sourcegraph MCP endpoint; `amp mcp add sg <url>` for Sourcegraph's server, and Amp-compatible MCP for external servers | Sourcegraph/Amp distribution is not an AIDA marketplace target; Sourcegraph's behavior is a design signal | Mirror Sourcegraph's split between curated default and full endpoint when designing AIDA profiles | Curated/default profile should be small; full profile explicit | 2026-05-26 | https://sourcegraph.com/changelog/mcp-ga |
| Devin | Devin workspace/project instructions; enterprise workspace settings | Devin MCP Marketplace for external tools; Devin also exposes an official Devin MCP server | Devin Marketplace and enterprise admin flows | AIDA remote MCP could eventually be a Devin marketplace server, but only after auth/audit/tool profiles exist | No write tools before remote auth/audit; enterprise review required | 2026-05-26 | https://docs.devin.ai/work-with-devin/mcp |
| Aider | `CONVENTIONS.md`, repo docs, and command-line context files; MCP support not confirmed as a first-class stable client surface in this pass | Unknown / not verified | No first-class AIDA marketplace target identified | Use AIDA CLI and repo docs; revisit if Aider adds stable MCP client support | CLI-only until verified | 2026-05-26 | https://aider.chat/docs/ |
| Antigravity | `AGENTS.md`; `docs/agents/antigravity-mcp-setup.md`; AIDA supervised launcher | AIDA-tested MCP path via local `aida mcp-serve` and/or CLI fallback | No external marketplace target identified | Keep AIDA-specific setup docs and launcher behavior authoritative | Trusted local coordination profile | 2026-05-26 | `docs/agents/antigravity-mcp-setup.md` |

## AIDA-Specific Profile Guidance

When STORY-474 lands, map profiles as follows:

| Profile | Intended clients | Tool class |
|---|---|---|
| `read-only` | Cursor, Windsurf, Continue, Cline marketplace installs, Copilot/Devin before auth | `list_requirements`, `show_requirement`, `search_requirements`, `history`, resources |
| `coordination` | Trusted local Codex/Claude/Antigravity implementers | read-only plus briefs, comments, findings, punts, claim/release |
| `operator` | Supervised local sessions owned by the repository operator | coordination plus directives, status/doctor/queue controls if exposed |
| `admin` / `full` | Maintainer-only debugging | broad mutation/destructive tools; never marketplace default |

## Packaging Implications

1. Claude Code should be the first package target because AIDA already ships skills/hooks and Claude's plugin docs explicitly support those assets plus MCP servers.
2. Cline and Windsurf are second-tier marketplace targets because both have MCP marketplaces/admin controls and can discover servers through marketplace-like flows.
3. Codex should remain documented via `AGENTS.md`, `codex mcp add`, and AIDA launcher integration until Codex's plugin/marketplace surface is stable enough to publish against.
4. Copilot coding agent and Devin require remote/auth-capable AIDA MCP before AIDA can safely expose write tools.
5. Sourcegraph's curated-vs-full endpoint split is a strong design precedent for AIDA's safe-profile work.

## Refresh Checklist

Run this checklist during ecosystem-watch refreshes and before minor/major AIDA releases:

- Verify each source link still documents the stated config surface.
- Re-run Codex local MCP setup from `docs/agents/codex-mcp-setup.md`.
- Verify Claude Code plugin marketplace docs for manifest or permission changes.
- Verify whether Cursor/Windsurf/Continue/Cline changed project-local config paths.
- Verify whether Copilot/Devin can consume local stdio, remote HTTP only, or both.
- Update the write-tool stance if AIDA gains MCP profiles, auth, or audit logging.
- File a `codex`-tagged TASK for any stale row that cannot be verified in the release window.
