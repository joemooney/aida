# Set up AIDA for conversational development

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-112 | ai:codex -->

Operator setup connects AIDA to the coding agent and project policy. It is separate from the normal user journey: people can keep discussing and implementing changes with their agent while the agent maintains structured intent. Start readers with the [conversational guide](getting-started.md).

## Install on a supported system

AIDA is alpha software and supports Linux x86_64 and arm64. Windows and macOS are unsupported. The release workflows publish the two Linux tarballs, containing `aida` and `aida-server`; the installer detects Linux architecture and installs the CLI. Follow the [README installation route](../README.md#install), or inspect the [installer source](../scripts/install.sh) before running it. Pin a release with its documented `--version` option when reproducibility matters.

`aida upgrade` updates an installed release. Building AIDA itself follows the repository's developer workflow, which is separate from installing a release for a project. Do not use old Darwin release examples as current instructions.

## Initialize or join deliberately

For a new Git project, `aida init` establishes the default git-canonical store and local cache. Selected agent profiles receive guidance and skills according to the footprint. CLI-capable agents use the CLI by default; MCP registration requires the explicit `--with-mcp` opt-in. `--agent` chooses profiles, and `--footprint` chooses full, minimal or memory-lane adoption. Inspect `aida init --help` before applying setup options. Non-interactive init does not answer terminal-only permission decisions for you.

Before creating specs in a fresh clone of an existing AIDA project, run `aida init` (or `aida node acquire`) in that clone to establish its local identity. Cloning the code and shared store alone is not sufficient for allocation. Follow the project's setup guidance and inspect existing adoption settings before changing scaffolding.

An existing project may use the adoption plane instead of tracked product-branch scaffolding. `aida adoption status` inspects the projected revision and `aida adoption inventory` is a dry-run migration inventory. Shared non-executable settings and guidance live under `adoption/` on the store branch and project into git-ignored `.aida/`; executable settings retain their separate trust rules. Do not replace an adopted projection with a scaffold dump.

For a multi-repository workspace, `aida init --sibling` creates a separate sibling store repository. `--store-path` selects a path and `--attach` joins an existing sibling store without reseeding its content. Linked code worktrees share their clone's common Git directory; separate clones do not gain the same clone-local authority merely by copying store content.

## Keep three kinds of state separate

- **Canonical history:** the requirements graph and its Git history, including locally committed but unpublished changes. The default orphan branch is `aida-store`; a sibling store is a separate configured repository. Code history is another Git history/branch set.
- **Derived cache:** `.aida/cache.db` is a read projection. `aida cache status` inspects it and `aida cache rebuild` rebuilds from available canonical history. Reads can report stale, deferred or busy context; a read is not an unconditional promise of a synchronous full rebuild.
- **Local operational state:** grants, mandates, runtime session files, verdict artifacts and handoffs have their own local scope and validation. Clone allocation/transport state resides in the common Git directory; machine preferences have their own home-directory scope. A cache rebuild or store clone does not restore all of these or grant authority.

Recovery starts by identifying which state is missing. Preserve unpublished store commits and needed local evidence before changing attachments. A remote branch contains only history that was successfully published to it. Read the [storage reference](cli/10-storage.md) and [mode comparison](storage-modes.md) before migrating a legacy backend.

## Understand clone identity and allocation

The default allocation mode is WORK. The clone has an opaque UUID binding and per-type node-local counters shared by its linked worktrees. Native IDs include that binding; old integer-node examples describe earlier formats, not a guarantee of today's ID shape.

Only an explicitly enrolled HOME clone may consume global agreed-ID blocks or promote IDs. Enrollment is a direct-human-terminal operation for the exact clone common Git directory; this guide does not enroll anything. Cached global blocks or copied shared metadata do not make a WORK clone authoritative. Inspect `aida config allocation show`; follow command help and the [allocation reference](cli/07-project-setup.md#clone-local-allocation-authority) for enrollment and recovery.

HOME and WORK are the shipped allocation-policy names. They do not mean a person's physical home/workplace and are not a privacy or outbound-sharing policy. Friendly aliases do not grant authority. Do not assume every pull or push turns a UUID-based ID into a short global ID. Hierarchical reservation authorities and per-remote sharing policies discussed as future designs are not shipped by this documentation change.

## Inspect transport independently

`aida config transport show` inspects the optional clone-local `work` profile. Its policy is separate from allocation mode and shared project settings. Enabled transport is store-only: push pins the store ref; pull scans incoming history and permits only a checked fast-forward, with a journal for interrupted-transition recovery. Divergence refuses rather than rebasing or merging.

Execution requires the documented reviewed private deny policy, trusted baseline and store/hook checks. Dry-run/JSON plans inspect metadata and do not prove content safety. Supported prepared sanitized writes use the scanner; unsupported writers refuse while the profile is enabled. Raw Git, foreign writers, receiving-side enforcement and automatic public forwarding remain outside this client boundary. Read the full [transport contract](cli/07-project-setup.md#aida-config-transport) before enabling it.

## Inspect queues at the right scope

Project queues can live in the store, while role-global queues live under `~/.aida/queue/`. A store push cannot transfer a different machine's role-global queue. Active roles, explicit filters and user identity affect what a queue view selects. Use the [multi-node guide](multi-node.md) to distinguish identity, routing and unpublished history before attempting repairs.

The [administrator guide](admin-guide.md), [multi-hub guide](multi-hub-sync.md) and [setup reference](cli/07-project-setup.md) provide detail. The [configuration guide](configuration-guide.md) explains feature-specific scopes. For source evidence, inspect [clone allocation](../aida-core/src/allocation_authority.rs), [transport dispatch](../aida-cli-lib/src/transport_profile.rs), [recoverable pull](../aida-cli-lib/src/transport_pull.rs), and [agent scaffolding](../aida-cli-lib/src/init_cmd.rs).
