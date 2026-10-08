# Marketplace Research: AIDA Distribution and System Improvement Roadmap

**Last updated**: 2026-05-26  
**Research owner**: Codex  
**Scope**: AI coding-agent marketplaces, MCP distribution, enterprise controls, and the implications for AIDA's product roadmap.

## Executive Thesis

AIDA should not position itself as another coding agent, IDE assistant, or generic agent framework. The market is consolidating around horizontal agents and extension marketplaces: Claude Code plugins, Codex skills and MCP configs, Cursor and Windsurf MCP settings, Cline's MCP marketplace, GitHub Copilot cloud-agent MCP, Linear and Atlassian remote MCP servers, Sourcegraph MCP, and Devin's enterprise agent surface.

The durable opening for AIDA is narrower and stronger:

> AIDA is the local, git-native intent and coordination control plane that makes multiple agents accountable to the same spec graph, lifecycle state, traceability rules, and operator-visible health checks.

That means the marketplace strategy is not "sell AIDA as an app marketplace competitor." It is "ship AIDA as the substrate that every agent marketplace package points at."

The near-term product goal should be: installable in one command, visible in each agent's native extension channel, and credible to a team that already has Linear/Jira/GitHub plus several agent clients.

## What Changed in the Market

### 1. Agent extension marketplaces are now first-class distribution channels

Claude Code plugins package slash commands, agents, MCP servers, hooks, and LSP servers with one-command installation. The official docs describe a marketplace manifest at `.claude-plugin/marketplace.json`, plugin metadata, versioning, marketplace allowlists, seed directories for containers, and managed restrictions for enterprise use.

Sources:
- https://code.claude.com/docs/en/discover-plugins
- https://code.claude.com/docs/en/plugin-marketplaces
- https://claude.com/docs/plugins/submit
- https://www.claude.com/blog/claude-code-plugins

Implication for AIDA: AIDA's current scaffolding is strong inside a repo, but marketplace-native packaging should become an explicit distribution artifact. A Claude plugin can carry AIDA skills, hooks, MCP config, setup guidance, and a "first project" workflow as a coherent bundle.

### 2. MCP is becoming the universal agent integration seam

MCP is no longer only a local Claude Code feature. Linear exposes a remote HTTP MCP server with OAuth and setup instructions for Claude Code, Codex, Cursor, and VS Code. GitHub Copilot cloud agent supports MCP tools configured at the repository or custom-agent level. Windsurf has an MCP marketplace, one-click deeplinks, remote HTTP MCP, team admin controls, custom registries, and whitelists. Continue, Sourcegraph, and Cline all expose MCP-based extension paths.

Sources:
- https://linear.app/docs/mcp
- https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-cloud-agent
- https://docs.windsurf.com/windsurf/cascade/mcp
- https://docs.continue.dev/customize/mcp-tools
- https://sourcegraph.com/changelog/mcp-ga
- https://sourcegraph.com/changelog/mcp-curated-default-tools
- https://cline.bot/mcp-marketplace
- https://registry.modelcontextprotocol.io/

Implication for AIDA: the AIDA MCP server is not a sidecar. It is the product's most portable API. The next level of maturity is remote/auth-capable MCP with a focused default tool surface, strong error contracts, audit logs, and marketplace metadata.

### 3. Enterprise buying criteria are converging on governance, audit, and scoped access

OpenAI's Codex safety writeup emphasizes bounded environments, managed configuration, network policies, credential handling, telemetry, and auditability. GitHub Copilot's MCP docs warn that MCP tools can be used autonomously without approval and recommend carefully limiting exposed tools. Windsurf documents enterprise MCP registry control and whitelisting. Sourcegraph added OAuth Dynamic Client Registration and scoped MCP access. Devin release notes call out MCP audit logs, OAuth install flows, consumption visibility, and session hard caps.

Sources:
- https://openai.com/index/running-codex-safely/
- https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-cloud-agent
- https://docs.windsurf.com/windsurf/cascade/mcp
- https://sourcegraph.com/changelog/mcp-ga
- https://docs.devin.ai/release-notes/2026

Implication for AIDA: AIDA's local-first model is a trust advantage, but only if the tool can explain exactly what agents did, what state they changed, and which gates enforced policy. The status/doctor/registry work is market-relevant, not just internal cleanup.

### 4. PM tools are adding agent context, not repo-native intent graphs

Linear and Atlassian are moving fast on remote MCP and AI agents. Atlassian's Rovo Dev handles planning, coding, reviews, and repetitive work inside Jira/Bitbucket/GitHub contexts, and Atlassian's MCP server exposes Jira, Confluence, and Compass data to compatible clients. These systems own human-visible workflow and enterprise permissions.

Sources:
- https://linear.app/docs/mcp
- https://www.atlassian.com/software/rovo-dev
- https://support.atlassian.com/atlassian-rovo-mcp-server/docs/getting-started-with-the-atlassian-remote-mcp-server/

Implication for AIDA: do not market against Linear/Jira as replacement PM. Market below them: AIDA is the in-repo engineering intent graph that agents can mutate safely, while SaaS PM remains the external stakeholder workflow.

### 5. Tool-surface size is becoming an operational constraint

Sourcegraph explicitly reduced its default MCP endpoint to a focused tool set to lower tool-list noise and context-budget cost. Windsurf exposes toggles for MCP tools and notes a 100-tool access limit. GitHub recommends exposing only necessary MCP tooling.

Sources:
- https://sourcegraph.com/changelog/mcp-curated-default-tools
- https://docs.windsurf.com/windsurf/cascade/mcp
- https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-cloud-agent

Implication for AIDA: AIDA should not keep growing one flat MCP tool list indefinitely. It needs tool profiles: `read-only`, `coordination`, `operator`, `admin`, and maybe `full`. Default should be conservative and agent-friendly.

## AIDA's Defensible Niche, Refined

The existing "agent-collaboration layer" positioning is still right, but the sharper 2026 marketplace phrasing is:

> AIDA is the repo-local system of record and control plane for agentic software work. Agents may run in Claude Code, Codex, Cursor, Windsurf, Copilot, Devin, or a future client. AIDA gives them one durable graph of intent, one lifecycle vocabulary, one lease/brief/punt/finding substrate, and one audit trail tied to git.

This avoids three traps:

- Do not compete with frontier agents on raw coding ability.
- Do not compete with PM SaaS on organization-wide workflow UX.
- Do not compete with MCP marketplaces on breadth of integrations.

AIDA competes on correctness, continuity, and coordination depth inside the repo.

## Ranked Product Recommendations

### P0: Publish an AIDA marketplace package for Claude Code

Build a first-class Claude Code plugin or marketplace repo that installs the AIDA skills, hooks, MCP config, setup guide, and first-project workflow. The plugin should be validated with `claude plugin validate`, include a `SETUP.md` flow for MCP setup, and avoid path assumptions that break after Claude copies the plugin into its cache.

Why: Claude's official plugin directory is now a distribution channel and a credibility signal. AIDA already has most of the components; packaging is the missing layer.

Acceptance shape:
- `.claude-plugin/marketplace.json` exists in a publishable package repo or subpackage.
- Plugin includes AIDA skills/hooks/MCP setup and a documented `aida init` path.
- Container seed instructions exist for teams using devcontainers or CI images.
- Security notes enumerate local executable permissions and MCP write tools.

### P0: Create an AIDA MCP registry/distribution package

Prepare AIDA for the official MCP registry and downstream MCP marketplaces. This should include server metadata, transport support notes, install snippets for Claude Code, Codex, Cursor, Windsurf, VS Code, Continue, and Copilot where applicable, plus a default safe profile.

Why: Linear and Sourcegraph show that remote MCP with OAuth/DCR is becoming the enterprise integration norm. AIDA's local stdio MCP is enough for dogfood, but marketplace discovery expects metadata and safe install snippets.

Acceptance shape:
- `docs/agents/aida-mcp-install-matrix.md` covers major clients.
- Registry metadata includes name, description, homepage, source, license, packages, transports, and auth stance.
- Default exported tool profile is read-mostly; write tools require explicit profile or project trust.

### P0: Add MCP tool profiles and a safe default surface

Split the MCP surface into named profiles:

- `read-only`: list/show/search/history/resources only.
- `coordination`: read-only plus briefs, claim/release, punt/finding/comment.
- `operator`: coordination plus directives, queue/drain controls, status/doctor.
- `admin`: destructive or broad mutation surfaces.

Why: market leaders are reducing tool noise and requiring scoped access. AIDA's flat tool list will become harder for agents to select from and harder for operators to trust.

Acceptance shape:
- `aida mcp-serve --profile read-only|coordination|operator|admin|full`.
- Tools advertise profile metadata.
- Docs recommend `read-only` for exploratory clients and `coordination` for trusted implementers.
- Tests verify profile filtering and that write tools are absent from `read-only`.

### P1: Build remote/auth-capable MCP for team and cloud-agent use

Add a remote HTTP MCP mode with explicit authentication, project scoping, and audit logging. Local stdio remains the default for solo use.

Why: GitHub Copilot cloud agent, Linear, Sourcegraph, Windsurf, and Atlassian all point toward remote MCP transports. If AIDA remains local-stdio-only, it cannot participate in cloud-agent workflows except through wrapper hacks.

Acceptance shape:
- `aida mcp-serve --transport http --bind 127.0.0.1:<port>` for local remote clients.
- Token-based local auth first; OAuth/DCR spike second.
- Audit log records tool name, caller identity if known, project, spec IDs touched, and result.
- Docs are explicit that write tools are disabled unless profile/auth permits them.

### P1: Turn `aida status` + `aida doctor` into the operator dashboard

Keep investing in passive detection, actionable healing, and release-readiness summaries. The market is converging on "agent activity must be governable"; AIDA can win this locally.

Acceptance shape:
- `aida status` summarizes agents, leases, briefs, queue, PRs, CI, MCP health, stale locks, unshipped commits, and human gates.
- `aida doctor --json` produces machine-readable drift findings for future dashboards.
- Every heal that can remove or overwrite local state writes salvage first.

### P1: Ship a "first project in 15 minutes" path with proof artifacts

Create a fresh-project demo that proves the core loop: initialize, add spec, launch agent, ship PR, auto-bump status, inspect trace. Include asciinema capture and a repeatable script.

Why: The Trojan-horse TUI story only works if the first user reaches the "quiet depth" moment fast. Marketplaces reward installable demos, not architectural essays.

Acceptance shape:
- `scripts/aida-demo.sh` works against a throwaway GitHub repo or local-only mode.
- A checked-in cast or generated cast path demonstrates the loop.
- README uses the demo as the primary "try AIDA" path.

### P1: Add Linear/Jira/GitHub issue bridge as composition, not replacement

Start with lightweight one-way refs: tags like `linear:LIN-123`, `jira:PROJ-456`, `github:owner/repo#123`, surfaced in `aida show`, MCP results, and status views. Defer full bidirectional sync until usage proves it.

Why: PM tools are where humans and enterprise permissions live. AIDA should attach engineering intent to those systems without trying to own their workflow.

Acceptance shape:
- `aida add --external-ref linear:LIN-123` or tag convention with validation.
- MCP search can filter by external ref.
- `aida show` links out to configured systems.
- Optional import command can seed specs from issue URLs.

### P2: Add agent-client install/adaptation matrix

Maintain a living matrix for Claude Code, Codex, Cursor, Windsurf, Continue, Copilot, Cline, Aider, Devin, and Sourcegraph/Amp:

- instruction file convention,
- MCP config file or command,
- hook/permission model,
- marketplace/package model,
- whether write tools are safe,
- known caveats.

Why: AIDA's cross-agent claim depends on operationally correct adapters. The matrix prevents "works in Claude, rots elsewhere."

Acceptance shape:
- Docs page generated or manually maintained under `docs/agents/`.
- Each row has last-verified date and source link.
- Release checklist warns when matrix is stale.

### P2: Create an AIDA marketplace/security review checklist

Document what must be true before publishing AIDA into third-party marketplaces:

- no hidden network calls,
- no unbounded filesystem writes,
- no surprise shell execution beyond documented `aida` binary,
- least-privilege MCP profile,
- source visible,
- update behavior understood,
- credentials never embedded.

Why: Marketplace trust is fragile. Claude's own docs warn users to review plugin permissions and connected services; AIDA should lead with the same discipline.

Acceptance shape:
- `docs/security/marketplace-publication-checklist.md`.
- CI validates plugin manifests and MCP metadata.
- Release checklist includes the security review.

### P2: Build proof metrics for "agent lift"

Measure what AIDA uniquely improves:

- manual interventions per drain,
- stale-base recoveries,
- unshipped local commits caught,
- time from brief to PR,
- auto-bump success rate,
- number of agents coordinated per day,
- requirement trace coverage.

Why: The market is noisy. Proof beats positioning. AIDA already has telemetry primitives; productize them.

Acceptance shape:
- `aida metrics agent-lift --since 7d`.
- Markdown report generator for release notes and case studies.
- Dogfood dashboard in this repo.

## Recommended Roadmap Shape

### Next 2 weeks

1. Package AIDA for Claude Code plugin marketplace.
2. Add MCP install matrix and registry metadata.
3. Add MCP tool profiles with safe default.
4. Finish first-project demo script/cast.
5. Add marketplace/security checklist.

### Next 1-2 months

1. Remote HTTP MCP with auth and audit log.
2. Linear/Jira/GitHub external refs.
3. `aida status`/`doctor` JSON dashboard contract.
4. Agent-client adaptation matrix with release freshness checks.
5. Agent-lift metrics report.

### Defer until usage proves demand

1. Full bidirectional PM sync.
2. Hosted multi-tenant AIDA service.
3. A public AIDA cloud dashboard.
4. General-purpose agent swarm orchestration.
5. Marketplace for third-party AIDA plugins.

## Positioning Copy to Test

Short:

> AIDA is the git-native control plane for teams running coding agents.

Medium:

> AIDA gives Claude, Codex, Cursor, Windsurf, and other agents one shared spec graph, one lifecycle, one lease/brief/punt substrate, and one auditable trail from intent to code.

Technical:

> AIDA is a repo-local MCP server, CLI, and lifecycle engine for agentic software delivery. It stores project intent as a git-versioned spec graph, exposes it to agents through MCP, coordinates work through leases and briefs, and verifies delivery through trace comments, PR gates, status drift detection, and doctor heal paths.

Anti-positioning:

> AIDA is not a better model, IDE, PM SaaS, or general agent swarm. It is the durable engineering-intent substrate underneath those tools.

## Open Questions

1. Should AIDA publish a standalone Claude plugin marketplace repo, or embed `.claude-plugin/marketplace.json` in this repo first?
2. Should remote MCP be local-loopback-only first, or should the first cut include team LAN/cloud deployment?
3. Should write-capable MCP tools be opt-in per profile, per tool, or per session role?
4. Which PM bridge should ship first: GitHub Issues, Linear, or Jira?
5. What is the minimum public demo that proves the "agent coordination substrate" claim without requiring a full multi-agent marathon?

## Source Notes

This memo intentionally leans on primary/vendor sources where possible. Third-party directories and Reddit posts were used only as directional signals during research, not as primary evidence for product claims.
