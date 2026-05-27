# AIDA Claude Code Plugin Package

**Last updated**: 2026-05-26  
**Package path**: `plugins/aida/`  
**Marketplace catalog**: `.claude-plugin/marketplace.json`

This repository now carries a first-party Claude Code plugin package skeleton for AIDA. It is intentionally thin:

- It exposes an `aida` MCP server definition that runs `aida mcp-serve`.
- It includes a small `aida-onboard` skill and `/aida-setup` command for setup verification.
- It points users to `aida init` for the full repo-local scaffold of project-specific skills, hooks, docs, and MCP config.

The package does **not** vendor the `aida` binary and does **not** duplicate the entire `aida-core/templates/skills` tree. Claude Code copies installed plugins into a cache, so plugin files should not depend on paths outside the plugin directory. AIDA's richer scaffold belongs in the project through `aida init`, where hooks and skills can be versioned with the repo.

## Local Test Flow

From the AIDA repository root:

```bash
/plugin marketplace add .
/plugin install aida@aida-plugins
```

Direct plugin-dir testing:

```bash
claude --plugin-dir ./plugins/aida
```

Validation before publication:

```bash
claude plugin validate ./plugins/aida
```

## Publication Checklist

Before submitting or publishing:

1. Run `docs/security/marketplace-publication-checklist.md`.
2. Verify `aida` installation instructions are current.
3. Verify the package version matches the intended AIDA release.
4. Verify `.claude-plugin/marketplace.json` uses a valid source path.
5. Verify the MCP server default is appropriate for the release's tool-profile/auth state.

## Future Work

- Replace the full-trust MCP server default with a read-only or coordination profile once STORY-474 lands.
- Add submission notes for the public Claude community marketplace once the package has been locally validated.
- Consider an npm-distributed plugin only if users need package-manager installation rather than git marketplace installation.
