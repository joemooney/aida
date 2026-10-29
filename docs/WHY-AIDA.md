# Why AIDA?

A coding agent can produce a change. A team of agents needs a shared way to decide what to build, preserve decisions, coordinate work and establish whether a change is ready to merge.

**AIDA is a governed software factory with selectable autonomy.** People describe outcomes, resolve important choices and set the limits of delegation. Agents use AIDA's CLI and MCP tools to maintain the project record and carry out the workflow.

## The human experience

You can ask your agent:

> Add rate limiting to login. Work out the acceptance criteria with me, record the requirement, and prepare the change for my review.

The agent checks existing requirements, files or updates the spec, asks about unresolved choices, and selects the permitted work path with the advisor. You do not need to translate that request into a series of `aida add`, `aida edit` and queue commands. Those are primarily agent-facing operations. They remain available to operators for inspection and troubleshooting.

The request above is an example of how to work with an AIDA-aware agent, not a built-in natural-language endpoint or a guarantee that any unconfigured chatbot will follow the workflow.

## What persists after the conversation

- **Intent:** versioned requirements, acceptance criteria and design decisions in a Git-backed graph.
- **Coordination:** queues, roles, session leases, worktrees and handoffs that identify owned work.
- **Evidence:** code traces, linked commits and review verdicts tied to exact commits.
- **Authority:** grants, mandates and merge gates that constrain delegated action.

The coding agents still implement and reason. AIDA provides the shared record and the controls around them.

## Choose how much to delegate

| Model | Mode | Human involvement |
|---|---|---|
| Human-led delivery | `operator` | A person does the work; no implementation agent is launched. |
| Collaborative delivery | `guided` | A person works through recorded design decisions with an agent and reviews the resulting change. |
| Supervised delivery | `drive` | Agents build through CI; a person reviews and merges. |
| Autonomous delivery / dark factory | `drain` | Agents implement, test, independently review and merge approved, authorized work; exceptions escalate. |

`decide` blocks execution until an outstanding decision is resolved. Modes are chosen per spec. Selecting a mode does not create authority or override gates.

## Configure the factory around the team

Autonomy is one of several choices. AIDA supports configurable agent launch defaults, work protocols stored as editable META requirements, review and integration policy, queue selection and ordering, reporting preferences, and project/store layouts. Settings have different trust scopes: some belong to the project, others to the operator's machine. A project setting cannot replace a required human grant.

See [Configuring AIDA](configuration.md) for a capability map and the reference for each setting. There is no single precedence rule for all settings; each reference names its own scope and resolution rules.

## Where AIDA fits

AIDA works alongside coding agents, Git, CI and forge pull requests. GitHub or GitLab can continue to hold code and PRs, while an existing project tracker can continue to serve stakeholders. AIDA adds the agent-oriented spec graph, coordination and authority model. See [composition](positioning/composition.md) for specific seams.

Start smaller when appropriate. A one-off edit may need only your coding agent and existing tests. AIDA is most useful when work must survive handoffs across sessions, roles and changes. It also offers a [memory lane](positioning/memory-lane.md) for adopting durable project context before the full delivery workflow.

## Current limits

AIDA is alpha software and Linux-only. Multi-human isolation under a shared Unix user is not a security boundary. Supported launch vendors do not have identical containment or harness capabilities. Agents may need networked model APIs even though the store is local. A merge is not a production deployment. Review evidence is useful accountability, not a certification of correctness or compliance.

## Next steps

- [Product showcase](showcase/index.html): the visual introduction and executive brief.
- [Getting started](getting-started.md): operator setup and the first agent conversation.
- [First project](first-project.md): a worked human/agent journey.
- [Detailed overview](what-is-aida.md): capabilities, evidence and limitations.
- [CLI reference](cli/README.md): the agent and operator command surface.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
