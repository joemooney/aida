# Getting started with AIDA

People describe the work. Agents operate AIDA. This guide separates one-time operator setup from the normal conversation with an AIDA-aware coding agent.

AIDA is alpha software for Linux x86_64 and arm64. You need Git and a supported coding-agent CLI for agent execution; authentication and model access are configured with that vendor. AIDA does not supply a coding model or eliminate its API requirements.

## Step 1: Install AIDA

These commands are for the operator performing setup. Use the shipped installer for a prebuilt Linux release:

```bash
curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
aida --version
```

The default destination is `~/.local/bin/`; ensure it is on your `PATH`. Pin a release with `--version <tag>` or change the destination with `--prefix <directory>` passed to the script. See the [repository README](../README.md#install) for the source-build alternative.

There are no supported macOS or Windows release artifacts. A container deployment is an operator choice, not a substitute for a documented supported coding-agent environment.

## Step 2: Initialize your project

Ask an agent to inspect the existing project setup first. For a new AIDA project, an operator or authorized setup agent runs:

```bash
cd my-project
git init                 # only if this is not already a Git repository
aida init
```

The ordinary full setup creates a Git-canonical spec store on an orphan `aida-store` branch, an attached store worktree, a rebuildable SQLite read cache, and agent workflow scaffolding. Agent selection follows PATH detection unless explicitly selected with `--agent`. Minimal footprints and adopted projects have different write behavior; inspect `aida init --help` before choosing them.

The CLI is the default interface for CLI-capable agents. MCP registration is an explicit opt-in with `aida init --with-mcp`, not an unconditional output of every initialization. See [agent setup](agents/aida-mcp-install-matrix.md) for client-specific connection details and profile boundaries.

Keep existing guidance and project files under review. Do not use `--force` as a routine way to refresh an existing project. Use the documented refresh operation for installed packs instead.

## Joining an existing AIDA project

An existing store can attach on the first supported read after cloning. Reading does not require allocating a new node identity. Before creating specs, ask your setup agent to check the local binding and initialize it using the supported lifecycle if needed. The [allocation reference](cli/07-project-setup.md#clone-local-allocation-authority) describes the current clone-local policy; a copied reservation ledger does not grant global allocation authority.

If a normal initialized project needs its local agent scaffolding, use its documented initialization/refresh flow. Adopted projects use the separate [scaffold projection](cli/07-project-setup.md#aida-scaffold) path. Do not assume every clone should regenerate or commit project files.

## Step 3: Describe an outcome to your agent

With an AIDA-aware agent in the project, try:

> I want failed login attempts to be rate limited. Check the existing requirements, help me define the behavior, and record the work in AIDA. Prepare it for my review before implementation.

The agent should check the graph, identify missing decisions, create or update a draft spec, and present the acceptance criteria. It performs the CLI or MCP calls. Spec approval and queueing still require the appropriate authority; asking an unprivileged agent does not grant it that authority.

This is a suggested conversation, not a separate AIDA chat service. Scaffolding, skills and instructions make the workflow available to the agent you are already using.

## Step 4: Choose the delegation level

Ask the advisor to recommend an execution mode for the spec:

- `operator`: a person performs the work.
- `guided`: interactive design and implementation with human participation.
- `drive`: agent execution through CI, followed by human review and merge.
- `drain`: agent execution through implementation, CI, independent review, merge and pull, within the applicable gates.
- `decide`: execution waits for a pending decision.

Start with supervised delivery when evaluating the workflow. The agent can explain the proposed mode and route the approved work; the operator supplies any human-only grants or authorization required. See [configuration](configuration.md) for other choices about protocols, agents and workflow policy.

## Step 5: Inspect the result

Ask the agent:

> Show me the acceptance criteria, what changed, how it was validated, the independent review evidence, and anything still waiting on me.

For direct inspection, operators can also use read commands such as `aida status`, `aida awaiting`, `aida show <SPEC-ID>` and `aida why <file>:<line>`. These are optional inspection tools, not a command vocabulary that every product stakeholder must learn.

Use [the first-project walkthrough](first-project.md) for a longer example. The [CLI manual](cli/README.md) is the reference for agent integrations and operators who need to inspect or troubleshoot the underlying calls.

## Uninstall and retained data

Before removing AIDA, stop the relevant agent sessions and review what data must be retained. The code branch, orphan spec-store history, local caches, scaffolding and machine-global configuration are different things. Do not treat deleting a cache or binary as deleting the requirements graph. For adopted projections, use [the scaffold uninstall operation](cli/07-project-setup.md#aida-scaffold), which preserves edited/unowned files and the store. See [administration](admin-guide.md) for backup and configuration boundaries.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
