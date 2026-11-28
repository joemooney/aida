# Configure the way your team works

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-107 | ai:codex -->

AIDA separates what work means, how it is dispatched and who may authorize its next step. You can start with conversational development and configure more delegation for selected work. Configuration is a product capability: teams can choose agent profiles, execution modes, work protocols and presentation preferences without adopting one fixed operating pattern.

## Six places to make a choice

- **Per-spec execution mode:** `operator`, `guided`, `drive`, `drain` and `decide` describe different human involvement contracts. They do not grant roles or merge authority. Use the [overview](what-is-aida.md) and `aida do --help` to inspect the dispatch contract.
- **Agent and launch defaults:** supported managed launchers include Claude Code, Codex and Antigravity. Machine and project `agents.toml` files carry vendor defaults. The launch commands support vendor-specific options; model names are opaque pass-through values, not AIDA validation of model availability. Effort, permission flags and containment support differ between harnesses.
- **Work protocols:** editable META requirements tagged `protocol:<type>` define type guidance. Optional `protocol:lane:<lane>` overlays tailor research, documentation or consequential design work. Use `aida protocol list` to inspect installed protocols. If the store has none, an operator can install defaults with `aida protocol seed`; then use `aida protocol show task --lane docs` to inspect the actual text before changing it.
- **Workflow policy:** project settings control such features as protocol enforcement, drain behavior and required CI treatment. Read the relevant command help and [lifecycle reference](cli/04-git-lifecycle.md) before editing policy. Changing documentation or guidance does not replace a runtime authority check.
- **Project footprint:** full, minimal and memory-lane footprints select how much scaffolding a project adopts. `aida init --footprint memory-lane` installs the store and reflex capture/learn guidance; `--footprint minimal` installs store/config/cache without that guidance. The distinct `--minimal` demonstration option is not the same as `--footprint minimal`.
- **Reporting preferences:** history layouts, output format, workflow hints and glyphs let operators and agents choose useful views. `aida history --help` describes named/inline templates; `aida config hints`, `aida config glyph` and the global `--format` option address different surfaces. A presentation setting does not change spec identity or authority.

## Scope is feature specific

There is no universal precedence ladder for every AIDA setting. Read each feature's contract.

Agent defaults are read from machine `~/.aida/agents.toml` and project `.aida/agents.toml`; individual readers determine merging and overrides. Default-flag lists and scalar permission fields do not necessarily combine the same way. `aida config permissions show` reports the effective launch posture. The interactive configuration menu needs a terminal; command help and read-only inspection remain available to non-TTY agents.

Spec execution mode and META protocols belong to the requirements graph. Project guidance and policy files live with their documented project scope. Clone-local allocation and transport settings belong to the clone's common Git directory; linked worktrees share that clone state. Machine preferences and grants have their own local scope and trust rules. Copying a project config file does not copy a live grant, and a friendly ID alias does not confer authority.

## Guidance and enforcement are different

Protocol precedence is type, then lane, then spec acceptance. Pickup carries labeled guidance with a combined 40-line body cap; a long protocol is not an unlimited instruction channel. The compact per-turn notice repeats type guidance while the spec lease is held. The CLI and MCP protocol resource expose the same rendered text.

The prose protocol is guidance. Only specifically implemented machine checks enforce protocol deliverables; the default protocol enforcement posture is warn. A claim that all written instructions are enforced would be incorrect. Reviewers still evaluate acceptance and evidence.

## Explore a policy request

For routine documentation you might ask: "Use our preferred agent, preserve independent review, and let approved bounded work proceed with minimal interruption." For a sensitive design change: "Keep the design conversation interactive and require the accountable person's decision."

These are illustrative requests, not live configuration writes or complete configuration recipes. Ask the agent to inspect current settings, identify the exact supported knobs and propose a reviewable change. Confirm the affected scope before applying it. AIDA's permission posture and role authority remain separate from the amount of conversation you select.

## Inspect before you change

```bash
aida init --help
aida do --help
aida protocol list
# If defaults have not been installed, run: aida protocol seed
aida protocol show task --lane docs
aida config permissions show
aida agent new codex --help
aida agent new claude --help
aida agent new antigravity --help
aida history --help
```

The [setup reference](cli/07-project-setup.md), [roles and sessions](cli/06-roles-sessions.md) and [reporting reference](cli/08-reporting.md) contain the operational details. Configuration should fit your team while preserving the source-backed limits of each feature.

## Landing policy

<!-- trace:STORY-2897 trace:TASK-3818 | ai:claude -->

`[landing] mode` in `.aida/config.toml` sets what a merge into the default branch waits for: `ci-gated` (the default) or `review-gated`.

`[landing] red_main.fix_window` (default `"2h"`, from one minute to one week) is how long a confirmed red main may wait for a fix before AIDA recommends a revert. Set it with `aida config menu`, which checks that the value is a duration in range. See [When main goes red](advanced/landing.md#when-main-goes-red).

<!-- trace:TASK-3912 | ai:claude -->
`[landing] max_prs_per_train` (default `10`) caps how many PRs the automatic merge train admits into one train; the rest wait for the next train, in order. A smaller train is cheaper to bisect when one member turns it red. An explicit `aida pr train N...` is not limited by it; pass `--max` there.

<!-- trace:STORY-3181 | ai:claude -->
`[landing] auto_train_supervised_under_mandate` (default `false`) decides whether the automatic merge train may land a supervised PR that an active mandate covers. While it is `false`, the automatic train leaves every supervised PR for a human. If the setting changes on the default branch while a merge waits, that merge is refused and runs again under the new value. The landing path for `true` is not available yet, so the automatic train still leaves those PRs for a human. A manual `aida pr train` is unchanged.

AIDA reads these from the default branch only, and `aida config show` lists the effective values with their source. A value AIDA does not recognise falls back to `ci-gated`, with a warning. See [Landing approved work](advanced/landing.md#landing-modes) for what each mode does, and [Who may do what](advanced/authority-and-seats.md) for who may merge.

## Unset execution mode

<!-- trace:STORY-2884 | ai:claude -->

Only `drain` lets automation merge a spec's PR without a person. A spec with no execution mode is supervised: `aida pr ship` and `aida pr train` leave its PR open for a human at a terminal or an active mandate. `[gate.unset_mode]` in `.aida/config.toml` changes what an unset mode means:

```toml
[gate.unset_mode]
default = "supervised"         # the default; or "drain"
drain_types = ["bug", "task"]  # with "drain": the spec types it covers
```

Set `default = "approved-small"` to preview drain eligibility at approval and in `aida show` for Approved specs tagged `small`. Ship and train keep unset modes supervised under this setting. Every changed path must match a case-sensitive, root-relative glob in `drain_safe_paths`. The default list is `["docs/cli/*.md"]`. An explicit list replaces it. Empty or malformed lists, unknown paths and empty diffs keep supervision. Deletes and both rename paths count. Widening the list requires advisor review per entry. This option ignores `drain_types`. Explicit modes take precedence. Strict-tier spec metadata or changed paths (including leases, claims, grants, containment, gates, guards and authority) keep the spec supervised.

With `default = "drain"`, a spec with no mode whose type is in `drain_types` merges as a `drain` spec would. All other merge checks still apply: an independent approval at the PR's head, green CI and no holds. The setting never applies to:

- a spec that sets its own mode (`drive`, `guided`, `operator` and `decide` stay as set)
- a sensitive spec: a keystone, epic, security, authority, containment or `operator-only` spec, the same classes a mandate excludes
- a PR that changes a protected gate path (`.aida/config.toml`, the gate code, and any `protected_paths` the project adds), or whose changed files cannot be read
- a project whose protected gate paths cannot be read: AIDA reads `protected_paths` from the same default-branch commit as the setting and from `~/.aida/config.toml`, and if either does not parse, no spec takes the default
- a PR whose head moves after its files were checked: `aida pr ship` stops, and a fresh run checks the new head

AIDA reads the setting from the default branch only, so a branch cannot turn it on for itself. An unknown default keeps supervision. Only `default = "drain"` reads `drain_types`. Unknown entries are ignored, and a missing list or one with no recognised types leaves every type supervised. When a PR is held, the message names the effective mode and why, for example `BUG-12 is marked unset (supervised: not defaulted, the change touches ...)`. `aida show` prints the effective mode of a spec with no mode, for example `Effective mode: drain (project default for unset bug)`, whenever the project sets a drain default. For a spec merged into another spec's PR (superseded into a carrier), it prints the carrier's mode and names the carrier, as the merge does.

The drain's own merge phase and the burndown wave merge still hold an unset spec. The merge seat lands those PRs with `aida pr train`, which applies the setting.

