# AIDA across clones and machines

People work with agents; agents use AIDA's CLI or MCP to inspect and synchronize the project record. This operator reference describes the boundaries that matter when work moves between clones.

## Code, store and local state are different

The ordinary Git-canonical setup has a code branch and an orphan `aida-store` branch. AIDA's normal sync commands distinguish those two legs. The attached `.aida-store/` worktree exposes the canonical graph locally; `.aida/cache.db` is a derived read cache.

Not all runtime state travels on the store branch. Machine-global role queues and grants, local operational files and active process ownership must not be treated as remote-shared state. Project queues have a different storage scope from role-global queues. See [queue behavior](cli/03-work-autonomy.md) and [roles/sessions](cli/06-roles-sessions.md).

## Ask the agent to establish the facts

> Inspect both code and store sync state. Show me local-only changes, remote divergence, the selected queue/user identity and any restricted transport policy before synchronizing. Do not discard local work.

For direct operator inspection, `aida status`, `aida queue list`, `aida config allocation show`, and `aida config transport show` provide supported views. Read-only status does not prove that another offline clone has published all its work.

## Clone identity and spec IDs

Ordinary clones default to WORK allocation: an opaque UUID node binding and persistent per-type counters in the common Git directory. Linked worktrees share that binding. Read-only use does not require minting a new identity; supported init/acquire lifecycle establishes the binding before allocation.

Node-local IDs have a form such as `BUG-<node UUID>-001`. Friendly node aliases provide readable display/input forms while canonical IDs remain stored. An alias confers no authority. Current HOME enrollment is a separate direct-human workflow that permits global allocation and promotion; copied block records do not confer that permission.

See [clone-local allocation authority](cli/07-project-setup.md#clone-local-allocation-authority) for the current contract. Broader per-remote sharing and hierarchical allocation proposals are not described here as shipped behavior.

## Normal synchronization

For ordinary clones, agents use the documented `aida pull` and `aida push` operations, or the supported scope-specific alternatives. Pull can reconcile store history and run authorized lifecycle processing. WORK/HUB post-pull processing skips agreed-ID promotion; HOME promotion retains native lookup aliases.

Do not assume an operation is read-only because it is called a pull or a dry run. The [Git lifecycle reference](cli/04-git-lifecycle.md) defines current flags, remote handling, wait behavior and failures.

## Restricted transport is a separate policy

An explicitly enrolled restricted transport profile has a pinned destination and reviewed trust policy. Its store-only push and fast-forward-only pull intentionally avoid ordinary lifecycle side effects. It is not a general-purpose object filter or a guarantee that arbitrary private history can be published safely.

Read the [transport contract](cli/07-project-setup.md#aida-config-transport) before enrolling or changing it. Different sharing intentions require deliberate store/history boundaries, not a label on a node.

## When a queue looks empty

Have the agent distinguish:

1. A different resolved shell user, role or queue scope.
2. Canonical changes that have not been published or received.
3. Local role-global queue state that does not travel with the project store.
4. Cache/read-context freshness where the particular command uses a projection.

Use supported command output rather than assuming one historical on-disk path still describes every queue. A cache rebuild cannot fetch another machine's unpublished commits or recreate its local grants.

<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex -->
