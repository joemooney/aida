# What you can do with AIDA

AIDA is a configurable, governed software factory. The human describes outcomes and resolves decisions; agents normally perform the CLI/MCP operations. These examples are requests to an AIDA-aware agent, not commands every newcomer must learn.

## Preserve project intent

> Record this decision and the alternatives we rejected. Before changing the cache, check whether an earlier requirement already explains the behavior.

Agents can query and maintain the Git-backed spec graph. Code traces and commit trailers connect implementation to intent. The [memory lane](positioning/memory-lane.md) is the entry path when durable context is enough and delivery orchestration is not yet needed.

## Select the human handoff

> Build this feature through CI and stop for my review. Keep architectural questions interactive. Recommend autonomous delivery for bounded fixes only when the criteria and authority permit it.

The per-spec modes are operator, guided, drive and drain. Decide blocks work pending a decision. Drain is the dark-factory lifecycle through implementation, CI, independent review and merge, within the applicable gates. A mode does not grant permission. See [the overview](what-is-aida.md) and [autonomy reference](cli/03-work-autonomy.md).

## Configure the operating model

> Show the agent defaults, work protocols and queue policy for this project. Propose changes in the right trust scope before applying them.

Supported settings include vendor/launch defaults, editable type/lane protocols, queue selection/order, review/integration behavior, footprints and reporting. They do not all share one precedence order. See [configuration](configuration.md).

## Coordinate concurrent work

> Identify independent approved work and propose isolated sessions. Report conflicts, unresolved decisions and anything that needs my attention.

AIDA combines queues, roles, worktrees, leases and handoffs. Concurrency is controlled by the selected launcher and current locks; independent worktrees are not permission to start unlimited competing drains. See [work and autonomy](cli/03-work-autonomy.md) and [sessions](cli/06-roles-sessions.md).

## Inspect the result

> Show the acceptance criteria, implementation links, validation results, review verdict and remaining issues for this change.

Review evidence is tied to commits; lifecycle and usage views provide additional context. Data coverage and attribution have limits. See [reporting](cli/08-reporting.md) and [the token ledger](token-usage-ledger.md). A merged change is not automatically a production deployment.

## Agent and operator references

The CLI contains launch, sync, queue, review and inspection commands. Agents choose the operation appropriate to their task and authority; operators use the same reference for setup and diagnosis. Begin with [the command manual](cli/README.md), rather than treating older one-line launch recipes as a guarantee of unattended delivery in every environment.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
