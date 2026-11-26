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

## Enable scheduled jobs, deputies and mandates

<!-- trace:STORY-2839 | ai:claude -->

`aida init` seeds the project's job registry; `aida schedule status` lists each job, its seats and its next run. To run the registry without a model polling it, install a driver for `aida schedule tick` at an interactive terminal:

```bash
aida schedule install-cron      # a 15-minute crontab entry for this repository
aida schedule install-systemd   # or a systemd user timer on Linux
```

For a multi-seat fleet (product, advisor, integration and implementer lanes), an operator at an interactive terminal can grant time-boxed shipping authority with `aida mandate start --ship-held --ttl 8h --reason "<why>"`. `aida mandate start --drive` also turns on the 15-minute drive tick that routes stalled lanes. Contained seats that need a host or terminal command submit it to a deputy the operator starts in their own terminal: `aida deputy` for single allow-listed commands, or `aida deputy shell run` for whole scripts, submitted with `aida deputy shell submit --file <path> --wait`.

## Size lane admission for the host

Lane admission limits are per machine, not per project. They live in the host file `/etc/aida/lanes.toml`, which root owns, under a `[lanes]` table. They do not belong in `.aida/config.toml`, which these knobs ignore. An unknown key or out-of-range value makes the file invalid, and an invalid file refuses every launch and lane job rather than falling back to defaults. A missing file means the defaults below.

```toml
[lanes]
max_concurrent_cold_starts = 4    # default 2
min_memory_headroom_gib = 4       # default 4
max_compile_concurrency = 4       # unset: no cap
cargo_build_jobs = 4              # unset: cargo's default
min_job_memory_headroom_gib = 8   # unset: no check
```

- `max_concurrent_cold_starts` caps how many managed headless implementer launches may be starting at once, across every checkout and user on the host. Further launches wait for a slot.
- `min_memory_headroom_gib` is the floor of available RAM plus free swap at launch. A launch below it, or one that cannot read memory, is refused with a diagnostic.
- `max_compile_concurrency` caps how many lane jobs (`aida lane job run`, including the host's fast gate) may run at once. Further jobs wait for a slot.
- `cargo_build_jobs` sets `CARGO_BUILD_JOBS` for every lane job; a job cannot set it itself.
- `min_job_memory_headroom_gib` makes a lane job wait at start until headroom reaches the floor, releasing its compile slot while it waits. An unreadable memory reading refuses the job.

The startup limits and their slot release rules are described in the [setup reference](cli/07-project-setup.md). To exceed the launch limits once, set `AIDA_OVER_HOST_LIMIT="<reason>"` in the launching command's environment; the reason is recorded. `aida doctor --lane-host` checks the machine before you run automated lanes.

## Setup that `aida init` does not do yet

`aida init` scaffolds the roles, the two-seat protocol, advisor and integration standing orders, scheduled guard jobs, merge trains and the mailbox. Three pieces of the reference multi-seat deployment are still configured by the operator.

- **A remote build host.** `aida queue work` and `aida agent new` start lanes on the machine where you run them; AIDA does not yet dispatch lanes to a separate build server. The reference setup runs headless implementer lanes and heavy Rust builds on a Linux build host over SSH, using small host-side scripts that sync the `aida-store` ref, run the lane in a remote worktree and publish its branch to the forge. Native remote lane hosts are planned.
- **Harness permission for deputy submissions.** In Claude Code, an allow-list entry for the bare `aida deputy shell submit` prefix does not match calls with `--file`, `--spec` or `--wait`. Add `Bash(aida deputy shell submit *)` to `.claude/settings.json` or `.claude/settings.local.json` so a seat can submit without stalling on a confirmation prompt. Allow-listing it lets any agent in that session run arbitrary scripts at the operator's terminal with operator authority, so the deputy runner, and your review of each submitted script, becomes the only gate on those jobs. Add it only for a supervised advisor seat whose deputy runner you trust, preferably in that seat's `.claude/settings.local.json`. Never add it for implementer or reviewer lanes.
- **Product-seat standing orders and fleet sizing.** Record product-seat orders with `aida session handoff --seat product --standing-orders add "<text>"`. Fleet rules, such as caps on concurrent implementer lanes and reviews or when to re-scope a spec after repeated rework, are kept there as text. First-class configuration for them is planned.

The [administrator guide](advanced/admin-guide.md), [multi-hub guide](advanced/multi-hub-sync.md) and [setup reference](cli/07-project-setup.md) provide detail. The [configuration guide](configuration-guide.md) explains feature-specific scopes. For source evidence, inspect [clone allocation](../aida-core/src/allocation_authority.rs), [transport dispatch](../aida-cli-lib/src/transport_profile.rs), [recoverable pull](../aida-cli-lib/src/transport_pull.rs), and [agent scaffolding](../aida-cli-lib/src/init_cmd.rs).
