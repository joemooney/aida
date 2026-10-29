# Skills, commands and product enforcement

AIDA's normal human interface is a conversation with an agent. Skills and commands connect that conversation to repeatable work; the CLI and MCP expose the operations the agent performs.

## Skills guide the agent

A skill is a documented procedure with a scoped purpose. AIDA ships skills for capturing requirements, planning, implementation, review and other workflows. An agent can select a relevant installed skill, and a person can explicitly ask it to use one.

Skills are not only passive reference material: many contain procedural steps and invoke tools. Their exact discovery and invocation behavior depends on the host agent. See [the shipped skill inventory](../aida-core/templates/skill-inventory.toml) and [skill conventions](skills-convention.md) for the repository's packaging rules.

## Commands are interfaces, with different audiences

A vendor slash command is a host-specific invocation surface. A shell command such as `aida add` is an AIDA operation, normally executed by the agent. MCP tools provide typed operations for MCP-native clients. An operator may use the same shell commands for setup, inspection or repair.

Do not require a stakeholder to learn a command sequence merely to request a feature. Prefer a human request such as “record this requirement and help me settle the acceptance criteria,” with the agent selecting the appropriate operations.

## Enforcement belongs in the product

A skill can tell an implementer to obtain independent review. The review/merge gates check the applicable evidence and authority. Natural-language instructions are useful guidance; they do not provide the same guarantee as a deterministic check.

AIDA combines these layers: configurable procedures for agents, typed durable state, and enforced boundaries around critical lifecycle actions.

## Related references

- [Using AIDA with Claude Code](using-aida-with-claude-code.md)
- [Extending skills](extending-skills.md)
- [Configuration](configuration.md)
- [CLI and MCP integrations](cli/09-integrations.md)

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
