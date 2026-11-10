# STORY-2704: porcelain crate sketch
<!-- trace:STORY-2704 | ai:codex -->

Date: 2026-11-10. Sketch signed off with changes at 04:13 UTC.
The binding 08:58 UTC ruling authorizes slice 1 implementation: the workspace
multi-call crate, flow tenant, public no-write ledger API and dev-build support.
The built-in stays registered and temporarily includes the tenant's report source
to keep the overlap identical. Nested dispatch is S2; release/install/upgrade is S3.
The remaining sections retain the broader sketch as future-slice design context.

## Evidence and boundary

- [external_cmd.rs](../../aida-cli-lib/src/external_cmd.rs) resolves only
  `argv[1]`: built-ins and aliases win, then an executable `aida-<name>` on
  PATH. First executable wins; discovery is sorted and excludes shadowed names
  and reserved companions (`server`, `generate-types`). Unix uses exec.
- [STORY-1641 black-box tests](../../aida-cli/tests/story_1641_external_subcommands.rs)
  cover argv/status/signals, built-in precedence, agent alias exclusion,
  context environment, missing-name diagnostics, listing and delegated help.
  They do not establish nested dispatch or read-only enforcement.
- ADR-71 (`aida show ADR-71 --full`) is currently draft. Its core/porcelain
  admission rule and no-direct-store-write boundary motivate this design.
  Its text specifies CLI-only access and independent distribution; the binding
  brief instead permits public `aida-core` APIs and a workspace companion.
  Record that amendment when accepting the design; do not imply ADR acceptance.
- The [crate-split deep dive](2026-11-07-aida-cli-lib-crate-split-deep-dive.md)
  finds a large command-module dependency cycle. Linking convenience code back
  to CLI internals would preserve that coupling, defeating this separation.
- [dev_build.rs](../../aida-cli-lib/src/dev_build.rs) currently builds the
  workspace with profile `agent`, but `LIVE_BINARIES` stages, promotes, rolls
  back and installs links for only `aida` and `aida-server`.
- [release.yml](../../.github/workflows/release.yml) builds/packages those two
  binaries for Linux x86_64/arm64; [install.sh](../../scripts/install.sh) installs
  named files. Neither currently installs porcelain. The documented host
  codegraph executable was absent in this lane; evidence uses source and rg.

## Placement and dependency direction

Add workspace crate `aida-porcelain/`, one binary `aida-porcelain`, with small
area modules (`flow/`, later `review/`, `queue/`) and a static command table.
The table maps invocation names to handlers, help summaries and read contracts.
Do not create speculative empty command families.

Dependency direction: `aida-porcelain -> aida-core` public APIs only, plus
ordinary parsing/rendering dependencies. No dependency on `aida-cli-lib`, its
private modules, proposed application-family crates, or copied internal helpers.
CLI dispatch remains in `aida-cli-lib`; there is no reverse Rust dependency on
porcelain. Share install metadata as data, not by linking the binary crate.

Use documented JSON queries through the invoking `AIDA_BIN` where public core
read APIs are insufficient. A needed ledger query belongs in a separately
reviewed plumbing API/JSON surface; porcelain must not parse private ledger,
verdict, cache or train files. Keep injected execution inputs on the existing
`exec_context` boundary, rather than adding ambient readers or allowlists.

## Multi-call invocation and installation

Initial owned names: `aida-porcelain` and `aida-flow`. The latter dispatches
`aida flow report ...`; `aida-porcelain flow report ...` is the explicit form.
Dispatch by the lexical basename of argv[0], before resolving executable links.
Unknown invocation names fail with usage; never fall back to another handler.
Both forms share parsers, output and exit codes, including `--help`/`--version`.
Do not install `aida-review` or `aida-queue`: those parents are built-ins.
Future approved nested tenants get names such as `aida-review-summary`, each
explicitly listed in the install manifest; no arbitrary prefix wildcard aliases.

Use relative symlinks from each owned alias to `aida-porcelain` in the same bin
directory. A plain `cargo install --path aida-porcelain` installs only the real
binary; document the supported installer for aliases. Keep `porcelain` visible
as an external entry offering the full catalog, rather than reserving it as a
server companion. PATH discovery must retain the alias name, not canonicalize
it into `aida-porcelain` before exec.

Development installation must extend the current build pipeline explicitly:

1. Include the real binary in staging, SHA/version smoke checks, live rotation,
   rollback, interrupted-swap recovery and promotion metadata. Build membership
   alone does not install it. Preserve the existing primary-main, cleanliness,
   origin, wave and lock checks; worktree builds still never promote.
2. Make `--install-links` preflight all owned destinations and conflicts before
   changing links. Install canonical links to live binaries plus alias links to
   the canonical porcelain link. Preserve existing replacement confirmation.
3. Smoke-test staged aliases as well as the canonical executable. Rollback
   restores the prior binary and alias inventory; first adoption must tolerate
   a previous generation with no porcelain. Store inventory with the generation
   so later alias removal does not leave broken links or delete unowned files.
   Existing per-binary rename promotion is not an atomic whole-generation swap;
   extend its recovery tests without claiming stronger atomicity.

Release must build `-p aida-porcelain`, stage its executable plus the explicit
alias manifest and relative links, and include them in both Linux archives.
Update `scripts/install.sh`, the tarball upgrade path in `upgrade_cmd.rs`, and
`make install` to install/replace that same inventory beside `aida`.
Validate archive members and link targets; do not unpack arbitrary links into
an installation prefix. Older archives lacking porcelain remain installable.
Preflight conflicts; remove obsolete aliases only when proven installer-owned.
Test upgrade from the old two-binary layout and rollback of a dev generation.

## Nested built-in-parent resolution and help

Retain today's top-level precedence: real built-in and built-in aliases, then
eligible human user aliases, then external. Agents/headless never expand user
aliases. External parents own all remaining argv: `aida flow report` runs
`aida-flow report`; core must not search for `aida-flow-report` afterward.

For a recognized built-in parent, add a narrowly scoped unresolved-child hook:

1. Resolve parent aliases to the canonical built-in and parse recognized global
   and parent options using clap metadata, including option values and `--`.
   Only a genuine unknown child at a subcommand slot qualifies. A positional
   argument, invalid option, missing value or invalid known-child arguments
   remains a clap error; never dispatch on an arbitrary parse failure.
2. Built-in child names, visible/hidden aliases and built-in argv rewrites win.
   Never retry a failed built-in handler through an extension. For a valid
   unknown child, search PATH for `aida-<canonical-parent>-<child>` and pass only
   its remaining arguments with the resolved output/context environment.
3. Restrict this slice to one child below a built-in top-level parent. Reject
   path-like names and reserved/help collisions using the existing name rules.
   No match preserves the original clap diagnostic and exit status; any hint
   is supplementary. Reuse first-PATH-match, signal and exit behavior.

`aida review --help` and `aida help review` append installed, unshadowed nested
children in a sorted External commands section, with source/path. Discovery
must not execute extensions. `aida review summary --help` and
`aida help review summary` delegate to the same nested executable with `--help`.
Top-level help and alias-list machine output retain invocation/path provenance;
list recognized nested entries under their parent instead of advertising only
an ambiguous flat `review-summary`. Do not list shadowed children. Uninstalled
extensions add no entries, and built-in help topics keep precedence.

## Read-only contract

Extension dispatch itself grants no read-only guarantee: it execs an ordinary
PATH program with inherited OS permissions. Arbitrary third-party extensions
remain outside this first-party porcelain contract.

Expose a narrow query interface to area handlers: immutable snapshot DTOs and
explicit read operations, no mutable store handle, writer, shell runner, arbitrary
argv callback or mutation flags. Audit public core readers for implicit cache
refresh, migration, locks or persistence before using them; a method named
`load` alone is insufficient. Add a public no-write reader if needed. The JSON
adapter accepts only fixed reviewed query templates via `AIDA_BIN`, never user
provided command strings. Forge adapters permit read operations only and bound
responses; no posting, filing, status updates or refresh jobs. Output goes to
stdout/stderr; redirecting a report is the caller's choice.

This is a capability boundary inside reviewed code, not OS isolation from all
public core writer symbols. Prove the shipped command's behavior with fixture
runs: compare store refs, tracked/untracked bytes and project cache/queue/lease
state before/after success, help and error paths; use a read-only mounted fixture
as an independent control and a fake AIDA/forge that rejects mutation attempts.
Any separately approved mutating tenant needs a new contract and review tier;
read-only tenants fit STORY-2698's proposed LOW tier without granting exemption.

## STORY-2699 migration and implementation checks

STORY-2699 is present in [flow_cmd.rs](../../aida-cli-lib/src/flow_cmd.rs),
with its parser in `flow_cmd_args.rs` and `FLOW` in `command_registry.rs`.
Its early handler declares `StakeholderAction::Read`, but still depends on
private project-root, gate-ledger, timeout, output-format and forge helpers.
Move its report parser, aggregation, single classifier table and fixtures into
`aida-porcelain/src/flow/`, preserving `aida flow report`, human/JSON output,
window semantics, medians/p90 and existing argument errors. Remove its built-in
registration only when the installed `aida-flow` replacement is available.
During overlap the built-in wins; after removal missing porcelain yields the
ordinary unknown-command error and installation guidance, never silent fallback.

Preserve open-to-land, commit-count push proxy, verdict kinds, first review,
approval-to-land, per-run CI durations, train lifetimes and finding
classification/severity. Keep recursive verdict semantics (kind `verdict`,
record verdict/findings/recorded_at), unknown missing measurements, and refusal
on unreadable evidence or truncated forge results. Extract ledger reads behind
public read-only plumbing before moving calculations; do not copy private-file
walks. Specifically replace `read_ledger` and `train_members` (which directly
walk `.aida/gate-ledger`) with a documented query for verdict events and train
coverage. Move the fixed read operations behind `forge::flow_report_read` to
a public read adapter or documented JSON query; do not export CLI internals
just to make the move compile. Preserve the bounded Git `origin/main` history
query, forge timeouts and truncation refusals. Use the existing report JSON as
a before/after compatibility fixture, not a call back into `aida flow report`
after its built-in registration is removed. Preserve fixture/fake-forge coverage for
each metric; no live network requirement for regression tests.

Implementation acceptance should extend STORY-1641 black-box coverage beside
the existing suite: top-level regressions; built-in parent/child aliases;
invalid flags/positionals; unknown-child miss; external-parent ownership;
parent and delegated help; global format/context; PATH order and exit/signals.
Add multi-call argv[0], install conflict, upgrade, rollback/recovery and no-write
controls above. Update docs/cli and affected help goldens in implementation.
Sketch signoff precedes those code changes; this document alone claims no
runtime tests or completion credit for STORY-2704 or STORY-2699.

## Round 2 implementation correction (2026-11-10)

The advisor ruling supersedes the migration sketch during built-in overlap.
The built-in owns flow evidence collection. Porcelain reads its public
`aida flow report --json` output and renders it; it never spawns `gh`.
The final round-3 ruling selects `AIDA_BIN`, then the running binary’s sibling
`aida`, then PATH. It adds no report timeout and preserves child failure output
and exit code instead of rendering the failure again.
No porcelain exemption is added to the forge-read ratchet. The parity test
runs without `ignore` against checkout-built binaries, with fixture forge
responses and filesystem snapshots.

Dev generations retain `.inventory.json` with their binary and alias names.
Rollback removes members absent from the restored generation, including when
rolling back the first porcelain adoption, and can roll forward again. Complete
snapshots keep interrupted generation swaps replayable. Legacy generations
without inventories use their actual installed members.
