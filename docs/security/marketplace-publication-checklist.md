# Marketplace Publication Security Checklist

**Last updated**: 2026-05-26  
**Scope**: AIDA packages published through Claude Code plugins, MCP registries, Cline/Windsurf-style marketplaces, or any other third-party agent extension channel.

Use this before publishing or materially updating an AIDA marketplace package. The goal is to make AIDA safe to install as an agent-facing substrate without hiding capabilities from the operator.

## Publication Decision

Before publishing, record the answers in the PR body or release notes:

- Which package or registry is being updated?
- Which AIDA version or commit does it install or reference?
- Which MCP transport does it configure: stdio, local HTTP, remote HTTP, or none?
- Which MCP tool profile does it expose by default?
- Which files does the package write into the target project?
- Which commands can run automatically after install?
- Which credentials, if any, must the user provide?

If any answer is unknown, do not publish. File a follow-up task instead.

## Required Checks

### 1. Source and provenance

- The package links to the canonical AIDA source repository.
- The package identifies the exact AIDA version, tag, or commit it installs.
- The package does not download unaudited scripts from mutable URLs.
- Any generated package artifact can be reproduced from tracked source files.
- The release notes explain whether the package is official, experimental, or community-maintained.

### 2. Filesystem writes

- Every project-local path written by the package is documented.
- No package step writes outside the target project except documented user-level config paths required by the client.
- No package step overwrites an existing user file without creating a backup or using AIDA's scaffold header/merge discipline.
- Any generated hooks, skills, rules, or MCP config files are readable text.
- Any destructive cleanup path has a salvage-first behavior or an explicit user confirmation.

### 3. Command execution

- The package documents every command it asks the agent client to run.
- Install-time commands do not run arbitrary shell snippets from remote content.
- Runtime commands invoke the `aida` binary or clearly documented helper scripts.
- Commands work when `aida` is installed on `PATH` and when a user provides an absolute binary path.
- Long-running commands such as `aida mcp-serve` document restart/update behavior.

### 4. MCP tool exposure

- The default MCP profile is least-privilege.
- Read-only or coordination-scoped profiles are preferred for marketplace installs.
- Write-capable tools are documented by category: spec writes, comments, findings, punts, leases, directives, briefs, queue/session/doctor operations.
- Destructive or operator-level tools are disabled by default unless the user explicitly chooses an admin/full profile.
- The package describes that current runtime responses are text envelopes unless structuredContent support has shipped.

### 5. Authentication and secrets

- No credentials are embedded in package files.
- Any required token is entered through the client or AIDA config mechanism, not hard-coded into docs.
- Remote MCP transports require authentication before exposing write tools.
- Logs and audit output do not print secrets.
- Secret unlink/revoke instructions are documented when a marketplace supports connected accounts.

### 6. Agent permission model

- The package states which agent permission layer applies: Claude plugin permissions, Codex sandbox/approval mode, IDE MCP allowlist, enterprise registry policy, or AIDA-local trust.
- Hooks that can halt or defer agent work document their exact control-flow semantics.
- Headless workflows avoid interactive prompts unless they use a resumable external approval path.
- The install docs explain how to disable or remove the package.

### 7. Auditability

- AIDA logs or status surfaces enough evidence to answer: who/what changed which spec, via which tool, and when?
- Marketplace packages link to `docs/agents/aida-mcp-install-matrix.md` for current client-specific caveats.
- Agent-facing instructions preserve AIDA's commit trailer and trace-comment conventions.
- Any package that enables write tools also points users at `aida status` and `aida doctor`.

### 8. Validation commands

Run the relevant checks before publishing:

```bash
cargo fmt --all
cargo test -p aida-core preview_includes_cross_agent_onboarding_docs --lib
tests/test_mcp_doc_consistency.sh
tests/test_mcp_stdio.sh --skip-agent-contract
git diff --check
```

When publishing a Claude Code plugin package, also run Claude's plugin validation command from the package root if available:

```bash
claude plugin validate
```

If the marketplace requires a manifest, validate the manifest against that marketplace's current schema.

## Release Checklist Hook

For AIDA major/minor releases:

1. Refresh `docs/competitive-analysis/ecosystem-watch.md`.
2. Check whether any published marketplace package needs a version, manifest, or install-doc update.
3. Re-run this security checklist if marketplace metadata, MCP defaults, install commands, or scaffolded agent docs changed.
4. File a `codex`-tagged task for any checklist item that cannot be completed before release.

Patch releases only need this checklist when they change marketplace-visible files, MCP defaults, hooks, install scripts, or scaffolded agent instructions.

## Publish / No-Publish Verdict

Use this verdict block in PR bodies for marketplace updates:

```markdown
Marketplace security verdict: publish / publish-with-conditions / do-not-publish

Package:
Version/commit:
Default MCP profile:
Write tools exposed:
Validation run:
Conditions / follow-ups:
```
