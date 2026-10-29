# Configuring AIDA around your team

AIDA's operating model is configurable. A person chooses the policies and boundaries; agents apply them through the CLI and MCP. Configuration is more than a change of appearance: it shapes which agent runs, what work contract it receives, how much autonomy it has and where the next human decision belongs.

This page maps the choices to their maintained references. It does not introduce a universal precedence rule: settings have different scopes and trust requirements.

## The configuration map

| Choice | What can change | Where to look |
|---|---|---|
| Autonomy per spec | `operator`, `guided`, `drive`, `drain`; `decide` blocks execution | [Work & autonomy](cli/03-work-autonomy.md), [mode definition](../aida-core/src/models.rs) |
| Coding-agent launch | Vendor defaults, per-agent flags, supported model/effort selections and launch posture | [Roles & sessions](cli/06-roles-sessions.md), [per-agent configuration](agents/per-agent-config.md) |
| Work contracts | Editable type protocols and optional research/docs/keystone lane overlays stored as META requirements | [Protocols](cli/02-specs.md), [project overview](../OVERVIEW.md) |
| Review and integration | Required checks, applicable review policies, integration strategy and escalation behavior | [Git lifecycle](cli/04-git-lifecycle.md), [review process](review-process.md) |
| Queue operation | Scope, batch selection, concurrency and priority/queue order where supported | [Work & autonomy](cli/03-work-autonomy.md) |
| Authority | Role/seat grants and time-boxed mandates, with their supported scopes | [Roles & sessions](cli/06-roles-sessions.md), [reporting and gates](cli/08-reporting.md) |
| Project footprint | Full scaffold, minimal/memory footprints, shared sibling store and adopted projection workflows | [Project setup](cli/07-project-setup.md) |
| Agent interface | Token-efficient CLI for shell-capable agents; explicit typed MCP profiles for MCP-native clients | [Integrations](cli/09-integrations.md), [MCP setup](agents/aida-mcp-install-matrix.md) |
| Reporting | Output formats, history layouts, hints, statusline and telemetry views | [Reporting](cli/08-reporting.md), [environment variables](environment-variables.md) |
| Transport | Supported clone-local restricted transport enrollment and destination policy | [Transport configuration](cli/07-project-setup.md#aida-config-transport) |

## A conversation about configuration

> Use Codex as the default implementation agent. Keep architectural decisions interactive, send routine fixes through independent review, and show me anything that still needs my decision. Explain which settings need to be project policy and which belong only on this machine before changing them.

An AIDA-aware agent can inspect the supported settings, propose the configuration and make authorized edits. The request does not create missing credentials, make unsupported harness features available or override a human-only operation.

## Work protocols are editable project knowledge

AIDA stores type protocols as META requirements, with optional lane overlays. Pickup injects the resolved work contract into the agent's context. Precedence is **type < lane < spec acceptance**, with a shared body cap. This lets a project change how a spike is investigated or a documentation task is validated without maintaining a separate policy prompt in every agent chat.

Keep protocols specific and check their effect on the next pickup. A written instruction guides the model; it is not equivalent to a deterministic gate.

## Trust scopes matter

Project policy, clone-local state and machine-global operator settings are different layers. Per-agent defaults can come from `~/.aida/agents.toml` and a project override; some settings instead use `.aida/config.toml`, validated grants or the common Git directory. Exact precedence is documented by each feature.

Configuration that can launch commands has stricter trust rules. Some permissions can be enabled only through machine-global settings or a direct human workflow. See [the configuration trust boundary](security/config-trust-boundary.md). Do not assume a tracked TOML file can authorize its own execution or grant a seat.

## Configure, inspect, then delegate

1. State the desired operating policy to the agent.
2. Have it show the supported settings, scope and proposed changes.
3. Apply authorized configuration and perform any required human-only setup.
4. Inspect the resolved policy and run a bounded example before expanding delegation.

Configurable does not mean every combination is supported. Containment differs across vendors, `decide` remains a block, and selecting autonomous delivery does not override independent review, CI or authority gates. Planned per-remote selective-sharing/domain policies are not presented here as shipped functionality.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
