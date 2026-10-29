# Using AIDA with Claude Code

Claude Code is a coding-agent runtime. AIDA provides shared project intent, workflow coordination and authority checks around that runtime. You describe outcomes to the agent; the agent uses AIDA to read requirements, record decisions and carry the work through its authorized lifecycle.

## Start with a conversation

> Check the existing requirements for login throttling. Help me settle the acceptance criteria, record the spec and prepare the implementation for human review.

An AIDA-aware session can follow the installed skills and project guidance. The agent normally performs the underlying CLI operations. The conversation above is an example, not a separate AIDA chat endpoint or a promise that an unconfigured session has project access.

## The interfaces behind the conversation

The CLI is the default AIDA interface for agents that can run shell commands. The same commands remain useful to operators for inspection and troubleshooting. Typed MCP tools are available through `aida mcp-serve` for explicitly configured MCP clients; MCP registration is opt-in during init with `--with-mcp`.

See [the client setup matrix](agents/aida-mcp-install-matrix.md) for profiles and client-specific details. An MCP tool profile controls the exposed tool set; it does not substitute for a validated role grant.

## Skills and project guidance

AIDA distributes workflow skills and guidance for the supported agent surfaces. The agent reads the relevant skill when performing work such as requirement capture, planning, implementation or review. Avoid hard-coded skill counts: inspect the installed inventory or the shipped template inventory for the current set.

People can explicitly invoke a skill or describe the task in ordinary language. The skill gives the agent a procedure; AIDA's deterministic checks enforce the applicable product boundaries. Those are different layers of assurance.

## Roles and isolated sessions

Advisor, implementer, reviewer and integrator are workflow responsibilities with scoped authority. They are not synonyms for the runtime's within-conversation subagents. AIDA launches and tracks supported sessions, associates work with specs and uses worktrees/leases to isolate owned work.

Supported launch configuration controls vendor, flags and permission posture. See [per-agent configuration](agents/per-agent-config.md) and [roles and sessions](cli/06-roles-sessions.md). Vendor-native features are not assumed to be identical across Claude Code, Codex and Antigravity; containment support must be checked for the selected vendor.

## Select the human handoff

Use `guided` for an interactive decision session, `drive` for implementation through CI with a human review/merge step, and `drain` for autonomous delivery through independent review and merge within the gates. `operator` leaves the work to a person; `decide` blocks execution.

The agent should explain the selected path before launching work. A role name, environment hint or configuration flag alone cannot create authority. Human-only grant and mandate operations remain human operations.

## Do not turn experiments into installation instructions

Older research explored compiling orchestration to vendor-native workflows. Those experiments are historical evidence, not a description of the current production drain. The current execution path and supported launch surfaces are documented in [Work & autonomy](cli/03-work-autonomy.md) and [the autonomy architecture](architecture/autonomy-and-escalation.md).

Continue with [the first project](first-project.md), [configuration](configuration.md) or [the CLI reference](cli/README.md).

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
