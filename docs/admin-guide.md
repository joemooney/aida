# AIDA administrator's guide

This guide is for the operator and the agents maintaining an AIDA installation. Product stakeholders normally work through an AIDA-aware agent; they do not need to administer the store or learn mutation commands to request work.

## Supported deployment

AIDA is alpha software for Linux x86_64 and arm64. The default deployment is a Git-canonical requirements store with a rebuildable local SQLite read cache. Coding agents bring their own vendor authentication and model access. Multi-human identity separation under one Unix user is not a security boundary.

See [configuration](configuration.md) for the configuration map and [project setup](cli/07-project-setup.md) for current command details.

## Separate the data layers

| Layer | Purpose | Operational treatment |
|---|---|---|
| Code repository | Application source, tracked project policy and guidance | Normal code review, CI and branch protection |
| `aida-store` branch and attached `.aida-store/` worktree | Canonical spec objects and supported durable coordination data | Preserve commits/history; sync using the supported store lifecycle |
| `.aida/cache.db` | Derived SQLite read projection | Rebuildable from canonical store; not the requirements backup |
| Local `.aida/` operational files | Feature-specific runtime evidence, sessions and review/dispatch state | Preserve relevant evidence; do not assume all local state is in the store |
| `~/.aida/` | Machine-global roles, grants, mandates and supported preferences | Machine-specific trust and recovery; do not copy blindly between users or hosts |

The CLI and MCP share the product's requirement and authority semantics. A UI or server is an optional surface, not the canonical database.

## Initialize and configure deliberately

For a new project, an operator or authorized setup agent runs `aida init`. Agent profiles are selected by detection or explicit flags; MCP scaffolding is opt-in with `--with-mcp`. Existing/adopted projects have their own attach/projection paths. Use the [getting-started guide](getting-started.md) and inspect the command help before choosing force, footprint or store-location options.

Settings have different scopes: project config, clone-local common-Git state, machine defaults and validated grants. There is no single blanket override order. Review [configuration](configuration.md), [agent defaults](agents/per-agent-config.md), and [configuration trust](security/config-trust-boundary.md).

## Storage choices and migration

Use the default orphan-branch store for ordinary projects, or the supported sibling-store layout when multiple code repositories intentionally share one store. Legacy standalone SQLite/YAML and opt-in PostgreSQL paths remain documented in [storage modes](storage-modes.md); they are not the primary onboarding path. Archived migration commands are not a routine way to change a current project's canonical store.

Do not edit canonical object files or counters to bypass validation. Ask the maintenance agent to inspect the actual format and supported command, make a backup, and report the planned migration before any identity-affecting operation.

## Identity and synchronization

Ordinary clones use an opaque node binding and persistent local counters. Explicit HOME allocation enrollment is a separate human workflow, and possession of a copied global-block ledger grants no authority. Friendly node labels affect display and lookup, not identity ownership. See [allocation authority](cli/07-project-setup.md#clone-local-allocation-authority).

AIDA's ordinary sync distinguishes the code and store legs. Restricted transport has different behavior and refuses lifecycle side effects; it is not generic selective sharing. Review [Git lifecycle](cli/04-git-lifecycle.md), [multi-node operation](multi-node.md) and [restricted transport](cli/07-project-setup.md#aida-config-transport) before changing remote policy.

## Backup and recovery

Preserve both the code repository and canonical store history. Include relevant local operational evidence and machine configuration according to your recovery requirements. A cache rebuild restores derived reads; it cannot recreate unpushed canonical changes, lost review evidence or grants.

Before repair, have the maintenance agent report store cleanliness, pending Git operations, local/remote divergence and the relevant locks. Use [storage commands](cli/10-storage.md), `aida doctor`, and the feature-specific recovery advice. Doctor's report is not authorization for an unchecked cleanup.

## Diagnose through the agent

Useful operator inspection commands include `aida status`, `aida doctor`, `aida cache status`, `aida config show`, `aida awaiting` and `aida ps`. Agents normally execute them and explain the result. Automatic jobs run only when their scheduler/tick infrastructure is configured; defining a job alone does not make it run.

For a diagnosis, ask:

> Inspect the store, cache, sessions and sync state. Show me the evidence and a bounded repair plan. Preserve canonical history and review records; identify anything requiring direct human authorization.

## Related references

- [CLI reference](cli/README.md): agent/operator command behavior.
- [Repository guide](agents/aida-repository-guide.md): developing AIDA itself.
- [Storage modes](storage-modes.md): current and legacy data layouts.
- [Monitor contract](monitor-contract.md): supported read-only dashboard integration.
- [Marketplace publication checklist](security/marketplace-publication-checklist.md): review before distributing integrations.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
