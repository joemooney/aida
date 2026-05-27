# AIDA Claude Code Plugin

This plugin connects Claude Code to AIDA's local, git-native agent-collaboration substrate.

AIDA itself remains a normal CLI installed separately. The plugin does not vendor or install the `aida` binary; it provides Claude Code-facing setup guidance, an MCP server definition, and lightweight command/skill entry points that point the user back to the repo-local AIDA workflow.

## Preconditions

- Install the `aida` CLI and ensure `aida` is on `PATH`.
- Run this from a repository initialized with AIDA, or initialize one with `aida init`.
- Review `docs/security/marketplace-publication-checklist.md` before publishing modified versions of this package.

## Install Locally for Testing

From the repository root:

```bash
/plugin marketplace add .
/plugin install aida@aida-plugins
```

Or load this plugin directory directly while testing:

```bash
claude --plugin-dir ./plugins/aida
```

Validate before publishing:

```bash
claude plugin validate ./plugins/aida
```

## What This Plugin Exposes

- MCP server: `aida`, launched as `aida mcp-serve`.
- Skill: `aida-onboard`, a quick orientation for joining an AIDA project.
- Command: `/aida-setup`, a checklist for verifying local CLI/MCP setup.

The full AIDA skill/hook scaffold is still produced by `aida init` inside each project. Keeping repo-local scaffolding under `aida init` avoids plugin-cache path assumptions and keeps project-specific hooks, skills, docs, and MCP config versioned with the repository.

## Security Posture

The default MCP server is local stdio and assumes a trusted local project. Until AIDA ships MCP tool profiles and remote/auth support, do not publish this package as a write-capable remote MCP integration. See `docs/agents/aida-mcp-install-matrix.md` for current client-specific guidance.
