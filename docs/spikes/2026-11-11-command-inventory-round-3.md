# SPIKE-714: invocation effects and advisor decision packet — 2026-11-11

<!-- trace:SPIKE-714 | ai:codex -->

**Blocked; not an acceptance report or extraction authorization.** Round-three
findings 1–3 remain open. The 167-row table still has 167 unresolved classes;
there are no verified per-class totals and no certified twenty-command batch.
This report adds production paths that the previous report did not investigate:
even the smallest view handlers inherit writes from the CLI invocation envelope.
It also identifies concrete extraction dependencies for three view candidates.
No production code, visibility, crate ownership or classification policy changes
are made here.

## Source and method

Production source, registry and writer ledger were inspected at
`1a189c33160ddaff869952cd2df36c1c2e7af086`, the reviewed HEAD before this report.
Line references below are relative to that commit. Reads used `rg` and bounded
source excerpts, following named calls rather than whole-file token matches.
The exact four writer IDs below were looked up in the parsed `writer` array of
`scripts/store-writers.toml`; this establishes ledger membership in addition to
the positive production paths. No Rust runtime probe was run. A positive path
does not establish a complete transitive closure or exclusion of other effects.

The required host query client remains absent at
`/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph`; `aida doctor check
codegraph` reports `codegraph,missing`. No query succeeded, so there is no
codegraph “no callers” result to confirm. No lane index was created, refreshed
or queried through the seat's link. Host recovery still requires deploying the
reviewed client and supplying its absolute path and indexed main revision.
The source inspection below is useful evidence, but does not substitute a claim
that the required graph workflow ran.

## New production evidence: effects outside the registered handler

The earlier question concerned direct writes by `away` and `home`. That is only
part of the scope issue. The shared invocation also writes for view commands:

| Effect | Reachable production path | Conditions and destination |
| --- | --- | --- |
| Presence rewrite before early dispatch | `lib_part1.rs:1818` `run` → `presence::auto_flip_if_interactive` at `lib_part1.rs:2079` → `presence.rs:858` → `set_home` at `presence.rs:872` → `write_presence_file` at `presence.rs:177–178` → `fs::write` at `presence.rs:171` | stdin and stdout are terminals, the presence file is readable and stores Away, and filesystem writes succeed. The destination is the presence file under the resolved home. This runs before `invocation.run_early` at `lib_part1.rs:2096`. |
| Usage append after a handler returns | `lib_part1.rs:213` `main_entry` → `run` at `lib_part1.rs:286` → successful return from early dispatch → `usage::is_enabled` at `lib_part1.rs:362` → `usage::append_event` at `lib_part1.rs:389` → `usage.rs:222–242` creates the directory and opens the log with `create(true).append(true)` → `writeln!` | Telemetry is enabled by default (`usage.rs:175–192`); opt-outs can disable it. A resolved log path and successful filesystem operations are needed. `usage.rs:165–166` resolves `.aida/usage.jsonl` beneath `aida_home_dir()`. It does not require a TTY. |

Exact matching writer-ledger IDs:

```text
aida-cli-lib/src/presence.rs|write_presence_file|fs::create_dir_all
aida-cli-lib/src/presence.rs|write_presence_file|fs::write
aida-cli-lib/src/usage.rs|append_event|OpenOptions::new
aida-cli-lib/src/usage.rs|append_event|fs::create_dir_all
```

These are reachable writes, not merely same-file candidates. They apply to
normal successful invocations that traverse these hooks; this is not a claim
that every early return, exit or special entry path traverses both hooks.
Turning telemetry off in this research seat would not eliminate its possible
effect in production. A test with captured stdout would also miss the TTY
presence path. No real presence state or usage log was changed for a probe.

For example, `run_schema` (`schema.rs:1806–1848`) returns normally after rendering
reflection data. Its `Early` registration is at `schema.rs:1852–1863`, so it
finishes through the shared early-dispatch return above. A handler-only
description of schema as a reflection read and an invocation-level observation
of schema writing telemetry can both be true. They are different audit scopes.

## Advisor ruling requested, with consequences

The research lane may surface this fork but must not choose it. The following
are alternatives for the advisor, not adopted policies:

| Ruling needed | Alternative | Consequence for the requested deliverable |
| --- | --- | --- |
| Effect boundary | Classify the registered handler and its descendants; separately enumerate common host-envelope effects | Pure view handlers can remain candidates despite host telemetry/presence. Every row must clearly state that boundary. |
| Effect boundary | Classify all reachable effects of the complete CLI invocation | The three view candidates below are writers on the observed normal paths. A strictly read-only first batch cannot include them without a further scope exception. This report does not infer the other 164 commands' classes. |
| Direct CLI-owned state and generated outputs | Include them under `core-write`, interpreted as a mutation/effect class rather than proof of routing through core | Direct state writers can receive the requested class while their current ownership and architectural debt remain explicit. |
| Direct CLI-owned state and generated outputs | Reserve `core-write` for actual store/core/gate routing | The requested three classes need an advisor-specified treatment for direct writers; calling them read-only would contradict the observed effects. |
| Mixed command families | Use the strongest reachable effect, with verb-level evidence | Read verbs under a mutating registration do not by themselves make that REGISTRY row a first-batch candidate. The ordering and authority criteria need to be explicit. |

This specifically qualifies **finding 3**: an unconditional demand to certify
about twenty *invocation-level read-only* commands is disputed pending the
effect-boundary ruling. It is not a dispute that AC3 is unmet, nor an assertion
that twenty handler-level views cannot be found. Findings 1 and 2 are accepted;
their missing audit work is real. Resolving the fork will not itself complete
the 167-command audit or make the missing client available.

## Three candidate dependency inspections

This is a bounded inspection, **not** the first porcelain batch or a global
ranking. All three require a command-registration boundary independent of
cli-lib: `cmdkit` is public but still lives in cli-lib, which cannot depend on
a peripheral crate that imports cli-lib. The alternatives below describe APIs
or coordinated movement, not instructions to make everything public.

| Command | Actual handler paths and dependencies | Exact observed visibility/ownership blockers | Conditional stage and home alternatives |
| --- | --- | --- | --- |
| `schema` | `run_schema` dispatches only to the local catalog/object/requirement renderers. `CATALOG` contains `aida_core::typescript::declaration::<T>` reflection callbacks. Non-core reflected types are `findings::FindingRow`, `punt::PuntRecord`, `worker::Directive`, root-imported `brief::BriefListEntry` and `leases::SessionLease` (`schema.rs:34–37, CATALOG`). `aida-core/src/typescript.rs:7–12` calls `T::decl` with reflection configuration, rather than a store writer. | `BriefListEntry` at `brief.rs:585` and `SessionLease` at `leases.rs:59` are `pub(crate)`. `FindingRow`, `PuntRecord` and `Directive` are public under public modules, but importing their current owning crate from a dependency of that crate creates a cycle. `run_schema`, `SchemaArgs` and `SCHEMA` are crate-private; moving them together avoids widening those local items. | After leaf extraction **and** provision of the reflected CLI-owned type declarations through an owner API or injected declaration data. Leaves alone are insufficient. Alternatives: porcelain-views renderer with host-supplied declarations, or retention with the schema/type owner until those models move. |
| `commands` | `help_catalog.rs:749` `run_commands` → local `print_command_catalog` (`:138`) → `catalog_rows` (`:39`) → `command_registry::built_command` (`command_registry.rs:207`) → `build_pristine_command` (`:226`) → each registered command factory (`:234`). The catalog recursion, flag projection, sorting and rendering are local. | `built_command` is `pub(crate)` in a public module. It depends on the host registry and `cli::Cli`; the peripheral renderer needs a passed clap tree or catalog data, or a host callback. Moving this handler does not require exposing unrelated SQLite semantic-help functions merely because they share `help_catalog.rs`. | After the cmdkit leaf **and** host tree injection, or after a coordinated registry-owner split. Porcelain-views over a passed `clap::Command`/catalog snapshot versus a host-owned help family. Factory effects have not been exhaustively audited here. |
| `help-all` | `help.rs:495` `run_help_all` → local `print_help_all` (`:294`) → local `command_groups` (`:35`), plus `external_cmd::print_help_section` (`external_cmd.rs:382`) → `discover` (`:193`) → `discover_in` (`:157`) and `shadowed_names` (`:81`) → `user_alias::real_subcommand_names` (`user_alias.rs:173`) → the host command tree. `discover_in` enumerates PATH directories and executable metadata; `print_help_section` does not call `run_external`. | `external_cmd` is a private module (`lib.rs:471`); its `print_help_section`, `discover` and `shadowed_names` are `pub(crate)`. `user_alias::real_subcommand_names` and `command_registry::built_command` are `pub(crate)`. Local groups and renderers can move together. A host-supplied installed-extension list avoids exporting the discovery/registry chain. | After the cmdkit leaf **and** extension-catalog injection or coordinated owner extraction. Alternatives: porcelain-views formatter fed by host discovery, or help/extension application owner. This is not “move now” merely because its main body prints static text. |

The external registry paths above are positive edges. Invoking every registered
factory while building a clap tree is an additional transitive audit obligation,
not proof that those factories mutate or do not mutate persistent state.
Likewise, `schema` mentioning a lease type does not itself exercise lease
authority: it reflects a type declaration rather than calling a grant operation.

Within this subset only, the cmdkit/registration boundary affects 3 commands;
the host tree provider affects `commands` and `help-all` (2); CLI-owned reflection
data affects `schema` (1); extension discovery affects `help-all` (1). These
counts do not rank globally shared blockers across all 167 registrations.
No count of lexical crate roots is presented as verified dependency fan-out.

## Verification and handoff

Re-run the bounded checks without mutating production state:

```sh
python3 -m unittest discover -s tests -p test_command_inventory.py -v
python3 - <<'PY'
from collections import Counter
from pathlib import Path
import tomllib
table = tomllib.loads(Path('scripts/command-inventory.toml').read_text())
rows = table['command']
assert len(rows) == 167
print(Counter(row['classification'] for row in rows))
writers = tomllib.loads(Path('scripts/store-writers.toml').read_text())['writer']
ids = {row['id'] for row in writers}
for item, sink in [('presence.rs|write_presence_file', 'fs::create_dir_all'),
                   ('presence.rs|write_presence_file', 'fs::write'),
                   ('usage.rs|append_event', 'OpenOptions::new'),
                   ('usage.rs|append_event', 'fs::create_dir_all')]:
    assert 'aida-cli-lib/src/' + item + '|' + sink in ids
print('four reachable sink IDs present in writer ledger')
PY
```

The four existing extraction regression tests passed. The bounded table/ledger
checks passed. They validate the extraction fixtures, denominator and cited
ledger membership, not the uncompleted command classifications. No Cargo run
was needed for this documentation-only change.

Recovery requires the advisor's effect-boundary/direct-writer ruling and the
host's reviewed query client. After recovery, audit every handler and command
factory path as applicable, retain exact writer/gate witnesses and blockers per
row, then rank the approximately twenty views from the completed dependency
matrix. This report supplies neither that matrix nor acceptance credit.

This seat leaves the report uncommitted for the host. SPIKE-714 is punted on the
explicit finding-3 scope dispute and the unresolved prerequisites for findings
1–2. The punt remains a publication veto; no ship declaration, push, PR creation,
review verdict or queue completion is authorized by this report.
