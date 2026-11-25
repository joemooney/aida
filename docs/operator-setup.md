# Set up AIDA for conversational development

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-112 STORY-2852 | ai:codex -->

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

Recovery starts by identifying which state is missing. Preserve unpublished store commits and needed local evidence before changing attachments. A remote branch contains only history that was successfully published to it. Read the [storage reference](cli/10-storage.md) and [mode comparison](advanced/storage-modes.md) before migrating a legacy backend.

## Understand clone identity and allocation

The default allocation mode is WORK. The clone has an opaque UUID binding and per-type node-local counters shared by its linked worktrees. Native IDs include that binding; old integer-node examples describe earlier formats, not a guarantee of today's ID shape.

Only an explicitly enrolled HOME clone may consume global agreed-ID blocks or promote IDs. Enrollment is a direct-human-terminal operation for the exact clone common Git directory; this guide does not enroll anything. Cached global blocks or copied shared metadata do not make a WORK clone authoritative. Inspect `aida config allocation show`; follow command help and the [allocation reference](cli/07-project-setup.md#clone-local-allocation-authority) for enrollment and recovery.

HOME and WORK are the shipped allocation-policy names. They do not mean a person's physical home/workplace and are not a privacy or outbound-sharing policy. Friendly aliases do not grant authority. Do not assume every pull or push turns a UUID-based ID into a short global ID. Hierarchical reservation authorities and per-remote sharing policies discussed as future designs are not shipped by this documentation change.

## Inspect transport independently

`aida config transport show` inspects the optional clone-local `work` profile. Its policy is separate from allocation mode and shared project settings. Enabled transport is store-only: push pins the store ref; pull scans incoming history and permits only a checked fast-forward, with a journal for interrupted-transition recovery. Divergence refuses rather than rebasing or merging.

Execution requires the documented reviewed private deny policy, trusted baseline and store/hook checks. Dry-run/JSON plans inspect metadata and do not prove content safety. Supported prepared sanitized writes use the scanner; unsupported writers refuse while the profile is enabled. Raw Git, foreign writers, receiving-side enforcement and automatic public forwarding remain outside this client boundary. Read the full [transport contract](cli/07-project-setup.md#aida-config-transport) before enabling it.

## Inspect queues at the right scope

Project queues can live in the store, while role-global queues live under `~/.aida/queue/`. A store push cannot transfer a different machine's role-global queue. Active roles, explicit filters and user identity affect what a queue view selects. Use the [multi-node guide](advanced/multi-node.md) to distinguish identity, routing and unpublished history before attempting repairs.

## Enable scheduled guards, deputies and multi-seat mandates

`aida init` seeds the project's job registry (`.aida-store/schedule/*.yaml`, inspected with `aida schedule status`). To drive the `guard → route` schedule automatically on a host machine without model-side polling, install the 15-minute `aida schedule tick` driver at an interactive terminal:

```bash
# Install a 15-minute crontab entry for this repository (idempotent)
aida schedule install-cron

# Or install a systemd user timer on Linux (requires an interactive terminal)
aida schedule install-systemd
```

When running a multi-seat fleet (`product`, `advisor`, `integration`, and `implementer` lanes), an operator at an interactive terminal can grant time-boxed shipping authority with `aida mandate start --ship-held --ttl 8h --reason "…"` (or `aida mandate start --drive` to also enable the 15-minute `mandate-drive` tick), and start a terminal deputy (`aida deputy` or `aida deputy shell run`) so contained seats can submit allow-listed host or TTY commands.

## Operator-specific setup

> **Operator-specific setup (beyond `aida init` today):** A fresh `aida init` scaffolds the roles, two-seat protocol, advisor and integration standing orders, `aida schedule` guard jobs, merge trains, and `aida mailbox`. Two high-scale pieces used in the reference multi-seat deployment are currently operator-configured rather than built into `aida init`, alongside the planned capabilities that will bring them into the core substrate:

- **1. Remote build host for implementer lanes (operator SSH scripts today; native remote lane host planned):** `aida queue work` and `aida agent new` spawn lanes on the local host where you invoke them; AIDA does not yet ship a built-in command to dispatch N headless lanes onto a separate remote build server over SSH. In the reference multi-seat setup, the operator offloads headless implementer lanes and heavy Rust compilation/test runs to a dedicated Linux build host over SSH using small host-side wrapper scripts (`~/lane-start.sh`, `lane-run.sh`, `seat-snap.sh`, `store-sync.sh`, and `end-finished.sh`) that synchronize the `aida-store` ref, run the lane in a remote worktree, and publish the resulting branch back to the forge. *Aspirational / planned direction:* native remote lane host support in AIDA so `aida queue work` and `aida lane job` can target an SSH build host directly from `.aida/config.toml` without custom wrapper scripts.
- **2. Sandbox and permission exemptions for the terminal deputy (`"aida deputy shell submit *"`):** When the `advisor` or `integration` seat runs inside a contained or permission-gated harness and needs to submit a multi-step host script to a human-started script deputy (`aida deputy shell run`), it invokes `aida deputy shell submit --file <path> --wait`. In Claude Code, allow-listing the bare prefix without a wildcard does not match invocations that pass `--file`, `--spec`, or `--wait` flags; add the wildcard pattern `"aida deputy shell submit *"` (or `"Bash(aida deputy shell submit *)"` in `.claude/settings.json` / `.claude/settings.local.json`) so the harness permits deputy submissions without stalling on an interactive confirmation prompt.
- **3. Product-seat standing orders, landing sweeps and fleet policy knobs (standing orders & handoffs today; first-class templates & config knobs planned):** While `aida init` seeds template standing orders for the advisor and integration seats, operators running a dedicated `product` seat today record product-seat standing orders via `aida session handoff --seat product --standing-orders add "<text>"` and encode fleet sizing rules (such as capping concurrent implementer lanes at 4 and concurrent reviews at 2, reserving top-tier reasoning models for conflict/design forks, re-scoping specs at rework round 3, and never moving an approved or train-pinned PR head via raw forge CLI commands) in repo-local standing orders. *Aspirational / planned direction:* promoting product standing orders, scheduled landing-health and landing-sweep seat jobs, head-pin refusal guards, and integration promotion pipelining (`integration.pipeline_depth`) into first-class `.aida/config.toml` knobs and default `aida init` templates.

The [administrator guide](advanced/admin-guide.md), [multi-hub guide](advanced/multi-hub-sync.md) and [setup reference](cli/07-project-setup.md) provide detail. The [configuration guide](configuration-guide.md) explains feature-specific scopes. For source evidence, inspect [clone allocation](../aida-core/src/allocation_authority.rs), [transport dispatch](../aida-cli-lib/src/transport_profile.rs), [recoverable pull](../aida-cli-lib/src/transport_pull.rs), and [agent scaffolding](../aida-cli-lib/src/init_cmd.rs).
